/**
 * predict_physics tool — two-tier static structural analysis.
 *
 * The fast inner loop of physics-validated generation: voxel FEA over a
 * part's volume (or a box) under given loads and supports. `fidelity`
 * picks the tier — `predict` answers in ~100 ms at coarse resolution and
 * stamps every claim `basis: predicted`; `verify` re-runs the SAME solver
 * at fine resolution and stamps `basis: verified`. A receipt whose passing
 * claims rest on predictions rolls up `provisional`, never `pass`
 * (crates/vcad-receipt) — predictions steer, verification certifies.
 *
 * Give a run a `label` (with limits) and the assertion persists on the
 * document as a {@link PhysicsSpec}, mirroring `check_clearance`: persisted
 * specs are re-solved by `build_receipt` (as `physics.static.<label>.*`
 * DesignReceipt claims) and re-verified by `verify_receipt`
 * (Holds / Stale / Violated), so structural limits stop being one-off hand
 * checks that silently rot when geometry changes.
 */

import type { Engine, StaticAnalysisSpec, StaticAnalysisResult } from "@vcad/engine";
import type {
  DesignReceipt,
  Document,
  OracleRef,
  PhysicsSpec,
  ReceiptClaim,
  ReceiptStatus,
} from "@vcad/ir";
import { getSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { resolvePartMesh } from "./topopt.js";
import { RECEIPT_SCHEMA, summarize, unverifiableClaim } from "../receipt-unified.js";

const PHYSICS_DOMAIN = "mechanical";
const ORACLE: OracleRef = { id: "vcad-kernel-topopt/static-fea", version: "0.9.4" };

/** Claim-id prefix for persisted physics specs (`physics.static.<label>.<metric>`). */
export const PHYSICS_CLAIM_PREFIX = "physics.static.";

/** Resolution per fidelity tier. Same solver; the grid is the only dial. */
const TIER_RESOLUTION = { predict: 32, verify: 72 } as const;

/** The voxel FEA is deterministic for identical inputs, so a re-solve on
 *  unchanged geometry reproduces the stored value exactly; any relative
 *  difference beyond float noise means the geometry (or grid) changed. */
const STALE_REL_EPS = 1e-9;

const regionSchema = {
  type: "object" as const,
  required: ["min", "max"],
  properties: {
    min: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Minimum corner [x, y, z] in mm.",
    },
    max: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Maximum corner [x, y, z] in mm.",
    },
  },
};

export const predictPhysicsSchema = {
  type: "object" as const,
  required: ["loads", "supports"],
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session document. Required with `part` or `label`.",
    },
    part: {
      type: "string" as const,
      description:
        "Part id or name to analyze (its evaluated volume is voxelized). " +
        "Mutually exclusive with `domain_box`.",
    },
    domain_box: {
      ...regionSchema,
      description:
        "Analyze a solid axis-aligned box (mm, world frame, Z-up) instead " +
        "of a part. Mutually exclusive with `part`.",
    },
    loads: {
      type: "array" as const,
      minItems: 1,
      description:
        "Loads: total force vectors (N) distributed over the grid nodes in " +
        "each world-frame box region. A zero-thickness box selects the " +
        "nearest plane of nodes.",
      items: {
        type: "object" as const,
        required: ["region", "force"],
        properties: {
          region: regionSchema,
          force: {
            type: "array" as const,
            items: { type: "number" as const },
            minItems: 3,
            maxItems: 3,
            description: "Total force [fx, fy, fz] in N.",
          },
        },
      },
    },
    supports: {
      type: "array" as const,
      minItems: 1,
      description: "Fixed (anchored) regions.",
      items: {
        type: "object" as const,
        required: ["region"],
        properties: {
          region: regionSchema,
          fix: {
            type: "array" as const,
            items: { type: "boolean" as const },
            minItems: 3,
            maxItems: 3,
            description:
              "Which translations are fixed [x, y, z]; default all true.",
          },
        },
      },
    },
    fidelity: {
      type: "string" as const,
      enum: ["predict", "verify"],
      description:
        "`predict` (default): coarse fast solve, claims stamped " +
        "basis=predicted — good enough to steer a design. `verify`: fine " +
        "solve with the same oracle, claims stamped basis=verified — good " +
        "enough to certify. A receipt passing only on predicted claims " +
        "reads `provisional`, never `pass`.",
    },
    resolution: {
      type: "number" as const,
      description:
        "Override voxels along the longest axis (predict=32, verify=72 by " +
        "default). Below ~4 elements through the thinnest section, bending " +
        "results are unreliable.",
    },
    youngs_modulus_mpa: {
      type: "number" as const,
      description: "Young's modulus in MPa. Default 69000 (6061 aluminum).",
    },
    poisson: {
      type: "number" as const,
      description: "Poisson's ratio. Default 0.33.",
    },
    max_displacement_mm: {
      type: "number" as const,
      description:
        "Optional limit: assert max displacement ≤ this (claim " +
        "physics.static.displacement).",
    },
    max_von_mises_mpa: {
      type: "number" as const,
      description:
        "Optional limit: assert max von Mises stress ≤ this (claim " +
        "physics.static.stress). E.g. yield/safety-factor.",
    },
    label: {
      type: "string" as const,
      description:
        "Optional assertion name (e.g. 'bracket-load'). When given (with at " +
        "least one limit and a document_id), the spec — loads, supports, " +
        "material, limits, fidelity — is persisted on the document and " +
        "re-solved by build_receipt / verify_receipt whenever geometry " +
        "changes.",
    },
  },
};

