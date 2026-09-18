import { describe, it, expect, beforeAll } from "vitest";
import type { Document, Node } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getKernelWasm } from "../wasm-singleton.js";
import {
  CamError,
  camChecks,
  camJob,
  camOutlineFromDocument,
  type CamHost,
  type CamJobRequest,
} from "../cam.js";

/**
 * The typed CAM client, on the contract it exists to enforce.
 *
 * Two layers, deliberately:
 *
 * 1. **A stub host**, for the shapes the kernel is *supposed* to answer and
 *    the ones it must never answer. These assert the fail-closed contract
 *    itself — a blocked job has no `gcode` key, and a blocked job that
 *    carried one would be refused here rather than reaching an export button.
 * 2. **The real kernel**, on a real part. These assert geometry, not
 *    existence: the plate sections to one boundary and three openings, the
 *    Ø5 holes are bored helically because they are narrower than two cutter
 *    diameters, and the same job on stock half as thick is refused by name
 *    with the overcut in millimetres.
 */

/** Building and replaying a real job is seconds of work, not milliseconds. */
const KERNEL_TIMEOUT = 120_000;

// ---------------------------------------------------------------------------
// The contract, against a stub
// ---------------------------------------------------------------------------

/** A host that answers exactly the document it is given. */
function stub(answer: unknown): CamHost {
  const give = <T,>(): T => answer as T;
  return {
    camJob: give,
    camVerifyGcode: give,
    camFit: give,
    camMaterials: give,
    camRecommendFeeds: give,
    camCheckFeeds: give,
    camGear: give,
    camOutlineFromDocument: give,
  };
}

const MINIMAL_REQUEST: CamJobRequest = {
  stock: { thickness: 6 },
  tools: [{ number: 1, diameter: 3.175 }],
  operations: [
    {
      tool: 1,
      kind: "contour_outside",
      contour: [
        [0, 0],
        [10, 0],
        [10, 10],
        [0, 10],
      ],
      depth: 6,
      stepdown: 0.5,
      feed: 400,
      plunge: 100,
      rpm: 10000,
    },
  ],
};

describe("camJob: the fail-closed contract", () => {
  it("a blocked job has no gcode key, and the discriminant narrows to that", () => {
    const result = camJob(
      stub({
        name: "refused",
        blocked: true,
        policy: { verified: true, blocked_by: ["depth"], warnings: [] },
        notes: [],
      }),
      MINIMAL_REQUEST,
    );

    expect(result.blocked).toBe(true);
    // Not `undefined` — absent. An export path that reads `result.gcode`
    // without narrowing does not compile, and the key is not there to read.
    expect("gcode" in result).toBe(false);
    expect(Object.keys(result)).not.toContain("gcode");

    if (result.blocked) {
      // @ts-expect-error `gcode` is `never` on the blocked arm: there is no
      // program, so there is nothing to name.
      expect(result.gcode).toBeUndefined();
      expect(result.policy.blocked_by).toEqual(["depth"]);
    } else {
      // The other arm is the one that has a program, and it is a string.
      expect(typeof result.gcode).toBe("string");
    }
  }, KERNEL_TIMEOUT);

  it("a passed job narrows to a program", () => {
    const result = camJob(
      stub({
        blocked: false,
        gcode: "G21\nG90\nM2\n",
        policy: { verified: true, blocked_by: [], warnings: [] },
        notes: [],
      }),
      MINIMAL_REQUEST,
    );
    expect(result.blocked).toBe(false);
    if (result.blocked) throw new Error("narrowing is broken");
    expect(result.gcode).toContain("G21");
  }, KERNEL_TIMEOUT);

  it("a malformed request throws rather than looking like a result", () => {
    expect(() =>
      camJob(stub({ error: "stock.thickness must be greater than zero, not 0." }), {
        ...MINIMAL_REQUEST,
        stock: { thickness: 0 },
      }),
    ).toThrow(CamError);
  }, KERNEL_TIMEOUT);

  it("a refusal that carries its own sentence is a result, not a throw", () => {
    // The one carve-out in the surface: a job refused before the oracle ran
    // answers `blocked` *and* `error`, and the sentence is the refusal.
    const result = camJob(
      stub({
        blocked: true,
        error: "the job cannot run as specified: T1 is too short for a 6.000 mm cut.",
        policy: { verified: false, blocked_by: ["tool_geometry"], warnings: [] },
        notes: [],
      }),
      MINIMAL_REQUEST,
    );
    expect(result.blocked).toBe(true);
    expect(result.error).toContain("too short");
  }, KERNEL_TIMEOUT);

  it("refuses a blocked job that came back carrying G-code anyway", () => {
    // If this ever fires the kernel handed an export path a program it had
    // just refused. Catching it here is cheaper than catching it at the spindle.
    expect(() =>
      camJob(
        stub({
          blocked: true,
          gcode: "G21\nM2\n",
          policy: { verified: true, blocked_by: ["gouge"], warnings: [] },
          notes: [],
        }),
        MINIMAL_REQUEST,
      ),
    ).toThrow(/blocked but carried G-code/);
  }, KERNEL_TIMEOUT);

  it("refuses an answer with no verdict at all", () => {
    expect(() => camJob(stub({ gcode: "G21" }), MINIMAL_REQUEST)).toThrow(
      /no .blocked. verdict/,
    );
  });
});

