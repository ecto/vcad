/**
 * Generic vcad estimator — a fallback adapter for processes with no contracted
 * fab yet (CNC, sheet_metal, and a floor for pcb/3dprint/cast_metal). It always
 * returns pricing_basis: "estimate" and is flagged NOT orderable by the broker,
 * so an agent can see a ballpark for any process while we stand up real fab
 * partners. Coefficients are deliberate placeholders.
 *
 * Honesty over theater: this exists so quote_manufacturing works end-to-end for
 * every process, NOT to imply we can fulfill CNC/sheet-metal today.
 */

import type { AdapterQuote, ManufacturerAdapter, Process, QuoteRequest } from "../types.js";

// Per-process cost driver + envelope (placeholder).
const MODELS: Record<
  Process,
  { perCm3: number; perCm2: number; base: number; lead: number; maxDimMm: number }
> = {
  pcb: { perCm3: 0, perCm2: 8, base: 200, lead: 8, maxDimMm: 500 },
  cnc: { perCm3: 120, perCm2: 0, base: 1500, lead: 10, maxDimMm: 500 },
  "3dprint": { perCm3: 35, perCm2: 0, base: 250, lead: 7, maxDimMm: 256 },
  sheet_metal: { perCm3: 0, perCm2: 12, base: 900, lead: 9, maxDimMm: 1000 },
  cast_metal: { perCm3: 90, perCm2: 0, base: 1200, lead: 10, maxDimMm: 350 },
};

export const genericEstimateAdapter: ManufacturerAdapter = {
  key: "vcad_estimate",
  label: "vcad estimate (no contracted fab yet)",
  region: "n/a",
  processes: ["pcb", "cnc", "3dprint", "sheet_metal", "cast_metal"],
  supportsDdp: false,
  async quote(req: QuoteRequest): Promise<AdapterQuote | null> {
    const m = MODELS[req.process];
    if (!m) return null;

    const volCm3 = Math.max(0.1, req.metrics.volume_mm3 / 1000);
    const areaCm2 = Math.max(1, (req.boardAreaMm2 ?? req.metrics.footprint_mm2) / 100);
    const unitMinor = Math.round(m.base + volCm3 * m.perCm3 + areaCm2 * m.perCm2);
    // Prefer the shared kernel cost model (consistent with the in-app quote);
    // fall back to local coefficients when it's unavailable.
    const fabCostMinor = req.baseCostMinor ?? unitMinor * req.quantity;

    const inSpec = !req.metrics.ok || req.metrics.max_dim_mm <= m.maxDimMm;
    const notes = [
      `Ballpark estimate — no contracted ${req.process} fab yet, NOT orderable.`,
    ];
    if (req.baseCostMinor != null) {
      notes.push("Priced via the shared kernel cost model — agrees with the in-app Build quote.");
    }
    if (!inSpec) {
      notes.push(
        `Part max dimension ${req.metrics.max_dim_mm.toFixed(0)} mm exceeds the ~${m.maxDimMm} mm envelope.`,
      );
    }
    return {
      fab_cost_minor: fabCostMinor,
      lead_time_days: m.lead,
      in_spec: inSpec,
      pricing_basis: "estimate",
      notes,
    };
  },
};
