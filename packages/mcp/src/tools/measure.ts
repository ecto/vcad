/**
 * Targeted measurement tools for MCP agents.
 *
 * `inspect_cad` is aggregate-only; these give an agent per-part detail and
 * part-pair distances without leaving the modeling loop:
 *
 *  - `inspect_part` — one part's world-space bbox, size, center, volume,
 *    center of mass, material, and the named anchors `place` accepts.
 *  - `describe_scene` — the same bbox/size/center snapshot for every part
 *    (or a chosen subset) in one call, so an agent needn't chain
 *    `inspect_part`.
 *  - `measure` — given two part ids, the minimum distance between them
 *    (0/negative = contact/overlap) plus each part's bbox; given one part
 *    id, that part's bbox, volume, and center of mass.
 *
 * `inspect_part` / `describe_scene` are the kernel chat surface's tools,
 * re-implemented here against evaluated tessellations and dispatched through
 * the registry surface (registry-dispatch.ts). `measure` is a standalone
 * ToolDef. All three measure from the evaluated triangle mesh, so accuracy is
 * tessellation-bound (increase segment counts for tighter numbers).
 *
 * That bound is not specific to imports: the kernel's own `volume()` /
 * `surface_area()` also tessellate, so there is currently no exact B-rep query
 * to prefer here — exact face-level measurement is a kernel-side change, not a
 * dispatch choice in this file. What did improve is the input: a STEP import
 * now arrives as B-rep (`step_import`) and tessellates at document resolution,
 * instead of being frozen at the importer's old 16-segment bake.
 */

import type { ClearanceResult, Engine, TriangleMesh } from "@vcad/engine";
import type { Document, JointSweep, Transform3D } from "@vcad/ir";
import { computeMeshProperties, type BoundingBox } from "./inspect.js";
import { getSession } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { applyJointState, jointStateSchemaProp, type PoseInfo } from "./pose.js";
import {
  MAX_SWEEP_SAMPLES,
  parseSweep,
  placeParts,
  poseGrid,
  poseableParts,
  poseTransforms,
  resolveSweepAxes,
  roundPose,
  sweepSchemaProp,
  worseThan,
  type Pose,
  type PoseablePart,
  type SweepSample,
} from "./sweep.js";

/** Round to micron precision so payloads don't carry float noise. */
const r6 = (v: number) => Math.round(v * 1e6) / 1e6;

const point = (p: { x: number; y: number; z: number }) => ({
  x: r6(p.x),
  y: r6(p.y),
  z: r6(p.z),
});

/** A part resolved to its evaluated (already-placed) mesh + material. */
interface ResolvedPart {
  id: string;
  name?: string;
  material?: string;
  mesh: TriangleMesh;
}

/** Evaluate a session document into measurable parts (id, name, material,
 *  placed mesh), skipping hidden roots and empty meshes — mirrors the
 *  clearance tool's part resolution. */
function evaluateParts(doc: Document, engine: Engine): ResolvedPart[] {
  return placeParts(poseableParts(doc, engine));
}

/** Resolve a part by id first, then by exact name. */
function findPart(
  candidates: ResolvedPart[],
  wanted: string,
): ResolvedPart | undefined {
  return (
    candidates.find((c) => c.id === wanted) ??
    candidates.find((c) => c.name === wanted)
  );
}

/** Human-readable list of what a caller could have asked for. */
function availableList(candidates: ResolvedPart[]): string {
  return (
    candidates.map((c) => `${c.id}${c.name ? ` (${c.name})` : ""}`).join(", ") ||
    "none"
  );
}

/** The bbox payload (rounded min/max/center/size) all three tools emit. */
function bboxPayload(bbox: BoundingBox) {
  const center = {
    x: (bbox.min.x + bbox.max.x) / 2,
    y: (bbox.min.y + bbox.max.y) / 2,
    z: (bbox.min.z + bbox.max.z) / 2,
  };
  const size = {
    x: bbox.max.x - bbox.min.x,
    y: bbox.max.y - bbox.min.y,
    z: bbox.max.z - bbox.min.z,
  };
  return {
    min: point(bbox.min),
    max: point(bbox.max),
    center: point(center),
    size: point(size),
  };
}

/**
 * Named world-space AABB anchors, matching the app's `place` semantics so an
 * agent can pass any of these back to a future `place` and get the point it
 * saw here.
 */
