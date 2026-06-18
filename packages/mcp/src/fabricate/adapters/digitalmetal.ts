/**
 * Digital Metal (digitalmetal.io) adapter — instant-quote CAST METAL parts.
 *
 * A strong adapter fit: their flow is STEP upload → instant quote + DFM →
 * configure (alloy / qty / finish) → online checkout, and vcad exports STEP
 * natively (export_cad on a sheet-metal/solid document). US-based (SF + ATX),
 * 1-piece minimum, 5-10 business-day lead, max part 350 × 350 × 200 mm,
 * tolerance ±0.1 mm / 10 mm. Casts stainless, steel, zinc, zamak.
 *
 * Phase 0 is QUOTE-ONLY with a LOCAL estimate (pricing_basis: "estimate"). A
 * live quote/order integration is a roadmap item once their /docs API surface
 * is confirmed; until then this is an estimate, not orderable.
 */

import type { AdapterQuote, ManufacturerAdapter, QuoteRequest } from "../types.js";

// Build envelope (mm) and supported alloys per digitalmetal.io.
const MAX_DIM_MM = 350;
const ALLOYS = ["stainless", "steel", "zinc", "zamak"];

export const digitalMetalAdapter: ManufacturerAdapter = {
  key: "digitalmetal",
  label: "Digital Metal",
  region: "US",
  processes: ["cast_metal"],
  supportsDdp: true, // US domestic — no import duty.
  async quote(req: QuoteRequest): Promise<AdapterQuote | null> {
    if (req.process !== "cast_metal") return null;

    const volCm3 = Math.max(0.1, req.metrics.volume_mm3 / 1000);
    // Cast-metal cost driver: prefer the shared kernel cost model (consistent
    // with the in-app quote); fall back to local coefficients.
    const unitMinor = Math.round(volCm3 * 90 + 1200);
    const fabCostMinor = req.baseCostMinor ?? unitMinor * req.quantity;

    const notes: string[] = [
      `Local estimate (pricing_basis=estimate): ~${volCm3.toFixed(1)} cm³ cast metal, qty ${req.quantity}.`,
      "Digital Metal: US-based, 1-piece min, 5-10 business-day lead, ±0.1 mm/10 mm. Live API is a roadmap item.",
    ];
    if (req.baseCostMinor != null) {
      notes.push("Priced via the shared kernel cost model — agrees with the in-app Build quote.");
    }

    let inSpec = true;
    if (req.metrics.ok && req.metrics.max_dim_mm > MAX_DIM_MM) {
      inSpec = false;
      notes.push(
        `Part max dimension ${req.metrics.max_dim_mm.toFixed(0)} mm exceeds the ${MAX_DIM_MM} mm build envelope.`,
      );
    }
    if (req.material && !ALLOYS.includes(req.material.toLowerCase())) {
      notes.push(
        `Material "${req.material}" not in Digital Metal's catalog (${ALLOYS.join(", ")}); estimate uses a default alloy.`,
      );
    }
    if (!req.metrics.ok) {
      notes.push("No solid geometry measured — estimate is a floor only.");
    }

    return {
      fab_cost_minor: fabCostMinor,
      lead_time_days: 8,
      in_spec: inSpec,
      pricing_basis: "estimate",
      notes,
    };
  },
};
