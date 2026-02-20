/**
 * Simple software 3D renderer for terminal display.
 * Renders triangle meshes with flat shading to a pixel buffer.
 */

import type { Vec3 } from "@vcad/ir";
import { vec3Sub, vec3Add, vec3Scale, vec3Cross, vec3Dot, vec3Normalize, vec3Length } from "@vcad/ir";

export type { Vec3 };

export interface Triangle {
  v0: Vec3;
  v1: Vec3;
  v2: Vec3;
  color: [number, number, number];
}

export interface Camera {
  position: Vec3;
  target: Vec3;
  up: Vec3;
  fov: number;
}

export interface RenderBuffer {
  width: number;
  height: number;
  pixels: Uint8Array; // RGBA
  depth: Float32Array;
}

export function createBuffer(width: number, height: number): RenderBuffer {
  return {
    width,
    height,
    pixels: new Uint8Array(width * height * 4),
    depth: new Float32Array(width * height).fill(Infinity),
  };
}

export function clearBuffer(buffer: RenderBuffer, r: number, g: number, b: number) {
  const { width, height, pixels, depth } = buffer;
  for (let i = 0; i < width * height; i++) {
    pixels[i * 4] = r;
    pixels[i * 4 + 1] = g;
    pixels[i * 4 + 2] = b;
    pixels[i * 4 + 3] = 255;
    depth[i] = Infinity;
  }
}

function lookAt(eye: Vec3, target: Vec3, up: Vec3): number[] {
  const z = vec3Normalize(vec3Sub(eye, target));
  const x = vec3Normalize(vec3Cross(up, z));
  const y = vec3Cross(z, x);

  return [
    x.x, y.x, z.x, 0,
    x.y, y.y, z.y, 0,
    x.z, y.z, z.z, 0,
    -vec3Dot(x, eye), -vec3Dot(y, eye), -vec3Dot(z, eye), 1,
  ];
}

function perspective(fov: number, aspect: number, near: number, far: number): number[] {
  const f = 1 / Math.tan(fov / 2);
  const nf = 1 / (near - far);

  return [
    f / aspect, 0, 0, 0,
    0, f, 0, 0,
    0, 0, (far + near) * nf, -1,
    0, 0, 2 * far * near * nf, 0,
  ];
}

function mat4Multiply(a: number[], b: number[]): number[] {
  const result = new Array(16).fill(0);
  for (let i = 0; i < 4; i++) {
    for (let j = 0; j < 4; j++) {
      for (let k = 0; k < 4; k++) {
        result[i * 4 + j] += a[i * 4 + k]! * b[k * 4 + j]!;
      }
    }
  }
  return result;
}

function transformPoint(m: number[], p: Vec3): { x: number; y: number; z: number; w: number } {
  const w = m[3]! * p.x + m[7]! * p.y + m[11]! * p.z + m[15]!;
  return {
    x: (m[0]! * p.x + m[4]! * p.y + m[8]! * p.z + m[12]!) / w,
    y: (m[1]! * p.x + m[5]! * p.y + m[9]! * p.z + m[13]!) / w,
    z: (m[2]! * p.x + m[6]! * p.y + m[10]! * p.z + m[14]!) / w,
    w,
  };
}

function edgeFunction(a: { x: number; y: number }, b: { x: number; y: number }, c: { x: number; y: number }): number {
  return (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x);
}