function anchors(bbox: BoundingBox) {
  const c = {
    x: (bbox.min.x + bbox.max.x) / 2,
    y: (bbox.min.y + bbox.max.y) / 2,
    z: (bbox.min.z + bbox.max.z) / 2,
  };
  return {
    center: point(c),
    min: point(bbox.min),
    max: point(bbox.max),
    top: point({ x: c.x, y: c.y, z: bbox.max.z }),
    bottom: point({ x: c.x, y: c.y, z: bbox.min.z }),
    front: point({ x: c.x, y: bbox.min.y, z: c.z }),
    back: point({ x: c.x, y: bbox.max.y, z: c.z }),
    left: point({ x: bbox.min.x, y: c.y, z: c.z }),
    right: point({ x: bbox.max.x, y: c.y, z: c.z }),
  };
}

/** One part's world-space snapshot (bbox/center/size + material). */
function partSnapshot(part: ResolvedPart) {
  const props = computeMeshProperties(part.mesh);
  return {
    id: part.id,
    ...(part.name ? { name: part.name } : {}),
    material: part.material ?? null,
    bbox: bboxPayload(props.bbox),
  };
}

/** Thrown by the helpers below; carries a recovery hint for the MCP error. */
export class MeasureError extends Error {}

/**
 * `inspect_part` payload: one part's world-space bbox, size, center, volume,
 * center of mass, material, and named anchors. Tessellation-bound.
 */
export function inspectPartResult(
  doc: Document,
  engine: Engine,
  partId: string,
): Record<string, unknown> {
  if (!partId) throw new MeasureError("inspect_part requires `part_id`.");
  const candidates = evaluateParts(doc, engine);
  const part = findPart(candidates, partId);
  if (!part) {
    throw new MeasureError(
      `No part with id or name "${partId}". Available: ${availableList(candidates)}`,
    );
  }
  const props = computeMeshProperties(part.mesh);
  return {
    id: part.id,
    ...(part.name ? { name: part.name } : {}),
    material: part.material ?? null,
    bbox: bboxPayload(props.bbox),
    volume_mm3: r6(props.volume),
    center_of_mass: point(props.centroid),
    anchors: anchors(props.bbox),
    note: "Measured from the evaluated tessellation — accuracy is tessellation-bound.",
  };
}

/**
 * `describe_scene` payload: a bbox/center/size + material snapshot for every
 * part (or the requested subset), in one call.
 */
export function describeSceneResult(
  doc: Document,
  engine: Engine,
  partIds: string[] | undefined,
  limit: number | undefined,
): Record<string, unknown> {
  const candidates = evaluateParts(doc, engine);
  const cap = typeof limit === "number" && limit > 0 ? limit : 100;
  const wanted =
    partIds && partIds.length > 0
      ? partIds.map(String)
      : candidates.map((c) => c.id).slice(0, cap);
  const parts: Array<Record<string, unknown>> = [];
  const missing: string[] = [];
  for (const id of wanted) {
    const part = findPart(candidates, id);
    if (!part) missing.push(id);
    else parts.push(partSnapshot(part));
  }
  return {
    part_count: parts.length,
    parts,
    ...(missing.length > 0 ? { missing } : {}),
    note: "Bounding boxes are measured from evaluated tessellations — accuracy is tessellation-bound.",
  };
}

/**
 * `measure` payload. With two ids: the minimum distance between the parts
 * (0/negative reported as contact/overlap via `contact`/`intersecting`) plus
 * each part's bbox. With one id: that part's bbox, volume, and center of mass.
 */
