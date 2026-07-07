/**
 * The order job — ONE durable run spans checkout to delivery.
 *
 * The step boundaries below ARE the contract; exact eve/workflow API
 * names (directives, hook creation) get aligned with `node_modules/eve/docs`
 * at bring-up. What must not change:
 *
 *   - every money-adjacent action is its own step (retried on infra
 *     failure, MEMOIZED once complete — a finished buy click never
 *     re-executes on replay);
 *   - the placing attempt is durably recorded BEFORE the click step runs;
 *     a resume that finds it goes to RECONCILING, never back to PLACING;
 *   - card entry happens inside a server-side step (PAN fetched from
 *     Stripe Issuing in-step, typed via CDP; never in any model context);
 *   - CONFIRMED requires confirmationSatisfied() — two independent
 *     oracles — else the job parks in RECONCILING;
 *   - lead time is a sleep, not a poll loop in a hot process.
 *
 * Input arrives only from the design surface's money plane, which has
 * already: verified the hash-bound mandate, debited the wallet, and issued
 * the single-use merchant-locked virtual card. kerf receives a card
 * REFERENCE, never custody of funds.
 */

import type { OrderIntent, VendorQuote } from "@kerf/core";
import {
  assertTransition,
  confirmationSatisfied,
  mayEnterPlacing,
} from "@kerf/core";

export interface OrderJobInput {
  job_id: string;
  intent: OrderIntent;
  quote: VendorQuote;
  /** Stripe Issuing card id — resolved to a PAN only inside the card-entry
   *  step, server-side. */
  card_ref: string;
  /** L1: stop at STAGED and hook for the human's click in the live view.
   *  L2: auditor-gated placing step clicks. Ceiling comes from the vendor
   *  manifest; the design surface may request lower, never higher. */
  autonomy: "L1" | "L2";
}

export async function orderJob(input: OrderJobInput): Promise<void> {
  "use workflow";

  // 1. SESSION_OPEN — BrowserHost session, domain allowlist from manifest.
  // 2. STAGING — run the vendor's order playbook steps; each is a step.
  //    Tier-2 agent is summoned per failed step; takeover hooks interleave.
  // 3. STAGED — assertions green. L1: await humanClick hook → CONFIRMING.
  // 4. AUDIT — auditor step re-extracts the review page, compares against
  //    input.intent (kerf/intent-audit oracle). Fail → AUDIT_FAILED, done.
  // 5. record PlacingAttempt (durable) — guard with mayEnterPlacing().
  // 6. PLACING — card-entry step (Stripe PAN via CDP) + the one click.
  // 7. CONFIRMING — hooks race a timeout: confirmation page extraction,
  //    inbound email (orders+<job>@), card settlement webhook. Two pass →
  //    CONFIRMED. Timeout/ambiguity → RECONCILING: scan order history +
  //    inbox for intent.idempotency_key; found → RECONCILED_PLACED,
  //    provably absent → RECONCILED_ABSENT (a re-attempt is a NEW job).
  // 8. TRACKING — sleep(lead_time), then tracking playbook until DELIVERED.
  // 9. Emit the EvidenceBundle; events stream to the design surface
  //    throughout via signed webhooks.

  void assertTransition; // wired at bring-up — imports document the contract
  void confirmationSatisfied;
  void mayEnterPlacing;
  throw new Error("kerf bringup: order workflow lands in Wave 1/2 — see docs/architecture.md");
}
