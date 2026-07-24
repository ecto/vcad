//! Extract enclosure features (cavity, standoffs, wall openings) from a
//! solid's triangle mesh.
//!
//! Inside/outside is tested with the **generalized winding number** (Jacobson
//! et al.), sampled on a coarse 3D grid. GWN is robust to the small holes,
//! coincident faces, and stray internal faces real kernel CSG meshes contain —
//! a plain even-odd ray cast is not. From the resulting voxel occupancy we read
//! the cavity (a column that is solid at the floor but open at the top), the
//! standoffs (pocket columns whose floor solid rises into a post), and the wall
//! cutouts (gaps in the wall ring).
//!
//! Assumes a Z-up, roughly box-shaped, open-top enclosure (the 3D-printed tray:
//! walls + floor + standoffs + side cutouts, lid mounting at the rim).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::fit::{EnclosureCavity, Standoff, Vec2, WallEdge, WallOpening};
use crate::round2;

/// Grid resolution in X/Y. Cheap enough for an interactive check, fine enough
/// to separate 5mm M3 posts and resolve a connector cutout.
const GRID_XY: usize = 48;
/// Grid resolution in Z.
const GRID_Z: usize = 28;
/// GWN magnitude above which a sample point counts as inside the solid.
const INSIDE: f64 = 0.5;
/// A post must rise at least this far above the floor to count as a standoff.
const MIN_POST_HEIGHT: f64 = 0.8;
/// Min contiguous open cells for a wall gap to count as a cutout.
const MIN_OPENING_CELLS: usize = 2;
/// Asymmetric sub-cell sample offsets so a sample never lands exactly on a
/// triangle vertex/edge (where GWN is ill-conditioned).
const JX: f64 = 0.5 + 0.137;
const JY: f64 = 0.5 - 0.077;
const JZ: f64 = 0.5 + 0.041;

/// Outer AABB of the analyzed mesh.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OuterBounds {
    /// Minimum X.
    #[serde(rename = "minX")]
    pub min_x: f64,
    /// Maximum X.
    #[serde(rename = "maxX")]
    pub max_x: f64,
    /// Minimum Y.
    #[serde(rename = "minY")]
    pub min_y: f64,
    /// Maximum Y.
    #[serde(rename = "maxY")]
    pub max_y: f64,
    /// Minimum Z.
    #[serde(rename = "minZ")]
    pub min_z: f64,
    /// Maximum Z.
    #[serde(rename = "maxZ")]
    pub max_z: f64,
}

/// Outer bounds + extracted interior features of an enclosure solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnclosureFeatures {
    /// Outer AABB of the solid.
    pub outer: OuterBounds,
    /// The interior void, or `None` when no open-top pocket is found.
    pub cavity: Option<EnclosureCavity>,
    /// Posts rising from the cavity floor.
    pub standoffs: Vec<Standoff>,
    /// Openings cut through the cavity walls.
    pub openings: Vec<WallOpening>,
}

/// One triangle's three vertices, gathered once so the per-sample inner loop
/// is a flat sequential walk (no index arithmetic, no bounds checks, and the
/// whole triangle in one cache line pair). The voxel pass evaluates every
/// triangle against every one of the ~64k cells, so this gather is amortized
/// tens of thousands of times over.
type Tri = [[f64; 3]; 3];

/// Gather triangles once, dropping any index that runs past the vertex buffer
/// (the TS implementation read `undefined` there, yielding NaN and poisoning
/// the whole sum — skipping is strictly better and never fires on valid input).
fn gather_tris(positions: &[f64], indices: &[u32]) -> Vec<Tri> {
    let vertex_count = positions.len() / 3;
    let mut tris = Vec::with_capacity(indices.len() / 3);
    for t in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if i0 >= vertex_count || i1 >= vertex_count || i2 >= vertex_count {
            continue;
        }
        let v = |i: usize| [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]];
        tris.push([v(i0), v(i1), v(i2)]);
    }
    tris
}