interface PhysicsArgs {
  document_id?: string;
  part?: string;
  domain_box?: { min: [number, number, number]; max: [number, number, number] };
  loads?: StaticAnalysisSpec["loads"];
  supports?: StaticAnalysisSpec["supports"];
  fidelity?: "predict" | "verify";
  resolution?: number;
  youngs_modulus_mpa?: number;
  poisson?: number;
  max_displacement_mm?: number;
  max_von_mises_mpa?: number;
  label?: string;
}

const round5 = (v: number) => Number(v.toPrecision(5));

/** The re-runnable payload riding in a persisted physics claim's `details`,
 *  so a stored receipt re-verifies without external context. */
export interface StoredPhysicsClaim {
  spec: PhysicsSpec;
  /** Which limit this claim asserts. */
  metric: "displacement" | "stress";
  limit: number;
  measured: number;
}

/**
 * Core solve for a spec: resolve the part (or box) and run the voxel FEA at
 * the spec's fidelity. Pure of MCP plumbing so `predict_physics`,
 * `build_receipt`, and `verify_receipt` share one implementation.
 */
export function solvePhysicsSpec(
  spec: PhysicsSpec,
  engine: Engine,
  doc: Document | undefined,
): { result?: StaticAnalysisResult; subject?: string; error?: string } {
  const analysisSpec: StaticAnalysisSpec = {
    loads: spec.loads,
    supports: spec.supports,
    resolution: spec.resolution ?? TIER_RESOLUTION[spec.fidelity],
    youngs_modulus_mpa: spec.youngs_modulus_mpa,
    poisson: spec.poisson,
  };
  try {
    if (spec.part) {
      if (!doc) return { error: "spec targets a part but no document is available" };
      const resolved = resolvePartMesh(doc, engine, spec.part);
      return {
        result: engine.analyzeStaticsMesh(resolved.mesh, analysisSpec),
        subject: `part:${resolved.name ?? spec.part}`,
      };
    }
    if (!spec.domain_box) return { error: "spec has neither `part` nor `domain_box`" };
    return {
      result: engine.analyzeStaticsBox(spec.domain_box.min, spec.domain_box.max, analysisSpec),
      subject: "domain_box",
    };
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
}

/** Insert or replace the named spec on the document (upsert by label). */
function upsertPhysicsSpec(doc: Document, spec: PhysicsSpec): void {
  const specs = doc.physics_specs ?? [];
  const idx = specs.findIndex((s) => s.label === spec.label);
  if (idx >= 0) specs[idx] = spec;
  else specs.push(spec);
  doc.physics_specs = specs;
}

/** The limits a spec asserts, as (metric, claim-id-suffix, limit) tuples. */
function specLimits(spec: PhysicsSpec): Array<{
  metric: StoredPhysicsClaim["metric"];
  limit: number;
  unit: string;
  what: string;
}> {
  const out: ReturnType<typeof specLimits> = [];
  if (spec.max_displacement_mm != null) {
    out.push({
      metric: "displacement",
      limit: spec.max_displacement_mm,
      unit: "mm",
      what: "max displacement under load",
    });
  }
  if (spec.max_von_mises_mpa != null) {
    out.push({
      metric: "stress",
      limit: spec.max_von_mises_mpa,
      unit: "MPa",
      what: "max von Mises stress",
    });
  }
  return out;
}

function measuredFor(result: StaticAnalysisResult, metric: StoredPhysicsClaim["metric"]): number {
  return metric === "displacement" ? result.maxDisplacementMm : result.maxVonMisesMpa;
}

function limitClaim(
  id: string,
  description: string,
  subject: string | undefined,
  limit: number,
  actual: number,
  unit: string,
  basis: "predicted" | "verified",
  converged: boolean,
  details?: string,
): ReceiptClaim {
  if (!converged || !Number.isFinite(actual)) {
    return {
      ...unverifiableClaim(id, PHYSICS_DOMAIN, description, ORACLE, "FE solve did not converge"),
      basis,
      subject,
    };
  }
  return {
    id,
    domain: PHYSICS_DOMAIN,
    description,
    subject,
    oracle: ORACLE,
    verdict: actual <= limit ? "pass" : "fail",
    basis,
    predicted: { value: limit, unit },
    measured: { value: round5(actual), unit },
    ...(details ? { details } : {}),
  };
}

/** Emit the claims for one solved spec (labeled → `physics.static.<label>.<metric>`). */
function claimsForSpec(
  spec: PhysicsSpec,
  solved: ReturnType<typeof solvePhysicsSpec>,
): ReceiptClaim[] {
  const basis = spec.fidelity === "verify" ? ("verified" as const) : ("predicted" as const);
  return specLimits(spec).map(({ metric, limit, unit, what }) => {
    const id = spec.label
      ? `${PHYSICS_CLAIM_PREFIX}${spec.label}.${metric}`
      : `${PHYSICS_CLAIM_PREFIX}${metric === "displacement" ? "displacement" : "stress"}`;
    const description = spec.label
      ? `physics "${spec.label}": ${what} ≤ ${limit} ${unit}`
      : `${what} ≤ ${limit} ${unit}`;
    if (solved.error || !solved.result) {
      return {
        ...unverifiableClaim(
          id,
          PHYSICS_DOMAIN,
          description,
          ORACLE,
          solved.error ?? "static analysis could not run",
        ),
        basis,
        ...(solved.subject ? { subject: solved.subject } : {}),
      };
    }
    const measured = measuredFor(solved.result, metric);
    const stored: StoredPhysicsClaim = { spec, metric, limit, measured: round5(measured) };
    return limitClaim(
      id,
      description,
      solved.subject,
      limit,
      measured,
      unit,
      basis,
      solved.result.converged,
      JSON.stringify(stored),
    );
  });
}

/**
 * Re-solve every persisted physics spec and emit unified receipt claims,
 * mirroring {@link clearanceReceiptClaims}: id `physics.static.<label>.<metric>`,
 * limit as predicted, solve result as measured, basis from the spec's stored
 * fidelity, and the typed {@link StoredPhysicsClaim} riding in `details` so a
 * stored receipt re-verifies without external context. A spec that cannot be
 * solved (missing part, non-converged solve) yields an unverifiable claim —
 * fail-closed, never a silent skip.
 */
export function physicsReceiptClaims(doc: Document, engine: Engine | undefined): ReceiptClaim[] {
  const specs = doc.physics_specs ?? [];
  return specs.flatMap((spec) => {
    if (!engine) {
      return specLimits(spec).map(({ metric, limit, unit, what }) => ({
        ...unverifiableClaim(
          `${PHYSICS_CLAIM_PREFIX}${spec.label}.${metric}`,
          PHYSICS_DOMAIN,
          `physics "${spec.label}": ${what} ≤ ${limit} ${unit}`,
          ORACLE,
          "static analysis needs the kernel engine; unavailable in this context",
        ),
        basis: spec.fidelity === "verify" ? ("verified" as const) : ("predicted" as const),
      }));
    }
    return claimsForSpec(spec, solvePhysicsSpec(spec, engine, doc));
  });
}

/** Per-assertion outcome of re-verifying a stored physics claim. */
export interface PhysicsCheckStatus {
  /** Claim id suffix, e.g. "bracket-load.displacement". */
  label: string;
  status: ReceiptStatus;
  limit?: number;
  /** Value recorded in the stored receipt. */
  stored?: number;
  /** Value re-solved against the current document. */
  measured?: number;
  reason?: string;
}

/**
 * Re-verify the `physics.static.*` claims of a stored DesignReceipt against
 * the current document by re-running the solve at each claim's stored
 * fidelity. Per claim: a limit that no longer holds — or a spec that can no
 * longer be solved (unresolvable part, non-converged solve: fail-closed) —
 * is Violated; one that still holds but re-solves to a different value is
 * Stale; an unchanged result Holds. Rollup takes the worst.
 */
export function verifyPhysicsClaims(
  doc: Document,
  engine: Engine,
  receipt: DesignReceipt,
): { status: ReceiptStatus; checks: PhysicsCheckStatus[] } {
  const checks: PhysicsCheckStatus[] = [];
  // One spec often carries two claims (displacement + stress) — solve once.
  const solveCache = new Map<string, ReturnType<typeof solvePhysicsSpec>>();
  for (const claim of receipt.claims ?? []) {
    if (!claim.id.startsWith(PHYSICS_CLAIM_PREFIX)) continue;
    const label = claim.id.slice(PHYSICS_CLAIM_PREFIX.length);
    const stored = parseStoredClaim(claim);
    if (!stored) {
      checks.push({
        label,
        status: "Violated",
        reason:
          "stored claim carries no re-verifiable payload (details is not a StoredPhysicsClaim)",
      });
      continue;
    }
    const key = JSON.stringify(stored.spec);
    let solved = solveCache.get(key);
    if (!solved) {
      solved = solvePhysicsSpec(stored.spec, engine, doc);
      solveCache.set(key, solved);
    }
    if (solved.error || !solved.result || !solved.result.converged) {
      checks.push({
        label,
        status: "Violated",
        limit: stored.limit,
        stored: stored.measured,
        reason: solved.error ?? "FE solve did not converge",
      });
      continue;
    }
    const measured = round5(measuredFor(solved.result, stored.metric));
    if (!Number.isFinite(measured) || measured > stored.limit) {
      checks.push({
        label,
        status: "Violated",
        limit: stored.limit,
        stored: stored.measured,
        measured,
        reason: `re-solved ${measured} exceeds the limit ${stored.limit}`,
      });
    } else if (
      Math.abs(measured - stored.measured) >
      STALE_REL_EPS * Math.max(1, Math.abs(stored.measured))
    ) {
      checks.push({
        label,
        status: "Stale",
        limit: stored.limit,
        stored: stored.measured,
        measured,
        reason: "geometry changed since the receipt was built, but the limit still holds",
      });
    } else {
      checks.push({
        label,
        status: "Holds",
        limit: stored.limit,
        stored: stored.measured,
        measured,
      });
    }
  }
  const status: ReceiptStatus = checks.some((c) => c.status === "Violated")
    ? "Violated"
    : checks.some((c) => c.status === "Stale")
      ? "Stale"
      : "Holds";
  return { status, checks };
}

/** Does this unified receipt carry any physics claims to re-verify? */
export function hasPhysicsClaims(receipt: DesignReceipt): boolean {
  return (receipt.claims ?? []).some((c) => c.id.startsWith(PHYSICS_CLAIM_PREFIX));
}

function parseStoredClaim(claim: ReceiptClaim): StoredPhysicsClaim | undefined {
  if (!claim.details) return undefined;
  try {
    const parsed = JSON.parse(claim.details) as Partial<StoredPhysicsClaim>;
    const spec = parsed.spec as Partial<PhysicsSpec> | undefined;
    if (
      spec &&
      Array.isArray(spec.loads) &&
      Array.isArray(spec.supports) &&
      (spec.fidelity === "predict" || spec.fidelity === "verify") &&
      (parsed.metric === "displacement" || parsed.metric === "stress") &&
      typeof parsed.limit === "number" &&
      typeof parsed.measured === "number"
    ) {
      return parsed as StoredPhysicsClaim;
    }
  } catch {
    /* not JSON — fall through to undefined */
  }
  return undefined;
}

export function predictPhysicsTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = args as PhysicsArgs;

  if (!a.loads?.length) throw new Error("predict_physics: `loads` required");
  if (!a.supports?.length) throw new Error("predict_physics: `supports` required");
  if (!!a.part === !!a.domain_box) {
    throw new Error("predict_physics: pass exactly one of `part` or `domain_box`");
  }
  if (a.part && !a.document_id) {
    throw new Error("predict_physics: `part` requires `document_id`");
  }
  const label = typeof a.label === "string" && a.label.trim() ? a.label.trim() : undefined;
  const hasLimit = a.max_displacement_mm !== undefined || a.max_von_mises_mpa !== undefined;
  if (label && !a.document_id) {
    throw new Error("predict_physics: `label` requires `document_id` (the spec persists on it)");
  }
  if (label && !hasLimit) {
    throw new Error(
      "predict_physics: `label` requires at least one limit " +
        "(max_displacement_mm and/or max_von_mises_mpa) — a persisted spec " +
        "with nothing to assert cannot be verified",
    );
  }
  const fidelity = a.fidelity ?? "predict";
  const basis = fidelity === "verify" ? "verified" : "predicted";

  const spec: PhysicsSpec = {
    label: label ?? "",
    loads: a.loads,
    supports: a.supports,
    fidelity,
    ...(a.domain_box ? { domain_box: a.domain_box } : {}),
    ...(a.resolution !== undefined ? { resolution: a.resolution } : {}),
    ...(a.youngs_modulus_mpa !== undefined
      ? { youngs_modulus_mpa: a.youngs_modulus_mpa }
      : {}),
    ...(a.poisson !== undefined ? { poisson: a.poisson } : {}),
    ...(a.max_displacement_mm !== undefined
      ? { max_displacement_mm: a.max_displacement_mm }
      : {}),
    ...(a.max_von_mises_mpa !== undefined ? { max_von_mises_mpa: a.max_von_mises_mpa } : {}),
  };

  let documentId: string | undefined;
  let doc: Document | undefined;
  if (a.part) {
    documentId = String(a.document_id);
    doc = getSession(documentId);
    // Persist by resolved root id so the assertion survives renames.
    const resolved = resolvePartMesh(doc, engine, a.part);
    spec.part = String(doc.roots[resolved.rootIndex].root);
  } else if (a.document_id) {
    // Box runs are pure computation unless a label persists the spec.
    documentId = String(a.document_id);
    if (label) doc = getSession(documentId);
  }

  const started = performance.now();
  const solved = solvePhysicsSpec(spec, engine, doc);
  if (solved.error || !solved.result) {
    throw new Error(`predict_physics: ${solved.error ?? "static analysis failed"}`);
  }
  const result = solved.result;
  const solveMs = Math.round(performance.now() - started);

  let specSaved = false;
  if (label && doc) {
    upsertPhysicsSpec(doc, spec);
    specSaved = true;
  }

  const claims: ReceiptClaim[] = claimsForSpec(spec, solved);

  const receipt: DesignReceipt | undefined = claims.length
    ? { schema: RECEIPT_SCHEMA, document_id: documentId, claims }
    : undefined;

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            fidelity,
            basis,
            ...(label ? { label } : {}),
            solve_ms: solveMs,
            analysis: {
              max_displacement_mm: round5(result.maxDisplacementMm),
              max_displacement_at: result.maxDisplacementAt.map(round5),
              max_von_mises_mpa: round5(result.maxVonMisesMpa),
              max_stress_at: result.maxStressAt.map(round5),
              compliance_n_mm: round5(result.compliance),
              grid: result.grid,
              voxel_size_mm: round5(result.voxelSizeMm),
              converged: result.converged,
            },
            ...(receipt
              ? { receipt, summary: summarize(receipt) }
              : {
                  note:
                    "No limits asserted — pass max_displacement_mm and/or " +
                    "max_von_mises_mpa to get receipt claims.",
                }),
            ...(specSaved
              ? {
                  spec_saved: true,
                  note:
                    "Spec persisted on the document — build_receipt emits it " +
                    "as physics.static claims and verify_receipt re-solves it " +
                    "as Holds/Stale/Violated.",
                }
              : {}),
            ...(fidelity === "predict"
              ? {
                  next:
                    "Estimates only (basis=predicted → summary.verdict=" +
                    "provisional). Re-run with fidelity=\"verify\" to certify.",
                }
              : {}),
          },
          null,
          2,
        ),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "predict_physics",
    pack: null,
    description:
      "Fast static structural analysis (voxel FEA): max displacement, max von Mises stress, and " +
      "compliance for a part or box under world-frame loads (N) and supports, in ~100 ms at " +
      "fidelity=predict. Pass max_displacement_mm / max_von_mises_mpa limits to get receipt " +
      "claims: predict-tier claims carry basis=predicted and roll up as a PROVISIONAL receipt — " +
      "use them to iterate on a design cheaply. When the design settles, re-run with " +
      "fidelity=verify (same solver, fine grid) to upgrade the claims to basis=verified and a " +
      "certifiable pass. Give it a `label` to persist the assertion on the document: " +
      "build_receipt then re-solves it as physics.static claims and verify_receipt re-verifies " +
      "it as Holds / Stale / Violated when geometry changes. Loads/supports are box regions " +
      "(mm, Z-up); zero-thickness boxes select a face. Voxel FEA smears stress concentrations — " +
      "treat stress as an estimate near fillets.",
    inputSchema: predictPhysicsSchema,
    handler: (args, ctx) => predictPhysicsTool(args, ctx.engine),
    behavior: behavior({ writesDoc: true }),
  },
];
