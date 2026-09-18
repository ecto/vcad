/**
 * The document side of the claim-family registry.
 *
 * vcad has had a dozen-odd Rust claim families since wave 2 —
 * `vcad.cam-claims/1`, `vcad.thermal-claims/1`, `vcad.tolerance-claims/1`,
 * … — each with a `design_claims` that turns its own report into unified
 * receipt claims, and **none of them ever reached a receipt**. `build_receipt`
 * hard-wired three families (PCB through WASM, mechanical clearance and
 * design constraints as hand-written TypeScript) and had nowhere to look the
 * rest up.
 *
 * This module is the two halves that were missing:
 *
 * 1. **A slot.** `document.claim_reports` holds, per deposit, a family's own
 *    serialized report plus the live inputs it rests on. A tool that ran an
 *    oracle deposits here ({@link depositClaimReport}); nothing else has to
 *    change for a new family to become certifiable.
 * 2. **A merge.** {@link registryReceiptClaims} re-states every deposit
 *    against its inputs as they stand *now* and asks the kernel registry for
 *    its claims, which `build_receipt` folds into the unified ledger.
 *
 * ── Fail-closed, three ways ────────────────────────────────────────────────
 *
 * A deposit can go wrong in exactly three ways, and none of them may end in
 * a claim quietly going missing — a receipt that silently dropped a family
 * would roll up clean on evidence nobody produced:
 *
 * - the kernel build carries no registry at all → one `unverifiable` claim
 *   per deposit, naming the build;
 * - the schema is not registered (a family this build trimmed, or a typo) →
 *   an `unverifiable` claim naming the schema and what *is* available;
 * - the report will not translate → an `unverifiable` claim carrying the
 *   reason.
 *
 * Re-stating is not optional either. A family that records an input basis
 * (CAM does) is always re-stated before its claims are read, so editing the
 * G-code turns `Holds` into `Stale` on the very next `build_receipt` rather
 * than certifying a job that no longer exists.
 */

import type { Document, ReceiptClaim } from "@vcad/ir";
import type { Engine } from "@vcad/engine";

/** A deposit as it lives on the document. Mirrors `vcad_ir::ClaimReport`. */
export interface ClaimReportEntry {
  /** Stable id; re-depositing under the same id replaces the report. */
  id: string;
  /** The family's schema tag — the registry key. */
  schema: string;
  /** What this report is about, for a human reading the ledger. */
  label?: string;
  /** RFC 3339 timestamp of the oracle run. */
  generated_at?: string;
  /** The family's serialized claim set: JSON **text**, not an object. */
  report: string;
  /** The live inputs the claims rest on, as JSON text, by basis key. */
  inputs?: Record<string, string>;
}

/** The `claims` block a CAM entry point answers, ready to be deposited. */
export interface ClaimDeposit {
  schema: string;
  report: string;
  inputs?: Record<string, string>;
  summary?: Record<string, unknown>;
}

/** The registry's view of one family, as the kernel reports it. */
export interface RegistryFamily {
  schema: string;
  domain: string;
  crate: string;
  summary: string;
  native_only: boolean;
  stale_aware: boolean;
  bindable: boolean;
}

const REGISTRY_ORACLE = {
  id: "vcad-claim-registry",
  version: "unknown" as const,
};

/** An unverifiable claim — the only honest answer when a family can't run. */
function unverifiable(
  id: string,
  domain: string,
  description: string,
  reason: string,
): ReceiptClaim {
  return {
    id,
    domain,
    description,
    oracle: REGISTRY_ORACLE,
    verdict: "unverifiable",
    details: reason,
  };
}

/** Every deposit on a document, oldest first. */
export function claimReports(doc: Document): ClaimReportEntry[] {
  const raw = (doc as { claim_reports?: ClaimReportEntry[] }).claim_reports;
  return Array.isArray(raw) ? raw : [];
}

/**
 * Store a report on the document, replacing any deposit under the same id.
 *
 * Replacement rather than accumulation is deliberate: one job has one current
 * answer. A re-posted job that appended would leave the receipt carrying both
 * the old verdict and the new one, and no way to tell which described the
 * G-code in the operator's hand.
 */
