/**
 * JLCPCB adapter — PCB fabrication + 3D printing.
 *
 * Phase 0 is QUOTE-ONLY and computes a LOCAL estimate (pricing_basis:
 * "estimate"). The live JLCPCB Partner API quote is gated behind credentials
 * AND is deliberately NOT wired yet: the binding quote + ordering endpoints
 * require an approval-gated ACCESS_KEY + SECRET_KEY (HMAC-signed requests) we
 * don't hold, and shipping an unverifiable HTTP integration is worse than a
 * labeled estimate. The credential seam is here so Phase 1 only has to fill in
 * `quoteViaApi`.
 *
 * The APP_ID is a public identifier read from JLCPCB_APP_ID (env, not source).
 * The cost coefficients below are deliberately conservative PLACEHOLDERS — they
 * produce sane positive numbers for the end-to-end loop, not real JLCPCB prices.
 */

import type { AdapterQuote, ManufacturerAdapter, QuoteRequest } from "../types.js";

function appConfigured(): boolean {
  return Boolean(process.env.JLCPCB_APP_ID);
}

/** True once the approval-gated signing secrets are present (Phase 1). */
function credentialed(): boolean {
  return Boolean(
    process.env.JLCPCB_APP_ID &&
      process.env.JLCPCB_ACCESS_KEY &&
      process.env.JLCPCB_SECRET_KEY,
  );
}

/** Local PCB cost estimate from board area, layer count, and quantity. */
function estimatePcb(req: QuoteRequest): AdapterQuote {
  const layers = Math.max(1, Math.round(req.layers ?? 2));
  const areaMm2 = req.boardAreaMm2 ?? req.metrics.footprint_mm2;
  const areaCm2 = Math.max(1, areaMm2 / 100);

  // Layer factor: 2L = 1.0, +0.4 per extra layer (placeholder).
  const layerFactor = 1 + Math.max(0, layers - 2) * 0.4;

  // Per-unit fab cost (minor units): area-driven + per-board handling.
  const unitMinor = Math.round(areaCm2 * 6 * layerFactor + 30);
  // Amortized panelization/setup, in minor units.
  const setupMinor = 200;
  const fabCostMinor = setupMinor + unitMinor * req.quantity;

  const notes = [
    `Local estimate (pricing_basis=estimate): ${layers}-layer, ~${areaCm2.toFixed(1)} cm² board, qty ${req.quantity}.`,
    appConfigured()
      ? "JLCPCB_APP_ID configured; binding Partner-API quote wired in Phase 1 (needs ACCESS_KEY + SECRET_KEY)."
      : "JLCPCB credentials not set — estimate only. Set JLCPCB_APP_ID/ACCESS_KEY/SECRET_KEY for binding quotes.",
  ];
  if (!req.boardAreaMm2 && !req.metrics.ok) {
    notes.push(
      "Board area defaulted: pass board_area_mm2 (and layers) for a sharper PCB estimate.",
    );
  }
  return {
    fab_cost_minor: fabCostMinor,
    lead_time_days: 7,
    in_spec: true,
    pricing_basis: "estimate",
    notes,
  };
}

/** Local 3D-print cost estimate from part volume and quantity. */
function estimate3dPrint(req: QuoteRequest): AdapterQuote {
  const volCm3 = Math.max(0.1, req.metrics.volume_mm3 / 1000);
  // Prefer the shared kernel cost model (consistent with the in-app quote);
  // fall back to local coefficients.
  const unitMinor = Math.round(volCm3 * 35 + 250);
  const fabCostMinor = req.baseCostMinor ?? unitMinor * req.quantity;
  const inSpec = req.metrics.max_dim_mm <= 256; // typical resin/FDM envelope
  const notes = [
    `Local estimate (pricing_basis=estimate): ~${volCm3.toFixed(1)} cm³, qty ${req.quantity}.`,
  ];
  if (req.baseCostMinor != null) {
    notes.push("Priced via the shared kernel cost model — agrees with the in-app Build quote.");
  }
  if (!inSpec) {
    notes.push(
      `Part max dimension ${req.metrics.max_dim_mm.toFixed(0)} mm exceeds the ~256 mm build envelope.`,
    );
  }
  if (!req.metrics.ok) {
    notes.push("No solid geometry measured — estimate is a floor only.");
  }
  return {
    fab_cost_minor: fabCostMinor,
    lead_time_days: 6,
    in_spec: inSpec,
    pricing_basis: "estimate",
    notes,
  };
}

export const jlcpcbAdapter: ManufacturerAdapter = {
  key: "jlcpcb",
  label: "JLCPCB",
  region: "CN",
  processes: ["pcb", "3dprint"],
  supportsDdp: true,
  async quote(req: QuoteRequest): Promise<AdapterQuote | null> {
    // Phase 1 seam: when credentialed, call the live Partner API for a binding
    // quote here (HMAC-SHA256 signed; APP_ID + ACCESS_KEY + SECRET_KEY).
    void credentialed; // referenced to document the seam; live call is Phase 1.

    if (req.process === "pcb") return estimatePcb(req);
    if (req.process === "3dprint") return estimate3dPrint(req);
    return null;
  },
};