/// Generalized winding number of point `q` w.r.t. the mesh. ~1 (or −1 for
/// inverted winding) inside a closed region, ~0 outside; robust to holes and
/// stray faces. Van Oosterom–Strackee signed solid angle per triangle.
fn gwn(tris: &[Tri], qx: f64, qy: f64, qz: f64) -> f64 {
    let mut w = 0.0;
    for t in tris {
        let ax = t[0][0] - qx;
        let ay = t[0][1] - qy;
        let az = t[0][2] - qz;
        let bx = t[1][0] - qx;
        let by = t[1][1] - qy;
        let bz = t[1][2] - qz;
        let cx = t[2][0] - qx;
        let cy = t[2][1] - qy;
        let cz = t[2][2] - qz;
        let la = (ax * ax + ay * ay + az * az).sqrt();
        let lb = (bx * bx + by * by + bz * bz).sqrt();
        let lc = (cx * cx + cy * cy + cz * cz).sqrt();
        // a · (b × c)
        let cbx = by * cz - bz * cy;
        let cby = bz * cx - bx * cz;
        let cbz = bx * cy - by * cx;
        let num = ax * cbx + ay * cby + az * cbz;
        let den = la * lb * lc
            + (ax * bx + ay * by + az * bz) * lc
            + (bx * cx + by * cy + bz * cz) * la
            + (cx * ax + cy * ay + cz * az) * lb;
        w += num.atan2(den);
    }
    w / (2.0 * std::f64::consts::PI)
}

/// GWN voxel occupancy over the mesh's AABB.
struct Occupancy {
    gw: usize,
    gh: usize,
    gz: usize,
    ox: f64,
    oy: f64,
    oz: f64,
    dx: f64,
    dy: f64,
    dz: f64,
    min_z: f64,
    max_z: f64,
    /// Flat `[i + gw*(j + gh*k)]` occupancy, 1 = solid.
    occ: Vec<u8>,
}

/// Sample the GWN occupancy grid over the mesh's AABB.
fn build_occupancy(positions: &[f64], indices: &[u32]) -> Option<Occupancy> {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut min_z = f64::INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    for p in positions.as_chunks::<3>().0 {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
        min_z = min_z.min(p[2]);
        max_z = max_z.max(p[2]);
    }
    // Reject a degenerate or NaN-poisoned AABB. Written as "not strictly
    // greater" rather than "<=" so NaN bounds (an empty or non-finite mesh)
    // fall into the reject branch instead of comparing false everywhere.
    let degenerate = |lo: f64, hi: f64| !matches!(hi.partial_cmp(&lo), Some(Ordering::Greater));
    if degenerate(min_x, max_x) || degenerate(min_y, max_y) || degenerate(min_z, max_z) {
        return None;
    }
    let (gw, gh, gz) = (GRID_XY, GRID_XY, GRID_Z);
    let dx = (max_x - min_x) / gw as f64;
    let dy = (max_y - min_y) / gh as f64;
    let dz = (max_z - min_z) / gz as f64;
    let tris = gather_tris(positions, indices);
    let mut occ = vec![0u8; gw * gh * gz];
    for k in 0..gz {
        let qz = min_z + (k as f64 + JZ) * dz;
        for j in 0..gh {
            let qy = min_y + (j as f64 + JY) * dy;
            for i in 0..gw {
                let qx = min_x + (i as f64 + JX) * dx;
                if gwn(&tris, qx, qy, qz).abs() > INSIDE {
                    occ[i + gw * (j + gh * k)] = 1;
                }
            }
        }
    }
    Some(Occupancy {
        gw,
        gh,
        gz,
        ox: min_x,
        oy: min_y,
        oz: min_z,
        dx,
        dy,
        dz,
        min_z,
        max_z,
        occ,
    })
}

impl Occupancy {
    /// Z of the top of the bottom solid run in a column (`None` if no floor).
    fn bottom_run_top_z(&self, i: usize, j: usize) -> Option<f64> {
        let base = i + self.gw * j;
        if self.occ[base] != 1 {
            return None; // no floor under this column
        }
        let mut k = 1;
        while k < self.gz && self.occ[base + self.gw * self.gh * k] == 1 {
            k += 1;
        }
        // Top of the run is the boundary between cell k-1 (solid) and k (empty).
        Some(self.oz + k as f64 * self.dz)
    }

    /// True when the column's top cell is empty (open above).
    fn open_at_top(&self, i: usize, j: usize) -> bool {
        self.occ[i + self.gw * (j + self.gh * (self.gz - 1))] != 1
    }

