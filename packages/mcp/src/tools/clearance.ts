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
  JointSweep,
  OracleRef,
  ReceiptClaim,
  ReceiptStatus,
  Transform3D,
} from "@vcad/ir";
import type { ClearanceResult, Engine, TriangleMesh } from "@vcad/engine";
import { solveForwardKinematics, transformMesh } from "@vcad/engine";
import { unverifiableClaim } from "../receipt-unified.js";
import { applyJointState, jointStateSchemaProp, type PoseInfo } from "./pose.js";
import { getSession } from "./session-core.js";
import { asBool } from "./arg-coerce.js";

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

/** Tolerance (mm) around zero within which parts count as "touching" rather
 *  than clear or intersecting. Keep in sync with `CONTACT_EPS_MM` in
 *  crates/vcad-receipt/src/mechanical.rs. */
const CONTACT_EPS_MM = 1e-3;

/** Three-way contact verdict derived from the signed minimum distance. */
export type ClearanceVerdict = "clear" | "touching" | "intersecting";

/** Classify a signed distance into clear / touching / intersecting. */
export function clearanceVerdict(distanceMm: number): ClearanceVerdict {
  if (distanceMm < -CONTACT_EPS_MM) return "intersecting";
  if (distanceMm <= CONTACT_EPS_MM) return "touching";
  return "clear";
}

/**
 * Does the measurement satisfy the spec, honoring allowed contact?
 *
 * `intersecting` is fail-closed and overrides everything: the kernel reports
 * a *zero* penetration depth for some interpenetrating pairs (two boxes
 * crossing face-to-face), which reads as "touching" on distance alone. An
 * overlap is never an allowed contact, whatever the depth came back as.
 */
export function clearanceHolds(
  distanceMm: number,
  minMm: number,
  allowContact: boolean,
  intersecting = false,
): boolean {
  if (intersecting) return false;
  return (
    distanceMm >= minMm ||
    (allowContact && clearanceVerdict(distanceMm) === "touching")
  );
}

/** Is measurement `a` worse than `b`? Interpenetration beats any distance. */
function worseThan(
  a: { distance: number; intersecting: boolean },
  b: { distance: number; intersecting: boolean },
): boolean {
  if (a.intersecting !== b.intersecting) return a.intersecting;
  return a.distance < b.distance;
}

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
    allow_contact: {
      type: "boolean" as const,
      description:
        "Treat exact surface contact (measured distance within 0.001 mm of zero) as passing even though it is below `min_mm` — for parts designed to touch, e.g. a stage bolted flush to the chamber floor. Penetration beyond the tolerance still fails. Persisted with the spec when `label` is given.",
    },
    joint_state: jointStateSchemaProp,
    sweep: {
      type: "array" as const,
      description:
        "Range-of-motion sweep: drive these joints across their travel and report the WORST pose, not the authored one. Each axis is {joint, from, to, steps}; multiple axes form a Cartesian grid (capped at 4096 poses). Joint states are restored afterwards — the sweep never edits the document.",
      items: {
        type: "object" as const,
        properties: {
          joint: { type: "string" as const, description: "Joint id or name to drive." },
          from: { type: "number" as const, description: "Start of travel (degrees for revolute, mm for prismatic)." },
          to: { type: "number" as const, description: "End of travel." },
          steps: { type: "number" as const, description: "Number of intervals; steps + 1 poses are sampled." },
        },
        required: ["joint", "from", "to", "steps"],
      },
    },
    ignore_pairs: {
      type: "array" as const,
      description:
        "Audit mode: part id/name pairs whose contact is intended (a bolt in its own hole, a bearing pressed into its bore). Each entry is a two-element array.",
      items: {
        type: "array" as const,
        items: { type: "string" as const },
      },
    },
    ignore_fixed_joints: {
      type: "boolean" as const,
      description:
        "Audit mode: skip pairs joined by a Fixed joint — they are bolted together and always 'interfere'. Default true.",
    },
    ignore_adjacent: {
      type: "boolean" as const,
      description:
        "Audit mode: skip every directly-jointed parent/child pair, not just Fixed ones. Default false — adjacent links can and do clash away from the joint axis.",
    },
  },
  required: ["document_id"],
} as const;

