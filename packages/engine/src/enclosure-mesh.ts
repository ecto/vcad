/**
 * Extract enclosure features (cavity, standoffs, wall openings) from a solid's
 * triangle mesh.
 *
 * This is the bridge from the BRep/CAD side to {@link checkEnclosureFit}: the
 * MCP layer evaluates an enclosure document to a mesh, hands the flat
 * positions/indices here, and gets back the axis-aligned interior void plus the
 * posts and wall cutouts inside it.
 *
 * Inside/outside is tested with the **generalized winding number** (Jacobson et
 * al.), sampled on a coarse 3D grid. GWN is robust to the small holes,
 * coincident faces, and stray internal faces real kernel CSG meshes contain —
 * a plain even-odd ray cast is not. From the resulting voxel occupancy we read
 * the cavity (a column that is solid at the floor but open at the top), the
 * standoffs (pocket columns whose floor solid rises into a post), and the wall
 * cutouts (gaps in the wall ring). Pure (number arrays in, features out) so it
 * tests without a kernel.
 *
 * Assumes a Z-up, roughly box-shaped, open-top enclosure (the 3D-printed tray:
 * walls + floor + standoffs + side cutouts, lid mounting at the rim).
 */

import type {
  EnclosureCavity,
  EnclosureFeatures,
  Standoff,
  WallEdge,
  WallOpening,
} from "./enclosure-fit.js";

/** Grid resolution. Cheap enough for an interactive check, fine enough to
 *  separate 5mm M3 posts and resolve a connector cutout. */
const GRID_XY = 48;
const GRID_Z = 28;
/** GWN magnitude above which a sample point counts as inside the solid. */
const INSIDE = 0.5;
/** A post must rise at least this far above the floor to count as a standoff. */
const MIN_POST_HEIGHT = 0.8;
/** Min contiguous open cells for a wall gap to count as a cutout. */
const MIN_OPENING_CELLS = 2;
/** Asymmetric sub-cell sample offsets so a sample never lands exactly on a
 *  triangle vertex/edge (where GWN is ill-conditioned). */
const JX = 0.5 + 0.137;
const JY = 0.5 - 0.077;
const JZ = 0.5 + 0.041;

interface Occupancy {
  gw: number;
  gh: number;
  gz: number;
  ox: number;
  oy: number;
  oz: number;
  dx: number;
  dy: number;
  dz: number;
  minZ: number;
  maxZ: number;
  /** Flat [i + gw*(j + gh*k)] occupancy, 1 = solid. */
  occ: Uint8Array;
}

/**
 * Generalized winding number of point (qx,qy,qz) w.r.t. the mesh. ~1 (or −1 for
 * inverted winding) inside a closed region, ~0 outside; robust to holes and
 * stray faces. Van Oosterom–Strackee signed solid angle per triangle.
 */
function gwn(
  positions: ArrayLike<number>,
  indices: ArrayLike<number>,
  qx: number,
  qy: number,
  qz: number,
): number {
  let w = 0;
  const tris = indices.length;
  for (let t = 0; t < tris; t += 3) {
    const i0 = indices[t] * 3;
    const i1 = indices[t + 1] * 3;
    const i2 = indices[t + 2] * 3;
    const ax = positions[i0] - qx;
    const ay = positions[i0 + 1] - qy;
    const az = positions[i0 + 2] - qz;
    const bx = positions[i1] - qx;
    const by = positions[i1 + 1] - qy;
    const bz = positions[i1 + 2] - qz;
    const cx = positions[i2] - qx;
    const cy = positions[i2 + 1] - qy;
    const cz = positions[i2 + 2] - qz;
    const la = Math.sqrt(ax * ax + ay * ay + az * az);
    const lb = Math.sqrt(bx * bx + by * by + bz * bz);
    const lc = Math.sqrt(cx * cx + cy * cy + cz * cz);
    // a · (b × c)
    const cbx = by * cz - bz * cy;
    const cby = bz * cx - bx * cz;
    const cbz = bx * cy - by * cx;
    const num = ax * cbx + ay * cby + az * cbz;
    const den =
      la * lb * lc +
      (ax * bx + ay * by + az * bz) * lc +
      (bx * cx + by * cy + bz * cz) * la +
      (cx * ax + cy * ay + cz * az) * lb;
    w += Math.atan2(num, den);
  }
  return w / (2 * Math.PI);
}

