/**
 * `joint_state` — pose an assembly before you look at it.
 *
 * Forward kinematics already exists in the kernel
 * (`vcad_eval::solve_forward_kinematics`, reached through the WASM binding);
 * without this shim it was only reachable at the *stored* joint states, so a
 * jointed assembly could only ever be rendered, measured, or exported in its
 * zero pose. Every read tool that evaluates a document therefore accepts an
 * optional `joint_state` map — joint id (or name) → angle in degrees for
 * revolute/ball joints, mm for sliders — which is applied to a **clone** of
 * the document before evaluation. Unspecified joints keep their stored state,
 * so omitting the argument is exactly today's behaviour.
 *
 * Two fail-closed rules:
 *  - an unknown joint key is an error, not a silent no-op (a pose you thought
 *    you set but didn't is a lie about the machine);
 *  - a state outside the joint's declared limits is **clamped**, and the clamp
 *    is reported as a warning on the result — an out-of-limits render would
 *    show a machine that cannot exist.
 */

import { solveForwardKinematics } from "@vcad/engine";
import type { Document, Joint, Transform3D } from "@vcad/ir";

/** JSON-schema fragment for the `joint_state` argument, shared by every tool
 *  that accepts a pose so the wording stays identical across the surface. */
export const jointStateSchemaProp = {
  type: "object" as const,
  additionalProperties: { type: "number" as const },
  description:
    "Pose the assembly before evaluating: map of joint id (or joint name) → state — " +
    "degrees for revolute/ball joints, mm for sliders. Joints not listed keep their " +
    "stored state, so omitting this renders the document exactly as stored. States " +
    "outside a joint's declared limits are clamped and reported in `pose.warnings`; " +
    "an unknown joint key is an error.",
};

/** Resolved pose report, echoed on the tool result so a caller can read off
 *  where the mechanism actually ended up instead of re-deriving it by hand. */
export interface PoseInfo {
  /** Joint id → state actually applied (post-clamp). */
  applied: Record<string, number>;
  /** Clamp notices, one per joint driven outside its limits. */
  warnings?: string[];
  /** Instance id → world transform resolved by forward kinematics. */
  transforms: Record<string, Transform3D>;
}

/** Bad `joint_state` input — surfaced as a tool error, never swallowed. */
export class PoseError extends Error {}

function limitsOf(joint: Joint): [number, number] | undefined {
  const kind = joint.kind as { type?: string; limits?: [number, number] };
  if (kind.type === "Revolute" || kind.type === "Slider") return kind.limits;
  return undefined;
}

function unitOf(joint: Joint): string {
  return (joint.kind as { type?: string }).type === "Slider" ? "mm" : "deg";
}

const round = (v: number) => Math.round(v * 1e6) / 1e6;

function roundTransform(t: Transform3D): Transform3D {
  return {
    translation: {
      x: round(t.translation.x),
      y: round(t.translation.y),
      z: round(t.translation.z),
    },
    rotation: {
      x: round(t.rotation.x),
      y: round(t.rotation.y),
      z: round(t.rotation.z),
    },
    scale: t.scale,
  } as Transform3D;
}

/**
 * Apply a `joint_state` map to a clone of `doc`.
 *
 * Returns the original document unchanged (and no pose report) when the
 * argument is absent — the default path costs nothing. The input document is
 * never mutated.
 */
export function applyJointState(
  doc: Document,
  raw: unknown,
): { doc: Document; pose?: PoseInfo } {
  if (raw === undefined || raw === null) return { doc };
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new PoseError(
      "`joint_state` must be an object mapping joint id (or name) to a number " +
        "(degrees for revolute/ball joints, mm for sliders).",
    );
  }
  const entries = Object.entries(raw as Record<string, unknown>);
  if (entries.length === 0) return { doc };

  const joints = doc.joints ?? [];
  if (joints.length === 0) {
    throw new PoseError(
      "`joint_state` was given but this document has no joints — nothing to pose. " +
        "Author the assembly's joints first (they are what make a pose meaningful).",
    );
  }

  // Resolve a key by joint id first, then by unique joint name.
  const byId = new Map(joints.map((j) => [j.id, j]));
  const byName = new Map<string, Joint | null>();
  for (const j of joints) {
    if (!j.name) continue;
    byName.set(j.name, byName.has(j.name) ? null : j);
  }

  const posed = structuredClone(doc);
  const posedById = new Map((posed.joints ?? []).map((j) => [j.id, j]));
  const applied: Record<string, number> = {};
  const warnings: string[] = [];

  for (const [key, value] of entries) {
    const joint = byId.get(key) ?? byName.get(key) ?? undefined;
    if (joint === null) {
      throw new PoseError(
        `joint_state key "${key}" is ambiguous — more than one joint carries that name. ` +
          "Use the joint id instead.",
      );
    }
    if (!joint) {
      const known = joints
        .map((j) => (j.name ? `${j.id} ("${j.name}")` : j.id))
        .join(", ");
      throw new PoseError(
        `joint_state key "${key}" matches no joint in this document. Known joints: ${known}.`,
      );
    }
    const state = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(state)) {
      throw new PoseError(
        `joint_state["${key}"] is not a finite number (got ${JSON.stringify(value)}).`,
      );
    }

    let effective = state;
    const limits = limitsOf(joint);
    if (limits) {
      const [lo, hi] = limits;
      if (state < lo || state > hi) {
        effective = Math.min(hi, Math.max(lo, state));
        warnings.push(
          `joint "${joint.id}"${joint.name ? ` ("${joint.name}")` : ""} was driven to ` +
            `${state} ${unitOf(joint)}, outside its limits [${lo}, ${hi}] — clamped to ` +
            `${effective}. The rendered pose is the clamped one, not the requested one.`,
        );
      }
    }

    const target = posedById.get(joint.id);
    if (target) target.state = effective;
    applied[joint.id] = effective;
  }

  const transforms: Record<string, Transform3D> = {};
  for (const [id, t] of solveForwardKinematics(posed)) {
    transforms[id] = roundTransform(t);
  }

  return {
    doc: posed,
    pose: {
      applied,
      ...(warnings.length > 0 ? { warnings } : {}),
      transforms,
    },
  };
}
