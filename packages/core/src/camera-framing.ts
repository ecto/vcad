/**
 * Camera framing math shared between the user camera controls hook and
 * the AI camera tool executors.
 *
 * All inputs and outputs are in kernel Z-up space — the same space mesh
 * positions live in. Callers that need Three.js display (Y-up) coords
 * must use `kernelToDisplay` / `displayToKernel` to convert.
 */

export type Vec3 = [number, number, number];

export interface Bbox {
  min: Vec3;
  max: Vec3;
}

export interface CameraGoal {
  /** Eye position, kernel Z-up. */
  position: Vec3;
  /** Look-at target, kernel Z-up. */
  target: Vec3;
}

export type SnapView =
  | "iso"
  | "hero"
  | "top"
  | "bottom"
  | "front"
  | "back"
  | "right"
  | "left";

export const SNAP_VIEWS: readonly SnapView[] = [
  "iso",
  "hero",
  "top",
  "bottom",
  "front",
  "back",
  "right",
  "left",
] as const;

export function isSnapView(v: string): v is SnapView {
  return (SNAP_VIEWS as readonly string[]).includes(v);
}

export function bboxCenter(b: Bbox): Vec3 {
  return [
    (b.min[0] + b.max[0]) / 2,
    (b.min[1] + b.max[1]) / 2,
    (b.min[2] + b.max[2]) / 2,
  ];
}

export function bboxSize(b: Bbox): Vec3 {
  return [
    b.max[0] - b.min[0],
    b.max[1] - b.min[1],
    b.max[2] - b.min[2],
  ];
}

export function bboxMaxDim(b: Bbox): number {
  const [sx, sy, sz] = bboxSize(b);
  return Math.max(sx, sy, sz);
}

/**
 * Union a flat [x,y,z, x,y,z, ...] positions array into an existing
 * bbox. Returns a new bbox; input is not mutated. Returns `box` unchanged
 * if positions is empty.
 */
export function expandBboxFromPositions(
  box: Bbox | null,
  positions: ArrayLike<number>,
): Bbox | null {
  if (positions.length === 0) return box;
  let minX = box ? box.min[0] : Infinity;
  let minY = box ? box.min[1] : Infinity;
  let minZ = box ? box.min[2] : Infinity;
  let maxX = box ? box.max[0] : -Infinity;
  let maxY = box ? box.max[1] : -Infinity;
  let maxZ = box ? box.max[2] : -Infinity;
  for (let i = 0; i < positions.length; i += 3) {
    const x = positions[i]!;
    const y = positions[i + 1]!;
    const z = positions[i + 2]!;
    if (x < minX) minX = x;
    if (y < minY) minY = y;
    if (z < minZ) minZ = z;
    if (x > maxX) maxX = x;
    if (y > maxY) maxY = y;
    if (z > maxZ) maxZ = z;
  }
  if (!isFinite(minX)) return null;
  return {
    min: [minX, minY, minZ],
    max: [maxX, maxY, maxZ],
  };
}

// Direction vectors (from target toward eye) per snap view, in kernel Z-up.
// Derived from the existing Y-up display snaps in ViewportContent via
// display-to-kernel conversion (x, y, z) → (x, -z, y), with magnitudes
// normalized away.
const SNAP_DIRS: Record<SnapView, Vec3> = {
  iso: [1, -1, 1],
  hero: [1, -1, 0.75],
  top: [0, 0, 1],
  bottom: [0, 0, -1],
  front: [0, -1, 0],
  back: [0, 1, 0],
  right: [1, 0, 0],
  left: [-1, 0, 0],
};

function normalize(v: Vec3): Vec3 {
  const len = Math.hypot(v[0], v[1], v[2]);
  if (len < 1e-9) return [0, 0, 1];
  return [v[0] / len, v[1] / len, v[2] / len];
}

/** Clamp a distance to the viewport's sensible framing range. */
export function clampFramingDistance(dist: number): number {
  return Math.max(30, Math.min(300, dist));
}

/**
 * Frame a bounding box from a given direction (or snap view).
 * Distance is 2.5× the max dimension, clamped to [30, 300].
 */
export function frameBbox(
  bbox: Bbox,
  opts: { view?: SnapView; dir?: Vec3 } = {},
): CameraGoal {
  const center = bboxCenter(bbox);
  const dist = clampFramingDistance(bboxMaxDim(bbox) * 2.5);
  const rawDir = opts.dir ?? SNAP_DIRS[opts.view ?? "iso"];
  const dir = normalize(rawDir);
  return {
    position: [
      center[0] + dir[0] * dist,
      center[1] + dir[1] * dist,
      center[2] + dir[2] * dist,
    ],
    target: center,
  };
}

/**
 * Default empty-scene camera goal: iso view of a ±20mm box at origin.
 * Matches the initial camera position for the empty scene.
 */
export function defaultCameraGoal(): CameraGoal {
  return frameBbox({ min: [-20, -20, -20], max: [20, 20, 20] }, { view: "iso" });
}

/**
 * Kernel Z-up → Three.js display Y-up. The viewport wraps all geometry
 * in a -90°X rotation so kernel +Z ends up as display +Y.
 */
export function kernelToDisplay([x, y, z]: Vec3): Vec3 {
  return [x, z, -y];
}

/** Three.js display Y-up → kernel Z-up. Inverse of `kernelToDisplay`. */
export function displayToKernel([x, y, z]: Vec3): Vec3 {
  return [x, -z, y];
}
