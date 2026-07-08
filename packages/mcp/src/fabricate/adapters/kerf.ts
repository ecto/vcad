/**
 * SendCutSend-via-kerf adapter — the first live kerf-rail driver (Wave 0,
 * quote-only).
 *
 * vcad calls kerf as a service: order.ts builds the ConfiguratorIntent (files
 * pinned by sha256 from the bound fab artifact + vendor-native config) and
 * threads it through QuoteRequest.kerfIntent; this adapter forwards it whole
 * to kerf's quote job and maps the VendorQuote back. In LIVE mode the price
 * that returns is the fab's OWN displayed price (pricing_basis "quoted");
 * scripted mode is a rehearsal against kerf's recorded fixture and is
 * downgraded to "estimate" (never money-gating). The broker's margin/landed
 * layers apply on top exactly as for any other adapter.
 *
 * Degradation is always null, never an error: no KERF_URL, no intent, no
 * files, an unreachable rail, or a quote job that didn't price — all return
 * null so the generic estimator covers and a quote fan-out never breaks.
 *
 * Ordering through kerf is Wave 1/2; the quote intent's budget_cap is pinned
 * to 0 (kerf canary discipline — a quote job can never fund an order).
 */

import { randomUUID } from "node:crypto";
import { KerfClient, KerfUnreachableError } from "../kerf/client.js";
import type { ConfiguratorIntent, FileRef, ShipTo } from "../kerf/contract.js";
import type { AdapterQuote, ManufacturerAdapter, QuoteRequest } from "../types.js";

/** The kerf registry vendor id this adapter drives. */
export const KERF_VENDOR = "sendcutsend";

/** Quote mode: "scripted" (kerf's recorded fixture flow — offline,
 *  deterministic, the default) or "live" (real cloud-browser run, opt-in via
 *  KERF_QUOTE_MODE=live). */
export function kerfQuoteMode(): "scripted" | "live" {
  return process.env.KERF_QUOTE_MODE === "live" ? "live" : "scripted";
}

/**
 * Quote-time ship-to. SendCutSend's configurator prices before any address is
 * entered, so at quote time ship_to affects NOTHING in the SCS flow — it
 * exists because kerf's intent schema requires one. Override with KERF_SHIP_TO
 * (JSON ShipTo) for rails where it matters; the default is a US placeholder.
 */
function quoteShipTo(): ShipTo {
  const raw = process.env.KERF_SHIP_TO;
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as ShipTo;
      if (parsed && typeof parsed.country === "string") return parsed;
    } catch {
      // fall through to the placeholder
    }
  }
  return {
    name: "vcad quote",
    line1: "548 Market St",
    city: "San Francisco",
    region: "CA",
    postal_code: "94104",
    country: "US",
  };
}

/**
 * Build the sheet-metal ConfiguratorIntent for a SendCutSend quote. Called by
 * quote_manufacturing (order.ts) so the SAME object is both hashed for
 * kerf_intent_hash persistence and sent by this adapter — one identity, no
 * drift. budget_cap is 0: this intent can quote, never buy.
 */
export function buildKerfSheetMetalIntent(p: {
  files: FileRef[];
  config: Record<string, string | number | boolean>;
  quantity: number;
}): ConfiguratorIntent {
  return {
    kind: "configurator",
    vendor: KERF_VENDOR,
    process: "sheet_metal",
    files: p.files,
    config: p.config,
    quantity: p.quantity,
    idempotency_key: `vq_${randomUUID()}`,
    ship_to: quoteShipTo(),
    budget_cap: { currency: "USD", amount_minor: 0 },
  };
}

// Log the unreachable-rail degradation once per process, not once per quote.
let loggedUnreachable = false;

export const kerfAdapter: ManufacturerAdapter = {
  key: KERF_VENDOR,
  label: "SendCutSend (via kerf)",
  region: "US",
  processes: ["sheet_metal"],
  supportsDdp: true,
  async quote(req: QuoteRequest): Promise<AdapterQuote | null> {
    const intent = req.kerfIntent?.intent;
    const client = new KerfClient();
    // Can't serve without the rail, an intent, or files — the generic
    // estimator covers (returning null is the adapter contract for "not us").
    if (!client.available || !intent || intent.files.length === 0) return null;

    const mode = kerfQuoteMode();
    let job;
    try {
      job = await client.quote(KERF_VENDOR, intent, { mode });
    } catch (err) {
      if (err instanceof KerfUnreachableError) {
        if (!loggedUnreachable) {
          loggedUnreachable = true;
          console.error("[kerf-adapter] rail unreachable — degrading to estimates:", err.message);
        }
        return null;
      }
      throw err;
    }

    const quote = job.quote;
    if (!quote) return null;

    // Scripted mode replays kerf's recorded fixture (a fixed price regardless
    // of the posted geometry/config) — that's a REHEARSAL of the rail, not a
    // vendor-displayed price, so its basis is downgraded to "estimate" (which
    // never gates money). Only a live run may carry "quoted".
    const scripted = mode !== "live";
    const pricingBasis = scripted ? "estimate" : quote.pricing_basis;

    return {
      // The vendor's displayed total IS the fab cost; broker margin + landed
      // cost stack on top like every adapter.
      fab_cost_minor: quote.total.amount_minor,
      lead_time_days: quote.lead_time_days,
      in_spec: true,
      pricing_basis: pricingBasis,
      notes: [
        ...quote.notes,
        `kerf job ${job.job_id}`,
        `intent ${quote.intent_hash.slice(0, 16)}`,
        ...(scripted
          ? ["kerf scripted rehearsal — not a vendor-displayed price"]
          : []),
      ],
    };
  },
};