    /// True when any cell in `(k_lo..=k_hi)` is empty — a gap through the column.
    fn has_gap(&self, i: usize, j: usize, k_lo: usize, k_hi: usize) -> bool {
        for k in k_lo..=k_hi {
            if self.occ[i + self.gw * (j + self.gh * k)] != 1 {
                return true;
            }
        }
        false
    }
}

/// Extract the cavity, standoffs, and wall openings from a solid mesh. Returns
/// `cavity: None` when no open-top pocket is found (e.g. a solid block).
pub fn extract_enclosure_features(positions: &[f64], indices: &[u32]) -> EnclosureFeatures {
    let Some(o) = build_occupancy(positions, indices) else {
        return EnclosureFeatures {
            outer: OuterBounds {
                min_x: 0.0,
                max_x: 0.0,
                min_y: 0.0,
                max_y: 0.0,
                min_z: 0.0,
                max_z: 0.0,
            },
            cavity: None,
            standoffs: vec![],
            openings: vec![],
        };
    };
    let outer = OuterBounds {
        min_x: o.ox,
        max_x: o.ox + o.gw as f64 * o.dx,
        min_y: o.oy,
        max_y: o.oy + o.gh as f64 * o.dy,
        min_z: o.min_z,
        max_z: o.max_z,
    };

    // Pocket columns: solid at the floor, open at the top.
    let mut is_pocket = vec![0u8; o.gw * o.gh];
    let mut first_tops: Vec<f64> = Vec::new();
    let mut count_x = vec![0i32; o.gw];
    let mut count_y = vec![0i32; o.gh];
    let mut pocket_count = 0usize;
    for j in 0..o.gh {
        for i in 0..o.gw {
            if let Some(top) = o.bottom_run_top_z(i, j) {
                if o.open_at_top(i, j) {
                    is_pocket[i + o.gw * j] = 1;
                    pocket_count += 1;
                    first_tops.push(top);
                    count_x[i] += 1;
                    count_y[j] += 1;
                }
            }
        }
    }
    if pocket_count == 0 {
        return EnclosureFeatures {
            outer,
            cavity: None,
            standoffs: vec![],
            openings: vec![],
        };
    }

    // Cavity bounds from per-axis occupancy profiles: keep the contiguous core
    // where pocket coverage is at least half the peak, trimming the thin notch
    // a wall cutout pokes into the interior (box-cavity assumption).
    let core = |counts: &[i32]| -> (usize, usize) {
        let peak = counts.iter().copied().max().unwrap_or(0);
        let thr = peak as f64 * 0.5;
        let mut lo = 0usize;
        let mut hi = counts.len() as i64 - 1;
        while lo < counts.len() && (counts[lo] as f64) < thr {
            lo += 1;
        }
        while hi >= 0 && (counts[hi as usize] as f64) < thr {
            hi -= 1;
        }
        (lo, hi.max(0) as usize)
    };
    let (pmin_i, pmax_i) = core(&count_x);
    let (pmin_j, pmax_j) = core(&count_y);

    // Floor Z: median first-run top over pocket columns (robust to the standoff
    // minority, whose first run tops out at the post).
    let mut sorted_tops = first_tops.clone();
    sorted_tops.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor_z = sorted_tops[sorted_tops.len() / 2];
    let ceil_z = o.max_z; // open-top tray: the lid mounts at the rim

    let cavity = EnclosureCavity {
        min_x: o.ox + pmin_i as f64 * o.dx,
        max_x: o.ox + (pmax_i + 1) as f64 * o.dx,
        min_y: o.oy + pmin_j as f64 * o.dy,
        max_y: o.oy + (pmax_j + 1) as f64 * o.dy,
        floor_z: round2(floor_z),
        ceil_z: round2(ceil_z),
        has_lid: false,
    };

    let standoffs = extract_standoffs(&o, &is_pocket, floor_z);
    let openings = extract_openings(&o, &cavity, floor_z, ceil_z);
    EnclosureFeatures {
        outer,
        cavity: Some(cavity),
        standoffs,
        openings,
    }
}

