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
