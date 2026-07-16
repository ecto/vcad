/**
 * Gear-train proof video — the animation-tools demo.
 *
 * A 3:1 spur pair (sun 30mm driving a 10mm pinion) on a base plate, with a
 * named air-gap clearance spec between the gear rims. The timeline spins the
 * drive gear one revolution (the pinion counter-rotates 3x, driven by its own
 * track at the ratio), sweeps a 360° turntable, and both render tools verify
 * the air gap across every sampled frame — the receipt under the film.
 *
 * Run from the repo root after building workspaces:
 *   node examples/animation/gear-train.mjs
 *
 * Outputs into this directory: gear-train.glb (animated, plays in any glTF
 * viewer) and gear-train.gif (kernel-rendered frames with the proof HUD).
 */
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Engine } from "@vcad/engine";
import { openDocument } from "../../packages/mcp/dist/tools/session.js";
import {
  animate,
  renderSequence,
  exportVideo,
} from "../../packages/mcp/dist/tools/animate.js";

const here = dirname(fileURLToPath(import.meta.url));
const out = (r) => JSON.parse(r.content.find((c) => c.type === "text").text);

// ---------------------------------------------------------------- document
const doc = {
  version: "0.1",
  nodes: {
    1: { id: 1, name: "plate", op: { type: "Cube", size: { x: 90, y: 50, z: 6 } } },
    2: { id: 2, name: "drive-gear", op: { type: "Cylinder", radius: 30, height: 8, segments: 96 } },
    3: { id: 3, name: "pinion", op: { type: "Cylinder", radius: 10, height: 8, segments: 64 } },
    4: { id: 4, name: "drive-boss", op: { type: "Cylinder", radius: 4, height: 14, segments: 32 } },
    5: { id: 5, name: "drive", op: { type: "Union", left: 2, right: 4 } },
  },
  materials: {},
  part_materials: {},
  roots: [],
  partDefs: {
    plate: { id: "plate", name: "Plate", root: 1, defaultMaterial: null },
    drive: { id: "drive", name: "Drive Gear", root: 5, defaultMaterial: null },
    pinion: { id: "pinion", name: "Pinion", root: 3, defaultMaterial: null },
  },
  instances: [
    { id: "plate_i", partDefId: "plate", name: "Plate", transform: null, material: null },
    { id: "drive_i", partDefId: "drive", name: "Drive Gear", transform: null, material: null },
    { id: "pinion_i", partDefId: "pinion", name: "Pinion", transform: null, material: null },
  ],
  joints: [
    {
      id: "drive_axis", name: "Drive Axis",
      parentInstanceId: "plate_i", childInstanceId: "drive_i",
      parentAnchor: { x: 30, y: 25, z: 6 }, childAnchor: { x: 0, y: 0, z: 0 },
      kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 }, limits: null }, state: 0,
    },
    {
      id: "pinion_axis", name: "Pinion Axis",
      parentInstanceId: "plate_i", childInstanceId: "pinion_i",
      // Center distance 40.5mm → 0.5mm design air gap between the rims.
      parentAnchor: { x: 70.5, y: 25, z: 6 }, childAnchor: { x: 0, y: 0, z: 0 },
      kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 }, limits: null }, state: 0,
    },
  ],
  groundInstanceId: "plate_i",
  clearance_specs: [
    { label: "gear-air-gap", group_a: ["drive_i"], group_b: ["pinion_i"], min_mm: 0.25 },
  ],
};

// ---------------------------------------------------------------- timeline
const timeline = {
  durationS: 3,
  fps: 16,
  tracks: [
    { target: { type: "Joint", jointId: "drive_axis" },
      keys: [{ t: 0, value: 0 }, { t: 3, value: 360, ease: "ease-in-out" }] },
    // 3:1 ratio, counter-rotating — the kinematic claim, keyframed.
    { target: { type: "Joint", jointId: "pinion_axis" },
      keys: [{ t: 0, value: 0 }, { t: 3, value: -1080, ease: "ease-in-out" }] },
  ],
  camera: [
    { startS: 0, endS: 3, kind: { type: "Turntable", degrees: 360, elevationDeg: 30 } },
  ],
};

// ---------------------------------------------------------------- run
await Engine.init().then(async (engine) => {
  const { document_id } = out(openDocument({ initial: doc }));
  console.log("document:", document_id);

  const set = out(await animate({ document_id, timeline }));
  console.log("timeline:", set.frames, "frames,", set.camera_shots, "camera shot(s)");

  const seq = await renderSequence({ document_id }, { engine });
  const seqBody = out(seq);
  if (seq.isError) throw new Error(seqBody.error);
  const glb = Buffer.from(seqBody.glb_base64, "base64");
  writeFileSync(join(here, "gear-train.glb"), glb);
  console.log(
    `animated GLB: ${glb.length} bytes, ${seqBody.channels} channels, ` +
    `verification: ${JSON.stringify(seqBody.verification?.specs)}`,
  );

  const vid = await exportVideo({ document_id, format: "gif", width_px: 480 }, { engine });
  const vidBody = out(vid);
  if (vid.isError) throw new Error(vidBody.error);
  const img = vid.content.find((c) => c.type === "image");
  if (img) writeFileSync(join(here, "gear-train.gif"), Buffer.from(img.data, "base64"));
  console.log(
    `video: ${vidBody.format}, ${vidBody.frames} frames @ ${vidBody.fps}fps, ` +
    `${vidBody.width}x${vidBody.height}, ${vidBody.bytes} bytes`,
  );
  console.log("receipt:", JSON.stringify(vidBody.verification, null, 2));
});
