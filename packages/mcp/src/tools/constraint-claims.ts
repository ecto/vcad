/**
 * Receipt claims for document-level design constraints.
 *
 * Every persisted `DesignConstraint` doubles as a verifiable claim: the
 * `constraint.<label-or-id>` family. `build_receipt` measures each
 * constraint's residual against current geometry (Pass when it holds within
 * tolerance, Unverifiable when an anchor can't be resolved — fail-closed);
 * `verify_receipt` re-measures and classifies Holds / Stale / Violated, the
 * same contract as `mech.clearance.*`.
 */

import type {
  DesignConstraint,
  DesignReceipt,
  Document,
  OracleRef,
  ReceiptClaim,
  ReceiptStatus,
} from "@vcad/ir";
import { checkDesignConstraints } from "@vcad/engine";

/** Claim-id prefix for the design-constraint family. */
export const CONSTRAINT_CLAIM_PREFIX = "constraint.";

const CONSTRAINT_DOMAIN = "constraint";

const CONSTRAINT_ORACLE: OracleRef = {
  id: "vcad-design-constraints/residual",
  version: "unknown",
};

/** Residual (mm or degrees) within which a constraint counts as holding. */
export const CONSTRAINT_HOLD_TOL = 1e-4;

/** Residual drift beyond this (while still holding) reads as Stale. */
const STALE_EPS = 1e-6;

/** Payload stored in a claim's `details` for later re-verification. */
interface StoredConstraintClaim {
  constraint_id: string;
  type: string;
  residual: number;
  driven: boolean;
}

function claimIdFor(c: DesignConstraint): string {
  return `${CONSTRAINT_CLAIM_PREFIX}${c.label ?? c.id}`;
}

function describe(c: DesignConstraint): string {
  const kind = (c.kind as { type?: string }).type ?? "constraint";
  return `${c.driven ? "driven " : ""}${kind} constraint "${c.label ?? c.id}" holds`;
}

/**
 * One claim per persisted constraint, measured against current geometry.
 * Fail-closed: a constraint whose residual can't be computed (lost part
 * anchor, bad ref) is Unverifiable, never silently passing.
 */
export async function constraintReceiptClaims(doc: Document): Promise<ReceiptClaim[]> {
  const constraints = doc.constraints ?? [];
  if (constraints.length === 0) return [];
  const outcome = await checkDesignConstraints(doc);
  const byId = new Map<string, { residual: number; driven: boolean }>();
  if (outcome.status === "ok") {
    for (const r of outcome.value.residuals) byId.set(r.id, r);
  }
  return constraints.map((c) => {
    const id = claimIdFor(c);
    const base = {
      id,
      domain: CONSTRAINT_DOMAIN,
      description: describe(c),
      subject: c.id,
      oracle: CONSTRAINT_ORACLE,
    };
    if (outcome.status !== "ok") {
      return {
        ...base,
        verdict: "unverifiable" as const,
        details: `constraint solver unavailable: ${outcome.reason}`,
      };
    }
    const r = byId.get(c.id);
    if (!r) {
      return {
        ...base,
        verdict: "unverifiable" as const,
        details: "residual could not be measured (unresolvable anchor?)",
      };
    }
    const holds = c.driven || r.residual <= CONSTRAINT_HOLD_TOL;
    const stored: StoredConstraintClaim = {
      constraint_id: c.id,
      type: (c.kind as { type?: string }).type ?? "unknown",
      residual: r.residual,
      driven: c.driven ?? false,
    };
    return {
      ...base,
      verdict: holds ? ("pass" as const) : ("fail" as const),
      measured: { value: r.residual, unit: "mm" },
      details: JSON.stringify(stored),
    };
  });
}

/**
 * Drop constraints whose board-outline vertex/edge indices no longer exist
 * (a rewritten outline invalidates indices). Returns the dropped ids so the
 * mutating tool can report them. Call after `set_board_outline`.
 */
