/**
 * The legal PCB layer names, in board order.
 *
 * These are the canonical serde variants of the Rust `PcbLayer` enum
 * (crates/vcad-ir/src/ecad.rs) — that enum is the single source of truth. This
 * module is the single *runtime* copy of the list on the TS side: both the
 * write-boundary validator (`pcb-validate.ts`, as `VALID_LAYERS`) and the ECAD
 * tools (`ecad.ts`, as `PCB_LAYERS`) import it here so the list is never
 * duplicated and can't drift between the two.
 */

import type { PcbLayer } from "@vcad/ir";

/** Every legal PCB layer name, in board order. */
/** Inner copper In1Cu..In30Cu (KiCad's full inner range). */
export const INNER_COPPER: readonly PcbLayer[] = Array.from({ length: 30 }, (_, i) => `In${i + 1}Cu` as PcbLayer);

export const PCB_LAYERS: readonly PcbLayer[] = [
  "FCu", "BCu", ...INNER_COPPER,
  "FSilkS", "BSilkS", "FMask", "BMask", "FPaste", "BPaste",
  "FFab", "BFab", "FCrtYd", "BCrtYd",
  "EdgeCuts", "UserDrawings", "UserComments",
];

/** Every copper layer, in PHYSICAL stack order top → bottom (mirrors Rust
 *  `PcbLayer::COPPER_STACK`). Use this for via-span ordering — PCB_LAYERS
 *  lists BCu second, so indexing it treats BCu as adjacent to FCu. */
export const COPPER_STACK: readonly PcbLayer[] = ["FCu", ...INNER_COPPER, "BCu"];
