/**
 * The 21 legal PCB layer names, in board order.
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
export const PCB_LAYERS: readonly PcbLayer[] = [
  "FCu", "BCu", "In1Cu", "In2Cu", "In3Cu", "In4Cu", "In5Cu", "In6Cu",
  "FSilkS", "BSilkS", "FMask", "BMask", "FPaste", "BPaste",
  "FFab", "BFab", "FCrtYd", "BCrtYd",
  "EdgeCuts", "UserDrawings", "UserComments",
];
