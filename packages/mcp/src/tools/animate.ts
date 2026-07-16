/**
 * Animation tools — the video rendering engine for agents.
 *
 * Three tools over the document's `timeline` (see `vcad-ir::animation`):
 *
 * - `animate` — author/replace the timeline as data (tracks keyframe named
 *   parameters, joint states, instance visibility, or a global explode
 *   factor; camera moves are declarative shot intents).
 * - `render_sequence` — compile the timeline into an **animated GLB**
 *   (glTF animation channels riding on the instance nodes) so the agent
 *   sees its own animation inline in the viewer, cheaply, before export.
 * - `export_video` — render every frame through the kernel SVG renderer
 *   and encode an animated GIF (gifenc, always available) or MP4 (ffmpeg,
 *   when present), with the verification HUD burned into each frame.
 *
 * A sequence is not spectacle — it's evidence. When the document carries
 * `clearance_specs`, both render tools re-measure every spec across the
 * sampled frames and report per-spec minima over time: the receipt that
 * the mechanism runs through its motion without violating the asserted
 * clearances.
 */

import {
  getKernelWasm,
  resetKernelWasm,
  solveForwardKinematics,
  sampleSequence,
  poseDocument,
  type SequenceFrame,
} from "@vcad/engine";
import type { Document, Timeline, AnimTrack, Vec3 } from "@vcad/ir";
import { spawn } from "node:child_process";
import { getSession, resolveDocInput } from "./session.js";
import { getEnvRecord } from "./gym.js";
import { resolveObservationJoints } from "./joint-order.js";
import { behavior, type ToolDef, type ToolContext } from "./tool-def.js";
import { ok, err, type ToolResult } from "./tool-result.js";
import { rasterize, loadGifenc } from "./record.js";
import { computeGroupClearance } from "./clearance.js";
import {
  buildGlb,
  eulerXyzDegToQuat,
  buildPartLabels,
  DEFAULT_MATERIAL,
  type GlbMesh,
  type GlbAnimationChannel,
  type GlbAnimationOptions,
} from "../export/glb.js";
import { storeArtifact } from "./artifact-store.js";
import { maxInlineExportBytes } from "./remote.js";
import type { Engine } from "@vcad/engine";

/* ------------------------------------------------------------------ */
/* Limits                                                              */
/* ------------------------------------------------------------------ */

/** Hard cap on sampled frames per sequence (matches record_simulation). */
const MAX_FRAMES = 600;
/** Cap on full geometry re-evaluations for parameter-morph GLBs. */
const MAX_GEO_SAMPLES = 24;
/** Cap on frames swept by the clearance verifier (evenly spaced). */
const MAX_VERIFY_FRAMES = 25;
/** Inline `_meta` preview cap, mirroring server.ts INLINE_PREVIEW_MAX_BASE64. */
const INLINE_GLB_MAX_BASE64 = 1_500_000;

const DEFAULT_WIDTH_PX = 480;
const MIN_WIDTH_PX = 64;
const MAX_WIDTH_PX = 1280;
const SVG_SCALE = 2.0;

/* ------------------------------------------------------------------ */
/* Timeline validation                                                 */
/* ------------------------------------------------------------------ */

interface TimelineIssue {
  track: number;
  problem: string;
}

/** Validate a timeline against the document it will animate. */
export function validateTimeline(
  doc: Document,
  timeline: Timeline,
): TimelineIssue[] {
  const issues: TimelineIssue[] = [];
  if (!(timeline.durationS > 0)) {
    issues.push({ track: -1, problem: "durationS must be > 0" });
  }
  if (timeline.fps !== undefined && !(timeline.fps > 0 && timeline.fps <= 60)) {
    issues.push({ track: -1, problem: "fps must be in (0, 60]" });
  }
  const paramNames = new Set(Object.keys(doc.parameters ?? {}));
  const jointIds = new Set((doc.joints ?? []).map((j) => j.id));
  const instanceIds = new Set((doc.instances ?? []).map((i) => i.id));
  (timeline.tracks ?? []).forEach((track, i) => {
    const t = track.target;
    if (t.type === "Parameter" && !paramNames.has(t.name)) {
      issues.push({
        track: i,
        problem: `unknown parameter "${t.name}" (declared: ${[...paramNames].join(", ") || "none"})`,
      });
    }
    if (t.type === "Joint" && !jointIds.has(t.jointId)) {
      issues.push({
        track: i,
        problem: `unknown joint "${t.jointId}" (joints: ${[...jointIds].join(", ") || "none"})`,
      });
    }
    if (t.type === "Visibility" && !instanceIds.has(t.instanceId)) {
      issues.push({
        track: i,
        problem: `unknown instance "${t.instanceId}"`,
      });
    }
    if (!track.keys || track.keys.length === 0) {
      issues.push({ track: i, problem: "track has no keys" });
    } else {
      for (let k = 1; k < track.keys.length; k++) {
        if (track.keys[k]!.t < track.keys[k - 1]!.t) {
          issues.push({ track: i, problem: "keys not ascending in t" });
          break;
        }
      }
    }
  });
  return issues;
}

/* ------------------------------------------------------------------ */
/* Per-frame clearance verification                                    */
/* ------------------------------------------------------------------ */

export interface SequenceClearanceReport {
  frames_checked: number;
  specs: Array<{
    label: string;
    required_min_mm: number;
    observed_min_mm: number;
    holds: boolean;
    worst_frame_t: number;
  }>;
  all_hold: boolean;
}

