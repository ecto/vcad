import { describe, it, expect } from "vitest";
import { copperLayerNames } from "@vcad/core";
import type { Pcb } from "@vcad/ir";
import {
  copperLayerColor,
  layerConfigFor,
  resolveLayerConfigs,
} from "@/lib/pcb-layer-config";
import { useElectronicsStore } from "@/stores/electronics-store";
import { getLayerColor, isLayerVisible } from "@/components/electronics/pcb3d/pcb-geometry";

const board = (n: number) =>
  ({ stackup: { layers: copperLayerNames(n).map((layer) => ({ layer })) } }) as Pick<Pcb, "stackup">;

const seeded = () => useElectronicsStore.getState().pcbLayers;

describe("resolveLayerConfigs", () => {
  for (const n of [2, 4, 10]) {
    it(`${n}-layer board gets one config per copper layer, top→bottom`, () => {
      const cfgs = resolveLayerConfigs(seeded(), board(n));
      const copper = cfgs.filter((c) => c.layer.endsWith("Cu"));
      expect(copper.map((c) => c.layer)).toEqual(copperLayerNames(n));
      // non-copper layers are kept
      expect(cfgs.some((c) => c.layer === "FSilkS")).toBe(true);
    });
  }

  it("10-layer: inner layers beyond the seeded set are visible with distinct colours", () => {
    const cfgs = resolveLayerConfigs(seeded(), board(10));
    const copper = cfgs.filter((c) => c.layer.endsWith("Cu"));
    for (const c of copper.filter((c) => !["In1Cu", "In2Cu"].includes(c.layer))) {
      expect(c.visible).toBe(true);
    }
    const colors = new Set(copper.map((c) => c.color));
    expect(colors.size).toBe(copper.length);
    expect([...colors]).not.toContain("#888");
  });

  it("keeps stored settings for seeded layers", () => {
    const cfgs = resolveLayerConfigs(seeded(), board(4));
    expect(cfgs.find((c) => c.layer === "FCu")?.color).toBe(copperLayerColor("FCu"));
  });
});

describe("layer lookups fall back for unseeded copper", () => {
  it("In5Cu renders and is coloured even with no stored config", () => {
    expect(isLayerVisible(seeded(), "In5Cu")).toBe(true);
    expect(getLayerColor(seeded(), "In5Cu")).not.toBe("#888");
  });
  it("unknown non-copper layers stay hidden", () => {
    expect(layerConfigFor(seeded(), "UserComments").visible).toBe(false);
  });
  it("toggling an unseeded layer stores it", () => {
    useElectronicsStore.getState().setLayerVisible("In7Cu", false);
    expect(isLayerVisible(seeded(), "In7Cu")).toBe(false);
    useElectronicsStore.getState().setLayerVisible("In7Cu", true);
    expect(isLayerVisible(seeded(), "In7Cu")).toBe(true);
  });
});
