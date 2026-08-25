/**
 * Joint-sweep machinery shared by `check_clearance` and `measure`.
 *
 * A sweep drives one or more joints across their travel, re-solves forward
 * kinematics per pose, and re-places the already-evaluated part meshes — so a
 * range-of-motion question costs one BRep evaluation plus one mesh transform
 * per part per pose, not a full re-evaluation. Joint states are restored
 * afterwards: a sweep is a question, never an edit.
 *
 * This module owns the pieces both tools need (pose grid, FK per pose,
 * re-poseable parts, argument parsing and the shared JSON-schema fragment);
 * `clearance.ts` re-exports the ones its callers already imported.
 */

import type { Document, JointSweep, Transform3D } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import { solveForwardKinematics, transformMesh } from "@vcad/engine";

/** One sampled configuration of the mechanism: joint id → state. */
export type Pose = Array<{ joint: string; state: number }>;

/**
 * Ceiling on the pose grid a single query may span. A sweep is O(poses ×
 * pairs × BVH query); silently truncating would report a clearance the
 * machine never proved, so an oversized grid is an error, not a sample.
 */
export const MAX_SWEEP_POSES = 4096;

/**
 * Per-pose sample payloads above this size are omitted — the margin curve is
 * a debugging aid, not a reason to ship a megabyte of JSON to a model.
 */
export const MAX_SWEEP_SAMPLES = 256;

/** Round values so payloads don't carry float noise. */
export const round6 = (v: number) => Math.round(v * 1e6) / 1e6;

/** Is measurement `a` worse than `b`? Interpenetration beats any distance. */
export function worseThan(
  a: { distance: number; intersecting: boolean },
  b: { distance: number; intersecting: boolean },
): boolean {
  if (a.intersecting !== b.intersecting) return a.intersecting;
  return a.distance < b.distance;
}

/** One pose of a swept query, with the distance measured there. */
export interface SweepSample {
  pose: Pose;
  distance_mm: number;
  intersecting: boolean;
}

/** JSON-schema fragment for the `sweep` argument, shared by every tool that
 *  accepts a range-of-motion query so the wording stays identical. */
export const sweepSchemaProp = {
  type: "array" as const,
  description:
    "Range-of-motion sweep: drive these joints across their travel and report the WORST pose, not the authored one. Each axis is {joint, from, to, steps}; multiple axes form a Cartesian grid (capped at 4096 poses). Endpoints outside a joint's declared limits are reported as warnings, not clamped. Joint states are restored afterwards — the sweep never edits the document.",
  items: {
    type: "object" as const,
    properties: {
      joint: { type: "string" as const, description: "Joint id or name to drive." },
      from: {
        type: "number" as const,
        description: "Start of travel (degrees for revolute, mm for prismatic).",
      },
      to: { type: "number" as const, description: "End of travel." },
      steps: {
        type: "number" as const,
        description: "Number of intervals; steps + 1 poses are sampled.",
      },
    },
    required: ["joint", "from", "to", "steps"],
  },
};