/**
 * Sweep the document's clearance specs across the sampled frames
 * (evenly thinned to MAX_VERIFY_FRAMES) and report per-spec minima.
 * This is what turns a rendered sequence into evidence.
 */
export function verifySequenceClearance(
  doc: Document,
  frames: SequenceFrame[],
  engine: Engine,
): SequenceClearanceReport | null {
  const specs = doc.clearance_specs ?? [];
  if (specs.length === 0 || frames.length === 0) return null;

  const stride = Math.max(1, Math.ceil(frames.length / MAX_VERIFY_FRAMES));
  const sampled = frames.filter(
    (_, i) => i % stride === 0 || i === frames.length - 1,
  );

  const worst = specs.map((s) => ({
    label: s.label,
    required_min_mm: s.min_mm,
    observed_min_mm: Number.POSITIVE_INFINITY,
    worst_frame_t: 0,
  }));

  for (const frame of sampled) {
    const posed = poseDocument(doc, frame);
    for (let si = 0; si < specs.length; si++) {
      const spec = specs[si]!;
      const { result } = computeGroupClearance(
        posed,
        engine,
        spec.group_a,
        spec.group_b,
      );
      if (!result) continue;
      if (result.distance_mm < worst[si]!.observed_min_mm) {
        worst[si]!.observed_min_mm = result.distance_mm;
        worst[si]!.worst_frame_t = frame.t;
      }
    }
  }

  const out = worst
    .filter((w) => Number.isFinite(w.observed_min_mm))
    .map((w) => ({
      ...w,
      observed_min_mm: Math.round(w.observed_min_mm * 1000) / 1000,
      holds: w.observed_min_mm >= w.required_min_mm,
    }));
  if (out.length === 0) return null;
  return {
    frames_checked: sampled.length,
    specs: out,
    all_hold: out.every((s) => s.holds),
  };
}

/* ------------------------------------------------------------------ */
/* animate — author the timeline                                       */
/* ------------------------------------------------------------------ */

export const animateSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session document id from open_document.",
    },
    timeline: {
      type: "object" as const,
      description:
        'Full timeline to set on the document: {durationS, fps?, tracks?, camera?}. Tracks keyframe targets: {target:{type:"Parameter",name}|{type:"Joint",jointId}|{type:"Visibility",instanceId}|{type:"Explode"}, keys:[{t,value,ease?:"linear"|"step"|"ease-in-out"}]}. Camera shots: {startS,endS,kind:{type:"Turntable",degrees,elevationDeg?}|{type:"Orbit",from:[az,el],to:[az,el]}|{type:"Focus",target,dolly?}|{type:"Static"}}. Pass null to clear the timeline.',
    },
  },
  required: ["document_id", "timeline"],
};

export async function animate(
  args: Record<string, unknown>,
): Promise<ToolResult> {
  const doc = getSession(String(args.document_id));
  const raw = args.timeline;
  if (raw === null) {
    doc.timeline = undefined;
    return ok({ timeline: null, note: "timeline cleared" });
  }
  const timeline = raw as Timeline;
  const issues = validateTimeline(doc, timeline);
  if (issues.length > 0) {
    return err(
      `timeline rejected:\n${issues
        .map((i) => (i.track >= 0 ? `track[${i.track}]: ${i.problem}` : i.problem))
        .join("\n")}`,
    );
  }
  doc.timeline = timeline;
  const frames = sampleSequence(doc);
  return ok({
    duration_s: timeline.durationS,
    fps: timeline.fps ?? 24,
    frames: frames.length,
    tracks: (timeline.tracks ?? []).map((t) => t.target),
    camera_shots: (timeline.camera ?? []).length,
    note: "Timeline set. Preview with render_sequence, ship with export_video.",
  });
}

/* ------------------------------------------------------------------ */
/* render_sequence — animated GLB                                      */
/* ------------------------------------------------------------------ */

export const renderSequenceSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session document id from open_document.",
    },
    timeline: {
      type: "object" as const,
      description:
        "Optional one-shot timeline override (same shape as `animate`). When omitted, the document's stored timeline is used.",
    },
    verify: {
      type: "boolean" as const,
      description:
        "Sweep the document's clearance_specs across the sequence and attach the per-spec report (default true when specs exist).",
    },
  },
  required: ["document_id"],
};

function vec3(v: Vec3 | undefined): [number, number, number] {
  return v ? [v.x, v.y, v.z] : [0, 0, 0];
}

/** Does the timeline drive any geometry-changing (parameter) track? */
function hasParamTracks(timeline: Timeline): boolean {
  return (timeline.tracks ?? []).some((t) => t.target.type === "Parameter");
}

function paramTrackNames(timeline: Timeline): string[] {
  return (timeline.tracks ?? [])
    .filter((t) => t.target.type === "Parameter")
    .map((t) => (t.target as { name: string }).name);
}

/** Name of the meshless carrier node whose rotation encodes the camera
 *  azimuth. The viewer reads it and orbits its own camera — the model
 *  itself never rotates. Generic glTF players simply ignore the empty node. */
export const CAMERA_NODE = "__camera";

/** Turntable/orbit/focus camera → channels on the invisible `__camera`
 *  node. Rotation encodes yaw (about Z, Z-up model space) composed with
 *  pitch (about X): q = Rz(azimuth) ⊗ Rx(elevation) — the viewer reads it
 *  back as a ZXY euler. Dolly rides on the scale channel. */