/** Sample the GWN occupancy grid over the mesh's AABB. */
function buildOccupancy(positions: ArrayLike<number>, indices: ArrayLike<number>): Occupancy | null {
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;
  for (let i = 0; i < positions.length; i += 3) {
    const x = positions[i];
    const y = positions[i + 1];
    const z = positions[i + 2];
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
    if (z < minZ) minZ = z;
    if (z > maxZ) maxZ = z;
  }
  if (!(maxX > minX) || !(maxY > minY) || !(maxZ > minZ)) return null;
  const gw = GRID_XY;
  const gh = GRID_XY;
  const gz = GRID_Z;
  const dx = (maxX - minX) / gw;
  const dy = (maxY - minY) / gh;
  const dz = (maxZ - minZ) / gz;
  const occ = new Uint8Array(gw * gh * gz);
  for (let k = 0; k < gz; k++) {
    const qz = minZ + (k + JZ) * dz;
    for (let j = 0; j < gh; j++) {
      const qy = minY + (j + JY) * dy;
      for (let i = 0; i < gw; i++) {
        const qx = minX + (i + JX) * dx;
        if (Math.abs(gwn(positions, indices, qx, qy, qz)) > INSIDE) {
          occ[i + gw * (j + gh * k)] = 1;
        }
      }
    }
  }
  return { gw, gh, gz, ox: minX, oy: minY, oz: minZ, dx, dy, dz, minZ, maxZ, occ };
}

/** Z of the top of the bottom solid run in a column (null if no floor). */
function bottomRunTopZ(o: Occupancy, i: number, j: number): number | null {
  const base = i + o.gw * j;
  if (o.occ[base] !== 1) return null; // no floor under this column
  let k = 1;
  while (k < o.gz && o.occ[base + o.gw * o.gh * k] === 1) k++;
  // Top of the run is the boundary between cell k-1 (solid) and k (empty).
  return o.oz + k * o.dz;
}

/** True when the column's top cell is empty (open above). */
function openAtTop(o: Occupancy, i: number, j: number): boolean {
  return o.occ[i + o.gw * (j + o.gh * (o.gz - 1))] !== 1;
}

/** True when any cell in (kLo,kHi) is empty — a gap through the column. */
function hasGap(o: Occupancy, i: number, j: number, kLo: number, kHi: number): boolean {
  for (let k = kLo; k <= kHi; k++) {
    if (o.occ[i + o.gw * (j + o.gh * k)] !== 1) return true;
  }
  return false;
}

/**
 * Extract the cavity, standoffs, and wall openings from a solid mesh. Returns
 * `cavity: null` when no open-top pocket is found (e.g. a solid block).
 */
