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
