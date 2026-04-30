/**
 * Preview PNG renderer.
 *
 * Produces a 384×288 base64 PNG of the evaluated scene using a simple
 * analytic painter: orthographic projection from a 3/4 isometric angle,
 * z-buffer + Lambert shading, neutral grey ground gradient.
 *
 * This is the "preview" tier that the universal envelope attaches to
 * geometry-producing tools (build, edit, etc.). For photoreal output the
 * agent calls the dedicated `render` tool.
 *
 * Pure JS, no dependencies beyond Node's zlib (for PNG IDAT compression).
 */

import { deflateSync, crc32 } from "node:zlib";
import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";

const W = 384;
const H = 288;

interface RGB {
  r: number;
  g: number;
  b: number;
}

const BG_TOP: RGB = { r: 0xf2, g: 0xf3, b: 0xf6 };
const BG_BOT: RGB = { r: 0xc4, g: 0xca, b: 0xd2 };
const DEFAULT_ALBEDO: RGB = { r: 0xc0, g: 0xc4, b: 0xcc };

/** Render an IR document to a base64-encoded PNG. Returns null on failure. */
export function renderPreviewPng(doc: Document, engine: Engine): string | null {
  let scene;
  try {
    scene = engine.evaluate(doc);
  } catch {
    return null;
  }
  if (!scene || scene.parts.length === 0) return null;

  // Aggregate bbox so the camera frames the whole scene.
  let minX = Infinity,
    minY = Infinity,
    minZ = Infinity;
  let maxX = -Infinity,
    maxY = -Infinity,
    maxZ = -Infinity;
  let triCount = 0;
  for (const part of scene.parts) {
    const m = part.mesh;
    triCount += m.indices.length / 3;
    for (let i = 0; i < m.positions.length; i += 3) {
      const x = m.positions[i],
        y = m.positions[i + 1],
        z = m.positions[i + 2];
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
  // Camera basis (3/4 iso, looking from +X +Y -Z).
  const dir = norm({ x: 1, y: 1, z: 0.7 });
  const up = norm({ x: 0, y: 0, z: 1 });
  const right = norm(cross(dir, up));
  const camUp = norm(cross(right, dir));
  const lightDir = norm({ x: 0.4, y: 0.5, z: 0.8 });

  // Orthographic scale: fit `2*radius` into the smaller axis with margin.
  const margin = 0.85;
  const ortho = (Math.min(W, H) * margin) / (2 * radius);

  // Pixel + depth buffers.
  const rgba = new Uint8Array(W * H * 4);
  const zbuf = new Float32Array(W * H);
  for (let i = 0; i < zbuf.length; i++) zbuf[i] = -Infinity;

  // Background gradient.
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

  // Project + rasterise each triangle.
  for (const part of scene.parts) {
    const m = part.mesh;
    const albedo = albedoFor(part.material, doc);
    const idx = m.indices;
    const pos = m.positions;
    for (let t = 0; t < idx.length; t += 3) {
      const a = idx[t] * 3,
        b = idx[t + 1] * 3,
        c = idx[t + 2] * 3;
      const va = { x: pos[a], y: pos[a + 1], z: pos[a + 2] };
      const vb = { x: pos[b], y: pos[b + 1], z: pos[b + 2] };
      const vc = { x: pos[c], y: pos[c + 1], z: pos[c + 2] };
      const eAB = sub(vb, va);
      const eAC = sub(vc, va);
      const n = norm(cross(eAB, eAC));

      // Lambert + ambient.
      const lambert = Math.max(0, dot(n, lightDir));
      const shade = 0.25 + 0.75 * lambert;
      const r = clampByte(albedo.r * shade);
      const g = clampByte(albedo.g * shade);
      const bb = clampByte(albedo.b * shade);

      const pa = project(va, cx, cy, cz, right, camUp, dir, ortho);
      const pb = project(vb, cx, cy, cz, right, camUp, dir, ortho);
      const pc = project(vc, cx, cy, cz, right, camUp, dir, ortho);
      rasterTri(rgba, zbuf, pa, pb, pc, r, g, bb);
    }
  }

  // Subtle ground shadow pass — drop a darker ellipse on the back-bottom.
  // Skipped for now; not worth the loop cost on every preview.

  return base64(encodePng(rgba));
}

interface V3 {
  x: number;
  y: number;
  z: number;
}
interface P2 {
  x: number;
  y: number;
  z: number;
}

function project(
  p: V3,
  cx: number,
  cy: number,
  cz: number,
  right: V3,
  up: V3,
  dir: V3,
  scale: number,
): P2 {
  const dx = p.x - cx;
  const dy = p.y - cy;
  const dz = p.z - cz;
  const u = dx * right.x + dy * right.y + dz * right.z;
  const v = dx * up.x + dy * up.y + dz * up.z;
  const w = dx * dir.x + dy * dir.y + dz * dir.z;
  return {
    x: W / 2 + u * scale,
    y: H / 2 - v * scale,
    z: w,
  };
}

function rasterTri(
  rgba: Uint8Array,
  zbuf: Float32Array,
  a: P2,
  b: P2,
  c: P2,
  r: number,
  g: number,
  bb: number,
): void {
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

// ── Minimal PNG encoder ─────────────────────────────────────────────
// Just enough to emit IHDR + IDAT (RGBA, 8-bit, no filtering) + IEND.

function encodePng(rgba: Uint8Array): Uint8Array {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(W, 0);
  ihdr.writeUInt32BE(H, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace

  // Filter byte 0 per scanline.
  const raw = Buffer.alloc((W * 4 + 1) * H);
  for (let y = 0; y < H; y++) {
    raw[y * (W * 4 + 1)] = 0;
    raw.set(rgba.subarray(y * W * 4, (y + 1) * W * 4), y * (W * 4 + 1) + 1);
  }
  const idat = deflateSync(raw);

  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const out: Buffer[] = [sig, chunk("IHDR", ihdr), chunk("IDAT", idat), chunk("IEND", Buffer.alloc(0))];
  return Buffer.concat(out);
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
