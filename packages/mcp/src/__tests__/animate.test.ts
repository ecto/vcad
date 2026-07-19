/**
 * End-to-end coverage for the animation tools: `animate` (timeline
 * authoring + validation), `render_sequence` (animated GLB with glTF
 * channels + clearance sweep), `export_video` (GIF frames with the
 * verification HUD burned in).
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Timeline } from "@vcad/ir";
import { spawn } from "node:child_process";
import {
  animate,
  renderSequence,
  exportVideo,
  validateTimeline,
  verifySequenceClearance,
  injectHud,
  compileRolloutTimeline,
  compileJointKeyframes,
  startMp4Encoder,
} from "../tools/animate.js";
import { documents, openDocument } from "../tools/session.js";
import {
  sampleSequence,
  poseDocument,
  solveForwardKinematics,
} from "@vcad/engine";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0]!.text);
}

/** Two-link revolute assembly: a base slab and a swinging arm above it,
 *  with a named clearance spec between them. */
function armDocument(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "base",
        op: { type: "Cube", size: { x: 80, y: 80, z: 10 } },
      },
      "2": {
        id: 2,
        name: "arm",
        op: { type: "Cube", size: { x: 10, y: 10, z: 60 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [],
    partDefs: {
      base: { id: "base", name: "Base", root: 1, defaultMaterial: null },
      arm: { id: "arm", name: "Arm", root: 2, defaultMaterial: null },
    },
    instances: [
      { id: "base_inst", partDefId: "base", name: "Base", transform: null, material: null },
      { id: "arm_inst", partDefId: "arm", name: "Arm", transform: null, material: null },
    ],
    joints: [
      {
        id: "shoulder",
        name: "Shoulder",
        parentInstanceId: "base_inst",
        childInstanceId: "arm_inst",
        parentAnchor: { x: 40, y: 40, z: 30 },
        childAnchor: { x: 5, y: 5, z: 0 },
        kind: { type: "Revolute", axis: { x: 0, y: 1, z: 0 }, limits: null },
        state: 0,
      },
    ],
    groundInstanceId: "base_inst",
    clearance_specs: [
      {
        label: "arm-base-gap",
        group_a: ["arm_inst"],
        group_b: ["base_inst"],
        min_mm: 1.0,
      },
    ],
  } as unknown as Document;
}

const spinTimeline: Timeline = {
  durationS: 1.0,
  fps: 12,
  tracks: [
    {
      target: { type: "Joint", jointId: "shoulder" },
      keys: [
        { t: 0, value: 0, ease: "linear" },
        { t: 1, value: 45, ease: "ease-in-out" },
      ],
    },
  ],
  camera: [
    {
      startS: 0,
      endS: 1,
      kind: { type: "Turntable", degrees: 90, elevationDeg: 30 },
    },
  ],
};

function openArm(): string {
  const opened = out(openDocument({ initial: armDocument() }));
  return opened.document_id as string;
}

/** Parse the JSON chunk out of a GLB binary. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function glbJson(bytes: Uint8Array): any {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const jsonLen = dv.getUint32(12, true);
  return JSON.parse(
    Buffer.from(bytes.buffer, bytes.byteOffset + 20, jsonLen).toString("utf8"),
  );
}

describe("animate", () => {
  it("sets a valid timeline and reports frame math", async () => {
    const docId = openArm();
    const res = out(await animate({ document_id: docId, timeline: spinTimeline }));
    expect(res.frames).toBe(13); // 1s @ 12fps inclusive
    expect(res.tracks[0].type).toBe("Joint");
    expect(res.camera_shots).toBe(1);
  });

  it("rejects unknown targets with actionable errors", async () => {
    const docId = openArm();
    const bad = {
      ...spinTimeline,
      tracks: [
        {
          target: { type: "Joint", jointId: "nope" },
          keys: [{ t: 0, value: 0 }],
        },
      ],
    };
    const res = await animate({ document_id: docId, timeline: bad });
    expect(res.isError).toBe(true);
    expect(res.content[0]!.text).toContain("unknown joint");
    expect(res.content[0]!.text).toContain("nope");
    expect(res.content[0]!.text).toContain("shoulder");
  });

  it("clears the timeline with null", async () => {
    const docId = openArm();
    await animate({ document_id: docId, timeline: spinTimeline });
    const res = out(await animate({ document_id: docId, timeline: null }));
    expect(res.timeline).toBeNull();
  });

  it("validateTimeline flags descending keys and bad duration", () => {
    const doc = armDocument();
    const issues = validateTimeline(doc, {
      durationS: -1,
      fps: 24,
      tracks: [
        {
          target: { type: "Joint", jointId: "shoulder" },
          keys: [
            { t: 1, value: 0, ease: "linear" },
            { t: 0, value: 1, ease: "linear" },
          ],
        },
      ],
      camera: [],
    } as unknown as Timeline);
    expect(issues.some((i) => i.problem.includes("durationS"))).toBe(true);
    expect(issues.some((i) => i.problem.includes("ascending"))).toBe(true);
  });
});

describe("render_sequence", () => {
  it("emits an animated GLB with per-instance channels and a turntable root", async () => {
    const docId = openArm();
    await animate({ document_id: docId, timeline: spinTimeline });
    const res = await renderSequence({ document_id: docId }, { engine } as never);
    expect(res.isError).toBeFalsy();
    const body = out(res);
    expect(body.frames).toBe(13);
    expect(body.instances).toBe(2);
    expect(body.channels).toBeGreaterThanOrEqual(5); // 2×(T+R) + camera yaw
    expect(body.glb_base64 ?? body.artifact).toBeTruthy();

    const bytes = body.glb_base64
      ? Uint8Array.from(Buffer.from(body.glb_base64, "base64"))
      : null;
    if (bytes) {
      const json = glbJson(bytes);
      expect(json.animations).toHaveLength(1);
      const anim = json.animations[0];
      expect(anim.name).toBe("timeline");
      // The turntable rides on an empty __camera carrier node the viewer
      // reads to orbit its own camera — the model nodes must NOT be
      // wrapped in a rotating root (that would spin the whole scene).
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const camIdx = json.nodes.findIndex((n: any) => n.name === "__camera");
      expect(camIdx).toBeGreaterThanOrEqual(0);
      expect(json.nodes[camIdx].mesh).toBeUndefined();
      expect(json.scenes[0].nodes).toContain(camIdx);
      // Instance nodes remain scene roots (no rotating wrapper).
      expect(json.scenes[0].nodes.length).toBe(3);
      const camChannels = anim.channels.filter(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (c: any) => c.target.node === camIdx,
      );
      expect(camChannels).toHaveLength(1);
      expect(camChannels[0].target.path).toBe("rotation");
    }

    // Verification rode along: the spec sweep across frames.
    expect(body.verification).toBeTruthy();
    expect(body.verification.specs[0].label).toBe("arm-base-gap");
    expect(body.verification.frames_checked).toBeGreaterThan(1);
  });

  it("fails helpfully with no timeline", async () => {
    const docId = openArm();
    const res = await renderSequence({ document_id: docId }, { engine } as never);
    expect(res.isError).toBe(true);
    expect(res.content[0]!.text).toContain("animate");
  });
});

describe("sequence clearance sweep", () => {
  it("catches a violation mid-motion that endpoints alone would miss", () => {
    const doc = armDocument();
    // Swing far enough that the arm sweeps down toward the base slab.
    const frames = sampleSequence(doc, {
      durationS: 1,
      fps: 12,
      tracks: [
        {
          target: { type: "Joint", jointId: "shoulder" },
          keys: [
            { t: 0, value: 0, ease: "linear" },
            { t: 0.5, value: 170, ease: "linear" },
            { t: 1, value: 0, ease: "linear" },
          ],
        },
      ],
      camera: [],
    } as unknown as Timeline);
    const report = verifySequenceClearance(doc, frames, engine);
    expect(report).toBeTruthy();
    expect(report!.specs[0]!.observed_min_mm).toBeLessThan(
      report!.specs[0]!.required_min_mm + 60, // sanity: sweep actually measured
    );
    expect(report!.frames_checked).toBeGreaterThan(3);
    // The worst frame is strictly inside the motion, not at t=0 or t=1.
    const worstT = report!.specs[0]!.worst_frame_t;
    expect(worstT).toBeGreaterThan(0);
    expect(worstT).toBeLessThan(1);
  });
});

describe("export_video", () => {
  it("renders a GIF with the HUD and attaches the verification report", async () => {
    const docId = openArm();
    await animate({ document_id: docId, timeline: spinTimeline });
    const res = await exportVideo(
      { document_id: docId, format: "gif", width_px: 160 },
      { engine } as never,
    );
    if (res.isError) throw new Error(`export_video failed: ${res.content[0]!.text}`);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const blocks = res.content as Array<any>;
    const text = JSON.parse(blocks.find((b) => b.type === "text")!.text);
    expect(text.format).toBe("gif");
    expect(text.frames).toBe(13);
    expect(text.hud).toBe(true);
    expect(text.verification?.specs?.[0]?.label).toBe("arm-base-gap");
    const image = blocks.find((b) => b.type === "image");
    if (image) {
      expect(image.mimeType).toBe("image/gif");
      const gif = Buffer.from(image.data, "base64");
      expect(gif.subarray(0, 6).toString("ascii")).toMatch(/^GIF8[79]a$/);
    } else {
      expect(text.artifact).toBeTruthy();
    }
  });
});

/** Probe once whether ffmpeg is spawnable, to gate the mp4 tests. */
async function ffmpegPresent(): Promise<boolean> {
  return new Promise((resolve) => {
    try {
      const p = spawn("ffmpeg", ["-version"], { stdio: "ignore" });
      p.on("error", () => resolve(false));
      p.on("exit", (code) => resolve(code === 0));
    } catch {
      resolve(false);
    }
  });
}

const hasFfmpeg = await ffmpegPresent();

describe("export_video mp4 (streaming ffmpeg pipe)", () => {
  it.skipIf(!hasFfmpeg)(
    "streams frames to ffmpeg and produces a playable mp4",
    async () => {
      const docId = openArm();
      await animate({ document_id: docId, timeline: spinTimeline });
      const res = await exportVideo(
        { document_id: docId, format: "mp4", width_px: 160 },
        { engine } as never,
      );
      if (res.isError)
        throw new Error(`export_video failed: ${res.content[0]!.text}`);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const blocks = res.content as Array<any>;
      const text = JSON.parse(blocks.find((b) => b.type === "text")!.text);
      expect(text.format).toBe("mp4");
      expect(text.frames).toBe(13);
      expect(text.bytes).toBeGreaterThan(0);
      // mp4 always offloads to an artifact.
      expect(text.artifact).toBeTruthy();
    },
    60_000,
  );

  it.skipIf(!hasFfmpeg)(
    "startMp4Encoder handles backpressure across many frames",
    async () => {
      const w = 320;
      const h = 240;
      const enc = startMp4Encoder(w, h, 24);
      const frame = new Uint8Array(w * h * 4).fill(200);
      for (let i = 0; i < 120; i++) await enc.writeFrame(frame);
      const bytes = await enc.finish();
      expect(bytes.byteLength).toBeGreaterThan(0);
      // ftyp box near the start of a fragmented mp4.
      expect(bytes.subarray(4, 8).toString("ascii")).toBe("ftyp");
    },
    30_000,
  );

  it.skipIf(!hasFfmpeg)(
    "surfaces ffmpeg early-exit as a write/finish error",
    async () => {
      // Invalid frame geometry makes ffmpeg reject its args and exit
      // immediately; the failure must surface instead of hanging.
      const enc = startMp4Encoder(0, 0, 24);
      await expect(
        (async () => {
          const frame = new Uint8Array(64 * 64 * 4);
          for (let i = 0; i < 100; i++) await enc.writeFrame(frame);
          return enc.finish();
        })(),
      ).rejects.toThrow(/ffmpeg/);
    },
    30_000,
  );
});

describe("compileRolloutTimeline", () => {
  const joints = armDocument().joints!;

  it("compiles a trajectory into linear joint tracks at the sim timestep", () => {
    const trajectory = Array.from({ length: 50 }, (_, s) => [s * 2]);
    const res = compileRolloutTimeline(
      { trajectory, jointIds: ["shoulder"], dt: 1 / 100, substeps: 2 },
      joints,
      { turntable: true },
    );
    if ("error" in res) throw new Error(res.error);
    const tl = res.timeline;
    expect(tl.durationS).toBeCloseTo(49 * 0.02, 9);
    expect(tl.tracks).toHaveLength(1);
    const keys = tl.tracks[0]!.keys;
    expect(keys[0]).toMatchObject({ t: 0, value: 0 });
    expect(keys[keys.length - 1]!.value).toBe(98);
    expect(keys[keys.length - 1]!.t).toBeCloseTo(49 * 0.02, 9);
    expect(tl.camera).toHaveLength(1);
    expect(tl.camera[0]!.kind.type).toBe("Turntable");
  });

  it("thins long trajectories to maxKeys and keeps the final sample", () => {
    const trajectory = Array.from({ length: 600 }, (_, s) => [s]);
    const res = compileRolloutTimeline(
      { trajectory, jointIds: ["shoulder"], dt: 1 / 240, substeps: 1 },
      joints,
      { maxKeys: 10 },
    );
    if ("error" in res) throw new Error(res.error);
    const keys = res.timeline.tracks[0]!.keys;
    expect(keys.length).toBeLessThanOrEqual(11);
    expect(keys[keys.length - 1]!.value).toBe(599);
  });

  it("rejects trajectories shorter than 2 steps", () => {
    const res = compileRolloutTimeline(
      { trajectory: [[0]], jointIds: ["shoulder"], dt: 0.01, substeps: 1 },
      joints,
    );
    expect("error" in res).toBe(true);
  });
});

describe("camera channels", () => {
  it("encodes elevation and dolly on the __camera carrier", async () => {
    const docId = openArm();
    const tl: Timeline = {
      durationS: 1,
      fps: 8,
      tracks: [
        {
          target: { type: "Joint", jointId: "shoulder" },
          keys: [
            { t: 0, value: 0, ease: "linear" },
            { t: 1, value: 10, ease: "linear" },
          ],
        },
      ],
      camera: [
        { startS: 0, endS: 0.5, kind: { type: "Orbit", from: [0, 10], to: [90, 60] } },
        { startS: 0.5, endS: 1, kind: { type: "Focus", target: "arm_inst", dolly: 0.5 } },
      ],
    } as unknown as Timeline;
    await animate({ document_id: docId, timeline: tl });
    const res = await renderSequence({ document_id: docId }, { engine } as never);
    const body = out(res);
    if (res.isError) throw new Error(body.error);
    const bytes = Uint8Array.from(Buffer.from(body.glb_base64, "base64"));
    const json = glbJson(bytes);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const camIdx = json.nodes.findIndex((n: any) => n.name === "__camera");
    expect(camIdx).toBeGreaterThanOrEqual(0);
    const paths = json.animations[0].channels
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .filter((c: any) => c.target.node === camIdx)
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .map((c: any) => c.target.path)
      .sort();
    // Orbit (yaw+pitch) → rotation; Focus dolly 1→0.5 → scale.
    expect(paths).toEqual(["rotation", "scale"]);
  });
});

describe("injectHud", () => {
  it("inserts the proof bar with clearance status", () => {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 300"><rect/></svg>`;
    const withHud = injectHud(svg, "t=0.50s  gap=1.00", {
      label: "air-gap",
      holds: true,
      observed_min_mm: 1.0,
    });
    expect(withHud).toContain("vcad-hud");
    expect(withHud).toContain("#22c55e");
    expect(withHud).toContain("air-gap 1.00mm ✓");
    const failing = injectHud(svg, "t=0", {
      label: "air-gap",
      holds: false,
      observed_min_mm: -0.2,
    });
    expect(failing).toContain("#ef4444");
    expect(failing).toContain("✗");
  });
});

/* ------------------------------------------------------------------ */
/* Kinematic keyframe playback                                         */
/* ------------------------------------------------------------------ */

/** Three-link machine: base → revolute arm → prismatic slider riding the
 *  arm. The acceptance fixture for kinematic playback: a mid-tree joint
 *  move (shoulder) must carry the slider with it. */
function machineDocument(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "base",
        op: { type: "Cube", size: { x: 80, y: 80, z: 10 } },
      },
      "2": {
        id: 2,
        name: "arm",
        op: { type: "Cube", size: { x: 60, y: 10, z: 10 } },
      },
      "3": {
        id: 3,
        name: "slider",
        op: { type: "Cube", size: { x: 15, y: 8, z: 8 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [],
    partDefs: {
      base: { id: "base", name: "Base", root: 1, defaultMaterial: null },
      arm: { id: "arm", name: "Arm", root: 2, defaultMaterial: null },
      slider: { id: "slider", name: "Slider", root: 3, defaultMaterial: null },
    },
    instances: [
      { id: "base_inst", partDefId: "base", name: "Base", transform: null, material: null },
      { id: "arm_inst", partDefId: "arm", name: "Arm", transform: null, material: null },
      { id: "slider_inst", partDefId: "slider", name: "Slider", transform: null, material: null },
    ],
    joints: [
      {
        id: "shoulder",
        name: "Shoulder",
        parentInstanceId: "base_inst",
        childInstanceId: "arm_inst",
        parentAnchor: { x: 40, y: 40, z: 10 },
        childAnchor: { x: 0, y: 5, z: 0 },
        kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 }, limits: null },
        state: 0,
      },
      {
        id: "slide",
        name: "Slide",
        parentInstanceId: "arm_inst",
        childInstanceId: "slider_inst",
        parentAnchor: { x: 10, y: 5, z: 10 },
        childAnchor: { x: 0, y: 4, z: 0 },
        kind: { type: "Slider", axis: { x: 1, y: 0, z: 0 }, limits: null },
        state: 0,
      },
    ],
    groundInstanceId: "base_inst",
  } as unknown as Document;
}

