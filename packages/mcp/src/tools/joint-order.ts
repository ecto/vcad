/**
 * Observation-to-document joint mapping.
 *
 * The physics kernel emits `joint_positions[i]` / `joint_velocities[i]`
 * indexed by the env's joint order. Kernels that expose `jointIds()` make
 * that order explicit; we map each observation slot onto the doc joint with
 * the matching id, immune to any ordering drift between the env and
 * `doc.joints`. Older kernel builds (no `jointIds`) fall back to positional
 * mapping — correct for kernels since the joint_order fix pinned the
 * observation order to document order, and the shipped assumption before it.
 */

import type { Document } from "@vcad/ir";

type DocJoint = NonNullable<Document["joints"]>[number];

/**
 * Resolve, once per recording, the doc joint that each observation slot
 * writes into. Returns the joints array aligned to observation order, or an
 * error naming the env joints that have no counterpart in the document.
 */
export function resolveObservationJoints(
  joints: DocJoint[],
  jointIds: string[] | null,
): { joints: DocJoint[] } | { error: string } {
  if (!jointIds) {
    return { joints };
  }
  const byId = new Map(joints.map((j) => [j.id, j]));
  const missing = jointIds.filter((id) => !byId.has(id));
  if (missing.length > 0) {
    return {
      error:
        `env joints [${missing.join(", ")}] do not exist in the document — ` +
        "the env must have been created from a different assembly.",
    };
  }
  return { joints: jointIds.map((id) => byId.get(id)!) };
}
