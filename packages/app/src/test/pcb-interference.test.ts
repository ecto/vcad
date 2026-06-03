import { describe, it, expect } from "vitest";
import type { ComponentMesh } from "@vcad/engine";
import { aabbOfPositions, aabbsOverlap, interferingRefs } from "@/lib/pcb-interference";

/** Unit cube [0,0,0]..[s,s,s] translated by (tx,ty,tz) as a flat position buffer. */
function boxPositions(s: number, tx = 0, ty = 0, tz = 0): number[] {
  const c = [
    [0, 0, 0], [s, 0, 0], [s, s, 0], [0, s, 0],
    [0, 0, s], [s, 0, s], [s, s, s], [0, s, s],
  ];
  return c.flatMap(([x, y, z]) => [x! + tx, y! + ty, z! + tz]);
}

function comp(ref: string, positions: number[]): ComponentMesh {
  return { footprint_ref: ref, positions, indices: [], normals: [], color: [0, 0, 0], metalness: 0 };
}

describe("pcb-interference", () => {
  it("computes an AABB from a position buffer", () => {
    const bb = aabbOfPositions(boxPositions(2, 1, 1, 1));
    expect(bb).toEqual({ min: [1, 1, 1], max: [3, 3, 3] });
  });

  it("returns null for empty/short buffers", () => {
    expect(aabbOfPositions([])).toBeNull();
    expect(aabbOfPositions(undefined)).toBeNull();
  });

  it("detects overlap and separation", () => {
    const a = { min: [0, 0, 0] as [number, number, number], max: [10, 10, 10] as [number, number, number] };
    const near = { min: [9, 9, 9] as [number, number, number], max: [12, 12, 12] as [number, number, number] };
    const far = { min: [20, 20, 20] as [number, number, number], max: [25, 25, 25] as [number, number, number] };
    expect(aabbsOverlap(a, near)).toBe(true);
    expect(aabbsOverlap(a, far)).toBe(false);
    // margin expands the test so near-misses flag
    expect(aabbsOverlap(a, { min: [11, 0, 0], max: [12, 5, 5] }, 1.5)).toBe(true);
  });

  it("flags only components overlapping a mechanical AABB (no false positives)", () => {
    const mech = [aabbOfPositions(boxPositions(10, 0, 0, 0))!]; // [0,10]^3
    const inside = comp("R1", boxPositions(2, 1, 1, 1)); // overlaps
    const outside = comp("C1", boxPositions(2, 50, 50, 50)); // clear
    expect(interferingRefs([inside, outside], mech)).toEqual(["R1"]);
  });

  it("returns no interference when there are no mechanical parts", () => {
    const c = comp("R1", boxPositions(2, 1, 1, 1));
    expect(interferingRefs([c], [])).toEqual([]);
  });

  it("de-duplicates refs when a footprint yields several sub-meshes", () => {
    const mech = [aabbOfPositions(boxPositions(10))!];
    // Same ref appears 3x (body + 2 caps), all overlapping.
    const parts = [
      comp("R1", boxPositions(1, 1, 1, 1)),
      comp("R1", boxPositions(1, 2, 2, 2)),
      comp("R1", boxPositions(1, 3, 3, 3)),
    ];
    expect(interferingRefs(parts, mech)).toEqual(["R1"]);
  });
});