const machineKeyframes = [
  { t: 0, joints: { shoulder: 0, slide: 0 } },
  { t: 1, joints: { shoulder: 90, slide: 30 } },
];

describe("compileJointKeyframes", () => {
  it("compiles time-major keyframes into per-joint linear tracks", () => {
    const compiled = compileJointKeyframes(machineKeyframes);
    if ("error" in compiled) throw new Error(compiled.error);
    const tl = compiled.timeline;
    expect(tl.durationS).toBe(1);
    expect(tl.fps).toBe(24);
    expect(tl.tracks).toHaveLength(2);
    const shoulder = tl.tracks.find(
      (t) => (t.target as { jointId?: string }).jointId === "shoulder",
    )!;
    expect(shoulder.keys).toEqual([
      { t: 0, value: 0, ease: "linear" },
      { t: 1, value: 90, ease: "linear" },
    ]);
  });

  it("rejects malformed keyframes loudly", () => {
    expect(compileJointKeyframes([])).toHaveProperty("error");
    expect(
      compileJointKeyframes([
        { t: 1, joints: { a: 0 } },
        { t: 0.5, joints: { a: 1 } },
      ]),
    ).toHaveProperty("error");
    expect(
      compileJointKeyframes([
        { t: 0, joints: { a: NaN } },
        { t: 1, joints: { a: 1 } },
      ]),
    ).toHaveProperty("error");
    expect(
      compileJointKeyframes([
        { t: 0, joints: {} },
        { t: 1, joints: { a: 1 } },
      ]),
    ).toHaveProperty("error");
  });
});