/** A part resolved to its evaluated (already-placed) mesh. */
interface ResolvedPart {
  id: string;
  name?: string;
  mesh: TriangleMesh;
}

/**
 * A part as evaluated once, kept re-poseable: static roots carry their world
 * mesh directly, assembly instances carry the *part-local* mesh plus the
 * world transform of the pose they were evaluated in. Sweeping then costs one
 * mesh transform per pose instead of one full BRep evaluation.
 */
interface PoseablePart {
  id: string;
  name?: string;
  /** Part-local mesh for instances; already-world mesh for static roots. */
  localMesh: TriangleMesh;
  /** World transform for instances; absent for static roots. */
  transform?: Transform3D;
  /** Instance id, when this part is an assembly instance (drives re-posing). */
  instanceId?: string;
}

/** Bake a world transform into a part-local mesh. */
function placeMesh(mesh: TriangleMesh, transform?: Transform3D): TriangleMesh {
  if (!transform) return mesh;
  return transformMesh(mesh, {
    translate: transform.translation,
    rotate: transform.rotation,
    scale: transform.scale,
  });
}

/** Evaluate a CAD session once into re-poseable parts. */
function poseableParts(doc: Document, engine: Engine): PoseablePart[] {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const out: PoseablePart[] = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = doc.nodes[String(rootId)];
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({ id: String(rootId), name: node?.name ?? undefined, localMesh: mesh });
  }
  // Assembly instances: bake the FK world transform into the part-local mesh
  // so clearances measure poses, not part-local geometry. Without this,
  // assembly-only documents had no clearance candidates at all.
  for (const inst of scene.instances ?? []) {
    if (!inst.mesh || inst.mesh.positions.length === 0) continue;
    out.push({
      id: inst.instanceId,
      name: inst.name ?? undefined,
      localMesh: inst.mesh,
      transform: inst.transform ?? undefined,
      instanceId: inst.instanceId,
    });
  }
  return out;
}

/**
 * Place every part for one pose. `worldTransforms` (from a re-solved FK pass)
 * overrides the evaluated transform for instances it names; static roots are
 * pose-independent and pass through.
 */
function placeParts(
  parts: PoseablePart[],
  worldTransforms?: Map<string, Transform3D>,
): ResolvedPart[] {
  const out: ResolvedPart[] = [];
  for (const p of parts) {
    const transform =
      (p.instanceId ? worldTransforms?.get(p.instanceId) : undefined) ?? p.transform;
    const mesh = placeMesh(p.localMesh, transform);
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({ id: p.id, ...(p.name ? { name: p.name } : {}), mesh });
  }
  return out;
}

/** Evaluate a CAD session into measurable parts (id, name, placed mesh). */
function partCandidates(doc: Document, engine: Engine): ResolvedPart[] {
  return placeParts(poseableParts(doc, engine));
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
  /** Joint states realizing the reported minimum (swept queries only). */
  worst_pose?: Pose;
  /** Number of poses evaluated (absent when unswept). */
  poses_checked?: number;
}

/** One sampled configuration of the mechanism: joint id → state. */
export type Pose = Array<{ joint: string; state: number }>;

/**
 * Ceiling on the pose grid a single query may span. A sweep is O(poses ×
 * pairs × BVH query); silently truncating would report a clearance the
 * machine never proved, so an oversized grid is an error, not a sample.
 */
export const MAX_SWEEP_POSES = 4096;

