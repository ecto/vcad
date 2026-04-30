/**
 * render (v2) — photoreal-ish PNG of a doc handle.
 *
 * Phase-1 scope: the kernel raytracer's WASM bindings aren't reachable
 * from Node yet (Phase 5a), so this tool ships an upgraded version of
 * the preview painter at agent-selectable resolution. The contract is
 * forward-compatible with the eventual raytraced backend — the agent
 * just sees the image quality improve when Phase 5a lands.
 *
 * Quality tiers:
 *   - draft   : 256×192, single-pass Lambert
 *   - preview : 512×384, single-pass Lambert (default)
 *   - high    : not yet — requires the raytracer
 *   - max     : not yet
 */

import { deflateSync, crc32 } from "node:zlib";
import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef, ResourceRef } from "../types.js";

export const renderSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle." },
    quality: { type: "string" as const, enum: ["draft", "preview", "high", "max"] },
    resolution: {
      type: "object" as const,
      properties: {
        width: { type: "integer" as const },
        height: { type: "integer" as const },
      },
    },
    format: { type: "string" as const, enum: ["png"] },
    camera: { type: "object" as const, description: "Optional { eye, look_at, up }." },
  },
  required: ["doc"],
};

interface RenderInput {
  doc: DocRef;
  quality?: "draft" | "preview" | "high" | "max";
  resolution?: { width: number; height: number };
  format?: "png";
  camera?: { eye?: { x: number; y: number; z: number }; look_at?: { x: number; y: number; z: number } };
}

const TIERS = {
  draft: { w: 256, h: 192, samples: 1, max_bounces: 1 },
  preview: { w: 512, h: 384, samples: 1, max_bounces: 1 },
  high: { w: 1024, h: 768, samples: 64, max_bounces: 4 },
  max: { w: 1920, h: 1080, samples: 256, max_bounces: 8 },
} as const;

export function render(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as RenderInput;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");

  const quality = args.quality ?? "preview";
  if (quality === "high" || quality === "max") {
    return fail(
      "raytracer_unavailable",
      "high/max quality requires the kernel raytracer (Phase 5a). Use 'preview' or 'draft' until then.",
    );
  }
  const tier = TIERS[quality];
  const W = args.resolution?.width ?? tier.w;
  const H = args.resolution?.height ?? tier.h;

  const { doc, handle } = resolveRef(args.doc);

  const png = paint(doc, engine, W, H);
  if (!png) return fail("render_failed", "Document has no parts to render.");

  const resource: ResourceRef = {
    kind: "embedded",
    mime: "image/png",
    data_base64: png,
  };

  return ok({
    result: {
      image: resource,
      width: W,
      height: H,
      quality,
      samples_per_pixel: tier.samples,
      cached: false,
    },
    handle,
    doc,
    engine,
    startedAt,
    previewPng: png,
  });
}

// ── Painter (extension of the preview-image renderer) ───────────────

interface RGB { r: number; g: number; b: number }
const BG_TOP: RGB = { r: 0xf2, g: 0xf3, b: 0xf6 };
const BG_BOT: RGB = { r: 0xc4, g: 0xca, b: 0xd2 };
const DEFAULT_ALBEDO: RGB = { r: 0xc0, g: 0xc4, b: 0xcc };

function paint(doc: Document, engine: Engine, W: number, H: number): string | null {
  let scene;
  try {
    scene = engine.evaluate(doc);
  } catch {
    return null;
  }
  if (!scene || scene.parts.length === 0) return null;

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  let triCount = 0;
  for (const part of scene.parts) {
    triCount += part.mesh.indices.length / 3;
    for (let i = 0; i < part.mesh.positions.length; i += 3) {
      const x = part.mesh.positions[i],
        y = part.mesh.positions[i + 1],
        z = part.mesh.positions[i + 2];
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (z < minZ) minZ = z;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
      if (z > maxZ) maxZ = z;
    }
  }
  if (!isFinite(minX) || triCount === 0) return null;

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;
  const radius = Math.max(maxX - minX, maxY - minY, maxZ - minZ) * 0.6 + 1e-6;

  const dir = norm({ x: 1, y: 1, z: 0.7 });
  const up = norm({ x: 0, y: 0, z: 1 });
  const right = norm(cross(dir, up));
  const camUp = norm(cross(right, dir));
  const lightDir = norm({ x: 0.4, y: 0.5, z: 0.8 });

  const margin = 0.85;
  const ortho = (Math.min(W, H) * margin) / (2 * radius);

  const rgba = new Uint8Array(W * H * 4);
  const zbuf = new Float32Array(W * H);
  for (let i = 0; i < zbuf.length; i++) zbuf[i] = -Infinity;

  for (let y = 0; y < H; y++) {
    const t = y / (H - 1);
    const r = Math.round(BG_TOP.r * (1 - t) + BG_BOT.r * t);
    const g = Math.round(BG_TOP.g * (1 - t) + BG_BOT.g * t);
    const b = Math.round(BG_TOP.b * (1 - t) + BG_BOT.b * t);
    for (let x = 0; x < W; x++) {
      const off = (y * W + x) * 4;
      rgba[off] = r;
      rgba[off + 1] = g;
      rgba[off + 2] = b;
      rgba[off + 3] = 0xff;
    }
  }

  for (const part of scene.parts) {
    const albedo = albedoFor(part.material, doc);
    const idx = part.mesh.indices;
    const pos = part.mesh.positions;
    for (let t = 0; t < idx.length; t += 3) {
      const a = idx[t] * 3, b = idx[t + 1] * 3, c = idx[t + 2] * 3;
      const va = { x: pos[a], y: pos[a + 1], z: pos[a + 2] };
      const vb = { x: pos[b], y: pos[b + 1], z: pos[b + 2] };
      const vc = { x: pos[c], y: pos[c + 1], z: pos[c + 2] };
      const n = norm(cross(sub(vb, va), sub(vc, va)));
      const lambert = Math.max(0, dot(n, lightDir));
      const shade = 0.25 + 0.75 * lambert;
      const r = clampByte(albedo.r * shade);
      const g = clampByte(albedo.g * shade);
      const bb = clampByte(albedo.b * shade);
      const pa = project(va, cx, cy, cz, right, camUp, dir, ortho, W, H);
      const pb = project(vb, cx, cy, cz, right, camUp, dir, ortho, W, H);
      const pc = project(vc, cx, cy, cz, right, camUp, dir, ortho, W, H);
      rasterTri(rgba, zbuf, W, H, pa, pb, pc, r, g, bb);
    }
  }

  return base64(encodePng(rgba, W, H));
}

