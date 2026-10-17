/**
 * Layer display config derived from the board's stackup, so boards with
 * any copper count get colours, visibility and toggles — not just the
 * layers the store happened to seed.
 */

import type { PcbLayer } from "@vcad/ir";
import { boardCopperLayers, innerCopperIndex, isCopperLayer } from "@vcad/core";
import type { LayerConfig } from "@/stores/electronics-store";

type BoardLike = Parameters<typeof boardCopperLayers>[0];

/** Seeded colours for well-known copper layers (KiCad convention). */
const KNOWN_COPPER_COLORS: Record<string, string> = {
  FCu: "#F44336",
  BCu: "#2196F3",
  In1Cu: "#FFEB3B",
  In2Cu: "#4CAF50",
};

/**
 * Distinct colour for any copper layer. Inner layers beyond the seeded
 * ones walk the hue wheel by the golden angle so neighbours never match.
 */
export function copperLayerColor(layer: string): string {
  const known = KNOWN_COPPER_COLORS[layer];
  if (known) return known;
  const n = innerCopperIndex(layer);
  const hue = Math.round((n * 137.508 + 20) % 360);
  return `hsl(${hue}, 75%, 58%)`;
}

/** Default config for a layer the store has no entry for. */
export function defaultLayerConfig(layer: PcbLayer): LayerConfig {
  if (isCopperLayer(layer)) {
    return { layer, color: copperLayerColor(layer), visible: true, opacity: 1.0 };
  }
  return { layer, color: "#888", visible: false, opacity: 1.0 };
}

/** Config for `layer`, falling back to the generic default (copper visible). */
export function layerConfigFor(layers: LayerConfig[], layer: string): LayerConfig {
  return layers.find((l) => l.layer === layer) ?? defaultLayerConfig(layer as PcbLayer);
}

/**
 * Configs for the active board: every copper layer the board carries
 * (top → bottom, stored settings kept, missing ones defaulted), followed
 * by the non-copper layers. Copper the board doesn't have is dropped.
 */
export function resolveLayerConfigs(layers: LayerConfig[], pcb: BoardLike): LayerConfig[] {
  const copper = boardCopperLayers(pcb).map((l) => layerConfigFor(layers, l));
  const rest = layers.filter((l) => !isCopperLayer(l.layer));
  return [...copper, ...rest];
}