function cameraChannels(frames: SequenceFrame[]): GlbAnimationChannel[] {
  const f0 = frames[0]!.camera;
  const rotating = frames.some(
    (f) =>
      Math.abs(f.camera.azimuthDeg - f0.azimuthDeg) > 1e-9 ||
      Math.abs(f.camera.elevationDeg - f0.elevationDeg) > 1e-9,
  );
  const dollying = frames.some((f) => Math.abs(f.camera.dolly - f0.dolly) > 1e-9);
  const channels: GlbAnimationChannel[] = [];
  if (rotating) {
    const times: number[] = [];
    const values: number[] = [];
    for (const f of frames) {
      times.push(f.t);
      const az = (f.camera.azimuthDeg * Math.PI) / 360;
      const el = (f.camera.elevationDeg * Math.PI) / 360;
      const s1 = Math.sin(az), c1 = Math.cos(az);
      const s2 = Math.sin(el), c2 = Math.cos(el);
      // qz(az) ⊗ qx(el), Hamilton product, [x, y, z, w].
      values.push(c1 * s2, s1 * s2, s1 * c2, c1 * c2);
    }
    channels.push({ nodeName: CAMERA_NODE, path: "rotation", times, values });
  }
  if (dollying) {
    const times: number[] = [];
    const values: number[] = [];
    for (const f of frames) {
      times.push(f.t);
      const d = Math.max(0.05, f.camera.dolly);
      values.push(d, d, d);
    }
    channels.push({ nodeName: CAMERA_NODE, path: "scale", times, values });
  }
  return channels;
}

/** Explode directions: per instance, outward from the centroid of instance
 *  translations; magnitude scales with assembly extent. */
function explodeDirections(
  doc: Document,
  transforms: Map<string, { translation: Vec3 }>,
): Map<string, [number, number, number]> {
  const ids = [...transforms.keys()];
  const pts = ids.map((id) => vec3(transforms.get(id)!.translation));
  const c = pts
    .reduce((acc, p) => [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]], [0, 0, 0])
    .map((v) => v / Math.max(1, pts.length));
  let extent = 0;
  for (const p of pts) {
    extent = Math.max(
      extent,
      Math.hypot(p[0] - c[0]!, p[1] - c[1]!, p[2] - c[2]!),
    );
  }
  const scale = extent > 1e-9 ? extent : 50;
  const dirs = new Map<string, [number, number, number]>();
  ids.forEach((id, i) => {
    const p = pts[i]!;
    const d: [number, number, number] = [
      p[0] - c[0]!,
      p[1] - c[1]!,
      p[2] - c[2]!,
    ];
    const len = Math.hypot(d[0], d[1], d[2]);
    if (len < 1e-9) {
      dirs.set(id, [0, 0, 0]);
    } else {
      dirs.set(id, [
        (d[0] / len) * scale,
        (d[1] / len) * scale,
        (d[2] / len) * scale,
      ]);
    }
  });
  return dirs;
}

/**
 * Compile document + timeline into an animated GLB. Exported for tests.
 * Returns null when there is nothing previewable.
 */
