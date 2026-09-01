import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { measure } from "../tools/measure.js";
import { dispatchRegistryTool } from "../tools/registry-dispatch.js";
import { documents, getSession, openDocument } from "../tools/session.js";

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
 * Two 10 mm cubes with a known analytic gap along X: cube A occupies
 * [0,10]³, cube B is translated so its near face sits `gap` mm past A's far
 * face. `gap` 10 → the faces are 10 mm apart.
 */
function twoCubesDocument(gap: number): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  const cubeA = add("cube-a", { type: "Cube", size: { x: 10, y: 10, z: 10 } });
  const cubeBSolid = add("cube-b-solid", {
    type: "Cube",
    size: { x: 10, y: 10, z: 10 },
  });
  const cubeB = add("cube-b", {
    type: "Translate",
    child: cubeBSolid,
    offset: { x: 10 + gap, y: 0, z: 0 },
  });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [
      { root: cubeA, material: "aluminum" },
      { root: cubeB, material: "steel" },
    ],
  } as unknown as Document;
}

function openTwoCubes(gap: number): string {
  const opened = out(openDocument({ initial: twoCubesDocument(gap) }));
  return opened.document_id as string;
}

describe("measure", () => {
  it("reports a known 10 mm gap between two cubes with per-part bboxes", () => {
    const docId = openTwoCubes(10);
    const res = out(
      measure({ document_id: docId, part_ids: ["cube-a", "cube-b"] }, engine),
    );
    expect(res.mode).toBe("pair");
    expect(Math.abs(res.distance_mm - 10)).toBeLessThan(0.02);
    expect(res.contact).toBe(false);
    expect(res.intersecting).toBe(false);
    // Cube A spans [0,10] on X; cube B starts at x=20.
    expect(res.parts.a.bbox.min.x).toBeCloseTo(0, 3);
    expect(res.parts.a.bbox.max.x).toBeCloseTo(10, 3);
    expect(res.parts.b.bbox.min.x).toBeCloseTo(20, 3);
    expect(res.parts.b.bbox.size.x).toBeCloseTo(10, 3);
  });

  it("reports contact/overlap with a negative distance when cubes interpenetrate", () => {
    const docId = openTwoCubes(-4); // cube B pushed 4 mm into cube A
    const res = out(
      measure({ document_id: docId, part_ids: ["cube-a", "cube-b"] }, engine),
    );
    expect(res.mode).toBe("pair");
    expect(res.contact).toBe(true);
    expect(res.intersecting).toBe(true);
    expect(res.distance_mm).toBeLessThanOrEqual(0);
  });

  it("returns bbox, volume, and center of mass for a single part id", () => {
    const docId = openTwoCubes(10);
    const res = out(measure({ document_id: docId, part_ids: ["cube-a"] }, engine));
    expect(res.mode).toBe("part");
    expect(res.part.bbox.size).toMatchObject({ x: 10, y: 10, z: 10 });
    expect(Math.abs(res.part.volume_mm3 - 1000)).toBeLessThan(1);
    expect(res.part.center_of_mass.x).toBeCloseTo(5, 2);
    expect(res.part.center_of_mass.z).toBeCloseTo(5, 2);
  });

  it("resolves parts by id as well as name", () => {
    const docId = openTwoCubes(10);
    const doc = getSession(docId);
    const [aRoot, bRoot] = doc.roots.map((r) => String(r.root));
    const res = out(
      measure({ document_id: docId, part_ids: [aRoot, bRoot] }, engine),
    );
    expect(res.parts.a.id).toBe(aRoot);
    expect(res.parts.b.id).toBe(bRoot);
  });

  it("errors with a recovery hint listing available parts", () => {
    const docId = openTwoCubes(10);
    const res = measure(
      { document_id: docId, part_ids: ["ghost", "cube-b"] },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("ghost");
    expect(res.content[0].text).toContain("cube-a");
  });

  it("rejects the same part given twice", () => {
    const docId = openTwoCubes(10);
    const res = measure(
      { document_id: docId, part_ids: ["cube-a", "cube-a"] },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("distinct");
  });

  it("rejects an empty or oversized part_ids list", () => {
    const docId = openTwoCubes(10);
    expect(measure({ document_id: docId, part_ids: [] }, engine).isError).toBe(true);
    expect(
      measure({ document_id: docId, part_ids: ["a", "b", "c"] }, engine).isError,
    ).toBe(true);
  });
});

describe("inspect_part / describe_scene (dispatched over the registry surface)", () => {
  it("inspect_part returns world-space bbox, volume, com, material, and anchors", () => {
    const docId = openTwoCubes(10);
    const res = out(
      dispatchRegistryTool(
        "inspect_part",
        { document_id: docId, part_id: "cube-b" },
        engine,
      ),
    );
    expect(res.name).toBe("cube-b");
    expect(res.material).toBe("steel");
    expect(res.bbox.min.x).toBeCloseTo(20, 3);
    expect(res.bbox.max.x).toBeCloseTo(30, 3);
    expect(Math.abs(res.volume_mm3 - 1000)).toBeLessThan(1);
    // Anchors match the app's place semantics.
    expect(res.anchors.center.x).toBeCloseTo(25, 3);
    expect(res.anchors.top.z).toBeCloseTo(10, 3);
    expect(res.anchors.right.x).toBeCloseTo(30, 3);
  });

  it("inspect_part errors on an unknown part, listing what is available", () => {
    const docId = openTwoCubes(10);
    expect(() =>
      dispatchRegistryTool(
        "inspect_part",
        { document_id: docId, part_id: "nope" },
        engine,
      ),
    ).toThrow(/nope/);
  });

  it("describe_scene snapshots every part in one call", () => {
    const docId = openTwoCubes(10);
    const res = out(
      dispatchRegistryTool("describe_scene", { document_id: docId }, engine),
    );
    expect(res.part_count).toBe(2);
    const names = res.parts.map((p: { name: string }) => p.name).sort();
    expect(names).toEqual(["cube-a", "cube-b"]);
    expect(res.parts[0].bbox).toBeDefined();
  });

  it("describe_scene scopes to requested part_ids and reports missing ones", () => {
    const docId = openTwoCubes(10);
    const res = out(
      dispatchRegistryTool(
        "describe_scene",
        { document_id: docId, part_ids: ["cube-a", "ghost"] },
        engine,
      ),
    );
    expect(res.part_count).toBe(1);
    expect(res.parts[0].name).toBe("cube-a");
    expect(res.missing).toEqual(["ghost"]);
  });

  it("both tools are dispatchable (removed from DEFERRED_TOOLS)", () => {
    const docId = openTwoCubes(10);
    // Would throw "not dispatchable" if still deferred.
    expect(() =>
      dispatchRegistryTool(
        "inspect_part",
        { document_id: docId, part_id: "cube-a" },
        engine,
      ),
    ).not.toThrow(/not dispatchable/);
    expect(() =>
      dispatchRegistryTool("describe_scene", { document_id: docId }, engine),
    ).not.toThrow(/not dispatchable/);
  });
});

/**
 * A crank arm swinging about the origin past a fixed post — the four-bar-knee
 * trap, minimized. Authored at 90° the arm points away from the post and
 * clears by a wide margin; at 0° it swings straight through it. Measuring the
 * authored pose is exactly the mistake: the authored pose is the pose that
 * works.
 */
function swingArmDocument(limits?: [number, number]): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  const arm = add("arm-solid", { type: "Cube", size: { x: 30, y: 4, z: 4 } });
  const postSolid = add("post-solid", { type: "Cube", size: { x: 6, y: 6, z: 10 } });
  const post = add("post", {
    type: "Translate",
    child: postSolid,
    offset: { x: 20, y: -3, z: -3 },
  });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [],
    partDefs: {
      arm: { id: "arm", name: "arm", root: arm },
      post: { id: "post", name: "post", root: post },
    },
    instances: [
      { id: "post-1", partDefId: "post", name: "post" },
      { id: "arm-1", partDefId: "arm", name: "arm" },
    ],
    joints: [
      {
        id: "shoulder",
        name: "shoulder",
        parentInstanceId: null,
        childInstanceId: "arm-1",
        parentAnchor: { x: 0, y: 0, z: 0 },
        childAnchor: { x: 0, y: 0, z: 0 },
        kind: {
          type: "Revolute",
          axis: { x: 0, y: 0, z: 1 },
          ...(limits ? { limits } : {}),
        },
        state: 90, // authored at the one angle that clears
      },
    ],
  } as unknown as Document;
}