describe("camOutlineFromDocument: a torn section is a result", () => {
  it("hands back the gaps rather than a sentence that loses them", () => {
    const outline = camOutlineFromDocument(
      stub({
        error: "the section at z 3.0000 does not close: 2 gap(s), the widest 0.4100 mm.",
        mesh_source: "export_mesh",
        z: 3,
        gaps: [{ distance: 0.41 }, { distance: 0.12 }],
      }),
      createDocument(),
      0,
      "auto",
    );
    expect(outline.torn).toBe(true);
    if (!outline.torn) throw new Error("narrowing is broken");
    expect(outline.gaps.map((g) => g.distance)).toEqual([0.41, 0.12]);
    expect(outline.z).toBe(3);
  }, KERNEL_TIMEOUT);

  it("a section that failed for any other reason throws", () => {
    expect(() =>
      camOutlineFromDocument(
        stub({ error: "this document has 0 part(s), so there is no part 0." }),
        createDocument(),
        0,
        "auto",
      ),
    ).toThrow(CamError);
  });
});

// ---------------------------------------------------------------------------
// The real kernel, on a real part
// ---------------------------------------------------------------------------

/* eslint-disable @typescript-eslint/no-explicit-any */
let host: CamHost;

beforeAll(async () => {
  const wasm = (await getKernelWasm()) as any;
  host = {
    camJob: (r) => JSON.parse(wasm.camJob(JSON.stringify(r))),
    camVerifyGcode: (r) => JSON.parse(wasm.camVerifyGcode(JSON.stringify(r))),
    camFit: (r) => JSON.parse(wasm.camFit(JSON.stringify(r))),
    camMaterials: () => JSON.parse(wasm.camMaterials()),
    camRecommendFeeds: (r) => JSON.parse(wasm.camRecommendFeeds(JSON.stringify(r))),
    camCheckFeeds: (r) => JSON.parse(wasm.camCheckFeeds(JSON.stringify(r))),
    camGear: (r) => JSON.parse(wasm.camGear(JSON.stringify(r))),
    camOutlineFromDocument: (d, i, z, a, o) =>
      JSON.parse(
        wasm.camOutlineFromDocument(JSON.stringify(d), i, z, a, JSON.stringify(o ?? {})),
      ),
  };
});

const node = (id: number, op: Node["op"]): Node => ({ id, name: `n${id}`, op });

/**
 * An 80×50×6 plate with a Ø16 bore and two Ø5 holes, authored as a chain of
 * differences — the shape that used to trap the kernel on wasm32 and lose its
 * B-rep, which is exactly the B-rep the sectioner needs.
 */
function plateDoc(): Document {
  const doc = createDocument();
  const nodes: Node[] = [
    node(1, { type: "Cube", size: { x: 80, y: 50, z: 6 } }),
    node(2, { type: "Cylinder", radius: 8, height: 8, segments: 96 }),
    node(3, { type: "Translate", child: 2, offset: { x: 40, y: 25, z: -1 } }),
    node(4, { type: "Cylinder", radius: 2.5, height: 8, segments: 64 }),
    node(5, { type: "Translate", child: 4, offset: { x: 12, y: 12, z: -1 } }),
    node(6, { type: "Cylinder", radius: 2.5, height: 8, segments: 64 }),
    node(7, { type: "Translate", child: 6, offset: { x: 68, y: 38, z: -1 } }),
    node(8, { type: "Difference", left: 1, right: 3 }),
    node(9, { type: "Difference", left: 8, right: 5 }),
    node(10, { type: "Difference", left: 9, right: 7 }),
  ] as Node[];
  for (const n of nodes) doc.nodes[String(n.id)] = n;
  doc.roots.push({ root: 10, material: "default" });
  return doc;
}