export function depositClaimReport(
  doc: Document,
  entry: ClaimReportEntry,
): void {
  const target = doc as { claim_reports?: ClaimReportEntry[] };
  const list = Array.isArray(target.claim_reports) ? target.claim_reports : [];
  const at = list.findIndex((r) => r.id === entry.id);
  if (at === -1) list.push(entry);
  else list[at] = entry;
  target.claim_reports = list;
}

/** Drop a deposit by id. Returns whether there was one. */
export function removeClaimReport(doc: Document, id: string): boolean {
  const target = doc as { claim_reports?: ClaimReportEntry[] };
  const list = target.claim_reports;
  if (!Array.isArray(list)) return false;
  const at = list.findIndex((r) => r.id === id);
  if (at === -1) return false;
  list.splice(at, 1);
  return true;
}

/**
 * Turn a tool's `claims` block into a deposit. Returns `undefined` when the
 * block is absent or malformed — a tool that produced no claims simply
 * deposits nothing, which is different from depositing an empty one.
 */
export function depositFrom(
  block: unknown,
  id: string,
  label?: string,
): ClaimReportEntry | undefined {
  if (!block || typeof block !== "object") return undefined;
  const b = block as ClaimDeposit;
  if (typeof b.schema !== "string" || typeof b.report !== "string") {
    return undefined;
  }
  return {
    id,
    schema: b.schema,
    ...(label ? { label } : {}),
    generated_at: new Date().toISOString(),
    report: b.report,
    ...(b.inputs && typeof b.inputs === "object" ? { inputs: b.inputs } : {}),
  };
}

/**
 * Refuse a deposit that does not record the inputs its claims rest on.
 *
 * Fail-closed, and deliberately at *deposit* time. A claim with no basis can
 * never be re-stated, so it can never go `Stale` — and to whoever reads the
 * receipt a month later, "never moved" is indistinguishable from "still
 * true". The producer is the only party that knows what its claims rest on,
 * and this is the last moment it is still around to say so.
 *
 * Nothing deposits without inputs today, so this blocks nobody now; it is
 * here so the solver families cannot be wired up later without naming a
 * basis. Throws with the registry's own message, which lists what was
 * required and what was missing.
 */
export function assertDepositHasBasis(
  engine: Engine | undefined,
  entry: ClaimReportEntry,
): void {
  if (!engine?.hasClaimRegistry?.()) {
    // No registry to ask. The floor still applies: a deposit with no inputs
    // is refused whether or not anyone can tell us which ones it needed.
    if (!entry.inputs || Object.keys(entry.inputs).length === 0) {
      throw new Error(
        `a '${entry.schema}' deposit must record the inputs its claims rest on, and this one records none. A claim with no basis can never go stale, so it would keep certifying a design that has since been edited.`,
      );
    }
    return;
  }
  const out = engine.receiptCheckDeposit<{ ok?: boolean; error?: string }>(
    entry.schema,
    entry.report,
    entry.inputs ?? {},
  );
  if (typeof out.error === "string") throw new Error(out.error);
}

/** The families this kernel build serves, or `[]` when it carries none. */
export function registryFamilies(engine?: Engine): RegistryFamily[] {
  if (!engine || !engine.hasClaimRegistry?.()) return [];
  try {
    const out = engine.receiptFamilies<{ families?: RegistryFamily[] }>();
    return Array.isArray(out.families) ? out.families : [];
  } catch {
    return [];
  }
}

/** The family entry for a schema, when this build carries it. */
function familyFor(
  engine: Engine | undefined,
  schema: string,
): RegistryFamily | undefined {
  return registryFamilies(engine).find((f) => f.schema === schema);
}

/** Holds / Stale / Violated, the vocabulary `verify_receipt` reports in. */
export type ClaimReportStatus = "Holds" | "Stale" | "Violated";

/** What re-stating one deposit found. */
export interface RestatedEntry {
  /** The deposit this is about. */
  id: string;
  schema: string;
  label?: string;
  /** Its claims, after re-stating, in unified form. */
  claims: ReceiptClaim[];
  /** Worst-wins verdict over those claims. */
  status: ClaimReportStatus;
  /** Names of the claims the re-state turned stale. */
  stale_claims: string[];
  /** The basis keys that moved. */
  drifted_inputs: string[];
  /** Ids of the claims that came back failed. */
  violated_claims: string[];
}

