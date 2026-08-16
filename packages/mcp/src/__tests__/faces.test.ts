import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { inspectFaces, measureOuterDiameter } from "../tools/faces.js";
import { documents, openDocument } from "../tools/session.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

/**
 * A motor-shaped part in miniature, reproducing the failure that motivated
 * these tools: an 80 mm-diameter body with a small radial connector boss
 * hanging off it, so the bounding box reads ~108 mm across while the true
 * outer diameter is exactly 80.0. The body axis is **Y**, not Z, so a tool
 * that assumes Z gets it wrong.
 */
function motorDocument(): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  // Body: r=40, h=30 along Z, rotated -90° about X so its axis becomes Y.
  const bodyRaw = add("body-raw", {
    type: "Cylinder",
    radius: 40,
    height: 30,
    segments: 64,
  });
  const body = add("body-y", {
    type: "Rotate",
    child: bodyRaw,
    angles: { x: -90, y: 0, z: 0 },
  });
  // Boss: r=6, sticking out along +X past the body wall.
  const bossRaw = add("boss-raw", {
    type: "Cylinder",
    radius: 6,
    height: 30,
    segments: 32,
  });
  const bossRot = add("boss-x", {
    type: "Rotate",
    child: bossRaw,
    angles: { x: 0, y: 90, z: 0 },
  });
  const boss = add("boss", {
    type: "Translate",
    child: bossRot,
    offset: { x: 24, y: -15, z: 0 },
  });
  const motor = add("motor", { type: "Union", left: body, right: boss });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [{ root: motor, material: "aluminum" }],
  } as unknown as Document;
}

function openMotor(): string {
  return out(openDocument({ initial: motorDocument() })).document_id as string;
}

describe("inspect_faces", () => {
  it("reports analytic cylinder parameters the mesh tools cannot", () => {
    const docId = openMotor();
    const res = out(
      inspectFaces({ document_id: docId, surface_type: ["cylinder"] }, engine),
    );
    expect(res.units).toBe("mm");
    const part = res.parts[0];
    expect(part.matched_faces).toBeGreaterThan(0);

    // The body wall: diameter exactly 80, axis Y (not Z). The boss union
    // splits the wall into several faces on that same cylinder, so this is a
    // `find`, not a uniqueness claim — reassembling them is the coaxial
    // group's job, exercised in measure_outer_diameter below.
    const body = part.faces.find(
      (f: { diameter_mm: number }) => Math.abs(f.diameter_mm - 80) < 1e-6,
    );
    expect(body, "an 80 mm body face exists").toBeTruthy();
    expect(Math.abs(body.axis[1])).toBeCloseTo(1, 9);
    expect(body.feature).toBe("shaft_or_boss");
    expect(body.axial_length_mm).toBeGreaterThan(0);
    expect(body.axial_length_mm).toBeLessThanOrEqual(30 + 1e-6);
  });

  it("summarises instead of dumping every face", () => {
    const docId = openMotor();
    const res = out(inspectFaces({ document_id: docId, summary_only: true }, engine));
    const part = res.parts[0];
    expect(part.faces).toBeUndefined();
    expect(part.groups.length).toBeGreaterThan(0);
    // Grouped tallies: each entry counts faces of one type (and radius).
    const total = part.groups.reduce(
      (n: number, g: { count: number }) => n + g.count,
      0,
    );
    expect(total).toBe(part.face_count);
    expect(part.coaxial_groups.length).toBeGreaterThan(0);
  });

  it("paginates and reports what was withheld", () => {
    const docId = openMotor();
    const res = out(inspectFaces({ document_id: docId, limit: 2 }, engine));
    const part = res.parts[0];
    expect(part.faces.length).toBe(2);
    if (part.matched_faces > 2) {
      expect(part.truncated.showing).toContain("of");
    }
    const page2 = out(
      inspectFaces({ document_id: docId, limit: 2, offset: 2 }, engine),
    ).parts[0];
    expect(page2.faces[0].face_id).not.toBe(part.faces[0].face_id);
  });

  it("filters by radius so 'every M4 clearance hole' is one call", () => {
    const docId = openMotor();
    const res = out(
      inspectFaces({ document_id: docId, radius_mm: 6 }, engine),
    ).parts[0];
    expect(res.matched_faces).toBeGreaterThan(0);
    for (const f of res.faces) {
      expect(Math.abs(f.radius_mm - 6)).toBeLessThan(0.01);
    }
  });

  it("errors with a recovery hint naming the available parts", () => {
    const docId = openMotor();
    const res = inspectFaces({ document_id: docId, part: "ghost" }, engine);
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("ghost");
    expect(res.content[0].text).toContain("motor");
  });
});

describe("measure_outer_diameter", () => {
  it("reads the true OD where the bounding box overstates it", () => {
    const docId = openMotor();
    const res = out(measureOuterDiameter({ document_id: docId }, engine));
    const part = res.parts[0];

    expect(part.axis_selection).toBe("dominant");
    expect(part.outer_diameter_mm).toBeCloseTo(80, 6);
    // The dominant axis is Y — the reason "assume Z" is a bug.
    expect(Math.abs(part.axis[1])).toBeCloseTo(1, 9);
    // ...while the bbox is inflated well past 80 by the boss.
    expect(Math.max(...part.bbox_size_mm)).toBeGreaterThan(90);
    // The split wall pieces are gathered back into one full-height extent.
    expect(part.face_count).toBeGreaterThan(1);
    expect(
      part.axial_range_mm[1] - part.axial_range_mm[0],
    ).toBeCloseTo(30, 6);
  });

  it("honours a requested axis, ignoring its sign", () => {
    const docId = openMotor();
    const pos = out(
      measureOuterDiameter({ document_id: docId, axis: [0, 1, 0] }, engine),
    ).parts[0];
    const neg = out(
      measureOuterDiameter({ document_id: docId, axis: [0, -1, 0] }, engine),
    ).parts[0];
    expect(pos.axis_selection).toBe("requested");
    expect(pos.outer_diameter_mm).toBeCloseTo(80, 6);
    expect(neg.outer_diameter_mm).toBe(pos.outer_diameter_mm);
  });

  it("explains which axes exist when the requested one has no cylinders", () => {
    const docId = openMotor();
    const res = out(
      measureOuterDiameter({ document_id: docId, axis: [0.6, 0.8, 0] }, engine),
    );
    expect(res.parts[0].error).toContain("Axes present");
  });

  it("rejects a zero axis rather than guessing", () => {
    const docId = openMotor();
    const res = measureOuterDiameter(
      { document_id: docId, axis: [0, 0, 0] },
      engine,
    );
    expect(res.isError).toBe(true);
  });
});