export function buildSequenceGlb(
  doc: Document,
  timeline: Timeline,
  frames: SequenceFrame[],
  engine: Engine,
): { glb: Uint8Array; stats: Record<string, unknown> } | null {
  const channels: GlbAnimationChannel[] = [];
  const meshes: GlbMesh[] = [];

  const paramFrames = frames.filter((f) => f.geometryDirty);
  const usesParams = hasParamTracks(timeline) && paramFrames.length > 0;

  // ---- assembly path: instance nodes + FK TRS channels ----
  const frame0 = poseDocument(doc, frames[0]!);
  let instanceCount = 0;
  try {
    const scene = engine.evaluate(frame0);
    const instances = scene?.instances ?? [];
    instanceCount = instances.length;
    if (instances.length > 0) {
      // Per-frame FK for every instance.
      const perInstance = new Map<
        string,
        { times: number[]; trans: number[]; rot: number[] }
      >();
      for (const inst of instances) {
        perInstance.set(inst.instanceId, { times: [], trans: [], rot: [] });
      }
      const frame0Transforms = solveForwardKinematics(frame0);
      const dirs = explodeDirections(
        doc,
        frame0Transforms as Map<string, { translation: Vec3 }>,
      );

      for (const frame of frames) {
        const posed = poseDocument(doc, frame);
        const world = solveForwardKinematics(posed);
        for (const inst of instances) {
          const rec = perInstance.get(inst.instanceId)!;
          const t = world.get(inst.instanceId) ?? inst.transform;
          const translation = vec3(t?.translation);
          if (frame.explode > 0) {
            const d = dirs.get(inst.instanceId) ?? [0, 0, 0];
            translation[0] += d[0] * frame.explode;
            translation[1] += d[1] * frame.explode;
            translation[2] += d[2] * frame.explode;
          }
          rec.times.push(frame.t);
          rec.trans.push(...translation);
          rec.rot.push(
            ...eulerXyzDegToQuat(
              t?.rotation ?? { x: 0, y: 0, z: 0 },
            ),
          );
        }
      }

      for (const inst of instances) {
        const name = `${inst.instanceId}:${inst.name ?? ""}`;
        meshes.push({
          name,
          positions: inst.mesh.positions,
          indices: inst.mesh.indices,
          normals: inst.mesh.normals,
          color: DEFAULT_MATERIAL.color,
          metallic: DEFAULT_MATERIAL.metallic,
          roughness: DEFAULT_MATERIAL.roughness,
          meshKey: inst.partDefId,
        });
        const rec = perInstance.get(inst.instanceId)!;
        channels.push({
          nodeName: name,
          path: "translation",
          times: rec.times,
          values: rec.trans,
        });
        channels.push({
          nodeName: name,
          path: "rotation",
          times: rec.times,
          values: rec.rot,
        });

        // Visibility tracks ride on scale (glTF has no visibility channel).
        const visTimes: number[] = [];
        const visValues: number[] = [];
        let prev: boolean | undefined;
        for (const frame of frames) {
          const vis = frame.visibility[inst.instanceId];
          if (vis === undefined) continue;
          if (prev === undefined || vis !== prev) {
            visTimes.push(frame.t);
            const s = vis ? 1 : 0.0001;
            visValues.push(s, s, s);
            prev = vis;
          }
        }
        if (visTimes.length > 0) {
          channels.push({
            nodeName: name,
            path: "scale",
            times: visTimes,
            values: visValues,
            interpolation: "STEP",
          });
        }
      }
    }
  } catch {
    // Assembly evaluation failure falls through to the param/parts path.
  }

  // ---- parameter-morph path: geometry samples with STEP visibility ----
  let geoSamples = 0;
  if (usesParams) {
    const stride = Math.max(1, Math.ceil(paramFrames.length / MAX_GEO_SAMPLES));
    const sampled = paramFrames.filter(
      (_, i) => i % stride === 0 || i === paramFrames.length - 1,
    );
    geoSamples = sampled.length;
    const labels = buildPartLabels(doc);
    for (let k = 0; k < sampled.length; k++) {
      const frame = sampled[k]!;
      const posed = poseDocument(doc, frame);
      let scene;
      try {
        scene = engine.evaluate(posed);
      } catch {
        continue;
      }
      const start = frame.t;
      const end = k + 1 < sampled.length ? sampled[k + 1]!.t : Infinity;
      for (let i = 0; i < (scene?.parts.length ?? 0); i++) {
        const part = scene!.parts[i]!;
        const name = `s${k}|${labels[i] ?? `part_${i}`}`;
        meshes.push({
          name,
          positions: part.mesh.positions,
          indices: part.mesh.indices,
          normals: part.mesh.normals,
          color: DEFAULT_MATERIAL.color,
          metallic: DEFAULT_MATERIAL.metallic,
          roughness: DEFAULT_MATERIAL.roughness,
        });
        // Visible during [start, end): STEP scale keys at the boundaries.
        const times: number[] = [];
        const values: number[] = [];
        if (start > 0) {
          times.push(0);
          values.push(0.0001, 0.0001, 0.0001);
        }
        times.push(start);
        values.push(1, 1, 1);
        if (Number.isFinite(end)) {
          times.push(end);
          values.push(0.0001, 0.0001, 0.0001);
        }
        channels.push({
          nodeName: name,
          path: "scale",
          times,
          values,
          interpolation: "STEP",
        });
      }
    }
  }

  if (meshes.length === 0) return null;

  const camChannels = cameraChannels(frames);
  channels.push(...camChannels);

  const animation: GlbAnimationOptions = {
    name: "timeline",
    channels,
    extraNodes: camChannels.length > 0 ? [CAMERA_NODE] : undefined,
  };
  const glb = buildGlb(meshes, "sequence", animation);
  return {
    glb,
    stats: {
      frames: frames.length,
      duration_s: timeline.durationS,
      fps: timeline.fps ?? 24,
      instances: instanceCount,
      geometry_samples: geoSamples,
      channels: channels.length,
      animated_params: paramTrackNames(timeline),
    },
  };
}

export async function renderSequence(
  args: Record<string, unknown>,
  ctx: ToolContext,
): Promise<ToolResult> {
  let doc: Document;
  try {
    ({ doc } = resolveDocInput(args));
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }

  const timeline = (args.timeline as Timeline | undefined) ?? doc.timeline;
  if (!timeline) {
    return err(
      "No timeline: set one with `animate` (or pass a one-shot `timeline` here).",
    );
  }
  const issues = validateTimeline(doc, timeline);
  if (issues.length > 0) {
    return err(
      `timeline invalid:\n${issues.map((i) => i.problem).join("\n")}`,
    );
  }

  const frames = sampleSequence(doc, timeline);
  if (frames.length > MAX_FRAMES) {
    return err(
      `sequence has ${frames.length} frames (max ${MAX_FRAMES}); lower fps or duration.`,
    );
  }

  const built = buildSequenceGlb(doc, timeline, frames, ctx.engine);
  if (!built) return err("document has no previewable geometry to animate.");

  const wantVerify = args.verify !== false;
  const verification = wantVerify
    ? verifySequenceClearance(doc, frames, ctx.engine)
    : null;

  const b64 = Buffer.from(built.glb).toString("base64");
  const body: Record<string, unknown> = {
    ...built.stats,
    glb_bytes: built.glb.byteLength,
    ...(verification ? { verification } : {}),
  };

  if (built.glb.byteLength > maxInlineExportBytes()) {
    body.artifact = storeArtifact([
      { name: "sequence.glb", content: built.glb },
    ]);
  } else {
    body.glb_base64 = b64;
  }

  const result = ok(body) as ToolResult & {
    _meta?: Record<string, unknown>;
  };
  // Inline viewer preview: same envelope the dispatch layer attaches for
  // geometry tools, so the mounted viewer paints the animation immediately.
  if (b64.length <= INLINE_GLB_MAX_BASE64 && typeof args.document_id === "string") {
    result._meta = {
      "vcad.io/preview": {
        document_id: args.document_id,
        glb: b64,
        version: `seq-${Date.now().toString(36)}`,
        mode: "animated",
      },
    };
  }
  return result;
}

/* ------------------------------------------------------------------ */
/* export_video — GIF / MP4 with HUD                                   */
/* ------------------------------------------------------------------ */