export function extractEnclosureFeatures(
  positions: ArrayLike<number>,
  indices: ArrayLike<number>,
): EnclosureFeatures {
  const o = buildOccupancy(positions, indices);
  if (!o) {
    return {
      outer: { minX: 0, maxX: 0, minY: 0, maxY: 0, minZ: 0, maxZ: 0 },
      cavity: null,
      standoffs: [],
      openings: [],
    };
  }
  const outer = {
    minX: o.ox,
    maxX: o.ox + o.gw * o.dx,
    minY: o.oy,
    maxY: o.oy + o.gh * o.dy,
    minZ: o.minZ,
    maxZ: o.maxZ,
  };

  // Pocket columns: solid at the floor, open at the top.
  const isPocket = new Uint8Array(o.gw * o.gh);
  const firstTops: number[] = [];
  const countX = new Int32Array(o.gw);
  const countY = new Int32Array(o.gh);
  let pocketCount = 0;
  for (let j = 0; j < o.gh; j++) {
    for (let i = 0; i < o.gw; i++) {
      const top = bottomRunTopZ(o, i, j);
      if (top != null && openAtTop(o, i, j)) {
        isPocket[i + o.gw * j] = 1;
        pocketCount++;
        firstTops.push(top);
        countX[i]++;
        countY[j]++;
      }
    }
  }
  if (pocketCount === 0) {
    return { outer, cavity: null, standoffs: [], openings: [] };
  }

  // Cavity bounds from per-axis occupancy profiles: keep the contiguous core
  // where pocket coverage is at least half the peak, trimming the thin notch a
  // wall cutout pokes into the interior (box-cavity assumption).
  const core = (counts: Int32Array): [number, number] => {
    let peak = 0;
    for (const c of counts) if (c > peak) peak = c;
    const thr = peak * 0.5;
    let lo = 0;
    let hi = counts.length - 1;
    while (lo < counts.length && counts[lo] < thr) lo++;
    while (hi >= 0 && counts[hi] < thr) hi--;
    return [lo, hi];
  };
  const [pminI, pmaxI] = core(countX);
  const [pminJ, pmaxJ] = core(countY);

  // Floor Z: median first-run top over pocket columns (robust to the standoff
  // minority, whose first run tops out at the post).
  const sortedTops = [...firstTops].sort((a, b) => a - b);
  const floorZ = sortedTops[Math.floor(sortedTops.length / 2)] ?? o.minZ;
  const ceilZ = o.maxZ; // open-top tray: the lid mounts at the rim

  const cavity: EnclosureCavity = {
    minX: o.ox + pminI * o.dx,
    maxX: o.ox + (pmaxI + 1) * o.dx,
    minY: o.oy + pminJ * o.dy,
    maxY: o.oy + (pmaxJ + 1) * o.dy,
    floorZ: round2(floorZ),
    ceilZ: round2(ceilZ),
    hasLid: false,
  };

  const standoffs = extractStandoffs(o, isPocket, floorZ);
  const openings = extractOpenings(o, cavity, floorZ, ceilZ);
  return { outer, cavity, standoffs, openings };
}

const round2 = (n: number) => Math.round(n * 100) / 100;

/** Cluster pocket columns whose floor solid rises above the floor into posts. */
function extractStandoffs(o: Occupancy, isPocket: Uint8Array, floorZ: number): Standoff[] {
  const isPost = new Uint8Array(o.gw * o.gh);
  for (let j = 0; j < o.gh; j++) {
    for (let i = 0; i < o.gw; i++) {
      if (!isPocket[i + o.gw * j]) continue;
      const top = bottomRunTopZ(o, i, j);
      if (top != null && top > floorZ + MIN_POST_HEIGHT) isPost[i + o.gw * j] = 1;
    }
  }
  const seen = new Uint8Array(o.gw * o.gh);
  const standoffs: Standoff[] = [];
  for (let j = 0; j < o.gh; j++) {
    for (let i = 0; i < o.gw; i++) {
      const k0 = i + o.gw * j;
      if (!isPost[k0] || seen[k0]) continue;
      const stack: Array<[number, number]> = [[i, j]];
      seen[k0] = 1;
      let sumX = 0;
      let sumY = 0;
      let n = 0;
      let topMax = -Infinity;
      let minI = i;
      let maxI = i;
      let minJ = j;
      let maxJ = j;
      while (stack.length) {
        const [ci, cj] = stack.pop()!;
        sumX += o.ox + (ci + 0.5) * o.dx;
        sumY += o.oy + (cj + 0.5) * o.dy;
        n++;
        const t = bottomRunTopZ(o, ci, cj);
        if (t != null && t > topMax) topMax = t;
        if (ci < minI) minI = ci;
        if (ci > maxI) maxI = ci;
        if (cj < minJ) minJ = cj;
        if (cj > maxJ) maxJ = cj;
        for (const [ni, nj] of [
          [ci - 1, cj],
          [ci + 1, cj],
          [ci, cj - 1],
          [ci, cj + 1],
        ]) {
          if (ni < 0 || nj < 0 || ni >= o.gw || nj >= o.gh) continue;
          const nk = ni + o.gw * nj;
          if (isPost[nk] && !seen[nk]) {
            seen[nk] = 1;
            stack.push([ni, nj]);
          }
        }
      }
      // Single stray cells are noise, not posts.
      if (n < 2) continue;
      const radius = Math.max(((maxI - minI + 1) * o.dx + (maxJ - minJ + 1) * o.dy) / 4, o.dx);
      standoffs.push({
        x: round2(sumX / n),
        y: round2(sumY / n),
        topZ: round2(topMax),
        radius: round2(radius),
      });
    }
  }
  return standoffs;
}

