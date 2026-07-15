/**
 * End-to-end coverage for the animation tools: `animate` (timeline
 * authoring + validation), `render_sequence` (animated GLB with glTF
 * channels + clearance sweep), `export_video` (GIF frames with the
 * verification HUD burned in).
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Timeline } from "@vcad/ir";
import {
  animate,
  renderSequence,
  exportVideo,
  validateTimeline,
  verifySequenceClearance,
  injectHud,
} from "../tools/animate.js";
import { documents, openDocument } from "../tools/session.js";
import { sampleSequence } from "@vcad/engine";

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
      // Scene root wraps the instances for the turntable yaw.
      const rootIdx = json.scenes[0].nodes[0];
      expect(json.nodes[rootIdx].name).toBe("__scene");
      expect(json.nodes[rootIdx].children).toHaveLength(2);
      const rootChannels = anim.channels.filter(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (c: any) => c.target.node === rootIdx,
      );
      expect(rootChannels).toHaveLength(1);
      expect(rootChannels[0].target.path).toBe("rotation");
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