export const exportVideoSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session document id from open_document.",
    },
    timeline: {
      type: "object" as const,
      description: "Optional one-shot timeline override (same shape as `animate`).",
    },
    format: {
      type: "string" as const,
      enum: ["gif", "mp4", "auto"],
      description:
        "Output container. 'auto' (default) prefers mp4 when ffmpeg is available, else gif.",
    },
    view: {
      type: "string" as const,
      enum: ["iso", "isometric", "top", "front", "side"],
      description: "Camera view for the kernel renderer; defaults to 'iso'.",
    },
    width_px: {
      type: "number" as const,
      description: `Raster width per frame (${MIN_WIDTH_PX}-${MAX_WIDTH_PX}, default ${DEFAULT_WIDTH_PX}).`,
    },
    hud: {
      type: "boolean" as const,
      description:
        "Burn the verification HUD (time, animated values, clearance status) into every frame (default true).",
    },
    verify: {
      type: "boolean" as const,
      description:
        "Sweep clearance_specs across frames and attach the report + HUD status (default true when specs exist).",
    },
  },
  required: ["document_id"],
};

/** Probe once per process whether ffmpeg is spawnable. */
let ffmpegAvailable: boolean | null = null;
async function hasFfmpeg(): Promise<boolean> {
  if (ffmpegAvailable !== null) return ffmpegAvailable;
  ffmpegAvailable = await new Promise<boolean>((resolve) => {
    try {
      const p = spawn("ffmpeg", ["-version"], { stdio: "ignore" });
      p.on("error", () => resolve(false));
      p.on("exit", (code) => resolve(code === 0));
    } catch {
      resolve(false);
    }
  });
  return ffmpegAvailable;
}

/**
 * Streaming MP4 encoder: spawns ffmpeg once and pipes frames to its stdin
 * as they rasterize, so a full sequence never sits in memory (600 frames
 * at 1280px is ~4GB of raw RGBA). Exported for tests.
 */
export interface Mp4Encoder {
  /** Write one raw RGBA frame; resolves after backpressure drains. */
  writeFrame(rgba: Uint8Array): Promise<void>;
  /** End stdin and resolve with the encoded MP4. */
  finish(): Promise<Buffer>;
  /** Kill ffmpeg when the render loop bails early. */
  abort(): void;
}

/** Spawn ffmpeg for a rawvideo→mp4 pipe at the given frame geometry. */
export function startMp4Encoder(
  width: number,
  height: number,
  fps: number,
): Mp4Encoder {
  const args = [
    "-y",
    "-f", "rawvideo",
    "-pix_fmt", "rgba",
    "-s", `${width}x${height}`,
    "-r", String(fps),
    "-i", "-",
    "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2",
    "-pix_fmt", "yuv420p",
    "-movflags", "frag_keyframe+empty_moov",
    "-f", "mp4",
    "-",
  ];
  const p = spawn("ffmpeg", args, { stdio: ["pipe", "pipe", "pipe"] });
  const out: Buffer[] = [];
  const errChunks: Buffer[] = [];
  let failure: Error | null = null;
  p.stdout.on("data", (c: Buffer) => out.push(c));
  p.stderr.on("data", (c: Buffer) => errChunks.push(c));
  // ffmpeg dying mid-stream EPIPEs stdin; the exit error surfaces instead.
  p.stdin.on("error", () => {});
  const exitError = (code: number | null) =>
    new Error(
      `ffmpeg exited ${code}: ${Buffer.concat(errChunks).toString().slice(-400)}`,
    );
  const done = new Promise<Buffer>((resolve, reject) => {
    p.on("error", (e) => {
      failure = e;
      reject(e);
    });
    p.on("close", (code) => {
      if (code === 0) resolve(Buffer.concat(out));
      else {
        failure ??= exitError(code);
        reject(failure);
      }
    });
  });
  // Callers may abort before awaiting finish(); don't leak a rejection.
  done.catch(() => {});

  return {
    writeFrame(rgba: Uint8Array): Promise<void> {
      return new Promise((resolve, reject) => {
        if (failure) return reject(failure);
        if (p.exitCode !== null || p.stdin.destroyed) {
          return reject(failure ?? exitError(p.exitCode));
        }
        if (p.stdin.write(rgba)) return resolve();
        // Backpressure: wait for drain, but bail if ffmpeg exits first
        // (drain would never fire and the loop would hang).
        const onDrain = () => {
          p.removeListener("close", onClose);
          resolve();
        };
        const onClose = () => {
          p.stdin.removeListener("drain", onDrain);
          reject(failure ?? exitError(p.exitCode));
        };
        p.once("close", onClose);
        p.stdin.once("drain", onDrain);
      });
    },
    finish(): Promise<Buffer> {
      p.stdin.end();
      return done;
    },
    abort(): void {
      p.kill("SIGKILL");
    },
  };
}

/** Center-pad/crop an RGBA frame to the target dimensions (white fill). */
function fitFrame(
  rgba: Uint8Array,
  w: number,
  h: number,
  tw: number,
  th: number,
): Uint8Array {
  const out = new Uint8Array(tw * th * 4).fill(255);
  const copyW = Math.min(w, tw);
  const copyH = Math.min(h, th);
  const srcX = Math.max(0, Math.floor((w - tw) / 2));
  const srcY = Math.max(0, Math.floor((h - th) / 2));
  const dstX = Math.max(0, Math.floor((tw - w) / 2));
  const dstY = Math.max(0, Math.floor((th - h) / 2));
  for (let y = 0; y < copyH; y++) {
    const src = ((srcY + y) * w + srcX) * 4;
    const dst = ((dstY + y) * tw + dstX) * 4;
    out.set(rgba.subarray(src, src + copyW * 4), dst);
  }
  return out;
}