function openSwingArm(limits?: [number, number]): string {
  return out(openDocument({ initial: swingArmDocument(limits) })).document_id as string;
}

describe("measure joint sweep", () => {
  it("measures the authored pose when unswept", () => {
    const docId = openSwingArm();
    const res = out(
      measure({ document_id: docId, part_ids: ["arm-1", "post-1"] }, engine),
    );
    expect(res.mode).toBe("pair");
    expect(res.intersecting).toBe(false);
    expect(res.poses_checked).toBeUndefined();
    expect(res.samples).toBeUndefined();
  });

  it("finds the collision the authored pose hides, and names the pose", () => {
    const docId = openSwingArm();
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9 }],
        },
        engine,
      ),
    );
    expect(res.mode).toBe("sweep");
    expect(res.intersecting).toBe(true);
    expect(res.distance_mm).toBeLessThanOrEqual(0);
    expect(res.poses_checked).toBe(10);
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 0 }]);
    expect(res.sweep).toEqual([{ joint: "shoulder", from: 0, to: 90, steps: 9 }]);
  });

  it("returns the margin curve, minimal at the known worst angle", () => {
    const docId = openSwingArm();
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          // 30°–90°: the arm never reaches the post, and the gap grows
          // monotonically with the angle, so the minimum is the 30° endpoint.
          sweep: [{ joint: "shoulder", from: 30, to: 90, steps: 6 }],
        },
        engine,
      ),
    );
    expect(res.intersecting).toBe(false);
    expect(res.samples).toHaveLength(7);
    expect(res.samples[0].pose).toEqual([{ joint: "shoulder", state: 30 }]);
    expect(res.samples[6].pose).toEqual([{ joint: "shoulder", state: 90 }]);
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 30 }]);
    expect(res.distance_mm).toBeCloseTo(res.samples[0].distance_mm, 6);
    const gaps = res.samples.map((s: { distance_mm: number }) => s.distance_mm);
    expect(Math.min(...gaps)).toBeCloseTo(res.distance_mm, 6);
    for (let i = 1; i < gaps.length; i++) expect(gaps[i]).toBeGreaterThan(gaps[i - 1]);
  });

  it("omits samples above the 256-pose cap but still reports the worst pose", () => {
    const docId = openSwingArm();
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          sweep: [{ joint: "shoulder", from: 30, to: 90, steps: 300 }],
        },
        engine,
      ),
    );
    expect(res.poses_checked).toBe(301);
    expect(res.samples).toBeUndefined();
    expect(res.samples_omitted).toBe(true);
    expect(res.samples_note).toContain("256");
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 30 }]);
  });

  it("restores joint states — a sweep asks a question, it does not pose the model", () => {
    const docId = openSwingArm();
    measure(
      {
        document_id: docId,
        part_ids: ["arm-1", "post-1"],
        sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 4 }],
      },
      engine,
    );
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("refuses a sweep with one part id instead of guessing what to measure", () => {
    const docId = openSwingArm();
    const res = measure(
      {
        document_id: docId,
        part_ids: ["arm-1"],
        sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 4 }],
      },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("TWO parts");
  });

  it("rejects an unknown joint and an oversized grid", () => {
    const docId = openSwingArm();
    const unknown = measure(
      {
        document_id: docId,
        part_ids: ["arm-1", "post-1"],
        sweep: [{ joint: "elbow", from: 0, to: 90, steps: 4 }],
      },
      engine,
    );
    expect(unknown.isError).toBe(true);

    const huge = measure(
      {
        document_id: docId,
        part_ids: ["arm-1", "post-1"],
        sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9999 }],
      },
      engine,
    );
    expect(huge.isError).toBe(true);
  });

  it("warns — without clamping — when the sweep exceeds the joint's limits", () => {
    const docId = openSwingArm([30, 90]);
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          sweep: [{ joint: "shoulder", from: 0, to: 90, steps: 9 }],
        },
        engine,
      ),
    );
    expect(res.sweep_warnings).toHaveLength(1);
    expect(res.sweep_warnings[0]).toContain("[30, 90]");
    // Not clamped: the out-of-limits pose was still measured and still won.
    expect(res.worst_pose).toEqual([{ joint: "shoulder", state: 0 }]);
    expect(res.intersecting).toBe(true);
  });
});

