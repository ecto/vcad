/**
 * Route-vs-enclosure interference (Phase 3).
 *
 * Axis-aligned bounding-box overlap test between PCB component 3D bodies
 * (from `componentMeshes`) and the mechanical parts sharing the canvas. This
 * is what makes the unified MCAD/ECAD canvas pay off: a tall capacitor or a
 * connector that pokes through the enclosure lights up live while you place
 * and route.
 *
 * AABB-vs-AABB in the world kernel frame: mechanical meshes already carry their
 * world transform, and component bodies (board-local) are mapped into the world
 * via the focused board's transform, so a board moved or rotated as a part
 * still clashes correctly. Mesh-accurate clash + assembly-instance obstacles are
 * follow-ups.
 */

import type { ComponentMesh } from "@vcad/engine";

export type Aabb = { min: [number, number, number]; max: [number, number, number] };

/** Maps a point into another frame (e.g. board-local → world). */
export type PointTransform = (x: number, y: number, z: number) => [number, number, number];

/**
 * Compute an AABB from a flat [x,y,z,...] position buffer, optionally mapping
 * each point through `transform` first (e.g. a board-local → world transform).
 */
export function aabbOfPositions(
  positions: ArrayLike<number> | undefined,
  transform?: PointTransform,
): Aabb | null {
  if (!positions || positions.length < 3) return null;
  let minX = Infinity,
    minY = Infinity,
    minZ = Infinity;
  let maxX = -Infinity,
    maxY = -Infinity,
    maxZ = -Infinity;
  for (let i = 0; i + 2 < positions.length; i += 3) {
    let x = positions[i]!;
    let y = positions[i + 1]!;
    let z = positions[i + 2]!;
    if (transform) [x, y, z] = transform(x, y, z);
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
    if (z < minZ) minZ = z;
    if (z > maxZ) maxZ = z;
  }
  if (!isFinite(minX)) return null;
  return { min: [minX, minY, minZ], max: [maxX, maxY, maxZ] };
}

/**
 * Per-axis inner-cavity span of a shell-like (hollow) mesh: clusters the
 * coordinates and returns the largest interior gap that has material (clusters)
 * on *both* sides — i.e. the empty region between the two inner walls. Returns
 * null for a solid body (the largest gap is the body itself, with no material
 * beyond it). Robust to fillets and coarse tessellation since the cavity gap
 * dominates the small wall/feature gaps.
 */
function innerSpan(coords: number[], tol = 0.3, minCavity = 1): [number, number] | null {
  if (coords.length < 6) return null;
  const sorted = [...coords].sort((a, b) => a - b);
  const clusters: number[] = [];
  for (const c of sorted) {
    if (clusters.length === 0 || c - clusters[clusters.length - 1]! > tol) clusters.push(c);
  }
  if (clusters.length < 4) return null; // need outer+inner wall on each side
  let best = -1, lo = 0, hi = 0;
  for (let i = 1; i < clusters.length - 2; i++) {
    // gap [clusters[i], clusters[i+1]] has clusters[0..i-1] left and the rest right
    const gap = clusters[i + 1]! - clusters[i]!;
    if (gap > best) { best = gap; lo = clusters[i]!; hi = clusters[i + 1]!; }
  }
  if (best < minCavity) return null;
  return [lo, hi];
}

/**
 * Estimate the internal cavity of a shell enclosure from its world-space mesh
 * vertices. Returns the inner box when both the X and Y axes show a clear
 * wall→cavity→wall structure; null otherwise (caller falls back to the AABB).
 * The Z span is the full vertical extent (cavities are usually open-topped).
 */
export function cavityBounds(positions: ArrayLike<number> | undefined): Aabb | null {
  if (!positions || positions.length < 24) return null; // need at least a box's worth
  // (a solid box yields only 2 coordinate clusters per axis → innerSpan returns
  // null regardless of vertex count; the real shell test lives there.)
  const xs: number[] = [], ys: number[] = [], zs: number[] = [];
  for (let i = 0; i + 2 < positions.length; i += 3) {
    xs.push(positions[i]!);
    ys.push(positions[i + 1]!);
    zs.push(positions[i + 2]!);
  }
  const ix = innerSpan(xs);
  const iy = innerSpan(ys);
  if (!ix || !iy) return null;
  let zmin = Infinity, zmax = -Infinity;
  for (const z of zs) { if (z < zmin) zmin = z; if (z > zmax) zmax = z; }
  return { min: [ix[0], iy[0], zmin], max: [ix[1], iy[1], zmax] };
}

/** Union a list of AABBs into one enclosing box. null if the list is empty. */
export function mergeAabbs(boxes: Aabb[]): Aabb | null {
  if (boxes.length === 0) return null;
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (const b of boxes) {
    for (let i = 0; i < 3; i++) {
      if (b.min[i]! < min[i]!) min[i] = b.min[i]!;
      if (b.max[i]! > max[i]!) max[i] = b.max[i]!;
    }
  }
  return { min, max };
}

/** True when two AABBs overlap (optionally expanded by a clearance margin). */
export function aabbsOverlap(a: Aabb, b: Aabb, margin = 0): boolean {
  return (
    a.min[0] - margin <= b.max[0] &&
    a.max[0] + margin >= b.min[0] &&
    a.min[1] - margin <= b.max[1] &&
    a.max[1] + margin >= b.min[1] &&
    a.min[2] - margin <= b.max[2] &&
    a.max[2] + margin >= b.min[2]
  );
}

/**
 * Footprint refs whose component body intersects any mechanical AABB.
 * `margin` adds a clearance band so near-misses also flag. `boardToWorld`
 * maps the board-local component bodies into the world frame the mechanical
 * AABBs live in (so a moved/rotated board clashes correctly); omit for a board
 * at the origin.
 */
export function interferingRefs(
  components: ComponentMesh[],
  mechanical: Aabb[],
  margin = 0,
  boardToWorld?: PointTransform,
): string[] {
  if (mechanical.length === 0) return [];
  const out = new Set<string>();
  for (const c of components) {
    const cb = aabbOfPositions(c.positions, boardToWorld);
    if (!cb) continue;
    if (mechanical.some((m) => aabbsOverlap(cb, m, margin))) out.add(c.footprint_ref);
  }
  // A footprint can yield several component sub-meshes (body + caps); collapse
  // to unique refs so callers get each interfering footprint once.
  return [...out];
}
