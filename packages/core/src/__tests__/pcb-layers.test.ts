import { describe, it, expect } from "vitest";
import type { Pcb } from "@vcad/ir";
import {
  assertSupportedCopperCount,
  boardCopperLayers,
  copperLayerNames,
  isCopperLayer,
  MAX_COPPER_LAYERS,
} from "../utils/pcb-layers.js";

describe("copperLayerNames", () => {
  it("2-layer", () => {
    expect(copperLayerNames(2)).toEqual(["FCu", "BCu"]);
  });
  it("4-layer", () => {
    expect(copperLayerNames(4)).toEqual(["FCu", "In1Cu", "In2Cu", "BCu"]);
  });
  it("10-layer names every inner layer (no undefined above 8)", () => {
    const names = copperLayerNames(10);
    expect(names).toHaveLength(10);
    expect(names.every((n) => typeof n === "string" && isCopperLayer(n))).toBe(true);
    expect(names.slice(-3)).toEqual(["In7Cu", "In8Cu", "BCu"]);
  });
});

describe("assertSupportedCopperCount", () => {
  it("accepts 1, 2, 4, 10", () => {
    for (const n of [1, 2, 4, 10]) expect(() => assertSupportedCopperCount(n)).not.toThrow();
  });
  it("rejects odd, zero, fractional and over-limit counts", () => {
    for (const n of [0, 3, 5, 2.5, MAX_COPPER_LAYERS + 2]) {
      expect(() => assertSupportedCopperCount(n)).toThrow(/unsupported PCB layer count/);
    }
  });
});

describe("boardCopperLayers", () => {
  const stack = (n: number): Pick<Pcb, "stackup"> => ({
    stackup: { layers: [...copperLayerNames(n).map((layer) => ({ layer })), { layer: "FSilkS" }] },
  });
  it("derives 2/4/10-layer sets from the stackup, ordered top→bottom", () => {
    expect(boardCopperLayers(stack(2))).toEqual(["FCu", "BCu"]);
    expect(boardCopperLayers(stack(4))).toEqual(copperLayerNames(4));
    expect(boardCopperLayers(stack(10))).toEqual(copperLayerNames(10));
  });
  it("picks up copper referenced by traces but missing from the stackup", () => {
    const pcb = {
      ...stack(2),
      traces: [{ layer: "In7Cu" }],
    } as unknown as Pick<Pcb, "stackup" | "traces">;
    expect(boardCopperLayers(pcb)).toEqual(["FCu", "In7Cu", "BCu"]);
  });
  it("falls back to FCu/BCu for an empty board", () => {
    expect(boardCopperLayers(null)).toEqual(["FCu", "BCu"]);
  });
});