export function renderTriangles(
  buffer: RenderBuffer,
  triangles: Triangle[],
  camera: Camera,
): void {
  const { width, height, pixels, depth } = buffer;
  const aspect = width / height;

  const view = lookAt(camera.position, camera.target, camera.up);
  const proj = perspective(camera.fov * Math.PI / 180, aspect, 0.1, 1000);
  const mvp = mat4Multiply(proj, view);

  // Debug: draw a border and crosshair to verify rendering works
  for (let x = 0; x < width; x++) {
    const topIdx = x * 4;
    const botIdx = ((height - 1) * width + x) * 4;
    pixels[topIdx] = 100; pixels[topIdx + 1] = 50; pixels[topIdx + 2] = 50;
    pixels[botIdx] = 100; pixels[botIdx + 1] = 50; pixels[botIdx + 2] = 50;
  }
  for (let y = 0; y < height; y++) {
    const leftIdx = (y * width) * 4;
    const rightIdx = (y * width + width - 1) * 4;
    pixels[leftIdx] = 100; pixels[leftIdx + 1] = 50; pixels[leftIdx + 2] = 50;
    pixels[rightIdx] = 100; pixels[rightIdx + 1] = 50; pixels[rightIdx + 2] = 50;
  }
  // Center crosshair
  const cx = Math.floor(width / 2);
  const cy = Math.floor(height / 2);
  for (let i = -5; i <= 5; i++) {
    if (cx + i >= 0 && cx + i < width) {
      const idx = (cy * width + cx + i) * 4;
      pixels[idx] = 255; pixels[idx + 1] = 255; pixels[idx + 2] = 0;
    }
    if (cy + i >= 0 && cy + i < height) {
      const idx = ((cy + i) * width + cx) * 4;
      pixels[idx] = 255; pixels[idx + 1] = 255; pixels[idx + 2] = 0;
    }
  }

  // Light direction (from top-right-front)
  const lightDir = vec3Normalize({ x: 0.5, y: 0.8, z: 0.3 });

  let renderedCount = 0;
  for (const tri of triangles) {
    // Transform vertices
    const p0 = transformPoint(mvp, tri.v0);
    const p1 = transformPoint(mvp, tri.v1);
    const p2 = transformPoint(mvp, tri.v2);

    // Clip triangles behind camera
    if (p0.w < 0.1 || p1.w < 0.1 || p2.w < 0.1) continue;

    // Convert to screen coordinates
    const s0 = { x: (p0.x + 1) * 0.5 * width, y: (1 - p0.y) * 0.5 * height, z: p0.z };
    const s1 = { x: (p1.x + 1) * 0.5 * width, y: (1 - p1.y) * 0.5 * height, z: p1.z };
    const s2 = { x: (p2.x + 1) * 0.5 * width, y: (1 - p2.y) * 0.5 * height, z: p2.z };

    // Compute face normal for lighting
    const edge1 = vec3Sub(tri.v1, tri.v0);
    const edge2 = vec3Sub(tri.v2, tri.v0);
    const normal = vec3Normalize(vec3Cross(edge1, edge2));

    // Compute screen area for winding check
    // Note: Y is flipped in screen space, so CCW triangles become CW (negative area)
    const screenArea = edgeFunction(s0, s1, s2);
    // Skip degenerate triangles, but allow both windings for now
    if (Math.abs(screenArea) < 0.001) continue;

    // Lighting - use absolute dot product for two-sided lighting
    const viewDir = vec3Normalize(vec3Sub(camera.position, tri.v0));
    const ndotv = vec3Dot(normal, viewDir);
    const shadingNormal = ndotv < 0 ? vec3Scale(normal, -1) : normal;
    const ndotl = Math.max(0, vec3Dot(shadingNormal, lightDir));
    const ambient = 0.3;
    const diffuse = 0.7;
    const intensity = ambient + diffuse * ndotl;

    const litColor: [number, number, number] = [
      Math.min(255, Math.floor(tri.color[0] * intensity)),
      Math.min(255, Math.floor(tri.color[1] * intensity)),
      Math.min(255, Math.floor(tri.color[2] * intensity)),
    ];

    // Bounding box
    const minX = Math.max(0, Math.floor(Math.min(s0.x, s1.x, s2.x)));
    const maxX = Math.min(width - 1, Math.ceil(Math.max(s0.x, s1.x, s2.x)));
    const minY = Math.max(0, Math.floor(Math.min(s0.y, s1.y, s2.y)));
    const maxY = Math.min(height - 1, Math.ceil(Math.max(s0.y, s1.y, s2.y)));

    // Rasterize
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const p = { x: x + 0.5, y: y + 0.5 };

        const w0 = edgeFunction(s1, s2, p);
        const w1 = edgeFunction(s2, s0, p);
        const w2 = edgeFunction(s0, s1, p);

        // Check if point is inside triangle (handle both windings)
        const inside = (w0 >= 0 && w1 >= 0 && w2 >= 0) || (w0 <= 0 && w1 <= 0 && w2 <= 0);
        if (inside) {
          // Interpolate depth
          const z = (w0 * s0.z + w1 * s1.z + w2 * s2.z) / screenArea;

          const idx = y * width + x;
          if (z < depth[idx]!) {
            depth[idx] = z;
            pixels[idx * 4] = litColor[0];
            pixels[idx * 4 + 1] = litColor[1];
            pixels[idx * 4 + 2] = litColor[2];
            pixels[idx * 4 + 3] = 255;
            renderedCount++;
          }
        }
      }
    }
  }

  // Debug: mark triangle centers with bright dots
  for (const tri of triangles) {
    const p0 = transformPoint(mvp, tri.v0);
    const p1 = transformPoint(mvp, tri.v1);
    const p2 = transformPoint(mvp, tri.v2);
    if (p0.w < 0.1 || p1.w < 0.1 || p2.w < 0.1) continue;

    // Center of triangle in screen space
    const cx = Math.floor(((p0.x + p1.x + p2.x) / 3 + 1) * 0.5 * width);
    const cy = Math.floor((1 - (p0.y + p1.y + p2.y) / 3) * 0.5 * height);

    // Draw a green dot at center
    if (cx >= 0 && cx < width && cy >= 0 && cy < height) {
      const idx = (cy * width + cx) * 4;
      pixels[idx] = 0; pixels[idx + 1] = 255; pixels[idx + 2] = 0;
    }
  }
}

