import { behavior, type ToolDef } from "./tool-def.js";
/**
 * `check_clearance` — named clearance/clash assertions between part groups.
 *
 * The generalization of `check_enclosure_fit`'s single-purpose geometry
 * cross-check: measure the minimum separation distance (or penetration
 * depth) between two groups of parts in a CAD session, compare it against a
 * required minimum, and optionally persist the assertion on the document as
 * a named {@link ClearanceSpec}. Persisted specs are re-measured by
 * `build_receipt` (as `mech.clearance.*` DesignReceipt claims) and
 * re-verified by `verify_receipt` (Holds / Stale / Violated), so the
 * safety-critical numbers — rotor air gaps, bearing fits, screw-head
 * clearances — stop being one-off hand checks that silently rot when
 * geometry changes.
 */

import type {
  ClearanceClaim,
  ClearanceSpec,
  DesignReceipt,
  Document,
  OracleRef,
  ReceiptClaim,
  ReceiptStatus,
} from "@vcad/ir";
import type { ClearanceResult, Engine, TriangleMesh } from "@vcad/engine";
import { transformMesh } from "@vcad/engine";
import { unverifiableClaim } from "../receipt-unified.js";
import { getSession } from "./session.js";

/** Claim-id prefix shared with the Rust mech adapter (`vcad-receipt`). */
export const CLEARANCE_CLAIM_PREFIX = "mech.clearance.";

const MECH_DOMAIN = "mechanical";

/** Mirrors the enclosure-fit precedent: name the oracle honestly. */
const CLEARANCE_ORACLE: OracleRef = {
  id: "vcad-kernel/mesh-clearance",
  version: "unknown",
};

/** Re-measured distances equal to the stored value within this are "same
 *  geometry"; beyond it (but still passing) the receipt reads Stale. */
const STALE_EPS_MM = 1e-6;

/** Round distances so payloads don't carry float noise. */
const round6 = (v: number) => Math.round(v * 1e6) / 1e6;

export const checkClearanceSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "CAD session id holding the parts to measure.",
    },
    group_a: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Part ids (or part names) of the first group, e.g. the rotor.",
    },
    group_b: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Part ids (or part names) of the second group, e.g. the stator and screws.",
    },
    min_mm: {
      type: "number" as const,
      description: "Required minimum separation in mm. The check passes when the measured minimum distance is at least this value.",
    },
    label: {
      type: "string" as const,
      description:
        "Optional assertion name (e.g. 'air-gap'). When given, the spec is persisted on the document and re-verified by build_receipt / verify_receipt whenever geometry changes.",
    },
  },
  required: ["document_id", "group_a", "group_b", "min_mm"],
} as const;

/** A part resolved to its evaluated (already-placed) mesh. */
interface ResolvedPart {
  id: string;
  name?: string;
  mesh: TriangleMesh;
}

/** Evaluate a CAD session into measurable parts (id, name, placed mesh). */
function partCandidates(doc: Document, engine: Engine): ResolvedPart[] {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const out: ResolvedPart[] = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = doc.nodes[String(rootId)];
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({ id: String(rootId), name: node?.name ?? undefined, mesh });
  }
  // Assembly instances: bake the FK world transform into the part-local mesh
  // so clearances measure poses, not part-local geometry. Without this,
  // assembly-only documents had no clearance candidates at all.
  for (const inst of scene.instances ?? []) {
    const mesh = inst.transform
      ? transformMesh(inst.mesh, {
          translate: inst.transform.translation,
          rotate: inst.transform.rotation,
          scale: inst.transform.scale,
        })
      : inst.mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({
      id: inst.instanceId,
      name: inst.name ?? undefined,
      mesh,
    });
  }
  return out;
}

/** Resolve group members by part id first, then by exact part name. */
function resolveGroup(
  candidates: ResolvedPart[],
  ids: string[],
): { parts: ResolvedPart[]; missing: string[] } {
  const parts = new Map<string, ResolvedPart>();
  const missing: string[] = [];
  for (const raw of ids) {
    const wanted = String(raw);
    const found =
      candidates.find((c) => c.id === wanted) ?? candidates.find((c) => c.name === wanted);
    if (found) parts.set(found.id, found);
    else missing.push(wanted);
  }
  return { parts: [...parts.values()], missing };
}