/** Ø3.175: the cutter most hobby routers ship with, and the one that makes
 *  the Ø5 holes a helical bore (5 < 2 × 3.175) and the Ø16 an opening. */
const TOOL_DIAMETER = 3.175;

const CUTTING = {
  stepdown: 0.5,
  stepover: 1.2,
  feed: 400,
  plunge: 100,
  rpm: 10000,
  bottom_allowance: 0,
} as const;

function plateJob(thickness: number): CamJobRequest {
  const outline = camOutlineFromDocument(host, plateDoc(), 0, "auto");
  if (outline.torn) throw new Error("the plate sectioned torn");
  const region = outline.regions[0];
  return {
    name: "plate",
    stock: { thickness, margin: 8 },
    machine: {
      name: "Anolex Ultra 2",
      spindle: "dial",
      class: "hobby",
      max_feed: 3000,
      max_accel: 300,
    },
    tools: [
      {
        number: 1,
        kind: "flat_end_mill",
        diameter: TOOL_DIAMETER,
        flutes: 2,
        flute_length: 12,
        stickout: 20,
        centre_cutting: true,
      },
    ],
    operations: [
      // The two Ø5 holes: wider than the cutter, narrower than two of it.
      ...([
        [12, 12],
        [68, 38],
      ] as Array<[number, number]>).map(([x, y], i) => ({
        name: `Pilot Ø5 · ${i + 1}`,
        tool: 1,
        kind: "helical_bore" as const,
        x,
        y,
        diameter: 5,
        pitch: 0.5,
        depth: 6,
        order: i,
        ...CUTTING,
      })),
      {
        name: "Bore Ø16",
        tool: 1,
        kind: "contour_inside",
        contour: region.holes[0] as Array<[number, number]>,
        depth: 6,
        direction: "climb",
        entry: "ramp",
        ramp_angle: 3,
        lead_in: true,
        thin_slot: { strategy: "refuse" },
        order: 2,
        ...CUTTING,
      },
      {
        name: "Outside profile",
        tool: 1,
        kind: "contour_outside",
        contour: region.outer as Array<[number, number]>,
        depth: 6,
        direction: "climb",
        entry: "ramp",
        ramp_angle: 3,
        lead_in: true,
        thin_slot: { strategy: "refuse" },
        tabs: 3,
        tab_width: 4,
        tab_height: 1,
        order: 3,
        ...CUTTING,
      },
    ],
    options: {
      arc_fit: { tolerance: 0.005 },
      tool_change: { type: "manual_pause_reprobe" },
      spin_up_seconds: 3,
      park_z: 5,
      safe_z: 5,
      wcs: "G54",
      verify: true,
      part: {
        outer: region.outer as Array<[number, number]>,
        holes: region.holes as Array<Array<[number, number]>>,
      },
    },
  };
}

