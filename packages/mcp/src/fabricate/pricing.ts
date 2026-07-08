/**
 * vcad Fabricate — margin + landed-cost model.
 *
 * Monetization (decided): cost-plus markup, folded into the line total and
 * NEVER exposed as a separate fee. No agentic-payment protocol carries a
 * take-rate field, so the margin lives in the price, not the protocol.
 *
 * All money is integer MINOR units (USD cents).
 */

import type { LandedCost } from "./types.js";

/**
 * Gross margin rate. ~25% — anchored below Xometry's ~35% marketplace gross so
 * it survives ~5-7% MoR/processing fees yet stays low enough to deter
 * disintermediation once a buyer can price-check fabs directly.
 *
 * Treat Xometry's ~35% as their VERTICALLY-INTEGRATED margin, not a markup
 * ceiling a pure reseller can copy — hence the conservative default.
 */
export const MARGIN_RATE = 0.25;

/** Mark a per-fab cost up to the customer-facing price (minor units). */
export function applyMargin(fabCostMinor: number): number {
  return Math.round(fabCostMinor * (1 + MARGIN_RATE));
}

/** The margin portion of a marked-up price (server-only). */
export function marginOf(fabCostMinor: number): number {
  return applyMargin(fabCostMinor) - fabCostMinor;
}

/**
 * Phase 0 landed-cost estimate. Deliberately simple and HONEST:
 *  - domestic (US) fab → flat domestic shipping, no duty.
 *  - offshore fab → DDP estimate: the fab is importer-of-record and bears duty
 *    volatility, so duty to the buyer is 0 and we say so. (Phase 1 makes DDP a
 *    hard requirement rather than an assumption — a 20-30% margin can't absorb
 *    a Section 301 tariff swing.)
 *
 * Never cached across quotes; the duty regime is volatile.
 */
export function estimateLandedCost(opts: {
  region: string;
  supportsDdp: boolean;
}): LandedCost {
  // "n/a" is the generic vcad estimator (no contracted fab). Its interim
  // fulfillment rail is the US instant-quote handoff shops (SendCutSend /
  // OSH Cut / Fabworks), so price it as domestic — not offshore DDP.
  const region = opts.region.toUpperCase();
  const offshore = region !== "US" && region !== "N/A";
  if (!offshore) {
    return { shipping_minor: 800, duty_minor: 0, basis: "domestic_estimate" };
  }
  return {
    shipping_minor: 2500,
    duty_minor: 0,
    basis: opts.supportsDdp ? "ddp_estimate" : "duty_unbundled_estimate",
  };
}