describe("measure joint_state", () => {
  it("measures at the requested pose without touching the session", () => {
    const docId = openSwingArm();
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          joint_state: { shoulder: 0 },
        },
        engine,
      ),
    );
    expect(res.mode).toBe("pair");
    expect(res.intersecting).toBe(true);
    expect(res.pose.applied.shoulder).toBe(0);
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("lets a sweep override the base pose on the joints it drives", () => {
    const docId = openSwingArm();
    const res = out(
      measure(
        {
          document_id: docId,
          part_ids: ["arm-1", "post-1"],
          joint_state: { shoulder: 0 },
          sweep: [{ joint: "shoulder", from: 60, to: 90, steps: 3 }],
        },
        engine,
      ),
    );
    // The base pose collides; the swept 60°–90° arc does not, and the sweep
    // wins on the joint it drives.
    expect(res.intersecting).toBe(false);
    expect(res.poses_checked).toBe(4);
    expect(res.pose.applied.shoulder).toBe(0);
    expect(getSession(docId).joints?.[0].state).toBe(90);
  });

  it("errors on an unknown joint key", () => {
    const docId = openSwingArm();
    const res = measure(
      { document_id: docId, part_ids: ["arm-1", "post-1"], joint_state: { elbow: 10 } },
      engine,
    );
    expect(res.isError).toBe(true);
  });
});
