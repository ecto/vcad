/**
 * Animation sequencer — samples a document's `Timeline` into per-frame
 * state (parameters, joints, visibility, explode, camera pose).
 *
 * Interpolation semantics mirror the Rust reference in
 * `crates/vcad-ir/src/animation.rs` (`Timeline::sample_track`): values
 * clamp before the first / after the last key; between keys the
 * destination key's ease drives the blend (linear, step, smoothstep).
 * Camera shots are declarative intents compiled to per-frame poses.
 */

import type {
  AnimTrack,
  CameraShot,
  Document,
  Timeline,
} from "@vcad/ir";

/** A per-frame camera pose in orbit coordinates. */
export interface CameraPose {
  /** Azimuth around the scene, degrees. */
  azimuthDeg: number;
  /** Elevation above the horizon, degrees. */
  elevationDeg: number;
  /** Distance factor (1 = default framing, 0.5 = half distance). */
  dolly: number;
  /** Optional part/instance id the camera is framing. */
  target?: string;
}

/** Sampled state for a single frame of the timeline. */
export interface SequenceFrame {
  /** Frame index (0-based). */
  index: number;
  /** Time in seconds. */
  t: number;
  /** Animated parameter name → value at `t`. */
  params: Record<string, number>;
  /** Joint id → state at `t`. */
  joints: Record<string, number>;
  /** Instance id → visible (track value > 0.5). */
  visibility: Record<string, boolean>;
  /** Global exploded-view factor (0 = assembled). */
  explode: number;
  /** Camera pose compiled from the timeline's shots. */
  camera: CameraPose;
  /** True iff params differ from the previous frame (frame 0: true if any parameter track exists). */
  geometryDirty: boolean;
}

const DEFAULT_POSE: CameraPose = { azimuthDeg: 0, elevationDeg: 30, dolly: 1 };

/**
 * Sample a track's value at time `t` with easing between keys.
 *
 * Matches `Timeline::sample_track` in the Rust IR: clamps outside the key
 * range; between keys `a` and `b`, `u = (t - a.t) / (b.t - a.t)` shaped by
 * the destination key `b`'s ease.
 */
export function sampleTrackValue(track: AnimTrack, t: number): number {
  const keys = track.keys;
  if (keys.length === 0) return 0;
  const first = keys[0];
  if (t <= first.t) return first.value;
  const last = keys[keys.length - 1];
  if (t >= last.t) return last.value;
  const idx = keys.findIndex((k) => k.t > t);
  const a = keys[idx - 1];
  const b = keys[idx];
  const span = b.t - a.t;
  let u = span <= 0 ? 1 : (t - a.t) / span;
  switch (b.ease ?? "linear") {
    case "step":
      u = u >= 1 ? 1 : 0;
      break;
    case "ease-in-out":
      u = u * u * (3 - 2 * u);
      break;
    default:
      break;
  }
  return a.value + (b.value - a.value) * u;
}

/** Compile the camera shots into a pose at time `t`, carrying `prev` state. */
function cameraPoseAt(
  shots: CameraShot[],
  t: number,
  prevPose: CameraPose,
): CameraPose {
  // Last shot whose [startS, endS) contains t wins on overlap.
  let active: CameraShot | undefined;
  for (const shot of shots) {
    if (t >= shot.startS && t < shot.endS) active = shot;
  }
  if (!active) return prevPose;
  const span = active.endS - active.startS;
  const u = span <= 0 ? 1 : (t - active.startS) / span;
  const kind = active.kind;
  switch (kind.type) {
    case "Turntable":
      return {
        azimuthDeg: kind.degrees * u,
        elevationDeg: kind.elevationDeg,
        dolly: 1,
      };
    case "Orbit":
      return {
        azimuthDeg: kind.from[0] + (kind.to[0] - kind.from[0]) * u,
        elevationDeg: kind.from[1] + (kind.to[1] - kind.from[1]) * u,
        dolly: 1,
      };
    case "Focus":
      return {
        azimuthDeg: prevPose.azimuthDeg,
        elevationDeg: prevPose.elevationDeg,
        dolly: 1 + (kind.dolly - 1) * u,
        target: kind.target,
      };
    case "Static":
      return { ...DEFAULT_POSE };
  }
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
  const fps = timeline.fps && timeline.fps > 0 ? timeline.fps : 24;
  const frameCount = Math.max(
    2,
    Math.round(timeline.durationS * fps) + 1,
  );
  const tracks = timeline.tracks ?? [];
  const shots = timeline.camera ?? [];
  const hasParamTracks = tracks.some((tr) => tr.target.type === "Parameter");

  const frames: SequenceFrame[] = [];
  let prevParams: Record<string, number> | undefined;
  let prevPose: CameraPose = { ...DEFAULT_POSE };

  for (let index = 0; index < frameCount; index++) {
    const t = index / fps;
    const params: Record<string, number> = {};
    const joints: Record<string, number> = {};
    const visibility: Record<string, boolean> = {};
    let explode = 0;

    for (const track of tracks) {
      const value = sampleTrackValue(track, t);
      const target = track.target;
      switch (target.type) {
        case "Parameter":
          params[target.name] = value;
          break;
        case "Joint":
          joints[target.jointId] = value;
          break;
        case "Visibility":
          visibility[target.instanceId] = value > 0.5;
          break;
        case "Explode":
          explode = value;
          break;
      }
    }

    const camera = cameraPoseAt(shots, t, prevPose);
    prevPose = camera;

    const geometryDirty =
      prevParams === undefined
        ? hasParamTracks
        : Object.keys(params).some((name) => params[name] !== prevParams![name]);
    prevParams = params;

    frames.push({
      index,
      t,
      params,
      joints,
      visibility,
      explode,
      camera,
      geometryDirty,
    });
  }
  return frames;
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