describe("kinematic playback (keyframes)", () => {
  it("animate accepts keyframes and refuses timeline+keyframes together", async () => {
    const opened = out(openDocument({ initial: machineDocument() }));
    const docId = opened.document_id as string;
    const both = await animate({
      document_id: docId,
      timeline: spinTimeline,
      keyframes: machineKeyframes,
    });
    expect(both.isError).toBe(true);

    const res = await animate({
      document_id: docId,
      keyframes: machineKeyframes,
      fps: 12,
    });
    const body = out(res);
    if (res.isError) throw new Error(body.error);
    expect(body.duration_s).toBe(1);
    expect(body.fps).toBe(12);
    expect(body.tracks).toHaveLength(2);
  });

  it("rejects keyframes referencing unknown joints", async () => {
    const opened = out(openDocument({ initial: machineDocument() }));
    const docId = opened.document_id as string;
    const res = await animate({
      document_id: docId,
      keyframes: [
        { t: 0, joints: { elbow: 0 } },
        { t: 1, joints: { elbow: 45 } },
      ],
    });
    expect(res.isError).toBe(true);
    expect(out(res).error).toContain('unknown joint "elbow"');
  });

  it("a mid-tree joint move carries children: the slider rides the arm", async () => {
    const doc = machineDocument();
    const compiled = compileJointKeyframes(machineKeyframes, { fps: 4 });
    if ("error" in compiled) throw new Error(compiled.error);
    const frames = sampleSequence(doc, compiled.timeline);
    const first = frames[0]!;
    const last = frames[frames.length - 1]!;
    expect(last.joints.shoulder).toBeCloseTo(90);
    expect(last.joints.slide).toBeCloseTo(30);

    const world0 = solveForwardKinematics(poseDocument(doc, first));
    const world1 = solveForwardKinematics(poseDocument(doc, last));
    const s0 = world0.get("slider_inst")!.translation;
    const s1 = world1.get("slider_inst")!.translation;
    // With shoulder at 90° the whole slide axis has rotated: the slider's
    // world position must move in BOTH x and y, not just along its own axis.
    expect(Math.abs(s1.x - s0.x)).toBeGreaterThan(1);
    expect(Math.abs(s1.y - s0.y)).toBeGreaterThan(1);

    // And with the shoulder pinned, the slide joint alone must translate
    // the slider along the (rotated) arm — pure kinematics, no sag.
    const pinned = { ...last, joints: { shoulder: 90, slide: 0 } };
    const worldPinned = solveForwardKinematics(poseDocument(doc, pinned));
    const sp = worldPinned.get("slider_inst")!.translation;
    const slideDist = Math.hypot(s1.x - sp.x, s1.y - sp.y, s1.z - sp.z);
    expect(slideDist).toBeCloseTo(30, 1);
  });

  it("export_video renders keyframes to a GIF (arm sweep + slider travel)", async () => {
    const opened = out(openDocument({ initial: machineDocument() }));
    const docId = opened.document_id as string;
    const res = await exportVideo(
      {
        document_id: docId,
        keyframes: machineKeyframes,
        fps: 6,
        format: "gif",
        width_px: 240,
      },
      { engine } as never,
    );
    const content = res.content as Array<{
      type: string;
      text?: string;
      data?: string;
      mimeType?: string;
    }>;
    const text = content.find((c) => c.type === "text")!;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const summary: any = JSON.parse(text.text!);
    if (res.isError) throw new Error(summary.error);
    expect(summary.format).toBe("gif");
    expect(summary.frames).toBe(7); // 1s at 6fps inclusive
    const image = content.find((c) => c.type === "image");
    expect(image?.mimeType).toBe("image/gif");
    const gif = Buffer.from(image!.data!, "base64");
    expect(gif.subarray(0, 6).toString("latin1")).toBe("GIF89a");
  });

  it("export_video accepts render_view-style azimuth/elevation", async () => {
    const opened = out(openDocument({ initial: machineDocument() }));
    const docId = opened.document_id as string;
    const res = await exportVideo(
      {
        document_id: docId,
        keyframes: machineKeyframes,
        fps: 2,
        format: "gif",
        width_px: 160,
        azimuth: 45,
        elevation: 20,
      },
      { engine } as never,
    );
    const content = res.content as Array<{ type: string; text?: string }>;
    const text = content.find((c) => c.type === "text")!;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const summary: any = JSON.parse(text.text!);
    if (res.isError) throw new Error(summary.error);
    expect(summary.view).toBe("orbit(azimuth=45, elevation=20)");
  });
});