/**
 * Re-state one deposit against today's inputs and read its claims.
 *
 * **This is the single code path `build_receipt` and `verify_receipt` share.**
 * They used to be able to disagree — one re-stated, the other did not — which
 * is the worst possible shape for a staleness check: the tool an operator
 * reaches for to ask "is this still good?" was the one that would not notice.
 * Everything either tool reports about a deposit comes from here.
 *
 * The re-state runs first and always, for any family that supports it: a
 * stored report says what *was* true of the inputs it was made against, and
 * only comparing those against today's inputs can tell a live claim from one
 * about a job that has been edited out from under it.
 */
export function restateEntry(
  engine: Engine,
  entry: ClaimReportEntry,
): RestatedEntry {
  const base = {
    id: entry.id,
    schema: entry.schema,
    ...(entry.label ? { label: entry.label } : {}),
  };
  /** A deposit that could not be read at all: unverifiable, and Stale to
   *  `verify_receipt` — never Holds, which is the only unsafe answer. */
  const blocked = (domain: string, reason: string): RestatedEntry => ({
    ...base,
    claims: [
      unverifiable(
        `claims.${entry.schema}`,
        domain,
        `claims from ${entry.schema}`,
        reason,
      ),
    ],
    status: "Stale",
    stale_claims: [],
    drifted_inputs: [],
    violated_claims: [],
  });

  const family = familyFor(engine, entry.schema);
  if (!family) {
    const known = registryFamilies(engine)
      .map((f) => f.schema)
      .join(", ");
    return blocked(
      "verification",
      `no claim family is registered for '${entry.schema}' in this kernel build` +
        (known ? ` — it carries [${known}]` : " — it carries none") +
        ". The report is kept on the document; rebuild the kernel WASM with that family enabled to certify it.",
    );
  }

  let report = entry.report;
  let staleClaims: string[] = [];
  let driftedInputs: string[] = [];
  if (family.stale_aware) {
    const restated = engine.receiptRestate<{
      report?: string;
      stale_claims?: string[];
      drifted_inputs?: string[];
      error?: string;
    }>(entry.schema, report, entry.inputs ?? {});
    if (typeof restated.error === "string") {
      return blocked(
        family.domain,
        `the stored report could not be re-stated against the current inputs: ${restated.error}`,
      );
    }
    if (typeof restated.report === "string") report = restated.report;
    staleClaims = restated.stale_claims ?? [];
    driftedInputs = restated.drifted_inputs ?? [];
  }

  const out = engine.receiptClaimsFor<{
    claims?: ReceiptClaim[];
    error?: string;
  }>(entry.schema, report);
  if (typeof out.error === "string" || !Array.isArray(out.claims)) {
    return blocked(
      family.domain,
      out.error ?? "the family returned no claims for a report it accepted",
    );
  }
  // A deposit's id disambiguates two jobs on one document: without it, two
  // `cam.job.no_gouge` claims would be indistinguishable in the ledger.
  const claims = out.claims.map((c) => ({
    ...c,
    subject: c.subject ?? entry.label ?? entry.id,
  }));
  const violated = claims.filter((c) => c.verdict === "fail").map((c) => c.id);
  // Worst wins, and a failed claim outranks a stale one: "this job cuts into
  // the part" is actionable now, where "this job has been edited" is a
  // request to re-run.
  const status: ClaimReportStatus =
    violated.length > 0 ? "Violated" : staleClaims.length > 0 ? "Stale" : "Holds";

  return {
    ...base,
    claims,
    status,
    stale_claims: staleClaims,
    drifted_inputs: driftedInputs,
    violated_claims: violated,
  };
}

/**
 * Every deposit on this document, re-stated. The shared entry point behind
 * both `build_receipt` (which wants the claims) and `verify_receipt` (which
 * wants the verdicts).
 */