/** Track values shown live in the HUD for a frame. */
function hudReadout(frame: SequenceFrame, timeline: Timeline): string {
  const bits: string[] = [`t=${frame.t.toFixed(2)}s`];
  for (const [name, v] of Object.entries(frame.params)) {
    bits.push(`${name}=${v.toFixed(2)}`);
  }
  const jointTracks = (timeline.tracks ?? []).filter(
    (t: AnimTrack) => t.target.type === "Joint",
  );
  for (const tr of jointTracks.slice(0, 3)) {
    const id = (tr.target as { jointId: string }).jointId;
    const v = frame.joints[id];
    if (v !== undefined) bits.push(`${id}=${v.toFixed(1)}`);
  }
  return bits.join("  ");
}

/**
 * Inject the proof HUD into a kernel-emitted SVG frame: a bottom bar with
 * the time/value readout (left) and the clearance status (right, green when
 * every spec holds — the trust glyph baked into every frame).
 */
export function injectHud(
  svg: string,
  readout: string,
  clearance: { label: string; holds: boolean; observed_min_mm: number } | null,
): string {
  const m = svg.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/);
  if (!m) return svg;
  const w = parseFloat(m[1]!);
  const h = parseFloat(m[2]!);
  const barH = Math.max(14, h * 0.045);
  const fontSize = barH * 0.55;
  const pad = barH * 0.3;
  let right = "";
  if (clearance) {
    const color = clearance.holds ? "#22c55e" : "#ef4444";
    const text = `${clearance.label} ${clearance.observed_min_mm.toFixed(2)}mm ${clearance.holds ? "✓" : "✗"}`;
    right = `<text x="${w - pad}" y="${h - barH + fontSize + pad * 0.8}" text-anchor="end" font-family="monospace" font-size="${fontSize}" fill="${color}">${text}</text>`;
  }
  const hud = `<g id="vcad-hud"><rect x="0" y="${h - barH}" width="${w}" height="${barH}" fill="rgba(10,10,10,0.82)"/><text x="${pad}" y="${h - barH + fontSize + pad * 0.8}" font-family="monospace" font-size="${fontSize}" fill="#e5e5e5">${readout}</text>${right}</g>`;
  return svg.replace(/<\/svg>\s*$/, `${hud}</svg>`);
}