// Also add wireframe rendering for edges
export function renderWireframe(
  buffer: RenderBuffer,
  triangles: Triangle[],
  camera: Camera,
  lineColor: [number, number, number] = [0, 200, 255],
): void {
  const { width, height, pixels } = buffer;
  const aspect = width / height;

  const view = lookAt(camera.position, camera.target, camera.up);
  const proj = perspective(camera.fov * Math.PI / 180, aspect, 0.1, 1000);
  const mvp = mat4Multiply(proj, view);

  function drawLine(x0: number, y0: number, x1: number, y1: number) {
    const dx = Math.abs(x1 - x0);
    const dy = Math.abs(y1 - y0);
    const sx = x0 < x1 ? 1 : -1;
    const sy = y0 < y1 ? 1 : -1;
    let err = dx - dy;

    let x = Math.floor(x0);
    let y = Math.floor(y0);
    const endX = Math.floor(x1);
    const endY = Math.floor(y1);

    while (true) {
      if (x >= 0 && x < width && y >= 0 && y < height) {
        const idx = (y * width + x) * 4;
        pixels[idx] = lineColor[0];
        pixels[idx + 1] = lineColor[1];
        pixels[idx + 2] = lineColor[2];
        pixels[idx + 3] = 255;
      }

      if (x === endX && y === endY) break;

      const e2 = 2 * err;
      if (e2 > -dy) {
        err -= dy;
        x += sx;
      }
      if (e2 < dx) {
        err += dx;
        y += sy;
      }
    }
  }

  for (const tri of triangles) {
    const p0 = transformPoint(mvp, tri.v0);
    const p1 = transformPoint(mvp, tri.v1);
    const p2 = transformPoint(mvp, tri.v2);

    if (p0.w < 0.1 || p1.w < 0.1 || p2.w < 0.1) continue;

    const s0 = { x: (p0.x + 1) * 0.5 * width, y: (1 - p0.y) * 0.5 * height };
    const s1 = { x: (p1.x + 1) * 0.5 * width, y: (1 - p1.y) * 0.5 * height };
    const s2 = { x: (p2.x + 1) * 0.5 * width, y: (1 - p2.y) * 0.5 * height };

    drawLine(s0.x, s0.y, s1.x, s1.y);
    drawLine(s1.x, s1.y, s2.x, s2.y);
    drawLine(s2.x, s2.y, s0.x, s0.y);
  }
}

/** Convert mesh data from engine to triangles */
export function meshToTriangles(
  positions: Float32Array,
  indices: Uint32Array,
  color: [number, number, number] = [180, 180, 190],
): Triangle[] {
  const triangles: Triangle[] = [];

  for (let i = 0; i < indices.length; i += 3) {
    const i0 = indices[i]!;
    const i1 = indices[i + 1]!;
    const i2 = indices[i + 2]!;

    triangles.push({
      v0: { x: positions[i0 * 3]!, y: positions[i0 * 3 + 1]!, z: positions[i0 * 3 + 2]! },
      v1: { x: positions[i1 * 3]!, y: positions[i1 * 3 + 1]!, z: positions[i1 * 3 + 2]! },
      v2: { x: positions[i2 * 3]!, y: positions[i2 * 3 + 1]!, z: positions[i2 * 3 + 2]! },
      color,
    });
  }

  return triangles;
}

/** Compute bounding box of triangles */
export function computeBounds(triangles: Triangle[]): { min: Vec3; max: Vec3; center: Vec3; size: number } {
  if (triangles.length === 0) {
    return {
      min: { x: -10, y: -10, z: -10 },
      max: { x: 10, y: 10, z: 10 },
      center: { x: 0, y: 0, z: 0 },
      size: 20,
    };
  }

  const min = { x: Infinity, y: Infinity, z: Infinity };
  const max = { x: -Infinity, y: -Infinity, z: -Infinity };

  for (const tri of triangles) {
    for (const v of [tri.v0, tri.v1, tri.v2]) {
      min.x = Math.min(min.x, v.x);
      min.y = Math.min(min.y, v.y);
      min.z = Math.min(min.z, v.z);
      max.x = Math.max(max.x, v.x);
      max.y = Math.max(max.y, v.y);
      max.z = Math.max(max.z, v.z);
    }
  }

  const center = {
    x: (min.x + max.x) / 2,
    y: (min.y + max.y) / 2,
    z: (min.z + max.z) / 2,
  };

  const size = Math.max(max.x - min.x, max.y - min.y, max.z - min.z);

  return { min, max, center, size };
}
