/**
 * VendorQuote — a price observed on (or committed by) a vendor, bound to
 * the intent that produced it.
 *
 * Follows the ACP-CM discipline: a quote is only meaningful WITH its
 * content hash. If the intent changes, the quote is dead — there is
 * nothing to re-price, only a new quote to issue.
 */

import type { Money } from "./intent";

/** Whether the price is a local estimate or vendor-committed. Only
 *  `binding` quotes may gate real money. */
export type PricingBasis = "estimate" | "binding";

export interface VendorQuote {
  quote_id: string;
  vendor: string;
  /** sha256 of the canonical JSON of the producing OrderIntent. The spend
   *  mandate upstream binds to this; `PLACING` re-verifies it. */
  intent_hash: string;
  pricing_basis: PricingBasis;
  unit_price: Money;
  total: Money;
  shipping?: Money;
  lead_time_days: number;
  /** Vendor-side expiry when shown; kerf treats missing expiry as 24 h. */
  expires_at?: string;
  /** Evidence item ids backing this price (screenshot + DOM snapshot of
   *  the priced configurator — a quote with no evidence is an estimate). */
  evidence: string[];
  notes: string[];
}
