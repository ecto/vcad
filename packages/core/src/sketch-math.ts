/**
 * Kernel-backed sketch math utilities.
 *
 * This module wraps the pure-function helpers exported by
 * `vcad-kernel-wasm` (plane projection, snap, hit-test, shape builders)
 * so the web app can share the same code as the TUI and the WASM
 * `SketchSession`. Each function has a lightweight JS fallback that runs
 * when the WASM module isn't hydrated yet — that makes the utilities
 * safe to call from synchronous Zustand reducers and React render paths.
 *
 * Prefer using these helpers over re-deriving the math in components —
 * that's how the old `SketchPlane3D.tsx` and `sketch-store.ts` ended up
 * with three different copies of the same rectangle builder.
 */

import type { Vec2, Vec3, SketchSegment2D } from "@vcad/ir";
import type { SketchPlane } from "./types.js";
import { getKernelWasmSync } from "./wasm-singleton.js";

// ---------------------------------------------------------------------------
// WASM accessor
// ---------------------------------------------------------------------------

type SketchMathWasm = {
  sketchPlaneBasis?: (planeJson: string) => string;
  sketchWorldToSketch?: (planeJson: string, wx: number, wy: number, wz: number) => string;
  sketchToWorld?: (planeJson: string, sx: number, sy: number) => string;
  sketchPlaneIntersectRay?: (
    planeJson: string,
    ox: number,
    oy: number,
    oz: number,
    dx: number,
    dy: number,
    dz: number,
  ) => string;
  sketchSnap?: (
    segmentsJson: string,
    x: number,
    y: number,
    gridEnabled: boolean,
    gridSize: number,
    pointEnabled: boolean,
    pointTolerance: number,
  ) => string;
  sketchHitTest?: (
    segmentsJson: string,
    x: number,
    y: number,
    tolerance: number,
  ) => number;
  sketchRectangleSegments?: (p1x: number, p1y: number, p2x: number, p2y: number) => string;
  sketchCircleSegments?: (
    cx: number,
    cy: number,
    radius: number,
    segments: number,
  ) => string;
};

function kernel(): SketchMathWasm | null {
  return getKernelWasmSync() as unknown as SketchMathWasm | null;
}

/**
 * Serialize a [`SketchPlane`] into the JSON shape accepted by the WASM
 * helpers. Axis-aligned planes become a string (`"XY"` / `"XZ"` / `"YZ"`);
 * face-derived planes become the `{origin, xDir, yDir}` object.
 */
function planeToJson(plane: SketchPlane): string {
  if (typeof plane === "string") {
    return JSON.stringify(plane);
  }
  return JSON.stringify({
    origin: [plane.origin.x, plane.origin.y, plane.origin.z],
    xDir: [plane.xDir.x, plane.xDir.y, plane.xDir.z],
    yDir: [plane.yDir.x, plane.yDir.y, plane.yDir.z],
  });
}

// ---------------------------------------------------------------------------
// Plane basis
// ---------------------------------------------------------------------------

export interface PlaneBasis {
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
  normal: Vec3;
}

/** Get a plane's origin + in-plane axes + normal as `Vec3`s. */
export function getPlaneBasis(plane: SketchPlane): PlaneBasis {
  const wasm = kernel();
  if (wasm?.sketchPlaneBasis) {
    const raw = wasm.sketchPlaneBasis(planeToJson(plane));
    const parsed = JSON.parse(raw) as {
      origin: [number, number, number];
      xDir: [number, number, number];
      yDir: [number, number, number];
      normal: [number, number, number];
    };
    return {
      origin: { x: parsed.origin[0], y: parsed.origin[1], z: parsed.origin[2] },
      xDir: { x: parsed.xDir[0], y: parsed.xDir[1], z: parsed.xDir[2] },
      yDir: { x: parsed.yDir[0], y: parsed.yDir[1], z: parsed.yDir[2] },
      normal: { x: parsed.normal[0], y: parsed.normal[1], z: parsed.normal[2] },
    };
  }
  return getPlaneBasisJs(plane);
}

