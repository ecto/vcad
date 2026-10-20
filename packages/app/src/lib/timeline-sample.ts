/**
 * Document animation-timeline sampling.
 *
 * Thin wrapper over `Timeline::sample_track` in
 * `crates/vcad-ir/src/animation.rs` (via the engine's `sampleTrackValue`) —
 * the kernel is the single source of truth for easing semantics.
 */

import type { Timeline } from "@vcad/ir";
import { sampleTrackValue } from "@vcad/engine";

/** All joint-track values at time `t`, keyed by jointId. */
export function sampleJointTracks(
  timeline: Timeline,
  t: number,
): Map<string, number> {
  const out = new Map<string, number>();
  for (const track of timeline.tracks) {
    if (track.target.type !== "Joint") continue;
    // The kernel samples an empty track to 0; skip it so it doesn't drive
    // the joint.
    if (track.keys.length === 0) continue;
    out.set(track.target.jointId, sampleTrackValue(track, t));
  }
  return out;
}

/** True when the timeline has at least one joint track with keys. */
export function hasJointTracks(timeline: Timeline | undefined | null): boolean {
  return !!timeline?.tracks.some(
    (tr) => tr.target.type === "Joint" && tr.keys.length > 0,
  );
}