/** Resolve sweep axes against the document's joints (by id, then by name). */
function resolveSweepAxes(
  doc: Document,
  axes: JointSweep[],
): { axes?: JointSweep[]; error?: string } {
  const joints = doc.joints ?? [];
  const resolved: JointSweep[] = [];
  for (const axis of axes) {
    const wanted = String(axis.joint);
    const found =
      joints.find((j) => j.id === wanted) ?? joints.find((j) => j.name === wanted);
    if (!found) {
      const available = joints.map((j) => `${j.id}${j.name ? ` (${j.name})` : ""}`).join(", ");
      return { error: `No joint with id or name "${wanted}". Available: ${available || "none"}` };
    }
    if (!Number.isFinite(axis.from) || !Number.isFinite(axis.to)) {
      return { error: `Sweep axis "${wanted}" needs finite \`from\` and \`to\`.` };
    }
    const steps = Math.trunc(axis.steps);
    if (!Number.isFinite(steps) || steps < 1) {
      return { error: `Sweep axis "${wanted}" needs \`steps\` >= 1.` };
    }
    resolved.push({ joint: found.id, from: axis.from, to: axis.to, steps });
  }
  if (resolved.length === 0) return { error: "`sweep` needs at least one joint axis." };
  const total = resolved.reduce((n, a) => n * (a.steps + 1), 1);
  if (total > MAX_SWEEP_POSES) {
    return {
      error: `Sweep grid is ${total} poses (limit ${MAX_SWEEP_POSES}). Lower \`steps\` or sweep fewer joints at once.`,
    };
  }
  return { axes: resolved };
}

/** Cartesian product of the axes: `steps + 1` samples each, endpoints included. */
export function poseGrid(axes: JointSweep[]): Pose[] {
  let poses: Pose[] = [[]];
  for (const axis of axes) {
    const n = Math.trunc(axis.steps);
    const next: Pose[] = [];
    for (const pose of poses) {
      for (let i = 0; i <= n; i++) {
        const state = axis.from + ((axis.to - axis.from) * i) / n;
        next.push([...pose, { joint: axis.joint, state }]);
      }
    }
    poses = next;
  }
  return poses;
}

/**
 * Solve forward kinematics once per pose. Joint states are driven on the
 * document and restored afterwards, so the session is left exactly as the
 * author posed it — a sweep is a question, not an edit.
 */
