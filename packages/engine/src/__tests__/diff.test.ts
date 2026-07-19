import { describe, expect, it } from "vitest";
import type { Document } from "@vcad/ir";
import { semanticDiffFallback } from "../diff.js";

function cubeDoc(size: number): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "base",
        op: { type: "Cube", size: { x: size, y: size, z: size } },
      },
    },
    roots: [{ root: 1, material: "aluminum" }],
    materials: {},
    part_materials: {},
  } as unknown as Document;
}

describe("semanticDiffFallback", () => {
  it("returns an empty diff for identical documents", () => {
    expect(semanticDiffFallback(cubeDoc(10), cubeDoc(10)).changes).toEqual([]);
  });

  it("reports field-level old→new changes on modified entities", () => {
    const diff = semanticDiffFallback(cubeDoc(10), cubeDoc(25));
    expect(diff.changes).toHaveLength(1);
    const change = diff.changes[0]!;
    expect(change.kind).toBe("node");
    expect(change.id).toBe("1");
    expect(change.name).toBe("base");
    if (change.type !== "modified") throw new Error("expected modified");
    expect(change.fields).toHaveLength(3);
    for (const f of change.fields) {
      expect(f.path.startsWith("op.size.")).toBe(true);
      expect(f.old).toBe(10);
      expect(f.new).toBe(25);
    }
  });

  it("reports added and removed entities by stable id", () => {
    const a = cubeDoc(10);
    const b = cubeDoc(10);
    (b.nodes as Record<string, unknown>)["2"] = {
      id: 2,
      name: "cyl",
      op: { type: "Cylinder", radius: 5, height: 30, segments: 0 },
    };
    b.roots.push({ root: 2, material: "steel" } as (typeof b.roots)[number]);

    const forward = semanticDiffFallback(a, b);
    expect(forward.changes).toHaveLength(2); // node 2 + root 2
    expect(forward.changes.every((c) => c.type === "added")).toBe(true);

    const reverse = semanticDiffFallback(b, a);
    expect(reverse.changes.every((c) => c.type === "removed")).toBe(true);
  });

  it("ignores root reordering (identity is the id, not the index)", () => {
    const a = cubeDoc(10);
    (a.nodes as Record<string, unknown>)["2"] = {
      id: 2,
      op: { type: "Cylinder", radius: 5, height: 30, segments: 0 },
    };
    a.roots.push({ root: 2, material: "steel" } as (typeof a.roots)[number]);
    const b = JSON.parse(JSON.stringify(a)) as Document;
    b.roots.reverse();
    expect(semanticDiffFallback(a, b).changes).toEqual([]);
  });
});
