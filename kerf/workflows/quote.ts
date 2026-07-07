/**
 * The quote job — Wave 0. Zero money, zero account: walk a vendor's
 * public instant-quote flow with the intent's files and configuration,
 * extract a priced, snapshotted, evidence-backed VendorQuote.
 *
 * This is the whole stack minus money: BrowserHost session, playbook
 * execution, assertions, Tier-2 fallback, evidence capture. It is also
 * the canary body — canaries run this with a fixture intent and
 * budget_minor 0.
 */

import type { ConfiguratorIntent, VendorQuote } from "@kerf/core";

export interface QuoteJobInput {
  job_id: string;
  intent: ConfiguratorIntent;
}

export async function quoteJob(input: QuoteJobInput): Promise<VendorQuote> {
  "use workflow";

  // 1. SESSION_OPEN — BrowserHost session, allowlist from the manifest.
  // 2. Per playbook step (each a memoized workflow step):
  //      upload (hash bytes in flight → kerf/upload-hash claim),
  //      select material/thickness by vendor-native label from the intent,
  //      set quantity (assert), extract price (assert present, parse
  //      money), screenshot the priced configurator.
  // 3. Tier-2 agent summoned for any failed step; repair → playbook patch.
  // 4. Return VendorQuote{ pricing_basis: "binding", evidence: [...] } —
  //    the design surface's broker flips its sheet-metal options from
  //    estimate to binding on this.

  void input;
  throw new Error("kerf bringup: quote workflow is the Wave 0 deliverable — see docs/architecture.md");
}