describe("the plate, through the real kernel", () => {
  it("sections to one boundary, three openings and the circles they are", () => {
    const outline = camOutlineFromDocument(host, plateDoc(), 0, "auto");
    expect(outline.torn).toBe(false);
    if (outline.torn) return;

    // The B-rep survived the chain, so the *raw* tessellation was sectioned —
    // the repaired export mesh can tear a tangent fillet by 0.4 mm, which on
    // this part would be a quarter of a slot mouth.
    expect(outline.mesh_source).toBe("raw_tessellation");
    // Auto Z is the mid-height of a 6 mm plate.
    expect(outline.z).toBeCloseTo(3, 6);
    expect(outline.suggested_stock_thickness).toBeCloseTo(6, 3);
    expect(outline.regions).toHaveLength(1);
    expect(outline.regions[0].holes).toHaveLength(3);
    expect(outline.bounds).toEqual([0, 0, 80, 50]);

    const diameters = outline.circles.map((c) => c.diameter).sort((a, b) => a - b);
    expect(diameters).toHaveLength(3);
    expect(diameters[0]).toBeCloseTo(5, 2);
    expect(diameters[1]).toBeCloseTo(5, 2);
    expect(diameters[2]).toBeCloseTo(16, 2);

    // Each Ø5 hole is wider than the cutter and narrower than two of it, which
    // is what makes it a helical bore rather than an opening.
    expect(diameters[0]).toBeGreaterThan(TOOL_DIAMETER);
    expect(diameters[0]).toBeLessThan(2 * TOOL_DIAMETER);
    expect(diameters[2]).toBeGreaterThan(2 * TOOL_DIAMETER);
  }, KERNEL_TIMEOUT);

  it("builds a job that verifies, with a program to export", () => {
    const request = plateJob(6);
    // Two helical bores, because two Ø5 holes.
    expect(request.operations.filter((o) => o.kind === "helical_bore")).toHaveLength(2);

    const job = camJob(host, request);
    expect(job.policy.blocked_by).toEqual([]);
    expect(job.blocked).toBe(false);
    if (job.blocked) return;

    expect(job.gcode.length).toBeGreaterThan(0);
    // The bores are in the program by name, and the profile runs last.
    expect(job.gcode).toContain("Pilot");
    expect(job.policy.verified).toBe(true);
    expect(job.policy.replayed).toBe("gcode");

    // The preview the panel draws: the ranges partition the moves, and every
    // operation has one.
    const ranges = (job.op_ranges ?? []).filter((r) => r.block === "operation");
    expect(ranges).toHaveLength(request.operations.length);
    expect(ranges[0].start).toBeGreaterThanOrEqual(0);
    for (const r of ranges) expect(r.end).toBeGreaterThan(r.start);
    expect((job.moves ?? []).length).toBeGreaterThan(ranges[ranges.length - 1].end - 1);
    // Both kinds of move are there — a preview that drew only cuts would hide
    // exactly the moves that must not touch metal.
    expect((job.moves ?? []).some((m) => m.rapid)).toBe(true);
    expect((job.moves ?? []).some((m) => !m.rapid)).toBe(true);

    // The cut floor is the underside of a 6 mm plate, not "somewhere below 0".
    expect(job.verification?.depth.deepest_z).toBeCloseTo(-6, 3);
    expect(job.verification?.depth.remaining_under_part).toBeCloseTo(0, 3);
    expect(job.verification?.gouge.pass).toBe(true);
    expect(job.verification?.material_left.check.pass).toBe(true);

    // Eight checks, reached the same way whether the kernel nests them or not.
    expect(camChecks(job.verification).map((c) => c.name)).toEqual([
      "gouge",
      "material_left",
      "rapids",
      "depth",
      "tabs",
      "envelope",
      "loose_pieces",
      "plunges",
    ]);

    // The Ø16 slug has nothing holding it: a warning, not a blocker, and the
    // one the panel makes the operator acknowledge before exporting.
    expect(job.policy.warnings).toContain("loose_pieces");
  }, KERNEL_TIMEOUT);

  it("refuses the same job on stock thinner than the cut, by name and in millimetres", () => {
    // 6 mm of cut into 3 mm of stock with no sacrificial board under it: the
    // cutter would finish in the machine bed.
    const job = camJob(host, plateJob(3));

    expect(job.blocked).toBe(true);
    expect("gcode" in job).toBe(false);
    expect(job.policy.blocked_by).toContain("depth");

    const depth = job.verification?.depth;
    expect(depth?.remaining_under_part).toBeCloseTo(-3, 3);
    expect(depth?.floor_z).toBeCloseTo(-3, 3);
    // The blocker says what happened, how far, and that the fix is a spoilboard.
    const worst = depth?.check.examples[0];
    expect(String(worst?.what)).toMatch(/past the stock underside/);
    expect(String(worst?.what)).toMatch(/spoilboard/);
    expect(depth?.check.worst).toBeCloseTo(3, 3);
  }, KERNEL_TIMEOUT);

  it("the same cut into a declared spoilboard is not refused for depth", () => {
    // The control for the test above: it is the *missing board* that blocks,
    // not the thin stock — otherwise the refusal would be measuring the wrong
    // thing and would still fire once the board was declared.
    const request = plateJob(3);
    request.stock.spoilboard = 12;
    for (const op of request.operations) op.bottom_allowance = -0.2;
    request.operations = request.operations.map((op) =>
      op.kind === "helical_bore" || op.kind === "contour_outside" || op.kind === "contour_inside"
        ? { ...op, depth: 3 }
        : op,
    );
    const job = camJob(host, request);
    expect(job.policy.blocked_by).not.toContain("depth");
  }, KERNEL_TIMEOUT);
});
