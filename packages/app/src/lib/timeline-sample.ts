/**
 * Document animation-timeline sampling.
 *
 * TS port of `Timeline::sample_track` in `crates/vcad-ir/src/animation.rs` —
 * the kernel is the source of truth for easing semantics (the incoming key's
 * `ease` governs its segment; ends clamp).
 */

import type { AnimTrack, Timeline } from "@vcad/ir";

/** Sample a track's value at time `t` (seconds). */
export function sampleTrack(track: AnimTrack, t: number): number | null {
  const keys = track.keys;
  if (keys.length === 0) return null;
  const first = keys[0]!;
  if (t <= first.t) return first.value;
  const last = keys[keys.length - 1]!;
  if (t >= last.t) return last.value;

  const idx = keys.findIndex((k) => k.t > t);
  if (idx <= 0) return last.value;
  const a = keys[idx - 1]!;
  const b = keys[idx]!;
  const span = b.t - a.t;
  let u = span <= 0 ? 1 : (t - a.t) / span;
  switch (b.ease) {
    case "step":
      u = u >= 1 ? 1 : 0;
      break;
    case "ease-in-out":
      u = u * u * (3 - 2 * u);
      break;
    default:
      break; // linear
  }
  return a.value + (b.value - a.value) * u;
}

/** All joint-track values at time `t`, keyed by jointId. */
export function sampleJointTracks(
  timeline: Timeline,
  t: number,
): Map<string, number> {
  const out = new Map<string, number>();
  for (const track of timeline.tracks) {
    if (track.target.type !== "Joint") continue;
    const v = sampleTrack(track, t);
    if (v !== null) out.set(track.target.jointId, v);
  }
  return out;
}

/** True when the timeline has at least one joint track with keys. */
export function hasJointTracks(timeline: Timeline | undefined | null): boolean {
  return !!timeline?.tracks.some(
    (tr) => tr.target.type === "Joint" && tr.keys.length > 0,
  );
}
