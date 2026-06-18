/**
 * Bridge between the Fabricate ordering vocabulary (Process) and the shared
 * kernel cost model (vcad-kernel-cost, exposed as DfmProcess + estimateCost in
 * @vcad/engine). This is the SAME estimator the app's "Build" → QuotePanel uses,
 * so a quote agrees whether it comes from the app UI or an agent over MCP.
 *
 * PCB has no volume×material cost model in the kernel, so it maps to null and
 * keeps its bespoke area/layers estimate in the JLCPCB adapter.
 */

import type { DfmProcess } from "@vcad/engine";
import type { Process } from "./types.js";

/** Map a Fabricate process to the kernel cost model's process, or null when
 *  the kernel has no estimator for it (PCB). */
export function toDfmProcess(p: Process): DfmProcess | null {
  switch (p) {
    case "cnc":
      return "cnc_3axis";
    case "3dprint":
      return "fdm";
    case "sheet_metal":
      return "sheet_metal";
    case "cast_metal":
      return "casting_sand"; // Digital Metal casts (gravity-fed sand).
    case "pcb":
      return null;
    default:
      return null;
  }
}

// Exact catalog names from vcad-kernel-cost::Material::catalog().
const CATALOG = [
  "PLA",
  "PETG",
  "ABS",
  "TPU",
  "SLA Resin",
  "Aluminum 6061",
  "Steel 1018",
  "Brass C360",
  "Polycarbonate",
  "Cast Aluminum A356",
  "Cast Iron",
];

// Friendly aliases → catalog names. Materials with no catalog entry (e.g.
// stainless, zinc, zamak) map to the nearest priced stand-in for the estimate.
const ALIASES: Record<string, string> = {
  pla: "PLA",
  petg: "PETG",
  abs: "ABS",
  tpu: "TPU",
  resin: "SLA Resin",
  "sla resin": "SLA Resin",
  aluminum: "Aluminum 6061",
  aluminium: "Aluminum 6061",
  al: "Aluminum 6061",
  steel: "Steel 1018",
  stainless: "Steel 1018",
  "stainless steel": "Steel 1018",
  brass: "Brass C360",
  polycarbonate: "Polycarbonate",
  pc: "Polycarbonate",
  "cast aluminum": "Cast Aluminum A356",
  zinc: "Cast Aluminum A356",
  zamak: "Cast Aluminum A356",
  "cast iron": "Cast Iron",
  iron: "Cast Iron",
};

// Default catalog material per process when the caller doesn't specify one.
// Mirrors the app QuotePanel's MATERIAL_MAPPINGS picks.
const DEFAULTS: Record<Process, string> = {
  pcb: "",
  "3dprint": "PLA",
  cnc: "Aluminum 6061",
  sheet_metal: "Aluminum 6061",
  cast_metal: "Cast Aluminum A356",
};

/** Resolve a (process, free-form material) pair to a kernel-cost catalog name. */
export function catalogMaterial(p: Process, material?: string): string {
  if (material) {
    const key = material.trim().toLowerCase();
    const exact = CATALOG.find((c) => c.toLowerCase() === key);
    if (exact) return exact;
    if (ALIASES[key]) return ALIASES[key];
  }
  return DEFAULTS[p];
}