export async function exportVideo(
  args: Record<string, unknown>,
  ctx: ToolContext,
): Promise<ToolResult> {
  const doc = getSession(String(args.document_id));
  const timeline = (args.timeline as Timeline | undefined) ?? doc.timeline;
  if (!timeline) {
    return err("No timeline: set one with `animate` first.");
  }
  const issues = validateTimeline(doc, timeline);
  if (issues.length > 0) {
    return err(`timeline invalid:\n${issues.map((i) => i.problem).join("\n")}`);
  }

  const frames = sampleSequence(doc, timeline);
  if (frames.length > MAX_FRAMES) {
    return err(
      `sequence has ${frames.length} frames (max ${MAX_FRAMES}); lower fps or duration.`,
    );
  }
  const fps = timeline.fps && timeline.fps > 0 ? Math.min(60, timeline.fps) : 24;
  const widthPx = Math.min(
    MAX_WIDTH_PX,
    Math.max(MIN_WIDTH_PX, Number(args.width_px) || DEFAULT_WIDTH_PX),
  );
  const view = String(args.view ?? "iso");
  const wantHud = args.hud !== false;
  const wantVerify = args.verify !== false;

  const requested = String(args.format ?? "auto");
  const ffmpeg = await hasFfmpeg();
  const format =
    requested === "auto" ? (ffmpeg ? "mp4" : "gif") : requested;
  if (format === "mp4" && !ffmpeg) {
    return err("mp4 requested but ffmpeg is not available; use format:'gif'.");
  }

  const wasm = (await getKernelWasm()) as unknown as {
    render_svg: (vcadJson: string, scale: number) => string;
    render_svg_view?: (vcadJson: string, scale: number, view: string) => string;
  };
  if (typeof wasm.render_svg !== "function") {
    return err(
      "export_video unavailable: kernel WASM build predates render_svg.",
    );
  }

  // Verification first, so the HUD can show per-spec status while frames render.
  const verification = wantVerify
    ? verifySequenceClearance(doc, frames, ctx.engine)
    : null;
  // HUD shows the WORST spec as a coherent unit — a failing spec always
  // wins over a holding one, then the smallest margin (observed - required)
  // — so the label, distance, and verdict all describe the same assertion.
  const worstSpec =
    verification && verification.specs.length > 0
      ? [...verification.specs].sort(
          (a, b) =>
            Number(a.holds) - Number(b.holds) ||
            a.observed_min_mm - a.required_min_mm -
              (b.observed_min_mm - b.required_min_mm),
        )[0]!
      : null;
  const hudClearance = worstSpec
    ? {
        label: worstSpec.label,
        holds: worstSpec.holds,
        observed_min_mm: worstSpec.observed_min_mm,
      }
    : null;

  const gifLoad = format === "gif" ? await loadGifenc() : { mod: null };
  if (format === "gif" && !("mod" in gifLoad && gifLoad.mod)) {
    return err("gif encoder unavailable: install `gifenc`.");
  }

  let encoder: ReturnType<
    NonNullable<typeof import("gifenc")>["GIFEncoder"]
  > | null = null;
  const gifMod = "mod" in gifLoad ? gifLoad.mod : null;
  if (format === "gif" && gifMod) encoder = gifMod.GIFEncoder();

  let mp4: Mp4Encoder | null = null;
  let size: { width: number; height: number } | null = null;
  const delayMs = Math.max(1, Math.round(1000 / fps));
  const bail = (message: string): ToolResult => {
    mp4?.abort();
    return err(message);
  };

  for (let i = 0; i < frames.length; i++) {
    const frame = frames[i]!;
    const posed = poseDocument(doc, frame);
    let svg: string;
    try {
      svg =
        view !== "iso" && typeof wasm.render_svg_view === "function"
          ? wasm.render_svg_view(JSON.stringify(posed), SVG_SCALE, view)
          : wasm.render_svg(JSON.stringify(posed), SVG_SCALE);
    } catch (e) {
      if (e instanceof WebAssembly.RuntimeError) {
        resetKernelWasm(`render_svg trapped during export_video: ${e.message}`);
        return bail(`kernel trap at frame ${i + 1}/${frames.length}: ${e.message}`);
      }
      return bail(
        `render failed at frame ${i + 1}/${frames.length}: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    if (wantHud) {
      svg = injectHud(svg, hudReadout(frame, timeline), hudClearance);
    }
    const raster = await rasterize(svg, widthPx);
    if (!raster.rgba) {
      return bail(
        `rasterization failed at frame ${i + 1}: ${"reason" in raster ? raster.reason : "unknown"}`,
      );
    }
    if (!size) size = { width: raster.width, height: raster.height };
    // The kernel SVG auto-fits its bounds per frame, so raster dimensions
    // can jitter by a few pixels as geometry moves. Encoders need constant
    // dimensions: pad/crop every frame to the first frame's size.
    const rgba =
      raster.width === size.width && raster.height === size.height
        ? raster.rgba
        : fitFrame(raster.rgba, raster.width, raster.height, size.width, size.height);
    if (encoder && gifMod) {
      const palette = gifMod.quantize(rgba, 256);
      const indexed = gifMod.applyPalette(rgba, palette);
      encoder.writeFrame(indexed, size.width, size.height, {
        palette,
        delay: delayMs,
      });
    } else {
      // Stream each frame straight into ffmpeg — no buffering of the
      // full sequence in memory. Spawned lazily off the first frame's
      // dimensions; writeFrame awaits stdin drain (backpressure).
      mp4 ??= startMp4Encoder(size.width, size.height, fps);
      try {
        await mp4.writeFrame(rgba);
      } catch (e) {
        return bail(
          `mp4 encode failed at frame ${i + 1}/${frames.length}: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    }
  }
  if (!size) return err("no frames rendered.");

  let bytes: Buffer;
  let mime: string;
  let filename: string;
  if (encoder) {
    encoder.finish();
    bytes = Buffer.from(encoder.bytes());
    mime = "image/gif";
    filename = "sequence.gif";
  } else {
    try {
      bytes = await mp4!.finish();
    } catch (e) {
      return bail(
        `mp4 encode failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    mime = "video/mp4";
    filename = "sequence.mp4";
  }

  const summary: Record<string, unknown> = {
    format,
    frames: frames.length,
    fps,
    duration_s: timeline.durationS,
    width: size.width,
    height: size.height,
    bytes: bytes.byteLength,
    hud: wantHud,
    ...(verification ? { verification } : {}),
  };

  const content: Array<
    | { type: "text"; text: string }
    | { type: "image"; data: string; mimeType: string }
  > = [];
  if (bytes.byteLength > maxInlineExportBytes() || mime === "video/mp4") {
    summary.artifact = storeArtifact([{ name: filename, content: bytes }]);
  } else {
    content.push({
      type: "image",
      data: bytes.toString("base64"),
      mimeType: mime,
    });
  }
  content.push({ type: "text", text: JSON.stringify(summary, null, 2) });
  return { content } as unknown as ToolResult;
}

/* ------------------------------------------------------------------ */
/* timeline_from_simulation — physics rollout → timeline               */
/* ------------------------------------------------------------------ */

export const timelineFromSimulationSchema = {
  type: "object" as const,
  properties: {
    env_id: {
      type: "string" as const,
      description: "Environment id from create_robot_env (must have recorded steps).",
    },
    document_id: {
      type: "string" as const,
      description:
        "Session document id from open_document. Must describe the same assembly the env was created from (same joint ids).",
    },
    max_keys: {
      type: "number" as const,
      description:
        "Max keyframes per joint track; longer trajectories are thinned evenly (2-600, default 120).",
    },
    camera: {
      type: "string" as const,
      enum: ["turntable", "static"],
      description: "Camera shot to attach: a full turntable over the rollout, or static (default).",
    },
  },
  required: ["env_id", "document_id"],
};

/**
 * Pure rollout→timeline compiler (exported for tests): one linear joint
 * track per observed trajectory column, keyed at the simulation timestep
 * and thinned to `maxKeys`. Physics becomes a first-class motion source.
 */
export function compileRolloutTimeline(
  env: {
    trajectory: number[][];
    jointIds: string[] | null;
    dt: number;
    substeps: number;
  },
  joints: NonNullable<Document["joints"]>,
  opts: { maxKeys?: number; turntable?: boolean } = {},
): { timeline: Timeline } | { error: string } {
  const obsJoints = resolveObservationJoints(joints, env.jointIds);
  if ("error" in obsJoints) return { error: obsJoints.error };

  const stepTime = env.dt * Math.max(1, env.substeps);
  const steps = env.trajectory.length;
  if (steps < 2) return { error: `trajectory has ${steps} step(s); need at least 2` };
  // N samples span (N-1) intervals — using N*stepTime would freeze the
  // final pose for one extra timestep at the tail.
  const durationS = (steps - 1) * stepTime;
  const maxKeys = Math.min(600, Math.max(2, opts.maxKeys ?? 120));
  const stride = Math.max(1, Math.ceil(steps / maxKeys));

  const tracks: AnimTrack[] = obsJoints.joints.map((joint, col) => {
    const keys: { t: number; value: number; ease: "linear" }[] = [];
    for (let s = 0; s < steps; s += stride) {
      const v = env.trajectory[s]?.[col];
      if (typeof v === "number") {
        keys.push({ t: s * stepTime, value: v, ease: "linear" });
      }
    }
    const lastT = (steps - 1) * stepTime;
    const lastV = env.trajectory[steps - 1]?.[col];
    if (typeof lastV === "number" && keys[keys.length - 1]?.t !== lastT) {
      keys.push({ t: lastT, value: lastV, ease: "linear" });
    }
    return {
      target: { type: "Joint", jointId: joint.id },
      keys,
    } as unknown as AnimTrack;
  });

  // Playback fps: dense enough to be smooth, bounded by the frame cap.
  const fps = Math.min(30, Math.max(4, Math.floor((MAX_FRAMES - 1) / durationS)));

  return {
    timeline: {
      durationS,
      fps,
      tracks,
      camera: opts.turntable
        ? [
            {
              startS: 0,
              endS: durationS,
              kind: { type: "Turntable", degrees: 360, elevationDeg: 30 },
            },
          ]
        : [],
    } as unknown as Timeline,
  };
}

/**
 * Compile a recorded physics rollout (the gym trajectory ring buffer) into
 * the document's timeline via {@link compileRolloutTimeline} — the rollout
 * then replays through render_sequence / export_video with the same
 * clearance verification as authored motion.
 */
export async function timelineFromSimulation(
  args: Record<string, unknown>,
): Promise<ToolResult> {
  const doc = getSession(String(args.document_id));
  const env = getEnvRecord(String(args.env_id));
  if (!env) {
    return err(`No environment "${String(args.env_id)}" — create_robot_env first.`);
  }
  if (env.trajectory.length < 2) {
    return err(
      `Environment has ${env.trajectory.length} recorded step(s); run gym_step at least twice first.`,
    );
  }
  const joints = doc.joints ?? [];
  if (joints.length === 0) {
    return err("Document has no joints to animate.");
  }
  const compiled = compileRolloutTimeline(
    {
      trajectory: env.trajectory,
      jointIds: env.jointIds,
      dt: env.dt,
      substeps: env.substeps,
    },
    joints,
    {
      maxKeys: Number(args.max_keys) || undefined,
      turntable: args.camera === "turntable",
    },
  );
  if ("error" in compiled) {
    return err(`timeline_from_simulation refused: ${compiled.error}`);
  }
  const timeline = compiled.timeline;

  const issues = validateTimeline(doc, timeline);
  if (issues.length > 0) {
    return err(
      `rollout produced an invalid timeline:\n${issues.map((i) => i.problem).join("\n")}`,
    );
  }
  doc.timeline = timeline;
  return ok({
    duration_s: Math.round(timeline.durationS * 1000) / 1000,
    fps: timeline.fps,
    steps_compiled: env.trajectory.length,
    keys_per_track: timeline.tracks[0]?.keys.length ?? 0,
    tracks: timeline.tracks.length,
    note: "Rollout compiled to the document timeline. Preview with render_sequence, ship with export_video.",
  });
}

/* ------------------------------------------------------------------ */
/* Tool defs                                                           */
/* ------------------------------------------------------------------ */

export const toolDefs: ToolDef[] = [
  {
    name: "animate",
    pack: null,
    description:
      "Set (or clear) the document's animation timeline: keyframe named parameters, joint states, instance visibility, or a global explode factor, plus declarative camera shots (turntable/orbit/focus). Preview with render_sequence; ship with export_video.",
    inputSchema: animateSchema,
    handler: async (a) => animate(a),
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "render_sequence",
    pack: null,
    description:
      "Compile the document's timeline into an animated GLB (glTF animation channels) — the cheap preview loop for motion. Joint/visibility/explode tracks animate instance nodes; parameter tracks re-evaluate geometry at sampled times. When the document has clearance_specs, they are re-measured across frames and reported (the sequence as evidence).",
    inputSchema: renderSequenceSchema,
    handler: async (a, ctx) => renderSequence(a, ctx),
    // Mounts the viewer: the animated GLB rides in _meta and autoplays —
    // the agent (and the human next to it) watch the dailies inline.
    behavior: behavior({ mount: true }),
  },
  {
    name: "timeline_from_simulation",
    pack: null,
    description:
      "Compile a recorded physics rollout (gym_step trajectory) into the document's animation timeline — one joint track per observed joint at the simulation timestep, optional turntable. The rollout then previews via render_sequence and ships via export_video with clearance verification.",
    inputSchema: timelineFromSimulationSchema,
    handler: async (a) => timelineFromSimulation(a),
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "export_video",
    pack: null,
    description:
      "Render the timeline to an animated GIF or MP4 through the kernel renderer, with the verification HUD (time, animated values, clearance status) burned into every frame. MP4 requires ffmpeg on the server; GIF always works. Large outputs offload to an artifact URL.",
    inputSchema: exportVideoSchema,
    handler: async (a, ctx) => exportVideo(a, ctx),
    behavior: behavior({}),
  },
];
