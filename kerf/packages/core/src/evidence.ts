/**
 * Evidence and oracles.
 *
 * Every claim a kerf job makes is checked by a named oracle and backed by
 * hash-manifested evidence. The house rule (inherited from vcad-receipt)
 * is FAIL-CLOSED: an oracle that could not run reports `unverifiable`,
 * which is never treated as `pass`.
 */

/** Stable oracle identifiers. Versioned externally by package release. */
export type OracleId =
  | "kerf/upload-hash" // uploaded bytes sha256 == intent FileRef sha256
  | "kerf/quote-extraction" // priced configurator scraped + snapshotted
  | "kerf/intent-audit" // review page matches the OrderIntent
  | "kerf/confirmation-page" // vendor confirmation page shows order ref
  | "kerf/confirmation-email" // inbound email carries order ref + total
  | "kerf/card-settlement" // Stripe Issuing settled == authorized total
  | "kerf/tracking" // carrier events observed for the order ref
  | "kerf/canary"; // scheduled quote playbook walked to a price

export type Verdict = "pass" | "fail" | "unverifiable";

/** A captured artifact. Payment fields are masked at capture time — the
 *  `redactions` list records what was masked, and a screenshot containing
 *  an unmasked PAN is a capture bug, not an evidence item. */
export interface EvidenceItem {
  id: string;
  kind:
    | "screenshot"
    | "dom_snapshot"
    | "email"
    | "settlement"
    | "upload_hash"
    | "tracking_event"
    | "trace";
  sha256: string;
  bytes: number;
  captured_at: string;
  /** Playbook step or workflow step that produced this. */
  step_ref?: string;
  redactions?: Array<"pan" | "cvc" | "address">;
}

export interface OracleClaim {
  oracle: OracleId;
  verdict: Verdict;
  /** What the oracle observed (order ref, settled amount, …). */
  observed?: string;
  /** Mandatory when verdict is `unverifiable`. */
  reason?: string;
  /** Evidence item ids backing the verdict. */
  evidence: string[];
}

/** The per-job bundle handed back to the design surface's receipt. */
export interface EvidenceBundle {
  job_id: string;
  created_at: string;
  items: EvidenceItem[];
  claims: OracleClaim[];
}

/** The oracles that independently witness "the order exists". They share
 *  no failure mode: one is the vendor's web UI, one is their mail
 *  pipeline, one is the card network. */
export const CONFIRMATION_ORACLES: readonly OracleId[] = [
  "kerf/confirmation-page",
  "kerf/confirmation-email",
  "kerf/card-settlement",
];

/** Invariant 2: CONFIRMED requires two distinct confirmation oracles
 *  passing. Anything less keeps the job in CONFIRMING/RECONCILING. */
export function confirmationSatisfied(claims: readonly OracleClaim[]): boolean {
  const passed = new Set(
    claims
      .filter(
        (c) => c.verdict === "pass" && CONFIRMATION_ORACLES.includes(c.oracle),
      )
      .map((c) => c.oracle),
  );
  return passed.size >= 2;
}
