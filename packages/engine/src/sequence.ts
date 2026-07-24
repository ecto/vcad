/**
 * Animation sequencer — samples a document's `Timeline` into per-frame
 * state (parameters, joints, visibility, explode, camera pose).
 *
 * Thin wrapper over the Rust reference implementation
 * (`Timeline::sample_sequence` in `crates/vcad-ir/src/animation.rs`),
 * reached through the kernel WASM bindings `sample_timeline_sequence` /
 * `sample_timeline_track` — one boundary crossing per sequence (or per
 * track sample), never per frame. The kernel WASM module must be
 * initialized (`await getKernelWasm()`) before calling the samplers;
 * `poseDocument` is pure document surgery and needs no WASM.
 */

import type {
  AnimTrack,
  CameraPose,
  Document,
  SequenceFrame,
  Timeline,
} from "@vcad/ir";
import { getKernelWasmSync } from "./wasm-singleton.js";

export type { CameraPose, SequenceFrame };

interface TimelineSamplerWasm {
  sample_timeline_sequence(timelineJson: string): string;
  sample_timeline_track(trackJson: string, t: number): number;
}

function requireWasm(): TimelineSamplerWasm {
  const mod = getKernelWasmSync() as unknown as Partial<TimelineSamplerWasm> | null;
  if (
    !mod ||
    typeof mod.sample_timeline_sequence !== "function" ||
    typeof mod.sample_timeline_track !== "function"
  ) {
    throw new Error(
      "kernel WASM not initialized (or too old for timeline sampling) — await getKernelWasm() before sampling sequences",
    );
  }
  return mod as TimelineSamplerWasm;
}

/**
 * Sample a track's value at time `t` with easing between keys.
 *
 * Delegates to `Timeline::sample_track` in the Rust IR: clamps outside the
 * key range; between keys `a` and `b`, `u = (t - a.t) / (b.t - a.t)` shaped
 * by the destination key `b`'s ease. An empty track samples to 0.
 */
export function sampleTrackValue(track: AnimTrack, t: number): number {
  return requireWasm().sample_timeline_track(JSON.stringify(track), t);
}

/**
 * Sample the document's timeline (or `timelineOverride`) into a full frame
 * sequence. Returns an empty array when no timeline exists.
 */
export function sampleSequence(
  doc: Document,
  timelineOverride?: Timeline,
): SequenceFrame[] {
  const timeline = timelineOverride ?? doc.timeline;
  if (!timeline) return [];
  const json = requireWasm().sample_timeline_sequence(JSON.stringify(timeline));
  return JSON.parse(json) as SequenceFrame[];
}

/**
 * Apply a sampled frame to a deep clone of `doc`: parameter values and
 * joint states are overwritten for entries that exist. The input document
 * is never mutated.
 */
export function poseDocument(doc: Document, frame: SequenceFrame): Document {
  const posed = structuredClone(doc);
  const parameters = posed.parameters;
  if (parameters) {
    for (const [name, value] of Object.entries(frame.params)) {
      const param = parameters[name];
      if (param) param.value = value;
    }
  }
  const joints = posed.joints;
  if (joints) {
    for (const joint of joints) {
      const state = frame.joints[joint.id];
      if (state !== undefined) joint.state = state;
    }
  }
  return posed;
}