/**
 * Detect openings in the four cavity walls: walk the cavity perimeter one cell
 * outside the pocket and find spans where the wall is absent at some level
 * between floor and rim (height-agnostic, so a low USB port reads the same as a
 * full-height slot).
 */
function extractOpenings(
  o: Occupancy,
  cavity: EnclosureCavity,
  floorZ: number,
  ceilZ: number,
): WallOpening[] {
  const pIminI = Math.round((cavity.minX - o.ox) / o.dx);
  const pImaxI = Math.round((cavity.maxX - o.ox) / o.dx) - 1;
  const pIminJ = Math.round((cavity.minY - o.oy) / o.dy);
  const pImaxJ = Math.round((cavity.maxY - o.oy) / o.dy) - 1;
  const kLo = Math.max(0, Math.floor((floorZ - o.oz) / o.dz) + 1);
  const kHi = Math.min(o.gz - 1, Math.ceil((ceilZ - o.oz) / o.dz) - 1);
  const openings: WallOpening[] = [];

  const wallOpen = (i: number, j: number): boolean => {
    if (i < 0 || j < 0 || i >= o.gw || j >= o.gh) return true;
    return hasGap(o, i, j, kLo, kHi);
  };

  type Scan = { edge: WallEdge; cells: Array<{ open: boolean; x: number; y: number; i: number; j: number }> };
  const scans: Scan[] = [];
  for (const [edge, wi] of [
    ["minX", pIminI - 1],
    ["maxX", pImaxI + 1],
  ] as Array<[WallEdge, number]>) {
    const cells = [];
    for (let j = pIminJ; j <= pImaxJ; j++) {
      cells.push({ open: wallOpen(wi, j), x: o.ox + (wi + 0.5) * o.dx, y: o.oy + (j + 0.5) * o.dy, i: wi, j });
    }
    scans.push({ edge, cells });
  }
  for (const [edge, wj] of [
    ["minY", pIminJ - 1],
    ["maxY", pImaxJ + 1],
  ] as Array<[WallEdge, number]>) {
    const cells = [];
    for (let i = pIminI; i <= pImaxI; i++) {
      cells.push({ open: wallOpen(i, wj), x: o.ox + (i + 0.5) * o.dx, y: o.oy + (wj + 0.5) * o.dy, i, j: wj });
    }
    scans.push({ edge, cells });
  }

  for (const scan of scans) {
    let run: Array<{ x: number; y: number; i: number; j: number }> = [];
    const flush = () => {
      if (run.length >= MIN_OPENING_CELLS) {
        const xs = run.map((p) => p.x);
        const ys = run.map((p) => p.y);
        const horiz = scan.edge === "minY" || scan.edge === "maxY";
        const center = {
          x: (Math.min(...xs) + Math.max(...xs)) / 2,
          y: (Math.min(...ys) + Math.max(...ys)) / 2,
        };
        const width = horiz
          ? Math.max(...xs) - Math.min(...xs) + o.dx
          : Math.max(...ys) - Math.min(...ys) + o.dy;
        const mid = run[Math.floor(run.length / 2)];
        const { zMin, zMax } = openingZSpan(o, mid.i, mid.j, kLo, kHi);
        openings.push({
          edge: scan.edge,
          center: { x: round2(center.x), y: round2(center.y) },
          width: round2(width),
          zMin: round2(zMin),
          zMax: round2(zMax),
        });
      }
      run = [];
    };
    for (const c of scan.cells) {
      if (c.open) run.push({ x: c.x, y: c.y, i: c.i, j: c.j });
      else flush();
    }
    flush();
  }
  return openings;
}

/** Vertical span of the open band at a wall column. */
function openingZSpan(
  o: Occupancy,
  i: number,
  j: number,
  kLo: number,
  kHi: number,
): { zMin: number; zMax: number } {
  let zMin = Infinity;
  let zMax = -Infinity;
  for (let k = kLo; k <= kHi; k++) {
    if (o.occ[i + o.gw * (j + o.gh * k)] !== 1) {
      const z = o.oz + (k + 0.5) * o.dz;
      if (z < zMin) zMin = z;
      if (z > zMax) zMax = z;
    }
  }
  if (!Number.isFinite(zMin)) return { zMin: o.oz, zMax: o.maxZ };
  return { zMin, zMax };
}