/** Parse the `sweep` argument into typed axes (undefined when absent). */
export function parseSweep(raw: unknown): { sweep?: JointSweep[]; error?: string } {
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

/** Declared travel limits of a joint, when it has any. */
function jointLimits(kind: unknown): [number, number] | undefined {
  const k = kind as { type?: string; limits?: [number, number] } | undefined;
  if (!k) return undefined;
  if (k.type === "Revolute" || k.type === "Slider") return k.limits;
  return undefined;
}

/** Unit label for a joint's state, for readable warnings. */
function jointUnit(kind: unknown): string {
  return (kind as { type?: string } | undefined)?.type === "Slider" ? "mm" : "deg";
}

/**
 * Resolve sweep axes against the document's joints (by id, then by name).
 *
 * Endpoints beyond a joint's declared limits are a **warning**, not an error
 * and not a clamp: an agent exploring travel it hasn't finalized still wants
 * the numbers, but must be told the machine cannot actually reach them.
 */
export function resolveSweepAxes(
  doc: Document,
  axes: JointSweep[],
): { axes?: JointSweep[]; warnings?: string[]; error?: string } {
  const joints = doc.joints ?? [];
  const resolved: JointSweep[] = [];
  const warnings: string[] = [];
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
    const limits = jointLimits(found.kind);
    if (limits) {
      const [lo, hi] = limits;
      const lowest = Math.min(axis.from, axis.to);
      const highest = Math.max(axis.from, axis.to);
      if (lowest < lo || highest > hi) {
        warnings.push(
          `sweep axis "${found.id}"${found.name ? ` ("${found.name}")` : ""} spans ` +
            `[${lowest}, ${highest}] ${jointUnit(found.kind)}, outside its declared limits ` +
            `[${lo}, ${hi}] — the swept range includes poses the joint cannot reach, so the ` +
            `reported worst case may not be physically attainable.`,
        );
      }
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
  return { axes: resolved, ...(warnings.length > 0 ? { warnings } : {}) };
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

/** Round a pose's states for the payload. */
export function roundPose(pose: Pose): Pose {
  return pose.map((p) => ({ joint: p.joint, state: round6(p.state) }));
}

/**
 * Solve forward kinematics once per pose. Joint states are driven on the
 * document and restored afterwards, so the session is left exactly as the
 * author posed it — a sweep is a question, not an edit.
 */
export function poseTransforms(
  doc: Document,
  poses: Pose[],
): Array<Map<string, Transform3D>> {
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
 * A part as evaluated once, kept re-poseable: static roots carry their world
 * mesh directly, assembly instances carry the *part-local* mesh plus the
 * world transform of the pose they were evaluated in. Sweeping then costs one
 * mesh transform per pose instead of one full BRep evaluation.
 */
export interface PoseablePart {
  id: string;
  name?: string;
  material?: string;
  /** Part-local mesh for instances; already-world mesh for static roots. */
  localMesh: TriangleMesh;
  /** World transform for instances; absent for static roots. */
  transform?: Transform3D;
  /** Instance id, when this part is an assembly instance (drives re-posing). */
  instanceId?: string;
}

/** A part resolved to its evaluated (already-placed) mesh. */
export interface PlacedPart {
  id: string;
  name?: string;
  material?: string;
  mesh: TriangleMesh;
}

/** Bake a world transform into a part-local mesh. */
export function placeMesh(mesh: TriangleMesh, transform?: Transform3D): TriangleMesh {
  if (!transform) return mesh;
  return transformMesh(mesh, {
    translate: transform.translation,
    rotate: transform.rotation,
    scale: transform.scale,
  });
}

/** Evaluate a CAD session once into re-poseable parts. */
export function poseableParts(doc: Document, engine: Engine): PoseablePart[] {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const partMaterials = (doc as { part_materials?: Record<string, string> }).part_materials;
  const out: PoseablePart[] = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const root = visibleRoots[i];
    const rootId = String(root.root);
    const node = doc.nodes[rootId];
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({
      id: rootId,
      name: node?.name ?? undefined,
      material: root.material ?? partMaterials?.[rootId] ?? undefined,
      localMesh: mesh,
    });
  }
  // Assembly instances: bake the FK world transform into the part-local mesh
  // so measurements report poses, not part-local geometry. Without this,
  // assembly-only documents had no candidates at all.
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
export function placeParts(
  parts: PoseablePart[],
  worldTransforms?: Map<string, Transform3D>,
): PlacedPart[] {
  const out: PlacedPart[] = [];
  for (const p of parts) {
    const transform =
      (p.instanceId ? worldTransforms?.get(p.instanceId) : undefined) ?? p.transform;
    const mesh = placeMesh(p.localMesh, transform);
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({
      id: p.id,
      ...(p.name ? { name: p.name } : {}),
      ...(p.material ? { material: p.material } : {}),
      mesh,
    });
  }
  return out;
}