/** The measured outcome of one group-vs-group clearance query. */
export interface GroupClearance {
  /** Signed minimum distance in mm (negative = penetration depth). */
  distance_mm: number;
  /** True when the closest pair intersects. */
  intersecting: boolean;
  /** The part pair realizing the minimum. */
  worst_pair: {
    a: { id: string; name?: string };
    b: { id: string; name?: string };
    point_a: [number, number, number];
    point_b: [number, number, number];
  };
  /** Number of part pairs measured. */
  pairs_checked: number;
  /** Resolved membership, for the payload/claim subject. */
  group_a: Array<{ id: string; name?: string }>;
  group_b: Array<{ id: string; name?: string }>;
}

/**
 * Core measurement: minimum distance between every part in `groupA` and
 * every part in `groupB` (pairwise BVH queries in the kernel), as placed in
 * the evaluated scene. Pure of MCP plumbing so `check_clearance`,
 * `build_receipt`, and `verify_receipt` share one implementation.
 */
export function computeGroupClearance(
  doc: Document,
  engine: Engine,
  groupA: string[],
  groupB: string[],
): { result?: GroupClearance; error?: string } {
  if (groupA.length === 0 || groupB.length === 0) {
    return { error: "Both `group_a` and `group_b` need at least one part id." };
  }
  let candidates: ResolvedPart[];
  try {
    candidates = partCandidates(doc, engine);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
  const a = resolveGroup(candidates, groupA);
  const b = resolveGroup(candidates, groupB);
  const missing = [...a.missing, ...b.missing];
  if (missing.length > 0) {
    const available = candidates
      .map((c) => `${c.id}${c.name ? ` (${c.name})` : ""}`)
      .join(", ");
    return {
      error: `No part with id or name ${missing.map((m) => `"${m}"`).join(", ")}. Available: ${available || "none"}`,
    };
  }
  const overlap = a.parts.filter((p) => b.parts.some((q) => q.id === p.id));
  if (overlap.length > 0) {
    return {
      error: `Parts cannot appear in both groups: ${overlap.map((p) => p.id).join(", ")}`,
    };
  }

  let worst:
    | { a: ResolvedPart; b: ResolvedPart; r: ClearanceResult }
    | undefined;
  let pairs = 0;
  for (const pa of a.parts) {
    for (const pb of b.parts) {
      let r: ClearanceResult;
      try {
        r = engine.meshClearance(pa.mesh, pb.mesh);
      } catch (e) {
        return { error: e instanceof Error ? e.message : String(e) };
      }
      pairs += 1;
      if (!worst || r.distance < worst.r.distance) {
        worst = { a: pa, b: pb, r };
      }
    }
  }
  if (!worst) {
    return { error: "No measurable part pairs (all resolved parts have empty meshes)." };
  }

  const named = (p: ResolvedPart) => ({ id: p.id, ...(p.name ? { name: p.name } : {}) });
  return {
    result: {
      distance_mm: round6(worst.r.distance),
      intersecting: worst.r.intersecting,
      worst_pair: {
        a: named(worst.a),
        b: named(worst.b),
        point_a: worst.r.pointA.map(round6) as [number, number, number],
        point_b: worst.r.pointB.map(round6) as [number, number, number],
      },
      pairs_checked: pairs,
      group_a: a.parts.map(named),
      group_b: b.parts.map(named),
    },
  };
}

/** Insert or replace the named spec on the document (upsert by label). */
function upsertClearanceSpec(doc: Document, spec: ClearanceSpec): void {
  const specs = doc.clearance_specs ?? [];
  const idx = specs.findIndex((s) => s.label === spec.label);
  if (idx >= 0) specs[idx] = spec;
  else specs.push(spec);
  doc.clearance_specs = specs;
}

/** `check_clearance` MCP handler. */
export async function checkClearance(args: Record<string, unknown>, engine: Engine) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) return err("Pass `document_id` (the CAD session).");
  const groupA = stringArray(args.group_a);
  const groupB = stringArray(args.group_b);
  if (!groupA || !groupB) {
    return err("Pass `group_a` and `group_b` as arrays of part ids (or part names).");
  }
  const minMm = typeof args.min_mm === "number" ? args.min_mm : NaN;
  if (!Number.isFinite(minMm)) return err("Pass `min_mm`, the required minimum separation in mm.");
  const label = typeof args.label === "string" && args.label.trim() ? args.label.trim() : undefined;

  const doc = getSession(documentId);
  const { result, error } = computeGroupClearance(doc, engine, groupA, groupB);
  if (error || !result) return err(error ?? "Clearance could not be computed.");

  const pass = result.distance_mm >= minMm;
  let specSaved = false;
  if (label) {
    // Persist by resolved part ids so the assertion survives renames.
    upsertClearanceSpec(doc, {
      label,
      group_a: result.group_a.map((p) => p.id),
      group_b: result.group_b.map((p) => p.id),
      min_mm: minMm,
    });
    specSaved = true;
  }

  const payload = {
    success: true,
    document_id: documentId,
    ...(label ? { label } : {}),
    required_mm: minMm,
    measured_mm: result.distance_mm,
    pass,
    intersecting: result.intersecting,
    worst_pair: result.worst_pair,
    pairs_checked: result.pairs_checked,
    group_a: result.group_a,
    group_b: result.group_b,
    ...(specSaved
      ? {
          spec_saved: true,
          note: "Spec persisted on the document — build_receipt emits it as a mech.clearance claim and verify_receipt re-verifies it as Holds/Stale/Violated.",
        }
      : {}),
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: {
      clearance: payload,
      document_id: documentId,
    },
  };
}

