import { describe, it, expect } from "vitest";
import { copperLayerNames } from "@vcad/core";
import { layerZOffset } from "@/components/electronics/pcb3d/pcb-geometry";

describe("copper Z offsets", () => {
  for (const n of [2, 4, 10]) {
    it(`${n}-layer copper is strictly ordered top→bottom with distinct Z`, () => {
      const z = copperLayerNames(n).map((l) => layerZOffset(l, 0, n));
      for (let i = 1; i < z.length; i++) expect(z[i]!).toBeLessThan(z[i - 1]!);
      expect(z[0]).toBeCloseTo(0.8);
      expect(z[z.length - 1]).toBeCloseTo(-0.8);
    });
  }
  it("without a count, In7/In8 don't collapse onto BCu", () => {
    expect(layerZOffset("In8Cu")).toBeGreaterThan(layerZOffset("BCu"));
    expect(layerZOffset("In6Cu")).toBeCloseTo(-0.4); // historical spacing kept
  });
});