function poseTransforms(doc: Document, poses: Pose[]): Array<Map<string, Transform3D>> {
  const joints = doc.joints ?? [];
  const byId = new Map(joints.map((j) => [j.id, j] as const));
  const saved = joints.map((j) => j.state);
  try {
    return poses.map((pose) => {
      for (const { joint, state } of pose) {
        const j = byId.get(joint);
        if (j) j.state = state;
      }
      return solveForwardKinematics(doc);
    });
  } finally {
    joints.forEach((j, i) => {
      j.state = saved[i];
    });
  }
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
  sweep?: JointSweep[],
): { result?: GroupClearance; error?: string } {
  if (groupA.length === 0 || groupB.length === 0) {
    return { error: "Both `group_a` and `group_b` need at least one part id." };
  }
  let base: PoseablePart[];
  try {
    base = poseableParts(doc, engine);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
  if (!sweep || sweep.length === 0) {
    return measureGroups(engine, placeParts(base), groupA, groupB);
  }

  const { axes, error: sweepError } = resolveSweepAxes(doc, sweep);
  if (sweepError || !axes) return { error: sweepError ?? "sweep could not be resolved" };
  const poses = poseGrid(axes);
  let transforms: Array<Map<string, Transform3D>>;
  try {
    transforms = poseTransforms(doc, poses);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  let worst: { result: GroupClearance; pose: Pose } | undefined;
  for (let i = 0; i < poses.length; i++) {
    const { result, error } = measureGroups(
      engine,
      placeParts(base, transforms[i]),
      groupA,
      groupB,
    );
    if (error || !result) return { error };
    if (
      !worst ||
      worseThan(
        { distance: result.distance_mm, intersecting: result.intersecting },
        { distance: worst.result.distance_mm, intersecting: worst.result.intersecting },
      )
    ) {
      worst = { result, pose: poses[i] };
    }
  }
  if (!worst) return { error: "Sweep produced no poses to measure." };
  return {
    result: {
      ...worst.result,
      worst_pose: worst.pose.map((p) => ({ joint: p.joint, state: round6(p.state) })),
      poses_checked: poses.length,
    },
  };
}

/** Single-pose group-vs-group minimum over already-placed parts. */
function measureGroups(
  engine: Engine,
  candidates: ResolvedPart[],
  groupA: string[],
  groupB: string[],
): { result?: GroupClearance; error?: string } {
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
      if (!worst || worseThan(r, worst.r)) {
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

/* ------------------------------------------------------------------ *
 * All-pairs interference audit
 * ------------------------------------------------------------------ */

/** One offending pair found by the audit. */
export interface InterferenceFinding {
  a: { id: string; name?: string };
  b: { id: string; name?: string };
  /** Worst (smallest) signed distance in mm; negative = penetration depth. */
  distance_mm: number;
  verdict: ClearanceVerdict;
  point_a: [number, number, number];
  point_b: [number, number, number];
  /** Joint states realizing the worst distance (swept audits only). */
  worst_pose?: Pose;
}

/** Whole-document audit outcome. */
export interface InterferenceAudit {
  parts_checked: number;
  /** Distinct part pairs in the document. */
  pairs_total: number;
  /** Pairs excluded by the ignore list / joint graph. */
  pairs_ignored: number;
  /** Pairs whose AABBs overlap (survived broadphase) at some pose. */
  pairs_broadphase: number;
  /** Exact BVH queries actually run (broadphase survivors × poses). */
  queries: number;
  poses_checked?: number;
  findings: InterferenceFinding[];
  /** Ignore-list entries that matched no part — reported, never silently dropped. */
  unresolved_ignores: string[];
}

/** Axis-aligned bounds of a placed mesh. */
function meshAabb(mesh: TriangleMesh): { min: [number, number, number]; max: [number, number, number] } {
  const p = mesh.positions;
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i + 2 < p.length; i += 3) {
    for (let k = 0; k < 3; k++) {
      const v = p[i + k];
      if (v < min[k]) min[k] = v;
      if (v > max[k]) max[k] = v;
    }
  }
  return { min, max };
}

/** Do two AABBs come within `margin` of each other on every axis? */
function aabbNear(
  a: { min: [number, number, number]; max: [number, number, number] },
  b: { min: [number, number, number]; max: [number, number, number] },
  margin: number,
): boolean {
  for (let k = 0; k < 3; k++) {
    if (a.min[k] - margin > b.max[k]) return false;
    if (b.min[k] - margin > a.max[k]) return false;
  }
  return true;
}

const pairKey = (a: string, b: string) => (a < b ? `${a} ${b}` : `${b} ${a}`);

/** Options for {@link computeInterferenceAudit}. */
export interface AuditOptions {
  /** Report pairs closer than this (mm). 0 = report only interpenetration. */
  minMm: number;
  /** Explicit whitelist of part id/name pairs. */
  ignorePairs: Array<[string, string]>;
  /** Skip pairs joined by a Fixed joint (default true). */
  ignoreFixedJoints: boolean;
  /** Skip pairs joined by *any* joint — adjacent links touch at their axis. */
  ignoreAdjacent: boolean;
  /** Optional range-of-motion sweep. */
  sweep?: JointSweep[];
}

/**
 * Broadphase every part pair in the document and report those that come
 * closer than `minMm` — the "does anything hit anything" check, as opposed
 * to the per-pair question the caller has to remember to ask. Cheap AABB
 * rejection first; exact BVH distance only on survivors. With `sweep`, each
 * pair's reported distance is its worst over the whole range of motion.
 */
export function computeInterferenceAudit(
  doc: Document,
  engine: Engine,
  opts: AuditOptions,
): { result?: InterferenceAudit; error?: string } {
  let base: PoseablePart[];
  try {
    base = poseableParts(doc, engine);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  let poses: Pose[] = [];
  let transforms: Array<Map<string, Transform3D> | undefined> = [undefined];
  if (opts.sweep && opts.sweep.length > 0) {
    const { axes, error } = resolveSweepAxes(doc, opts.sweep);
    if (error || !axes) return { error: error ?? "sweep could not be resolved" };
    poses = poseGrid(axes);
    try {
      transforms = poseTransforms(doc, poses);
    } catch (e) {
      return { error: e instanceof Error ? e.message : String(e) };
    }
  }

  const first = placeParts(base, transforms[0]);
  if (first.length < 2) {
    return { error: "Fewer than two measurable parts — nothing to audit." };
  }

  // Ignore set: explicit pairs first, then the joint graph.
  const ignore = new Set<string>();
  const unresolved: string[] = [];
  const resolveId = (raw: string): string | undefined => {
    const wanted = String(raw);
    return (first.find((c) => c.id === wanted) ?? first.find((c) => c.name === wanted))?.id;
  };
  for (const [rawA, rawB] of opts.ignorePairs) {
    const a = resolveId(rawA);
    const b = resolveId(rawB);
    if (!a) unresolved.push(String(rawA));
    if (!b) unresolved.push(String(rawB));
    if (a && b) ignore.add(pairKey(a, b));
  }
  for (const joint of doc.joints ?? []) {
    const parent = joint.parentInstanceId;
    if (!parent) continue;
    const fixed = joint.kind?.type === "Fixed";
    if (opts.ignoreAdjacent || (fixed && opts.ignoreFixedJoints)) {
      ignore.add(pairKey(parent, joint.childInstanceId));
    }
  }

  const margin = Math.max(opts.minMm, 0);
  const worstByPair = new Map<
    string,
    { a: ResolvedPart; b: ResolvedPart; r: ClearanceResult; pose?: Pose }
  >();
  const broadphase = new Set<string>();
  let queries = 0;
  let ignored = 0;

  for (let p = 0; p < transforms.length; p++) {
    const parts = p === 0 ? first : placeParts(base, transforms[p]);
    const boxes = parts.map((c) => meshAabb(c.mesh));
    for (let i = 0; i < parts.length; i++) {
      for (let j = i + 1; j < parts.length; j++) {
        const key = pairKey(parts[i].id, parts[j].id);
        if (ignore.has(key)) {
          if (p === 0) ignored += 1;
          continue;
        }
        if (!aabbNear(boxes[i], boxes[j], margin + CONTACT_EPS_MM)) continue;
        broadphase.add(key);
        let r: ClearanceResult;
        try {
          r = engine.meshClearance(parts[i].mesh, parts[j].mesh);
        } catch (e) {
          return { error: e instanceof Error ? e.message : String(e) };
        }
        queries += 1;
        const prev = worstByPair.get(key);
        if (!prev || worseThan(r, prev.r)) {
          worstByPair.set(key, {
            a: parts[i],
            b: parts[j],
            r,
            ...(poses.length > 0 ? { pose: poses[p] } : {}),
          });
        }
      }
    }
  }

  const named = (p: ResolvedPart) => ({ id: p.id, ...(p.name ? { name: p.name } : {}) });
  const findings: InterferenceFinding[] = [...worstByPair.values()]
    .filter((w) => !clearanceHolds(round6(w.r.distance), opts.minMm, false, w.r.intersecting))
    .map((w) => ({
      a: named(w.a),
      b: named(w.b),
      distance_mm: round6(w.r.distance),
      verdict: w.r.intersecting
        ? ("intersecting" as ClearanceVerdict)
        : clearanceVerdict(round6(w.r.distance)),
      point_a: w.r.pointA.map(round6) as [number, number, number],
      point_b: w.r.pointB.map(round6) as [number, number, number],
      ...(w.pose
        ? { worst_pose: w.pose.map((s) => ({ joint: s.joint, state: round6(s.state) })) }
        : {}),
    }))
    .sort(
      (x, y) =>
        Number(y.verdict === "intersecting") - Number(x.verdict === "intersecting") ||
        x.distance_mm - y.distance_mm,
    );

  const n = first.length;
  return {
    result: {
      parts_checked: n,
      pairs_total: (n * (n - 1)) / 2,
      pairs_ignored: ignored,
      pairs_broadphase: broadphase.size,
      queries,
      ...(poses.length > 0 ? { poses_checked: poses.length } : {}),
      findings,
      unresolved_ignores: [...new Set(unresolved)],
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
  const label = typeof args.label === "string" && args.label.trim() ? args.label.trim() : undefined;
  const allowContact = asBool(args.allow_contact);

  const { sweep, error: sweepParseError } = parseSweep(args.sweep);
  if (sweepParseError) return err(sweepParseError);

  // A persisted spec is re-measured by build_receipt / verify_receipt against
  // the document's *stored* joint states, so a spec captured at an ad-hoc pose
  // would re-verify against different geometry and read Violated for no
  // reason. Refuse the combination rather than persist an unre-verifiable
  // assertion. (`sweep` is exempt: the grid is persisted with the spec, so a
  // swept assertion re-verifies over exactly the range it was made over.)
  if (label && args.joint_state !== undefined && args.joint_state !== null) {
    return err(
      "`label` and `joint_state` cannot be combined: a persisted clearance spec is " +
        "re-measured at the document's stored joint states, so an assertion captured at " +
        "an ad-hoc pose could not be re-verified. Either drop `label` (one-off pose " +
        "measurement), use `sweep` (persisted with the spec and re-verified over the same " +
        "range), or set the joint states on the document and assert there.",
    );
  }

  const stored = getSession(documentId);
  // Measure the requested pose. The pose is a measurement condition applied to
  // a clone — the session document is never mutated by it. A `sweep` then
  // drives its own axes on top of that pose, restoring them afterwards.
  let doc: Document;
  let pose: PoseInfo | undefined;
  try {
    ({ doc, pose } = applyJointState(stored, args.joint_state));
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }

  // Audit mode: no pair named → broadphase the whole document.
  if (!groupA && !groupB) {
    return auditClearance(documentId, doc, engine, args, sweep, label, pose);
  }
  if (!groupA || !groupB) {
    return err(
      "Pass both `group_a` and `group_b` as arrays of part ids (or part names) — or neither, to audit every pair in the document.",
    );
  }

  const minMm = typeof args.min_mm === "number" ? args.min_mm : NaN;
  if (!Number.isFinite(minMm)) return err("Pass `min_mm`, the required minimum separation in mm.");

  const { result, error } = computeGroupClearance(doc, engine, groupA, groupB, sweep);
  if (error || !result) return err(error ?? "Clearance could not be computed.");

  const verdict = clearanceVerdict(result.distance_mm);
  const pass = clearanceHolds(result.distance_mm, minMm, allowContact, result.intersecting);
  let specSaved = false;
  if (label) {
    // Persist by resolved part ids so the assertion survives renames.
    upsertClearanceSpec(stored, {
      label,
      group_a: result.group_a.map((p) => p.id),
      group_b: result.group_b.map((p) => p.id),
      min_mm: minMm,
      ...(allowContact ? { allow_contact: true } : {}),
      ...(sweep && sweep.length > 0 ? { sweep } : {}),
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
    verdict,
    ...(allowContact ? { allow_contact: true } : {}),
    ...(pose ? { pose } : {}),
    intersecting: result.intersecting,
    worst_pair: result.worst_pair,
    ...(result.worst_pose ? { worst_pose: result.worst_pose } : {}),
    ...(result.poses_checked !== undefined ? { poses_checked: result.poses_checked } : {}),
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

/** Parse the `sweep` argument into typed axes (undefined when absent). */
function parseSweep(raw: unknown): { sweep?: JointSweep[]; error?: string } {
  if (raw === undefined || raw === null) return {};
  if (!Array.isArray(raw)) {
    return { error: "`sweep` must be an array of {joint, from, to, steps} axes." };
  }
  const sweep: JointSweep[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") {
      return { error: "Each `sweep` axis must be an object {joint, from, to, steps}." };
    }
    const e = entry as Record<string, unknown>;
    const joint = typeof e.joint === "string" ? e.joint.trim() : "";
    const from = Number(e.from);
    const to = Number(e.to);
    const steps = Number(e.steps);
    if (!joint || !Number.isFinite(from) || !Number.isFinite(to) || !Number.isFinite(steps)) {
      return {
        error: "Each `sweep` axis needs a `joint` id/name plus numeric `from`, `to`, and `steps`.",
      };
    }
    sweep.push({ joint, from, to, steps });
  }
  return sweep.length > 0 ? { sweep } : {};
}

/** Parse `ignore_pairs` into id/name couples, tolerating loose shapes. */
function parseIgnorePairs(raw: unknown): { pairs: Array<[string, string]>; error?: string } {
  if (raw === undefined || raw === null) return { pairs: [] };
  if (!Array.isArray(raw)) {
    return { pairs: [], error: "`ignore_pairs` must be an array of [part_a, part_b] pairs." };
  }
  const pairs: Array<[string, string]> = [];
  for (const entry of raw) {
    if (!Array.isArray(entry) || entry.length !== 2) {
      return { pairs: [], error: "Each `ignore_pairs` entry must be a two-element array." };
    }
    pairs.push([String(entry[0]), String(entry[1])]);
  }
  return { pairs };
}

/** All-pairs audit mode of `check_clearance`. */
function auditClearance(
  documentId: string,
  doc: Document,
  engine: Engine,
  args: Record<string, unknown>,
  sweep: JointSweep[] | undefined,
  label: string | undefined,
  pose: PoseInfo | undefined,
) {
  const minMm = typeof args.min_mm === "number" && Number.isFinite(args.min_mm) ? args.min_mm : 0;
  const { pairs: ignorePairs, error: ignoreError } = parseIgnorePairs(args.ignore_pairs);
  if (ignoreError) return err(ignoreError);
  const ignoreFixedJoints = args.ignore_fixed_joints === undefined ? true : asBool(args.ignore_fixed_joints);
  const ignoreAdjacent = asBool(args.ignore_adjacent);

  const { result, error } = computeInterferenceAudit(doc, engine, {
    minMm,
    ignorePairs,
    ignoreFixedJoints,
    ignoreAdjacent,
    sweep,
  });
  if (error || !result) return err(error ?? "Interference audit could not be computed.");

  const payload = {
    success: true,
    document_id: documentId,
    mode: "audit" as const,
    required_mm: minMm,
    ...(pose ? { pose } : {}),
    pass: result.findings.length === 0,
    findings: result.findings,
    interference_count: result.findings.length,
    parts_checked: result.parts_checked,
    pairs_total: result.pairs_total,
    pairs_ignored: result.pairs_ignored,
    pairs_broadphase: result.pairs_broadphase,
    queries: result.queries,
    ...(result.poses_checked !== undefined ? { poses_checked: result.poses_checked } : {}),
    ...(result.unresolved_ignores.length > 0
      ? {
          unresolved_ignores: result.unresolved_ignores,
          note: `ignore_pairs named parts that do not exist: ${result.unresolved_ignores.join(", ")} — those pairs were NOT whitelisted.`,
        }
      : {}),
    ...(label
      ? {
          spec_saved: false,
          label_note:
            "`label` persists only pairwise assertions; an audit is a whole-document scan, so nothing was saved. Name the offending pair and re-run with group_a/group_b to persist it.",
        }
      : {}),
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: { clearance: payload, document_id: documentId },
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
    const description = spec.sweep?.length
      ? `clearance "${spec.label}" at least ${spec.min_mm} mm across the swept range of motion`
      : `clearance "${spec.label}" at least ${spec.min_mm} mm`;
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
    const { result, error } = computeGroupClearance(
      doc,
      engine,
      spec.group_a,
      spec.group_b,
      spec.sweep,
    );
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
    const allowContact = spec.allow_contact === true;
    const assertion: ClearanceClaim = {
      label: spec.label,
      group_a: spec.group_a,
      group_b: spec.group_b,
      required_mm: spec.min_mm,
      measured_mm: result.distance_mm,
      holds: clearanceHolds(result.distance_mm, spec.min_mm, allowContact, result.intersecting),
      ...(allowContact ? { allow_contact: true } : {}),
      // A swept claim carries its grid so verify_receipt re-checks the same
      // range of motion — re-verifying at one pose would silently weaken it.
      ...(spec.sweep?.length
        ? {
            sweep: spec.sweep,
            worst_pose: result.worst_pose ?? [],
            poses_checked: result.poses_checked ?? 0,
          }
        : {}),
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
  /** Pose realizing the re-measured worst case (swept assertions only). */
  worst_pose?: Pose;
  /** Poses re-evaluated (swept assertions only). */
  poses_checked?: number;
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
    const { result, error } = computeGroupClearance(
      doc,
      engine,
      stored.group_a,
      stored.group_b,
      stored.sweep,
    );
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
    const sweptFields = result.poses_checked
      ? { worst_pose: result.worst_pose, poses_checked: result.poses_checked }
      : {};
    const common = {
      label,
      required_mm: stored.required_mm,
      stored_mm: stored.measured_mm,
      measured_mm: measured,
      ...sweptFields,
    };
    if (
      !clearanceHolds(measured, stored.required_mm, stored.allow_contact === true, result.intersecting)
    ) {
      checks.push({
        ...common,
        status: "Violated",
        reason: result.poses_checked
          ? `measured ${measured} mm is below the required ${stored.required_mm} mm somewhere in the swept range of motion`
          : `measured ${measured} mm is below the required ${stored.required_mm} mm`,
      });
    } else if (Math.abs(measured - stored.measured_mm) > STALE_EPS_MM) {
      checks.push({
        ...common,
        status: "Stale",
        reason: "geometry changed since the receipt was built, but the clearance still holds",
      });
    } else {
      checks.push({ ...common, status: "Holds" });
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
      "Measure the minimum distance between two groups of parts in a CAD session and assert it stays above `min_mm` \u2014 air gaps, press fits, screw-head clearances. Reports the measured minimum (negative = penetration depth), a clear/touching/intersecting verdict, the worst part pair, and pass/fail. Pass `allow_contact: true` for parts designed to touch (e.g. bolted flush) so exact contact passes instead of reading as an intersection. Pass `joint_state` to measure at one real pose (joint id or name \u2192 degrees, or mm for sliders) instead of the stored one. Give it a `label` to persist the assertion on the document: build_receipt then emits it as a mech.clearance claim and verify_receipt re-verifies it as Holds / Stale / Violated when geometry changes. Two modes beyond the single-pair snapshot: (1) `sweep` \u2014 pass [{joint, from, to, steps}] to drive the mechanism through its range of motion and report the WORST pose, not the one it was authored in (a linkage modelled at mid-travel routinely clears there and collides at both ends); persisted with the label, so the assertion re-verifies over the same range. (2) audit \u2014 omit `group_a`/`group_b` entirely to broadphase EVERY part pair in the document and list everything that interpenetrates, with penetration depth; whitelist intended contact with `ignore_pairs` (Fixed-joint pairs are skipped by default). Combine both for 'does this machine ever hit itself anywhere in its workspace'.",
    inputSchema: checkClearanceSchema,
    handler: (a, c) => checkClearance(a, c.engine),
    behavior: behavior({ writesDoc: true }),
  },
];