function getPlaneBasisJs(plane: SketchPlane): PlaneBasis {
  if (typeof plane === "string") {
    switch (plane) {
      case "XY":
        return {
          origin: { x: 0, y: 0, z: 0 },
          xDir: { x: 1, y: 0, z: 0 },
          yDir: { x: 0, y: 1, z: 0 },
          normal: { x: 0, y: 0, z: 1 },
        };
      case "XZ":
        return {
          origin: { x: 0, y: 0, z: 0 },
          xDir: { x: 1, y: 0, z: 0 },
          yDir: { x: 0, y: 0, z: 1 },
          normal: { x: 0, y: -1, z: 0 },
        };
      case "YZ":
        return {
          origin: { x: 0, y: 0, z: 0 },
          xDir: { x: 0, y: 1, z: 0 },
          yDir: { x: 0, y: 0, z: 1 },
          normal: { x: 1, y: 0, z: 0 },
        };
    }
  }
  return {
    origin: plane.origin,
    xDir: plane.xDir,
    yDir: plane.yDir,
    normal: plane.normal,
  };
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

/** Project a 3D world point onto a plane and return local 2D coordinates. */
export function worldToSketch(plane: SketchPlane, world: Vec3): Vec2 {
  const wasm = kernel();
  if (wasm?.sketchWorldToSketch) {
    const raw = wasm.sketchWorldToSketch(planeToJson(plane), world.x, world.y, world.z);
    const [x, y] = JSON.parse(raw) as [number, number];
    return { x, y };
  }
  const { origin, xDir, yDir } = getPlaneBasisJs(plane);
  const dx = world.x - origin.x;
  const dy = world.y - origin.y;
  const dz = world.z - origin.z;
  return {
    x: dx * xDir.x + dy * xDir.y + dz * xDir.z,
    y: dx * yDir.x + dy * yDir.y + dz * yDir.z,
  };
}

/** Convert 2D sketch coordinates to a 3D world point. */
export function sketchToWorld(plane: SketchPlane, pt: Vec2): Vec3 {
  const wasm = kernel();
  if (wasm?.sketchToWorld) {
    const raw = wasm.sketchToWorld(planeToJson(plane), pt.x, pt.y);
    const [x, y, z] = JSON.parse(raw) as [number, number, number];
    return { x, y, z };
  }
  const { origin, xDir, yDir } = getPlaneBasisJs(plane);
  return {
    x: origin.x + pt.x * xDir.x + pt.y * yDir.x,
    y: origin.y + pt.x * xDir.y + pt.y * yDir.y,
    z: origin.z + pt.x * xDir.z + pt.y * yDir.z,
  };
}

/**
 * Intersect a world-space ray with the sketch plane and return the hit
 * in 2D sketch coordinates. Returns `null` for rays parallel to the
 * plane.
 */
export function intersectRay(
  plane: SketchPlane,
  rayOrigin: Vec3,
  rayDir: Vec3,
): Vec2 | null {
  const wasm = kernel();
  if (wasm?.sketchPlaneIntersectRay) {
    const raw = wasm.sketchPlaneIntersectRay(
      planeToJson(plane),
      rayOrigin.x,
      rayOrigin.y,
      rayOrigin.z,
      rayDir.x,
      rayDir.y,
      rayDir.z,
    );
    if (raw === "null") return null;
    const [x, y] = JSON.parse(raw) as [number, number];
    return { x, y };
  }
  const { origin, normal } = getPlaneBasisJs(plane);
  const denom = rayDir.x * normal.x + rayDir.y * normal.y + rayDir.z * normal.z;
  if (Math.abs(denom) < 1e-12) return null;
  const diff = {
    x: origin.x - rayOrigin.x,
    y: origin.y - rayOrigin.y,
    z: origin.z - rayOrigin.z,
  };
  const t = (diff.x * normal.x + diff.y * normal.y + diff.z * normal.z) / denom;
  const hit = {
    x: rayOrigin.x + t * rayDir.x,
    y: rayOrigin.y + t * rayDir.y,
    z: rayOrigin.z + t * rayDir.z,
  };
  return worldToSketch(plane, hit);
}

// ---------------------------------------------------------------------------
// Snap & hit-test
// ---------------------------------------------------------------------------

export interface SnapOptions {
  gridEnabled: boolean;
  gridSize: number;
  pointEnabled: boolean;
  pointTolerance: number;
}

export interface SnapResult {
  /** Snapped position in sketch coordinates. */
  snapped: Vec2;
  /** The vertex the cursor snapped to, if any. */
  target: Vec2 | null;
}

/**
 * Snap a cursor position against a segment list. Vertex snaps take
 * precedence over grid snaps — same rules the TUI uses.
 */
export function snapPoint(
  segments: SketchSegment2D[],
  point: Vec2,
  opts: SnapOptions,
): SnapResult {
  const wasm = kernel();
  if (wasm?.sketchSnap) {
    const raw = wasm.sketchSnap(
      JSON.stringify(segments),
      point.x,
      point.y,
      opts.gridEnabled,
      opts.gridSize,
      opts.pointEnabled,
      opts.pointTolerance,
    );
    const parsed = JSON.parse(raw) as {
      x: number;
      y: number;
      snapTarget: [number, number] | null;
    };
    return {
      snapped: { x: parsed.x, y: parsed.y },
      target: parsed.snapTarget
        ? { x: parsed.snapTarget[0], y: parsed.snapTarget[1] }
        : null,
    };
  }
  return snapPointJs(segments, point, opts);
}

function snapPointJs(
  segments: SketchSegment2D[],
  point: Vec2,
  opts: SnapOptions,
): SnapResult {
  if (opts.pointEnabled) {
    const tol = opts.pointTolerance;
    const tol2 = tol * tol;
    for (const seg of segments) {
      for (const v of [seg.start, seg.end]) {
        const dx = point.x - v.x;
        const dy = point.y - v.y;
        if (dx * dx + dy * dy < tol2) {
          return { snapped: { x: v.x, y: v.y }, target: { x: v.x, y: v.y } };
        }
      }
    }
  }
  if (opts.gridEnabled && opts.gridSize > 0) {
    const g = opts.gridSize;
    return {
      snapped: {
        x: Math.round(point.x / g) * g,
        y: Math.round(point.y / g) * g,
      },
      target: null,
    };
  }
  return { snapped: point, target: null };
}

/**
 * Find the index of the segment closest to `(point.x, point.y)` within
 * `tolerance`, or `null` if nothing is close enough.
 */
export function hitTestSegments(
  segments: SketchSegment2D[],
  point: Vec2,
  tolerance: number,
): number | null {
  const wasm = kernel();
  if (wasm?.sketchHitTest) {
    const idx = wasm.sketchHitTest(
      JSON.stringify(segments),
      point.x,
      point.y,
      tolerance,
    );
    return idx < 0 ? null : idx;
  }
  return hitTestSegmentsJs(segments, point, tolerance);
}

function hitTestSegmentsJs(
  segments: SketchSegment2D[],
  point: Vec2,
  tolerance: number,
): number | null {
  let best: { idx: number; dist: number } | null = null;
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i]!;
    let d: number;
    if (seg.type === "Line") {
      d = pointToSegmentDistance(point, seg.start, seg.end);
    } else {
      const r = Math.hypot(seg.start.x - seg.center.x, seg.start.y - seg.center.y);
      d = Math.abs(Math.hypot(point.x - seg.center.x, point.y - seg.center.y) - r);
    }
    if (d < tolerance && (best === null || d < best.dist)) {
      best = { idx: i, dist: d };
    }
  }
  return best ? best.idx : null;
}