interface V3 { x: number; y: number; z: number }
interface P2 { x: number; y: number; z: number }

function project(p: V3, cx: number, cy: number, cz: number, right: V3, up: V3, dir: V3, scale: number, W: number, H: number): P2 {
  const dx = p.x - cx, dy = p.y - cy, dz = p.z - cz;
  return {
    x: W / 2 + (dx * right.x + dy * right.y + dz * right.z) * scale,
    y: H / 2 - (dx * up.x + dy * up.y + dz * up.z) * scale,
    z: dx * dir.x + dy * dir.y + dz * dir.z,
  };
}

function rasterTri(rgba: Uint8Array, zbuf: Float32Array, W: number, H: number, a: P2, b: P2, c: P2, r: number, g: number, bb: number): void {
  const minX = Math.max(0, Math.floor(Math.min(a.x, b.x, c.x)));
  const maxX = Math.min(W - 1, Math.ceil(Math.max(a.x, b.x, c.x)));
  const minY = Math.max(0, Math.floor(Math.min(a.y, b.y, c.y)));
  const maxY = Math.min(H - 1, Math.ceil(Math.max(a.y, b.y, c.y)));
  if (minX >= maxX || minY >= maxY) return;
  const denom = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
  if (Math.abs(denom) < 1e-9) return;
  const invDenom = 1 / denom;
  for (let y = minY; y <= maxY; y++) {
    for (let x = minX; x <= maxX; x++) {
      const w0 = ((b.y - c.y) * (x - c.x) + (c.x - b.x) * (y - c.y)) * invDenom;
      const w1 = ((c.y - a.y) * (x - c.x) + (a.x - c.x) * (y - c.y)) * invDenom;
      const w2 = 1 - w0 - w1;
      if (w0 < 0 || w1 < 0 || w2 < 0) continue;
      const z = w0 * a.z + w1 * b.z + w2 * c.z;
      const idx = y * W + x;
      if (z <= zbuf[idx]) continue;
      zbuf[idx] = z;
      const off = idx * 4;
      rgba[off] = r;
      rgba[off + 1] = g;
      rgba[off + 2] = bb;
      rgba[off + 3] = 0xff;
    }
  }
}

function albedoFor(materialKey: string | undefined, doc: Document): RGB {
  if (!materialKey) return DEFAULT_ALBEDO;
  const m = doc.materials?.[materialKey];
  if (!m) return DEFAULT_ALBEDO;
  return {
    r: clampByte(m.color[0] * 255),
    g: clampByte(m.color[1] * 255),
    b: clampByte(m.color[2] * 255),
  };
}

const sub = (a: V3, b: V3): V3 => ({ x: a.x - b.x, y: a.y - b.y, z: a.z - b.z });
const cross = (a: V3, b: V3): V3 => ({
  x: a.y * b.z - a.z * b.y,
  y: a.z * b.x - a.x * b.z,
  z: a.x * b.y - a.y * b.x,
});
const dot = (a: V3, b: V3) => a.x * b.x + a.y * b.y + a.z * b.z;
const norm = (v: V3): V3 => {
  const l = Math.hypot(v.x, v.y, v.z) || 1;
  return { x: v.x / l, y: v.y / l, z: v.z / l };
};
const clampByte = (n: number) => (n < 0 ? 0 : n > 255 ? 255 : Math.round(n));

function encodePng(rgba: Uint8Array, W: number, H: number): Uint8Array {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(W, 0);
  ihdr.writeUInt32BE(H, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  const raw = Buffer.alloc((W * 4 + 1) * H);
  for (let y = 0; y < H; y++) {
    raw[y * (W * 4 + 1)] = 0;
    raw.set(rgba.subarray(y * W * 4, (y + 1) * W * 4), y * (W * 4 + 1) + 1);
  }
  const idat = deflateSync(raw);
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  return Buffer.concat([sig, chunk("IHDR", ihdr), chunk("IDAT", idat), chunk("IEND", Buffer.alloc(0))]);
}

function chunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])) >>> 0, 0);
  return Buffer.concat([len, t, data, crc]);
}

function base64(b: Uint8Array): string {
  return Buffer.from(b).toString("base64");
}