/**
 * Measure every persisted clearance spec and emit unified receipt claims,
 * mirroring the Rust mech adapter (`vcad_receipt::mechanical::clearance_claims`):
 * id `mech.clearance.<label>`, required as predicted, measured as measured,
 * and the typed {@link ClearanceClaim} riding in `details` so a stored
 * receipt re-verifies without external context. A spec that cannot be
 * measured (missing part, empty mesh) yields an unverifiable claim —
 * fail-closed, never a silent skip.
 */
export function clearanceReceiptClaims(doc: Document, engine: Engine | undefined): ReceiptClaim[] {
  const specs = doc.clearance_specs ?? [];
  return specs.map((spec) => {
    const id = `${CLEARANCE_CLAIM_PREFIX}${spec.label}`;
    const description = `clearance "${spec.label}" at least ${spec.min_mm} mm`;
    const subject = `${spec.group_a.join("+")} vs ${spec.group_b.join("+")}`;
    if (!engine) {
      return {
        ...unverifiableClaim(
          id,
          MECH_DOMAIN,
          description,
          CLEARANCE_ORACLE,
          "clearance measurement needs the kernel engine; unavailable in this context",
        ),
        subject,
      };
    }
    const { result, error } = computeGroupClearance(doc, engine, spec.group_a, spec.group_b);
    if (error || !result) {
      return {
        ...unverifiableClaim(
          id,
          MECH_DOMAIN,
          description,
          CLEARANCE_ORACLE,
          error ?? "clearance could not be computed",
        ),
        subject,
      };
    }
    const assertion: ClearanceClaim = {
      label: spec.label,
      group_a: spec.group_a,
      group_b: spec.group_b,
      required_mm: spec.min_mm,
      measured_mm: result.distance_mm,
      holds: result.distance_mm >= spec.min_mm,
    };
    return {
      id,
      domain: MECH_DOMAIN,
      description,
      subject,
      oracle: CLEARANCE_ORACLE,
      verdict: assertion.holds ? ("pass" as const) : ("fail" as const),
      predicted: { value: spec.min_mm, unit: "mm" },
      measured: { value: result.distance_mm, unit: "mm" },
      details: JSON.stringify(assertion),
    };
  });
}