function pointToSegmentDistance(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  if (len2 < 1e-8) return Math.hypot(p.x - a.x, p.y - a.y);
  const t = Math.max(
    0,
    Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2),
  );
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

// ---------------------------------------------------------------------------
// Shape builders
// ---------------------------------------------------------------------------

/** Build the four line segments of an axis-aligned rectangle. */
export function buildRectangle(p1: Vec2, p2: Vec2): SketchSegment2D[] {
  const wasm = kernel();
  if (wasm?.sketchRectangleSegments) {
    const raw = wasm.sketchRectangleSegments(p1.x, p1.y, p2.x, p2.y);
    return JSON.parse(raw) as SketchSegment2D[];
  }
  const minX = Math.min(p1.x, p2.x);
  const maxX = Math.max(p1.x, p2.x);
  const minY = Math.min(p1.y, p2.y);
  const maxY = Math.max(p1.y, p2.y);
  return [
    { type: "Line", start: { x: minX, y: minY }, end: { x: maxX, y: minY } },
    { type: "Line", start: { x: maxX, y: minY }, end: { x: maxX, y: maxY } },
    { type: "Line", start: { x: maxX, y: maxY }, end: { x: minX, y: maxY } },
    { type: "Line", start: { x: minX, y: maxY }, end: { x: minX, y: minY } },
  ];
}

/** Build an N-sided polygonal approximation of a circle as arc segments. */
export function buildCircle(
  center: Vec2,
  radius: number,
  segments: number = 32,
): SketchSegment2D[] {
  const wasm = kernel();
  if (wasm?.sketchCircleSegments) {
    const raw = wasm.sketchCircleSegments(center.x, center.y, radius, segments);
    return JSON.parse(raw) as SketchSegment2D[];
  }
  const n = Math.max(3, segments);
  const out: SketchSegment2D[] = [];
  for (let i = 0; i < n; i++) {
    const a0 = (2 * Math.PI * i) / n;
    const a1 = (2 * Math.PI * (i + 1)) / n;
    out.push({
      type: "Arc",
      start: {
        x: center.x + radius * Math.cos(a0),
        y: center.y + radius * Math.sin(a0),
      },
      end: {
        x: center.x + radius * Math.cos(a1),
        y: center.y + radius * Math.sin(a1),
      },
      center,
      ccw: true,
    });
  }
  return out;
}
