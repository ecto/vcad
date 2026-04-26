/**
 * Sub-feature hit testing for the viewport.
 *
 * Given an R3F triangle intersection, pick the most-specific selection
 * item under the cursor: vertex (within ~8px screen), edge (within ~6px),
 * or face (the hit triangle). The selection filter restricts the candidate
 * set when the user has switched modes.
 *
 * The mesh's `positions` are kernel (Z-up) space; `intersection.point` is
 * already in display (Y-up) space because R3F passes the world-space hit
 * through the rotation group. We convert vertex positions on the fly via
 * the standard `(x, y, z) → (x, z, -y)` mapping.
 */

import type { Camera } from "three";
import { Vector2, Vector3 } from "three";
import type { TriangleMesh } from "@vcad/engine";
import type { SelectionFilter, SelectionItem } from "@vcad/core";

const VERTEX_PX = 10;
const EDGE_PX = 6;

export interface SubFeaturePickContext {
  /** Triangle index from the R3F intersection (`e.faceIndex`). */
  triIndex: number;
  /** Hit point in display (Y-up) space, from `intersection.point`. */
  hitPoint: Vector3;
  mesh: TriangleMesh;
  partId: string;
  filter: SelectionFilter;
  camera: Camera;
  /** Viewport pixel dimensions, from `useThree(s => s.size)`. */
  viewport: { width: number; height: number };
}

/** Convert a kernel-space mesh vertex to display-space (Y-up). */
function kernelVertexToDisplay(
  mesh: TriangleMesh,
  vertexIndex: number,
  out: Vector3 = new Vector3(),
): Vector3 {
  const i = vertexIndex * 3;
  const x = mesh.positions[i] ?? 0;
  const y = mesh.positions[i + 1] ?? 0;
  const z = mesh.positions[i + 2] ?? 0;
  // Same rotation the scene group applies: (x, y, z) → (x, z, -y).
  out.set(x, z, -y);
  return out;
}

/** Project a world-space (display, Y-up) point to viewport pixels. */
function projectToScreen(
  world: Vector3,
  camera: Camera,
  viewport: { width: number; height: number },
  out: Vector2 = new Vector2(),
): Vector2 {
  const ndc = world.clone().project(camera);
  out.set(
    ((ndc.x + 1) / 2) * viewport.width,
    ((1 - ndc.y) / 2) * viewport.height,
  );
  return out;
}

/** Closest distance from a 2D point to a 2D segment, in pixels. */
function pointToSegmentDist(p: Vector2, a: Vector2, b: Vector2): number {
  const abx = b.x - a.x;
  const aby = b.y - a.y;
  const apx = p.x - a.x;
  const apy = p.y - a.y;
  const lenSq = abx * abx + aby * aby;
  if (lenSq < 1e-6) return Math.hypot(apx, apy);
  let t = (apx * abx + apy * aby) / lenSq;
  t = Math.max(0, Math.min(1, t));
  const dx = a.x + t * abx - p.x;
  const dy = a.y + t * aby - p.y;
  return Math.hypot(dx, dy);
}

/** Build a stable edge ID from two vertex indices: `min * N + max`, where
 *  N = vertex count. Survives within a single mesh evaluation. */
function makeEdgeId(va: number, vb: number, vertexCount: number): number {
  const lo = Math.min(va, vb);
  const hi = Math.max(va, vb);
  return lo * vertexCount + hi;
}

/**
 * Pick the most-specific selection item under the cursor.
 *
 * Priority when filter is "auto": vertex > edge > face > body. With a
 * specific filter, only items of that kind are considered.
 *
 * Returns null when:
 *   - filter restricts to a kind that has no candidate within threshold, OR
 *   - the triangle index is out of range.
 */
export function pickSubFeature(
  ctx: SubFeaturePickContext,
): SelectionItem | null {
  const { triIndex, hitPoint, mesh, partId, filter, camera, viewport } = ctx;

  if (filter === "body") {
    return { kind: "part", id: partId };
  }

  const triCount = mesh.indices.length / 3;
  if (triIndex < 0 || triIndex >= triCount) return null;

  // Triangle vertex indices and their display-space positions.
  const ia = mesh.indices[triIndex * 3]!;
  const ib = mesh.indices[triIndex * 3 + 1]!;
  const ic = mesh.indices[triIndex * 3 + 2]!;
  const triVerts: number[] = [ia, ib, ic];

  const vertexCount = mesh.positions.length / 3;
  const _va = new Vector3();
  const _vb = new Vector3();
  const _vc = new Vector3();
  kernelVertexToDisplay(mesh, ia, _va);
  kernelVertexToDisplay(mesh, ib, _vb);
  kernelVertexToDisplay(mesh, ic, _vc);

  // Project hit + triangle to screen space once.
  const _hitScreen = new Vector2();
  projectToScreen(hitPoint, camera, viewport, _hitScreen);
  const _aScreen = new Vector2();
  const _bScreen = new Vector2();
  const _cScreen = new Vector2();
  projectToScreen(_va, camera, viewport, _aScreen);
  projectToScreen(_vb, camera, viewport, _bScreen);
  projectToScreen(_vc, camera, viewport, _cScreen);

  const wantVertex = filter === "vertex" || filter === "auto";
  const wantEdge = filter === "edge" || filter === "auto";
  const wantFace = filter === "face" || filter === "auto";

  // ── Vertex pick ──────────────────────────────────────────────────────
  if (wantVertex) {
    const screens = [_aScreen, _bScreen, _cScreen];
    let bestIdx = -1;
    let bestDist = VERTEX_PX;
    for (let k = 0; k < 3; k++) {
      const d = screens[k]!.distanceTo(_hitScreen);
      if (d < bestDist) {
        bestDist = d;
        bestIdx = k;
      }
    }
    if (bestIdx >= 0) {
      return {
        kind: "vertex",
        partId,
        vertexId: triVerts[bestIdx]!,
      };
    }
  }

  // ── Edge pick (within the hit triangle's three edges) ───────────────
  if (wantEdge) {
    const edges: Array<[Vector2, Vector2, number, number]> = [
      [_aScreen, _bScreen, ia, ib],
      [_bScreen, _cScreen, ib, ic],
      [_cScreen, _aScreen, ic, ia],
    ];
    let bestK = -1;
    let bestDist = EDGE_PX;
    for (let k = 0; k < 3; k++) {
      const [s0, s1] = edges[k]!;
      const d = pointToSegmentDist(_hitScreen, s0, s1);
      if (d < bestDist) {
        bestDist = d;
        bestK = k;
      }
    }
    if (bestK >= 0) {
      const [, , va, vb] = edges[bestK]!;
      return {
        kind: "edge",
        partId,
        edgeId: makeEdgeId(va, vb, vertexCount),
      };
    }
  }

  // ── Face pick ────────────────────────────────────────────────────────
  if (wantFace) {
    return { kind: "face", partId, faceIndex: triIndex };
  }

  // Filter was "auto" but no candidate found, or filter narrowed too far.
  return null;
}

/** Decode a stable edge ID back into its two vertex indices. */
export function decodeEdgeId(
  edgeId: number,
  vertexCount: number,
): { a: number; b: number } {
  return {
    a: Math.floor(edgeId / vertexCount),
    b: edgeId % vertexCount,
  };
}
