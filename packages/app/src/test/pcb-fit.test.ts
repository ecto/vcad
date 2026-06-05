import { describe, it, expect } from "vitest";
import type { PcbBoardTransform } from "@vcad/core";
import { computeBoardFit } from "@/lib/pcb-fit";
import type { Aabb } from "@/lib/pcb-interference";

const xf = (over: Partial<PcbBoardTransform> = {}): PcbBoardTransform => ({
  position: { x: 0, y: 0, z: 0 },
  rotationDeg: { x: 0, y: 0, z: 0 },
  scale: { x: 1, y: 1, z: 1 },
  ...over,
});

const box = (min: [number, number, number], max: [number, number, number]): Aabb => ({ min, max });

describe("computeBoardFit", () => {
  it("identity board: fits to the inset enclosure box", () => {
    const fit = computeBoardFit(box([0, 0, 0], [20, 20, 20]), xf(), 2)!;
    expect(fit.width).toBeCloseTo(16);
    expect(fit.height).toBeCloseTo(16);
    expect(fit.position.x).toBeCloseTo(2);
    expect(fit.position.y).toBeCloseTo(2);
  });

  it("preserves the board's Z while fitting XY", () => {
    const fit = computeBoardFit(box([0, 0, 0], [20, 20, 4]), xf({ position: { x: 9, y: 9, z: 7 } }), 0)!;
    expect(fit.position.z).toBeCloseTo(7);
  });

  it("90° Z-rotated board: swaps W/H and places to cover the enclosure", () => {
    const enc = box([0, 0, 0], [80, 40, 10]);
    const fit = computeBoardFit(enc, xf({ rotationDeg: { x: 0, y: 0, z: 90 }, position: { x: 0, y: 0, z: 5 } }), 0)!;
    expect(fit.width).toBeCloseTo(40);
    expect(fit.height).toBeCloseTo(80);
    // Forward-mapping the outline by (T, R) must reproduce the enclosure bbox.
    expect(fit.position.x).toBeCloseTo(80);
    expect(fit.position.y).toBeCloseTo(0);
    expect(fit.position.z).toBeCloseTo(5);
  });

  it("scaled board: outline is pre-scale (world footprint still matches)", () => {
    const fit = computeBoardFit(box([0, 0, 0], [20, 20, 5]), xf({ scale: { x: 2, y: 2, z: 1 } }), 0)!;
    expect(fit.width).toBeCloseTo(10); // 20 / 2
    expect(fit.height).toBeCloseTo(10);
  });

  it("returns null when the inset target collapses", () => {
    expect(computeBoardFit(box([0, 0, 0], [3, 3, 3]), xf(), 2)).toBeNull();
  });
});