/// Cluster pocket columns whose floor solid rises above the floor into posts.
fn extract_standoffs(o: &Occupancy, is_pocket: &[u8], floor_z: f64) -> Vec<Standoff> {
    let mut is_post = vec![0u8; o.gw * o.gh];
    for j in 0..o.gh {
        for i in 0..o.gw {
            if is_pocket[i + o.gw * j] == 0 {
                continue;
            }
            if let Some(top) = o.bottom_run_top_z(i, j) {
                if top > floor_z + MIN_POST_HEIGHT {
                    is_post[i + o.gw * j] = 1;
                }
            }
        }
    }
    let mut seen = vec![0u8; o.gw * o.gh];
    let mut standoffs = Vec::new();
    for j in 0..o.gh {
        for i in 0..o.gw {
            let k0 = i + o.gw * j;
            if is_post[k0] == 0 || seen[k0] == 1 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(i, j)];
            seen[k0] = 1;
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut n = 0usize;
            let mut top_max = f64::NEG_INFINITY;
            let (mut min_i, mut max_i, mut min_j, mut max_j) = (i, i, j, j);
            while let Some((ci, cj)) = stack.pop() {
                sum_x += o.ox + (ci as f64 + 0.5) * o.dx;
                sum_y += o.oy + (cj as f64 + 0.5) * o.dy;
                n += 1;
                if let Some(t) = o.bottom_run_top_z(ci, cj) {
                    if t > top_max {
                        top_max = t;
                    }
                }
                min_i = min_i.min(ci);
                max_i = max_i.max(ci);
                min_j = min_j.min(cj);
                max_j = max_j.max(cj);
                let neighbors = [
                    (ci.wrapping_sub(1), cj),
                    (ci + 1, cj),
                    (ci, cj.wrapping_sub(1)),
                    (ci, cj + 1),
                ];
                for (ni, nj) in neighbors {
                    if ni >= o.gw || nj >= o.gh {
                        continue;
                    }
                    let nk = ni + o.gw * nj;
                    if is_post[nk] == 1 && seen[nk] == 0 {
                        seen[nk] = 1;
                        stack.push((ni, nj));
                    }
                }
            }
            // Single stray cells are noise, not posts.
            if n < 2 {
                continue;
            }
            let radius = (((max_i - min_i + 1) as f64 * o.dx + (max_j - min_j + 1) as f64 * o.dy)
                / 4.0)
                .max(o.dx);
            standoffs.push(Standoff {
                x: round2(sum_x / n as f64),
                y: round2(sum_y / n as f64),
                top_z: round2(top_max),
                radius: round2(radius),
            });
        }
    }
    standoffs
}