export function measureResult(
  doc: Document,
  engine: Engine,
  partIds: string[],
): Record<string, unknown> {
  const candidates = evaluateParts(doc, engine);
  const resolved: ResolvedPart[] = [];
  const missing: string[] = [];
  for (const id of partIds.map(String)) {
    const part = findPart(candidates, id);
    if (!part) missing.push(id);
    else resolved.push(part);
  }
  if (missing.length > 0) {
    throw new MeasureError(
      `No part with id or name ${missing
        .map((m) => `"${m}"`)
        .join(", ")}. Available: ${availableList(candidates)}`,
    );
  }

  if (resolved.length === 1) {
    const part = resolved[0];
    const props = computeMeshProperties(part.mesh);
    return {
      mode: "part",
      part: {
        id: part.id,
        ...(part.name ? { name: part.name } : {}),
        material: part.material ?? null,
        bbox: bboxPayload(props.bbox),
        volume_mm3: r6(props.volume),
        center_of_mass: point(props.centroid),
      },
      note: "Measured from the evaluated tessellation — accuracy is tessellation-bound.",
    };
  }

  const [a, b] = resolved;
  if (a.id === b.id) {
    throw new MeasureError(
      `measure needs two distinct parts — "${a.id}" was given twice.`,
    );
  }
  let clearance: ClearanceResult;
  try {
    clearance = engine.meshClearance(a.mesh, b.mesh);
  } catch (e) {
    throw new MeasureError(e instanceof Error ? e.message : String(e));
  }
  const distance = r6(clearance.distance);
  return {
    mode: "pair",
    distance_mm: distance,
    contact: distance <= 0,
    intersecting: clearance.intersecting,
    closest_points: {
      a: clearance.pointA.map(r6) as [number, number, number],
      b: clearance.pointB.map(r6) as [number, number, number],
    },
    parts: {
      a: partSnapshot(a),
      b: partSnapshot(b),
    },
    note: "Distance is negative when the parts overlap (penetration depth). Measured from evaluated tessellations — accuracy is tessellation-bound.",
  };
}

/**
 * `measure` in sweep mode: the two parts' minimum distance across a whole
 * range of motion, not the one pose the assembly happens to be stored in.
 *
 * The document is evaluated ONCE into re-poseable parts; each pose then costs
 * one FK solve plus two mesh transforms. The reported `distance_mm` is the
 * worst case over the grid (interpenetration beats any distance, mirroring
 * `check_clearance`), and `samples` carries the full margin curve so an agent
 * can see *where* in the travel the margin collapses rather than only how far.
 */
export function measureSweepResult(
  doc: Document,
  engine: Engine,
  partIds: string[],
  sweep: JointSweep[],
): Record<string, unknown> {
  if (partIds.length !== 2) {
    throw new MeasureError(
      "`sweep` measures the distance between TWO parts across a range of motion — " +
        `${partIds.length} part id was given. Pass two \`part_ids\`, or drop \`sweep\` ` +
        "(a single part's bbox/volume/center of mass is a per-pose property; use " +
        "`joint_state` to read it at one pose).",
    );
  }
  let base: PoseablePart[];
  try {
    base = poseableParts(doc, engine);
  } catch (e) {
    throw new MeasureError(e instanceof Error ? e.message : String(e));
  }

  const pick = (wanted: string): PoseablePart => {
    const found =
      base.find((p) => p.id === wanted) ?? base.find((p) => p.name === wanted);
    if (!found) {
      const available =
        base.map((p) => `${p.id}${p.name ? ` (${p.name})` : ""}`).join(", ") || "none";
      throw new MeasureError(
        `No part with id or name "${wanted}". Available: ${available}`,
      );
    }
    return found;
  };
  const [pa, pb] = partIds.map((id) => pick(String(id)));
  if (pa.id === pb.id) {
    throw new MeasureError(
      `measure needs two distinct parts — "${pa.id}" was given twice.`,
    );
  }

  const { axes, warnings, error } = resolveSweepAxes(doc, sweep);
  if (error || !axes) throw new MeasureError(error ?? "sweep could not be resolved");
  const poses = poseGrid(axes);
  let transforms: Array<Map<string, Transform3D>>;
  try {
    transforms = poseTransforms(doc, poses);
  } catch (e) {
    throw new MeasureError(e instanceof Error ? e.message : String(e));
  }

  const keepSamples = poses.length <= MAX_SWEEP_SAMPLES;
  const samples: SweepSample[] = [];
  let worst:
    | { r: ClearanceResult; pose: Pose; a: ResolvedPart; b: ResolvedPart }
    | undefined;
  for (let i = 0; i < poses.length; i++) {
    const placed = placeParts([pa, pb], transforms[i]);
    if (placed.length < 2) {
      throw new MeasureError(
        "One of the parts has an empty mesh at some pose — nothing to measure.",
      );
    }
    let r: ClearanceResult;
    try {
      r = engine.meshClearance(placed[0].mesh, placed[1].mesh);
    } catch (e) {
      throw new MeasureError(e instanceof Error ? e.message : String(e));
    }
    if (keepSamples) {
      samples.push({
        pose: roundPose(poses[i]),
        distance_mm: r6(r.distance),
        intersecting: r.intersecting,
      });
    }
    if (!worst || worseThan(r, worst.r)) {
      worst = { r, pose: poses[i], a: placed[0], b: placed[1] };
    }
  }
  if (!worst) throw new MeasureError("Sweep produced no poses to measure.");

  const distance = r6(worst.r.distance);
  return {
    mode: "sweep",
    distance_mm: distance,
    contact: distance <= 0,
    intersecting: worst.r.intersecting,
    worst_pose: roundPose(worst.pose),
    poses_checked: poses.length,
    sweep: axes,
    closest_points: {
      a: worst.r.pointA.map(r6) as [number, number, number],
      b: worst.r.pointB.map(r6) as [number, number, number],
    },
    parts: {
      a: partSnapshot(worst.a),
      b: partSnapshot(worst.b),
    },
    ...(keepSamples
      ? { samples }
      : {
          samples_omitted: true,
          samples_note: `Sweep grid is ${poses.length} poses (over the ${MAX_SWEEP_SAMPLES}-pose sample cap) — the per-pose margin curve was omitted. Lower \`steps\` to get it.`,
        }),
    ...(warnings && warnings.length > 0 ? { sweep_warnings: warnings } : {}),
    note: "`distance_mm` is the WORST (minimum) distance over the swept range, not the authored pose; negative means the parts overlap there (penetration depth). `parts` bboxes are those of the worst pose. Measured from evaluated tessellations — accuracy is tessellation-bound. Joint states are restored: a sweep never edits the document.",
  };
}