/** Per-assertion outcome of re-verifying a stored clearance claim. */
export interface ClearanceCheckStatus {
  label: string;
  status: ReceiptStatus;
  required_mm?: number;
  /** Distance recorded in the stored receipt. */
  stored_mm?: number;
  /** Distance measured against the current document. */
  measured_mm?: number;
  reason?: string;
}

/**
 * Re-verify the `mech.clearance.*` claims of a stored DesignReceipt against
 * the current document. Per claim: a spec that no longer holds (or can no
 * longer be measured — fail-closed) is Violated; one that still holds but
 * measures a different distance is Stale; an unchanged measurement Holds.
 * The rollup takes the worst: Violated > Stale > Holds.
 */
export function verifyClearanceClaims(
  doc: Document,
  engine: Engine,
  receipt: DesignReceipt,
): { status: ReceiptStatus; checks: ClearanceCheckStatus[] } {
  const checks: ClearanceCheckStatus[] = [];
  for (const claim of receipt.claims ?? []) {
    if (!claim.id.startsWith(CLEARANCE_CLAIM_PREFIX)) continue;
    const label = claim.id.slice(CLEARANCE_CLAIM_PREFIX.length);
    const stored = parseStoredClaim(claim);
    if (!stored) {
      checks.push({
        label,
        status: "Violated",
        reason: "stored claim carries no re-verifiable payload (details is not a ClearanceClaim)",
      });
      continue;
    }
    const { result, error } = computeGroupClearance(doc, engine, stored.group_a, stored.group_b);
    if (error || !result) {
      checks.push({
        label,
        status: "Violated",
        required_mm: stored.required_mm,
        stored_mm: stored.measured_mm,
        reason: error ?? "clearance could not be re-measured",
      });
      continue;
    }
    const measured = result.distance_mm;
    if (measured < stored.required_mm) {
      checks.push({
        label,
        status: "Violated",
        required_mm: stored.required_mm,
        stored_mm: stored.measured_mm,
        measured_mm: measured,
        reason: `measured ${measured} mm is below the required ${stored.required_mm} mm`,
      });
    } else if (Math.abs(measured - stored.measured_mm) > STALE_EPS_MM) {
      checks.push({
        label,
        status: "Stale",
        required_mm: stored.required_mm,
        stored_mm: stored.measured_mm,
        measured_mm: measured,
        reason: "geometry changed since the receipt was built, but the clearance still holds",
      });
    } else {
      checks.push({
        label,
        status: "Holds",
        required_mm: stored.required_mm,
        stored_mm: stored.measured_mm,
        measured_mm: measured,
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

/** Does this unified receipt carry any clearance claims to re-verify? */
export function hasClearanceClaims(receipt: DesignReceipt): boolean {
  return (receipt.claims ?? []).some((c) => c.id.startsWith(CLEARANCE_CLAIM_PREFIX));
}

function parseStoredClaim(claim: ReceiptClaim): ClearanceClaim | undefined {
  if (!claim.details) return undefined;
  try {
    const parsed = JSON.parse(claim.details) as Partial<ClearanceClaim>;
    if (
      typeof parsed.label === "string" &&
      Array.isArray(parsed.group_a) &&
      Array.isArray(parsed.group_b) &&
      typeof parsed.required_mm === "number" &&
      typeof parsed.measured_mm === "number"
    ) {
      return parsed as ClearanceClaim;
    }
  } catch {
    /* not JSON — fall through to undefined */
  }
  return undefined;
}

function stringArray(v: unknown): string[] | undefined {
  if (!Array.isArray(v)) return undefined;
  return v.map((x) => String(x));
}

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "check_clearance",
    pack: null,
    description:
      "Measure the minimum distance between two groups of parts in a CAD session and assert it stays above `min_mm` \u2014 air gaps, press fits, screw-head clearances. Reports the measured minimum (negative = penetration depth), the worst part pair, and pass/fail. Give it a `label` to persist the assertion on the document: build_receipt then emits it as a mech.clearance claim and verify_receipt re-verifies it as Holds / Stale / Violated when geometry changes.",
    inputSchema: checkClearanceSchema,
    handler: (a, c) => checkClearance(a, c.engine),
    behavior: behavior({ writesDoc: true }),
  },
];