/// Detect openings in the four cavity walls: walk the cavity perimeter one
/// cell outside the pocket and find spans where the wall is absent at some
/// level between floor and rim (height-agnostic, so a low USB port reads the
/// same as a full-height slot).
fn extract_openings(
    o: &Occupancy,
    cavity: &EnclosureCavity,
    floor_z: f64,
    ceil_z: f64,
) -> Vec<WallOpening> {
    let p_min_i = ((cavity.min_x - o.ox) / o.dx).round() as i64;
    let p_max_i = ((cavity.max_x - o.ox) / o.dx).round() as i64 - 1;
    let p_min_j = ((cavity.min_y - o.oy) / o.dy).round() as i64;
    let p_max_j = ((cavity.max_y - o.oy) / o.dy).round() as i64 - 1;
    let k_lo = (((floor_z - o.oz) / o.dz).floor() as i64 + 1).max(0) as usize;
    let k_hi = (((ceil_z - o.oz) / o.dz).ceil() as i64 - 1).min(o.gz as i64 - 1) as usize;
    let mut openings = Vec::new();

    let wall_open = |i: i64, j: i64| -> bool {
        if i < 0 || j < 0 || i >= o.gw as i64 || j >= o.gh as i64 {
            return true;
        }
        o.has_gap(i as usize, j as usize, k_lo, k_hi)
    };

    struct Cell {
        open: bool,
        x: f64,
        y: f64,
        i: i64,
        j: i64,
    }
    let mut scans: Vec<(WallEdge, Vec<Cell>)> = Vec::new();
    for (edge, wi) in [(WallEdge::MinX, p_min_i - 1), (WallEdge::MaxX, p_max_i + 1)] {
        let mut cells = Vec::new();
        for j in p_min_j..=p_max_j {
            cells.push(Cell {
                open: wall_open(wi, j),
                x: o.ox + (wi as f64 + 0.5) * o.dx,
                y: o.oy + (j as f64 + 0.5) * o.dy,
                i: wi,
                j,
            });
        }
        scans.push((edge, cells));
    }
    for (edge, wj) in [(WallEdge::MinY, p_min_j - 1), (WallEdge::MaxY, p_max_j + 1)] {
        let mut cells = Vec::new();
        for i in p_min_i..=p_max_i {
            cells.push(Cell {
                open: wall_open(i, wj),
                x: o.ox + (i as f64 + 0.5) * o.dx,
                y: o.oy + (wj as f64 + 0.5) * o.dy,
                i,
                j: wj,
            });
        }
        scans.push((edge, cells));
    }

    for (edge, cells) in &scans {
        let mut run: Vec<&Cell> = Vec::new();
        let mut flush = |run: &mut Vec<&Cell>| {
            if run.len() >= MIN_OPENING_CELLS {
                let xs: Vec<f64> = run.iter().map(|p| p.x).collect();
                let ys: Vec<f64> = run.iter().map(|p| p.y).collect();
                let horiz = matches!(edge, WallEdge::MinY | WallEdge::MaxY);
                let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
                let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let center = Vec2 {
                    x: (min(&xs) + max(&xs)) / 2.0,
                    y: (min(&ys) + max(&ys)) / 2.0,
                };
                let width = if horiz {
                    max(&xs) - min(&xs) + o.dx
                } else {
                    max(&ys) - min(&ys) + o.dy
                };
                let mid = run[run.len() / 2];
                let (z_min, z_max) = opening_z_span(o, mid.i, mid.j, k_lo, k_hi);
                openings.push(WallOpening {
                    edge: *edge,
                    center: Vec2 {
                        x: round2(center.x),
                        y: round2(center.y),
                    },
                    width: round2(width),
                    z_min: round2(z_min),
                    z_max: round2(z_max),
                });
            }
            run.clear();
        };
        for c in cells {
            if c.open {
                run.push(c);
            } else {
                flush(&mut run);
            }
        }
        flush(&mut run);
    }
    openings
}