export const measureSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document holding the parts to measure.",
    },
    part_ids: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "One or two part ids (or part names). One id → that part's bbox, volume, and center of mass. Two ids → the minimum distance between them (0/negative = contact/overlap) plus each part's bbox.",
    },
    joint_state: jointStateSchemaProp,
    sweep: sweepSchemaProp,
  },
  required: ["document_id", "part_ids"],
} as const;

/** `measure` MCP handler. */
export function measure(
  args: Record<string, unknown>,
  engine: Engine,
) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) {
    return err("Pass `document_id` (the open CAD session).");
  }
  const partIds = Array.isArray(args.part_ids)
    ? args.part_ids.map((x) => String(x))
    : undefined;
  if (!partIds || partIds.length < 1 || partIds.length > 2) {
    return err(
      "Pass `part_ids` as an array of one or two part ids (or part names).",
    );
  }
  const { sweep, error: sweepParseError } = parseSweep(args.sweep);
  if (sweepParseError) return err(sweepParseError);

  let stored: Document;
  try {
    stored = getSession(documentId);
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }
  // `joint_state` is the base pose, applied to a clone — the session document
  // is never mutated. A `sweep` then drives its own axes on top of that pose
  // (overriding the base state for the joints it names) and restores them.
  let doc: Document;
  let pose: PoseInfo | undefined;
  try {
    ({ doc, pose } = applyJointState(stored, args.joint_state));
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }

  let payload: Record<string, unknown>;
  try {
    payload =
      sweep && sweep.length > 0
        ? measureSweepResult(doc, engine, partIds, sweep)
        : measureResult(doc, engine, partIds);
  } catch (e) {
    if (e instanceof MeasureError) return err(e.message);
    throw e;
  }
  const body = {
    document_id: documentId,
    ...(pose ? { pose } : {}),
    ...payload,
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(body) }],
    structuredContent: { measure: body, document_id: documentId },
  };
}

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "measure",
    pack: null,
    description:
      "Measure geometry in an open session. Pass `part_ids` with two ids to get the minimum distance between those parts (0/negative = contact/overlap) plus each part's world-space bounding box; pass one id to get that part's bbox, volume, and center of mass. Pass `joint_state` (joint id or name → degrees, mm for sliders) to measure a jointed assembly at one real pose instead of the stored one. Pass `sweep` — [{joint, from, to, steps}] — with TWO part ids to drive the mechanism through its range of motion and get the WORST (minimum) distance over the whole travel, the pose that realizes it (`worst_pose`), and `samples`: the full per-pose margin curve, so you can see where the clearance collapses rather than only that it does (omitted above 256 poses). Sweep endpoints outside a joint's declared limits are reported in `sweep_warnings`, not clamped; joint states are always restored, so a sweep never edits the document. Complements `inspect_cad` (whole-document aggregate) and `check_clearance` (named, persisted air-gap assertions with pass/fail and whole-document audits). Distances are measured from evaluated tessellations — swept poses re-place those same meshes — so accuracy is tessellation-bound.",
    inputSchema: measureSchema,
    handler: (a, c) => measure(a, c.engine),
    behavior: behavior({}),
  },
];