export function pruneOutlineConstraints(
  doc: Document,
  outlineVertexCount: (node: number) => number | undefined,
): string[] {
  const constraints = doc.constraints ?? [];
  const dropped: string[] = [];
  doc.constraints = constraints.filter((c) => {
    for (const v of Object.values(c.kind as unknown as Record<string, unknown>)) {
      const a = v as { kind?: string; node?: number; index?: number };
      if (
        a &&
        typeof a === "object" &&
        (a.kind === "pcbOutlineVertex" || a.kind === "pcbOutlineEdge")
      ) {
        const n = outlineVertexCount(a.node ?? -1);
        if (n !== undefined && (a.index ?? 0) >= n) {
          dropped.push(c.id);
          return false;
        }
      }
    }
    return true;
  });
  return dropped;
}

/** Standard "constraints may now be violated" warning for board mutators. */
export function constraintStaleWarning(doc: Document): string | undefined {
  const n = doc.constraints?.length ?? 0;
  if (n === 0) return undefined;
  return `${n} design constraint(s) exist and may now be violated — run solve_constraints (or list_constraints to inspect)`;
}

/** Does a receipt carry any constraint claims? */
export function hasConstraintClaims(receipt: DesignReceipt): boolean {
  return (receipt.claims ?? []).some((c) => c.id.startsWith(CONSTRAINT_CLAIM_PREFIX));
}

/** Per-constraint re-verification status. */
export interface ConstraintCheckStatus {
  label: string;
  status: "Holds" | "Stale" | "Violated";
  residual?: number;
  stored_residual?: number;
  reason?: string;
}

function worst(statuses: ConstraintCheckStatus[]): ReceiptStatus {
  if (statuses.some((s) => s.status === "Violated")) return "Violated";
  if (statuses.some((s) => s.status === "Stale")) return "Stale";
  return "Holds";
}

/**
 * Re-verify every constraint claim in a receipt against current geometry:
 * Violated (residual exceeds tolerance, or unmeasurable — fail-closed),
 * Stale (holds, but the residual moved from the stored snapshot), Holds.
 */
export async function verifyConstraintClaims(
  doc: Document,
  receipt: DesignReceipt,
): Promise<{ status: ReceiptStatus; checks: ConstraintCheckStatus[] }> {
  const checks: ConstraintCheckStatus[] = [];
  const outcome = await checkDesignConstraints(doc);
  const byId = new Map<string, { residual: number; driven: boolean }>();
  if (outcome.status === "ok") {
    for (const r of outcome.value.residuals) byId.set(r.id, r);
  }
  for (const claim of receipt.claims ?? []) {
    if (!claim.id.startsWith(CONSTRAINT_CLAIM_PREFIX)) continue;
    const label = claim.id.slice(CONSTRAINT_CLAIM_PREFIX.length);
    let stored: StoredConstraintClaim | undefined;
    try {
      const parsed = claim.details ? (JSON.parse(claim.details) as StoredConstraintClaim) : undefined;
      if (parsed && typeof parsed.constraint_id === "string" && typeof parsed.residual === "number") {
        stored = parsed;
      }
    } catch {
      /* not JSON */
    }
    if (!stored) {
      checks.push({
        label,
        status: "Violated",
        reason: "stored claim carries no re-verifiable payload",
      });
      continue;
    }
    if (outcome.status !== "ok") {
      checks.push({
        label,
        status: "Violated",
        stored_residual: stored.residual,
        reason: `cannot re-measure: ${outcome.reason}`,
      });
      continue;
    }
    const now = byId.get(stored.constraint_id);
    if (!now) {
      checks.push({
        label,
        status: "Violated",
        stored_residual: stored.residual,
        reason: "constraint no longer exists on the document (or its anchor is unresolvable)",
      });
      continue;
    }
    if (!stored.driven && now.residual > CONSTRAINT_HOLD_TOL) {
      checks.push({
        label,
        status: "Violated",
        residual: now.residual,
        stored_residual: stored.residual,
        reason: `residual ${now.residual} exceeds tolerance ${CONSTRAINT_HOLD_TOL}`,
      });
    } else if (Math.abs(now.residual - stored.residual) > STALE_EPS) {
      checks.push({
        label,
        status: "Stale",
        residual: now.residual,
        stored_residual: stored.residual,
        reason: "still holds, but geometry moved since the receipt was built",
      });
    } else {
      checks.push({ label, status: "Holds", residual: now.residual });
    }
  }
  return { status: worst(checks), checks };
}