export function restateAll(doc: Document, engine?: Engine): RestatedEntry[] {
  const entries = claimReports(doc);
  if (entries.length === 0) return [];

  if (!engine || !engine.hasClaimRegistry?.()) {
    // Deposits exist and cannot be read. Saying nothing would be the one
    // outcome that reads as clean, so say it loudly, once per deposit.
    return entries.map((e) => ({
      id: e.id,
      schema: e.schema,
      ...(e.label ? { label: e.label } : {}),
      claims: [
        unverifiable(
          `claims.${e.schema}`,
          "verification",
          `claims from ${e.schema}`,
          "this kernel build carries no claim registry, so the deposited report could not be translated — rebuild packages/kernel-wasm",
        ),
      ],
      status: "Stale" as const,
      stale_claims: [],
      drifted_inputs: [],
      violated_claims: [],
    }));
  }

  return entries.map((entry) => {
    try {
      return restateEntry(engine, entry);
    } catch (e) {
      return {
        id: entry.id,
        schema: entry.schema,
        ...(entry.label ? { label: entry.label } : {}),
        claims: [
          unverifiable(
            `claims.${entry.schema}`,
            "verification",
            `claims from ${entry.schema}`,
            `the claim registry failed on this report: ${e instanceof Error ? e.message : String(e)}`,
          ),
        ],
        status: "Stale" as const,
        stale_claims: [],
        drifted_inputs: [],
        violated_claims: [],
      };
    }
  });
}

/**
 * Every registered family's claims for this document, merged.
 *
 * A document with no deposits yields no claims, which is exactly the point:
 * `build_receipt` on a document that never ran an oracle is unchanged from
 * before this existed.
 */
export function registryReceiptClaims(
  doc: Document,
  engine?: Engine,
): ReceiptClaim[] {
  return restateAll(doc, engine).flatMap((e) => e.claims);
}


/** What a measurement binding did. */
export interface BindResult {
  /** The deposit that was updated. */
  entry: ClaimReportEntry;
  /** Family-specific follow-ups — for CAM, the cutter compensation. */
  derived: unknown[];
  /** What happened, in a sentence. */
  note: string;
}

/**
 * Bind a measurement of the real part to a claim, and store the result.
 *
 * The deposit's own `inputs` become the binding context, which is what makes
 * the gear loop work end to end: `cam_gear` recorded the gear it predicted
 * from, so a reading over pins can be worked back through the involute into
 * the cutter offset for the next part — rather than only being told the teeth
 * came out fat.
 *
 * Throws with the family's own refusal when the measurement cannot close the
 * claim (it is arithmetic, not a dimension; the numbers are unusable; no such
 * claim). A refusal is the point: a measurement bound to the wrong claim would
 * launder a caliper reading into evidence about a G-code program.
 */
export function bindMeasurement(
  doc: Document,
  engine: Engine,
  entryId: string,
  measurements: unknown,
): BindResult {
  const entry = claimReports(doc).find((r) => r.id === entryId);
  if (!entry) {
    const have = claimReports(doc).map((r) => r.id);
    throw new Error(
      `no claim report '${entryId}' on this document` +
        (have.length
          ? `. It holds: ${have.join(", ")}.`
          : ". It holds none — run cam_job or cam_gear with this document_id first."),
    );
  }
  if (!engine.hasClaimRegistry?.()) {
    throw new Error(
      "this kernel build carries no claim registry, so a measurement cannot be bound — rebuild packages/kernel-wasm",
    );
  }
  const family = familyFor(engine, entry.schema);
  if (!family) {
    throw new Error(
      `this kernel build has no family for '${entry.schema}', so its claims cannot be closed by a measurement`,
    );
  }
  if (!family.bindable) {
    throw new Error(
      `claim family '${entry.schema}' binds no measurements: nothing in it is a dimension of a physical part`,
    );
  }

  // The inputs are JSON text on the wire; the family's binder wants values.
  const context: Record<string, unknown> = {};
  for (const [key, text] of Object.entries(entry.inputs ?? {})) {
    try {
      context[key] = JSON.parse(text) as unknown;
    } catch {
      // An input that will not parse simply is not offered as context; the
      // family says what it could not derive rather than guessing.
    }
  }

  const out = engine.receiptBind<{
    report?: string;
    derived?: unknown[];
    note?: string;
    error?: string;
  }>(entry.schema, entry.report, measurements, context);
  if (typeof out.error === "string" || typeof out.report !== "string") {
    throw new Error(
      out.error ?? "the claim family accepted the measurement but returned no report",
    );
  }

  const updated: ClaimReportEntry = { ...entry, report: out.report };
  depositClaimReport(doc, updated);
  return {
    entry: updated,
    derived: Array.isArray(out.derived) ? out.derived : [],
    note: out.note ?? "measurement bound",
  };
}
