/**
 * Generic copper-layer helpers — derive layer sets from a board's stackup
 * instead of hardcoded 4-layer lists, so any copper count renders.
 */

import type { Pcb, PcbLayer } from "@vcad/ir";

const COPPER_RE = /^(FCu|BCu|In(\d+)Cu)$/;

/**
 * Highest copper count `initPcb` will mint: FCu + BCu + the IR's inner
 * layers (In1Cu..In30Cu → 32, KiCad's maximum).
 */
export const MAX_COPPER_LAYERS = 32;

// Compile-time guard: fails if the IR's highest inner layer is not In30Cu.
type _MaxInnerInIr = "In30Cu" extends PcbLayer ? ("In31Cu" extends PcbLayer ? never : true) : never;
const _maxInnerInIr: _MaxInnerInIr = true;
void _maxInnerInIr;

/** True for FCu, BCu and any In{n}Cu. */
export function isCopperLayer(layer: string): boolean {
  return COPPER_RE.test(layer);
}

/** Inner index (In3Cu → 3); 0 for non-inner layers. */
export function innerCopperIndex(layer: string): number {
  const m = COPPER_RE.exec(layer);
  return m?.[2] ? Number(m[2]) : 0;
}

/** Name of the n-th inner copper layer (1-based). */
export function innerCopperLayerName(n: number): PcbLayer {
  return `In${n}Cu` as PcbLayer;
}

/** Ordered copper names (top → bottom) for a board with `count` copper layers. */
export function copperLayerNames(count: number): PcbLayer[] {
  if (!Number.isInteger(count) || count < 1) {
    throw new Error(`invalid copper layer count ${count}`);
  }
  if (count === 1) return ["FCu"];
  const inner = Array.from({ length: count - 2 }, (_, i) => innerCopperLayerName(i + 1));
  return ["FCu", ...inner, "BCu"];
}

/** Validate a copper count for board creation; throws on unsupported values. */
export function assertSupportedCopperCount(count: number): void {
  const ok =
    Number.isInteger(count) &&
    count >= 1 &&
    (count === 1 || count % 2 === 0) &&
    count <= MAX_COPPER_LAYERS;
  if (!ok) {
    throw new Error(
      `unsupported PCB layer count ${count}: expected 1 or an even number from 2 to ${MAX_COPPER_LAYERS}`,
    );
  }
}

/** Sort key: FCu first, inner layers by index, BCu last. */
export function copperSortKey(layer: string): number {
  if (layer === "FCu") return 0;
  if (layer === "BCu") return Number.MAX_SAFE_INTEGER;
  return innerCopperIndex(layer);
}

type LayerSource = Partial<Pick<Pcb, "stackup" | "traces" | "zones" | "vias">>;

/**
 * Copper layers present on a board, top → bottom. Reads the stackup, then
 * adds any copper layer referenced by traces/zones/vias that the stackup
 * omits (imported boards are not always consistent).
 */
export function boardCopperLayers(pcb: LayerSource | null | undefined): PcbLayer[] {
  const seen = new Set<PcbLayer>();
  const add = (l: PcbLayer | undefined) => {
    if (l && isCopperLayer(l)) seen.add(l);
  };
  for (const l of pcb?.stackup?.layers ?? []) add(l.layer);
  for (const t of pcb?.traces ?? []) add(t.layer);
  for (const z of pcb?.zones ?? []) add(z.layer);
  for (const v of pcb?.vias ?? []) {
    add(v.startLayer);
    add(v.endLayer);
  }
  if (seen.size === 0) return ["FCu", "BCu"];
  return [...seen].sort((a, b) => copperSortKey(a) - copperSortKey(b));
}