/// Vertical span of the open band at a wall column. A column outside the grid
/// (the cavity core reaching the AABB edge) reads as fully open.
fn opening_z_span(o: &Occupancy, i: i64, j: i64, k_lo: usize, k_hi: usize) -> (f64, f64) {
    let in_grid = i >= 0 && j >= 0 && i < o.gw as i64 && j < o.gh as i64;
    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    for k in k_lo..=k_hi {
        if !in_grid || o.occ[i as usize + o.gw * (j as usize + o.gh * k)] != 1 {
            let z = o.oz + (k as f64 + 0.5) * o.dz;
            z_min = z_min.min(z);
            z_max = z_max.max(z);
        }
    }
    if !z_min.is_finite() {
        return (o.oz, o.max_z);
    }
    (z_min, z_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxm(min: [f64; 3], max: [f64; 3]) -> (Vec<f64>, Vec<u32>) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let v = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        let faces: [u32; 36] = [
            0, 1, 2, 0, 2, 3, // bottom
            4, 6, 5, 4, 7, 6, // top
            0, 4, 5, 0, 5, 1, // -Y
            3, 2, 6, 3, 6, 7, // +Y
            0, 3, 7, 0, 7, 4, // -X
            1, 5, 6, 1, 6, 2, // +X
        ];
        (v.concat(), faces.to_vec())
    }

    fn merge(parts: &[(Vec<f64>, Vec<u32>)]) -> (Vec<f64>, Vec<u32>) {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        let mut off = 0u32;
        for (p, idx) in parts {
            positions.extend_from_slice(p);
            indices.extend(idx.iter().map(|i| i + off));
            off += (p.len() / 3) as u32;
        }
        (positions, indices)
    }

    /// An open-top tray: 40×40×12 outer, 2mm walls, 2mm floor, four M3
    /// standoffs on a 30.5mm pattern (posts top at z=5), and a 10mm
    /// full-height USB cutout in the +X wall centered at y=20. Mirrors the
    /// fixture in `packages/engine/src/__tests__/enclosure-fit.test.ts`.
    fn tray_mesh() -> (Vec<f64>, Vec<u32>) {
        let (w, d, h, t, fz) = (40.0, 40.0, 12.0, 2.0, 2.0);
        let post = |cx: f64, cy: f64| boxm([cx - 1.5, cy - 1.5, fz], [cx + 1.5, cy + 1.5, 5.0]);
        let c = 20.0;
        let half = 30.5 / 2.0; // 15.25
        merge(&[
            boxm([0.0, 0.0, 0.0], [w, d, fz]),    // floor
            boxm([0.0, 0.0, fz], [t, d, h]),      // -X wall
            boxm([w - t, 0.0, fz], [w, 15.0, h]), // +X wall, lower
            boxm([w - t, 25.0, fz], [w, d, h]),   // +X wall, upper (cutout y 15..25)
            boxm([t, 0.0, fz], [w - t, t, h]),    // -Y wall
            boxm([t, d - t, fz], [w - t, d, h]),  // +Y wall
            post(c - half, c - half),
            post(c + half, c - half),
            post(c - half, c + half),
            post(c + half, c + half),
        ])
    }

    #[test]
    fn gwn_of_cube_inside_and_outside() {
        let (p, i) = boxm([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        // Magnitude ~1 inside (sign depends on the fixture's winding).
        let tris = gather_tris(&p, &i);
        assert!((gwn(&tris, 5.0, 5.0, 5.0).abs() - 1.0).abs() < 1e-9);
        assert!(gwn(&tris, 20.0, 5.0, 5.0).abs() < 1e-9);
        assert!(gwn(&tris, -3.0, -3.0, -3.0).abs() < 1e-9);
        // Just outside a face is still clearly outside.
        assert!(gwn(&tris, 5.0, 5.0, 10.1).abs() < 0.4);
    }

    #[test]
    fn tray_features_match_ts_fixture() {
        let (p, i) = tray_mesh();
        let f = extract_enclosure_features(&p, &i);
        assert!((f.outer.max_z - 12.0).abs() < 0.05);
        let cav = f.cavity.expect("tray has a cavity");
        // Within ~1 grid cell of the true [2,38] pocket (GWN samples cell centers).
        let near = |got: f64, want: f64| (got - want).abs() <= 1.2;
        assert!(near(cav.min_x, 2.0), "minX {}", cav.min_x);
        assert!(near(cav.max_x, 38.0), "maxX {}", cav.max_x);
        assert!(near(cav.min_y, 2.0), "minY {}", cav.min_y);
        assert!(near(cav.max_y, 38.0), "maxY {}", cav.max_y);
        assert!((cav.floor_z - 2.0).abs() < 0.5, "floorZ {}", cav.floor_z);
        assert!((cav.ceil_z - 12.0).abs() < 0.5); // open top → rim height
        assert!(!cav.has_lid);

        assert_eq!(f.standoffs.len(), 4);
        for s in &f.standoffs {
            assert!((s.top_z - 5.0).abs() < 0.5, "topZ {}", s.top_z);
        }
        // The 30.5mm pattern centers (±15.25 about 20).
        let mut xs: Vec<f64> = f.standoffs.iter().map(|s| s.x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(xs[0] >= 3.5 && xs[0] <= 6.5, "min post x {}", xs[0]);

        assert_eq!(f.openings.len(), 1);
        let o = &f.openings[0];
        assert_eq!(o.edge, WallEdge::MaxX);
        assert!((o.center.y - 20.0).abs() < 0.5, "center.y {}", o.center.y);
        assert!(o.width > 8.0 && o.width < 12.0, "width {}", o.width);
    }

    #[test]
    fn solid_block_has_no_cavity() {
        let (p, i) = boxm([0.0, 0.0, 0.0], [20.0, 20.0, 10.0]);
        let f = extract_enclosure_features(&p, &i);
        assert!(f.cavity.is_none());
        assert!(f.standoffs.is_empty());
    }

    #[test]
    fn degenerate_mesh_yields_empty_features() {
        let f = extract_enclosure_features(&[], &[]);
        assert!(f.cavity.is_none());
        assert_eq!(f.outer.max_x, 0.0);
    }
}
