/**
 * kerf contract types — the ACP-CM execution rail's wire vocabulary.
 *
 * Mirrored from github.com/ecto/kerf packages/core — keep in sync; drift is
 * checked by kerf-contract.test. vcad integrates kerf as a service (quotes,
 * jobs, evidence over HTTP) and never vendors its engine; these types are the
 * whole surface area we depend on. Money is integer MINOR units, never floats.
 */

/** Integer minor-unit money (USD cents). Mirrors kerf core/intent.ts. */
export interface Money {
  currency: string;
  amount_minor: number;
}

/** A content-hash-pinned file reference. kerf's `kerf/upload-hash` oracle
 *  verifies the exact bytes named here were uploaded. */
export interface FileRef {
  name: string;
  bytes: number;
  sha256: string;
  media_type?: string;
  /**
   * WIRE-ONLY (vcad→kerf POST /api/quote): posted intents must inline the
   * file bytes as base64 — kerf's validatePostedIntent requires it, hash-
   * checks sha256(bytes) === `sha256` at the door, and STRIPS the field from
   * the FileRef it keeps. Not part of kerf core's FileRef, and NEVER part of
   * `intentHash` (which hashes file sha256s only — see intent-hash.ts).
   */
  bytes_base64?: string;
}

/** Shipping address (vendor-native field granularity). */
export interface ShipTo {
  name: string;
  line1: string;
  line2?: string;
  city: string;
  region: string;
  postal_code: string;
  country: string;
}

/** Common intent fields. `idempotency_key` is unique per order attempt and
 *  never reused after PLACING; `budget_cap` is the hard mandate ceiling. */
export interface IntentBase {
  idempotency_key: string;
  /** Registry vendor id, e.g. "sendcutsend". */
  vendor: string;
  ship_to: ShipTo;
  budget_cap: Money;
  /** ISO-8601, advisory. */
  deadline?: string;
}

/** An intent against a vendor's web configurator (upload + options + qty).
 *  `config` uses VENDOR-NATIVE labels per the vendor manifest's config_schema. */
export interface ConfiguratorIntent extends IntentBase {
  kind: "configurator";
  files: FileRef[];
  /** e.g. "sheet_metal". */
  process: string;
  config: Record<string, string | number | boolean>;
  quantity: number;
}

/** How firm a price is. kerf's browser rail always emits "quoted" (the fab's
 *  own displayed price); "binding" is fab-committed; "estimate" never gates
 *  money. */
export type PricingBasis = "estimate" | "quoted" | "binding";

/** A vendor quote bound to the `intent_hash` of the producing intent —
 *  geometry (or config/quantity) edit ⇒ new hash ⇒ this quote is dead. */
export interface VendorQuote {
  quote_id: string;
  vendor: string;
  /** sha256 of canonical JSON of the producing OrderIntent (see intent-hash). */
  intent_hash: string;
  pricing_basis: PricingBasis;
  unit_price: Money;
  total: Money;
  shipping?: Money;
  /** Currently 0 from the browser rail; raw lead text lives in notes. */
  lead_time_days: number;
  /** Missing ⇒ kerf treats as 24h. */
  expires_at?: string;
  /** Evidence item ids. */
  evidence: string[];
  notes: string[];
}

/** All 17 kerf job states (core/job.ts). PLACING is entered at most once per
 *  job, ever; CONFIRMED requires two independent oracles. */
export const KERF_JOB_STATES = [
  "QUEUED",
  "SESSION_OPEN",
  "STAGING",
  "TAKEOVER_WAIT",
  "STAGED",
  "AUDIT",
  "AUDIT_FAILED",
  "PLACING",
  "CONFIRMING",
  "CONFIRMED",
  "RECONCILING",
  "RECONCILED_PLACED",
  "RECONCILED_ABSENT",
  "TRACKING",
  "DELIVERED",
  "FAILED",
  "CANCELED",
] as const;

/** kerf job lifecycle state. */
export type JobState = (typeof KERF_JOB_STATES)[number];

/** The oracles kerf's evidence layer can attest with. */
export type OracleId =
  | "kerf/upload-hash"
  | "kerf/quote-extraction"
  | "kerf/intent-audit"
  | "kerf/confirmation-page"
  | "kerf/confirmation-email"
  | "kerf/card-settlement"
  | "kerf/tracking"
  | "kerf/canary";

/** Fail-closed verdict vocabulary (shared with vcad-receipt): unverifiable is
 *  NEVER a pass. */
export type Verdict = "pass" | "fail" | "unverifiable";

/** One captured evidence artifact (hash-pinned, PII-redacted). */
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
  step_ref?: string;
  redactions?: Array<"pan" | "cvc" | "address">;
}

/** One oracle's attestation over evidence items. */
export interface OracleClaim {
  oracle: OracleId;
  verdict: Verdict;
  observed?: string;
  reason?: string;
  /** Evidence item ids backing the claim. */
  evidence: string[];
}

/** The per-job bundle handed back to the design surface's receipt. */
export interface EvidenceBundle {
  job_id: string;
  created_at: string;
  items: EvidenceItem[];
  claims: OracleClaim[];
}
