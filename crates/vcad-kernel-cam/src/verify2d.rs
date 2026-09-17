//! Replay a prismatic 2.5D job against the part it is meant to make.
//!
//! This is the oracle the app asks before it lets a job run: every move is
//! replayed against the target region and the answers are structured data, not
//! a picture. The checks are the ones that caught real defects on the first
//! cut (`docs/native-app-friction-log.md` items 32–41, 53, 54), each reported
//! with a pass flag, a count, the worst value and example locations:
//!
//! 1. [`JobVerification::gouge`] — a cutting move whose swept disc enters the
//!    part. This is the inside contour that offset outward (item 32).
//! 2. [`JobVerification::material_left`] — wall the full-depth passes never
//!    reached. Corners the cutter cannot physically reach are *not* counted
//!    here; they belong to [`crate::fit`] (item 38).
//! 3. [`JobVerification::rapids`] — G0 with XY motion below a safe height, or
//!    descending into material.
//! 4. [`JobVerification::depth`] — deepest Z against the stock thickness and
//!    the declared bottom allowance (items 40, 50), and every feature reaching
//!    its final depth.
//! 5. [`JobVerification::tabs`] — where the tabs are, how much metal each
//!    leaves, and whether every pass that goes below a tab steps over it
//!    (item 33).
//! 6. [`JobVerification::envelope`] — swept extents in work and machine
//!    coordinates, against travel limits, plus the stock margin the job needs
//!    (item 41).
//! 7. [`JobVerification::loose`] — stock the job frees completely: an inside
//!    slug, or the wedges a slot mouth sheds when two passes overlap (item 39).
//! 8. [`JobVerification::plunges`] — vertical entries into uncut material.
//!
//! Inputs are a [`Toolpath`] ([`verify_toolpath`]) or G-code text
//! ([`verify_gcode`]). The G-code reader is deliberately strict: a word it
//! does not understand is an error, never a skipped line, because a skipped
//! line is a move the oracle silently did not check.
//!
//! Frame: mm, Z up, stock top at Z0, stock XY lower-left at the origin.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use crate::fit::{offset_loop, OffsetOptions};
use crate::{Toolpath, ToolpathSegment};

/// A closed 2D loop. The closing edge from the last point back to the first is
/// implicit; a repeated last point is dropped on construction.
pub type Loop2 = Vec<[f64; 2]>;

/// Things the oracle refuses to guess at.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum VerifyError {
    /// A loop has fewer than three distinct points.
    #[error("a contour loop needs at least 3 distinct points, got {0}")]
    DegenerateLoop(usize),

    /// A number in the job description is not usable.
    #[error("{0}")]
    BadInput(String),

    /// The toolpath uses something this 2D oracle cannot replay.
    #[error("cannot replay: {0}")]
    Unsupported(String),

    /// The G-code reader met something it does not understand.
    #[error("G-code line {line}: {what}")]
    Gcode {
        /// 1-based line number.
        line: usize,
        /// What could not be understood.
        what: String,
    },
}

// ---------------------------------------------------------------------------
// 2D geometry core
// ---------------------------------------------------------------------------

/// Squared length of a 2D vector.
fn len2(v: [f64; 2]) -> f64 {
    v[0] * v[0] + v[1] * v[1]
}

/// Distance from `p` to the segment `a`–`b`.
pub(crate) fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = len2(ab);
    let t = if l2 <= 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
    };
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    (p[0] - q[0]).hypot(p[1] - q[1])
}

/// Do the segments `a`–`b` and `c`–`d` cross (or touch)?
fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return true;
    }
    // Collinear/touching cases: fall back to a distance test.
    let on = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| point_segment_distance(r, p, q) <= 0.0;
    on(c, d, a) || on(c, d, b) || on(a, b, c) || on(a, b, d)
}

/// Distance between two segments; zero when they cross.
fn segment_segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

/// Twice the signed area of a closed loop (positive when counter-clockwise).
pub(crate) fn signed_area2(points: &[[f64; 2]]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (p, q) = (points[i], points[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum()
}

/// Signed area of a closed loop (positive when counter-clockwise).
pub(crate) fn signed_area(points: &[[f64; 2]]) -> f64 {
    signed_area2(points) * 0.5
}

/// Drop a repeated closing point and consecutive duplicates.
pub(crate) fn clean_loop(points: &[[f64; 2]]) -> Loop2 {
    let mut out: Loop2 = Vec::with_capacity(points.len());
    for p in points {
        if out
            .last()
            .is_some_and(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < 1e-12)
        {
            continue;
        }
        out.push(*p);
    }
    while out.len() > 1 {
        let (a, b) = (out[0], out[out.len() - 1]);
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-12 {
            out.pop();
        } else {
            break;
        }
    }
    out
}

/// A uniform-grid index over the edges of a set of closed loops.
///
/// Answers the two questions every check asks — "how far is this from the
/// boundary" and "is this inside" — in time that does not grow with the loop
/// count, which is what makes the area grids affordable.
pub(crate) struct Poly {
    loops: Vec<Loop2>,
    /// `(a, b)` endpoints of every edge, in loop order.
    edges: Vec<([f64; 2], [f64; 2])>,
    bbox: [f64; 4],
    cell: f64,
    nx: usize,
    ny: usize,
    buckets: Vec<Vec<u32>>,
    /// Edge ids per cell row, for the point-in-polygon ray cast.
    rows: Vec<Vec<u32>>,
}

impl Poly {
    /// Index a set of closed loops. Loops are cleaned; degenerate ones are
    /// rejected rather than silently dropped.
    pub(crate) fn new(loops: Vec<Loop2>) -> Result<Self, VerifyError> {
        let loops: Vec<Loop2> = loops.iter().map(|l| clean_loop(l)).collect();
        for l in &loops {
            if l.len() < 3 {
                return Err(VerifyError::DegenerateLoop(l.len()));
            }
        }
        let mut edges = Vec::new();
        let mut bbox = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for l in &loops {
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                edges.push((a, b));
                for p in [a, b] {
                    bbox[0] = bbox[0].min(p[0]);
                    bbox[1] = bbox[1].min(p[1]);
                    bbox[2] = bbox[2].max(p[0]);
                    bbox[3] = bbox[3].max(p[1]);
                }
            }
        }
        Ok(Self::index(loops, edges, bbox))
    }

    /// Index a bare set of segments (a swept path, not a region): only the
    /// distance queries are meaningful.
    pub(crate) fn from_segments(edges: Vec<([f64; 2], [f64; 2])>) -> Self {
        let mut bbox = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for (a, b) in &edges {
            for p in [a, b] {
                bbox[0] = bbox[0].min(p[0]);
                bbox[1] = bbox[1].min(p[1]);
                bbox[2] = bbox[2].max(p[0]);
                bbox[3] = bbox[3].max(p[1]);
            }
        }
        if edges.is_empty() {
            bbox = [0.0, 0.0, 0.0, 0.0];
        }
        Self::index(Vec::new(), edges, bbox)
    }

    fn index(loops: Vec<Loop2>, edges: Vec<([f64; 2], [f64; 2])>, bbox: [f64; 4]) -> Self {
        let n = edges.len().max(1) as f64;
        let (w, h) = ((bbox[2] - bbox[0]).max(1e-9), (bbox[3] - bbox[1]).max(1e-9));
        // About one edge per cell. The area rule alone collapses when the
        // segments are collinear — a set of tab stretches along one straight
        // wall has zero height, which made the cells microscopic and the ring
        // search effectively endless — so the longer span and the span itself
        // both put a floor under it.
        let span = w.max(h);
        let mut cell = (w * h / n).sqrt().max(span / n).max(span * 1e-4).max(1e-9);
        let mut nx = ((w / cell).ceil() as usize).max(1);
        let mut ny = ((h / cell).ceil() as usize).max(1);
        while nx.saturating_mul(ny) > 1 << 22 {
            cell *= 2.0;
            nx = ((w / cell).ceil() as usize).max(1);
            ny = ((h / cell).ceil() as usize).max(1);
        }
        let mut me = Self {
            loops,
            edges,
            bbox,
            cell,
            nx,
            ny,
            buckets: vec![Vec::new(); nx * ny],
            rows: vec![Vec::new(); ny],
        };
        for (id, (a, b)) in me.edges.iter().enumerate() {
            let (ix0, iy0) = me.cell_of([a[0].min(b[0]), a[1].min(b[1])]);
            let (ix1, iy1) = me.cell_of([a[0].max(b[0]), a[1].max(b[1])]);
            for iy in iy0..=iy1 {
                for ix in ix0..=ix1 {
                    me.buckets[iy * nx + ix].push(id as u32);
                }
                me.rows[iy].push(id as u32);
            }
        }
        me
    }

    fn cell_of(&self, p: [f64; 2]) -> (usize, usize) {
        let ix = ((p[0] - self.bbox[0]) / self.cell).floor();
        let iy = ((p[1] - self.bbox[1]) / self.cell).floor();
        (
            (ix.max(0.0) as usize).min(self.nx - 1),
            (iy.max(0.0) as usize).min(self.ny - 1),
        )
    }

    /// Bounding box `[min_x, min_y, max_x, max_y]`.
    pub(crate) fn bbox(&self) -> [f64; 4] {
        self.bbox
    }

    /// Unsigned distance from `p` to the nearest edge.
    pub(crate) fn distance(&self, p: [f64; 2]) -> f64 {
        if self.edges.is_empty() {
            return f64::INFINITY;
        }
        let (cx, cy) = self.cell_of(p);
        // Distance from p to the indexed area: rings are measured from the
        // clamped anchor, so a query outside the grid still gets a valid bound.
        let clamped = [
            p[0].clamp(self.bbox[0], self.bbox[2]),
            p[1].clamp(self.bbox[1], self.bbox[3]),
        ];
        let outside = (p[0] - clamped[0]).hypot(p[1] - clamped[1]);
        let mut best = f64::INFINITY;
        // Past this every ring is entirely out of bounds, wherever the anchor
        // sits.
        let max_ring = self.nx.max(self.ny);
        for k in 0..=max_ring {
            let ring_min = ((k as f64 - 1.0).max(0.0) * self.cell).hypot(outside);
            if ring_min > best {
                break;
            }
            let mut any = false;
            let x0 = cx as isize - k as isize;
            let x1 = cx as isize + k as isize;
            let y0 = cy as isize - k as isize;
            let y1 = cy as isize + k as isize;
            for iy in y0..=y1 {
                if iy < 0 || iy >= self.ny as isize {
                    continue;
                }
                for ix in x0..=x1 {
                    if ix < 0 || ix >= self.nx as isize {
                        continue;
                    }
                    // Ring k is the cells at Chebyshev distance exactly k.
                    if k > 0 && ix != x0 && ix != x1 && iy != y0 && iy != y1 {
                        continue;
                    }
                    any = true;
                    for &id in &self.buckets[iy as usize * self.nx + ix as usize] {
                        let (a, b) = self.edges[id as usize];
                        best = best.min(point_segment_distance(p, a, b));
                    }
                }
            }
            if !any && k > self.nx.max(self.ny) {
                break;
            }
        }
        best
    }

    /// Even-odd containment test against all loops.
    pub(crate) fn contains(&self, p: [f64; 2]) -> bool {
        if self.loops.is_empty() {
            return false;
        }
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3]
        {
            return false;
        }
        let (_, iy) = self.cell_of(p);
        let mut inside = false;
        for &id in &self.rows[iy] {
            let (a, b) = self.edges[id as usize];
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let t = (p[1] - a[1]) / (b[1] - a[1]);
                if a[0] + t * (b[0] - a[0]) > p[0] {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// Signed distance: positive inside the region, negative outside.
    pub(crate) fn signed_distance(&self, p: [f64; 2]) -> f64 {
        let d = self.distance(p);
        if self.contains(p) {
            d
        } else {
            -d
        }
    }

    /// Distance from the segment `a`–`b` to the nearest edge; zero when the
    /// segment crosses one.
    pub(crate) fn distance_to_segment(&self, a: [f64; 2], b: [f64; 2]) -> f64 {
        if self.edges.is_empty() {
            return f64::INFINITY;
        }
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let chunks = ((len / (4.0 * self.cell)).ceil() as usize).max(1);
        let mut best = f64::INFINITY;
        for c in 0..chunks {
            let (t0, t1) = (c as f64 / chunks as f64, (c + 1) as f64 / chunks as f64);
            let p = [a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0];
            let q = [a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1];
            let mid = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
            // Anything nearer than this bound lies within `bound` of the chunk,
            // so its cell overlaps the inflated box.
            let bound = self.distance(mid).min(best);
            if bound <= 0.0 {
                return 0.0;
            }
            let lo = [p[0].min(q[0]) - bound, p[1].min(q[1]) - bound];
            let hi = [p[0].max(q[0]) + bound, p[1].max(q[1]) + bound];
            let (ix0, iy0) = self.cell_of(lo);
            let (ix1, iy1) = self.cell_of(hi);
            for iy in iy0..=iy1 {
                for ix in ix0..=ix1 {
                    for &id in &self.buckets[iy * self.nx + ix] {
                        let (c, d) = self.edges[id as usize];
                        best = best.min(segment_segment_distance(p, q, c, d));
                        if best <= 0.0 {
                            return 0.0;
                        }
                    }
                }
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// Area measurement on an implicit field
// ---------------------------------------------------------------------------

/// One connected region of `{phi > 0}` found by [`march`].
#[derive(Debug, Clone)]
pub(crate) struct FieldRegion {
    /// Area in mm².
    pub area: f64,
    /// Area-weighted centroid.
    pub centroid: [f64; 2],
    /// Largest auxiliary value seen on the region's positive nodes.
    pub peak_aux: f64,
    /// Where that peak sits.
    pub peak_at: [f64; 2],
    /// Largest second auxiliary value, for a question the first channel is
    /// already answering — "does this region reach the stock edge" and "does
    /// it carry part material" are both needed, and neither can be settled
    /// from a centroid: the centroid of a frame that rings the part sits in
    /// the middle of the part.
    pub peak_aux2: f64,
    /// The region reaches the border of the sampled box.
    pub touches_border: bool,
}

/// Measure `{phi > 0}` over a box: area, centroid and connectivity of each
/// region, by marching squares on a regular grid.
///
/// `f` must return `(phi, aux)` with `phi` 1-Lipschitz — a minimum of signed
/// distances is, which is how every field here is built. That lets whole
/// coarse blocks be decided from their centre alone, so the cost follows the
/// length of the zero set rather than the area of the box.
pub(crate) fn march<F>(bbox: [f64; 4], h: f64, f: F) -> Vec<FieldRegion>
where
    F: Fn([f64; 2]) -> (f64, f64),
{
    march2(bbox, h, |p| {
        let (phi, aux) = f(p);
        (phi, aux, f64::NEG_INFINITY)
    })
}

/// [`march`], with a second auxiliary channel.
pub(crate) fn march2<F>(bbox: [f64; 4], h: f64, f: F) -> Vec<FieldRegion>
where
    F: Fn([f64; 2]) -> (f64, f64, f64),
{
    const BIG: f64 = 1.0e9;
    let nx = (((bbox[2] - bbox[0]) / h).ceil() as usize).max(1);
    let ny = (((bbox[3] - bbox[1]) / h).ceil() as usize).max(1);
    let (mx, my) = (nx + 1, ny + 1);
    let at = |ix: usize, iy: usize| [bbox[0] + ix as f64 * h, bbox[1] + iy as f64 * h];

    // Coarse classification: -1 all negative, 1 all positive, 0 mixed.
    let cf = 8usize;
    let (cnx, cny) = (nx.div_ceil(cf), ny.div_ceil(cf));
    let c_size = cf as f64 * h;
    let half_diag = c_size * std::f64::consts::SQRT_2 / 2.0;
    let mut coarse = vec![0i8; cnx * cny];
    for cy in 0..cny {
        for cx in 0..cnx {
            let centre = [
                bbox[0] + (cx as f64 + 0.5) * c_size,
                bbox[1] + (cy as f64 + 0.5) * c_size,
            ];
            let (phi, _, _) = f(centre);
            coarse[cy * cnx + cx] = if phi > half_diag {
                1
            } else if phi < -half_diag {
                -1
            } else {
                0
            };
        }
    }

    let mut phi = vec![f64::NAN; mx * my];
    let mut aux = vec![f64::NEG_INFINITY; mx * my];
    let mut aux2 = vec![f64::NEG_INFINITY; mx * my];
    for iy in 0..my {
        for ix in 0..mx {
            // The coarse cells this node belongs to (a node on a block edge
            // belongs to both).
            let cxs = [ix.saturating_sub(1) / cf, (ix / cf).min(cnx - 1)];
            let cys = [iy.saturating_sub(1) / cf, (iy / cf).min(cny - 1)];
            let mut decided = 0i8;
            let mut mixed = false;
            for &cy in &cys {
                for &cx in &cxs {
                    match coarse[cy.min(cny - 1) * cnx + cx.min(cnx - 1)] {
                        0 => mixed = true,
                        s => decided = s,
                    }
                }
            }
            let v = if mixed {
                let p = at(ix, iy);
                let (a, b, c) = f(p);
                aux[iy * mx + ix] = b;
                aux2[iy * mx + ix] = c;
                a
            } else if decided > 0 {
                BIG
            } else {
                -BIG
            };
            phi[iy * mx + ix] = v;
        }
    }

    // Union-find over positive nodes, 4-connected: two adjacent positive nodes
    // are joined by a segment that stays positive under linear interpolation.
    let mut parent: Vec<u32> = (0..(mx * my) as u32).collect();
    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    let union = |parent: &mut Vec<u32>, a: u32, b: u32| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[rb as usize] = ra;
        }
    };
    for iy in 0..my {
        for ix in 0..mx {
            let i = iy * mx + ix;
            if phi[i] <= 0.0 {
                continue;
            }
            if ix + 1 < mx && phi[i + 1] > 0.0 {
                union(&mut parent, i as u32, (i + 1) as u32);
            }
            if iy + 1 < my && phi[i + mx] > 0.0 {
                union(&mut parent, i as u32, (i + mx) as u32);
            }
        }
    }

    let mut acc: HashMap<u32, FieldRegion> = HashMap::new();
    let add = |acc: &mut HashMap<u32, FieldRegion>,
               root: u32,
               area: f64,
               cx: f64,
               cy: f64,
               border: bool| {
        let e = acc.entry(root).or_insert(FieldRegion {
            area: 0.0,
            centroid: [0.0, 0.0],
            peak_aux: f64::NEG_INFINITY,
            peak_at: [0.0, 0.0],
            peak_aux2: f64::NEG_INFINITY,
            touches_border: false,
        });
        e.area += area;
        e.centroid[0] += cx * area;
        e.centroid[1] += cy * area;
        e.touches_border |= border;
    };

    for iy in 0..ny {
        for ix in 0..nx {
            let idx = [
                iy * mx + ix,
                iy * mx + ix + 1,
                (iy + 1) * mx + ix + 1,
                (iy + 1) * mx + ix,
            ];
            let v = [phi[idx[0]], phi[idx[1]], phi[idx[2]], phi[idx[3]]];
            let pos = v.iter().filter(|x| **x > 0.0).count();
            if pos == 0 {
                continue;
            }
            let corner = [
                at(ix, iy),
                at(ix + 1, iy),
                at(ix + 1, iy + 1),
                at(ix, iy + 1),
            ];
            let border = ix == 0 || iy == 0 || ix + 1 == nx || iy + 1 == ny;
            if pos == 4 {
                let c = [corner[0][0] + h / 2.0, corner[0][1] + h / 2.0];
                add(
                    &mut acc,
                    find(&mut parent, idx[0] as u32),
                    h * h,
                    c[0],
                    c[1],
                    border,
                );
                continue;
            }
            let lerp = |a: usize, b: usize| {
                let t = v[a] / (v[a] - v[b]);
                [
                    corner[a][0] + (corner[b][0] - corner[a][0]) * t,
                    corner[a][1] + (corner[b][1] - corner[a][1]) * t,
                ]
            };
            // Saddle: the two positive corners are diagonal, so the positive
            // set is two separate triangles inside this cell.
            let saddle = pos == 2 && ((v[0] > 0.0) == (v[2] > 0.0));
            if saddle {
                for c in [0usize, 1] {
                    let k = if v[0] > 0.0 { c * 2 } else { c * 2 + 1 };
                    let prev = (k + 3) % 4;
                    let next = (k + 1) % 4;
                    let tri = [corner[k], lerp(k, next), lerp(k, prev)];
                    let (a, cent) = poly_area_centroid(&tri);
                    add(
                        &mut acc,
                        find(&mut parent, idx[k] as u32),
                        a,
                        cent[0],
                        cent[1],
                        border,
                    );
                }
                continue;
            }
            let mut poly: Vec<[f64; 2]> = Vec::with_capacity(6);
            let mut root = None;
            for k in 0..4 {
                let n = (k + 1) % 4;
                if v[k] > 0.0 {
                    poly.push(corner[k]);
                    root.get_or_insert_with(|| find(&mut parent, idx[k] as u32));
                }
                if (v[k] > 0.0) != (v[n] > 0.0) {
                    poly.push(lerp(k, n));
                }
            }
            let (a, cent) = poly_area_centroid(&poly);
            if let Some(root) = root {
                add(&mut acc, root, a, cent[0], cent[1], border);
            }
        }
    }

    // Peak auxiliary values per region, from the evaluated positive nodes.
    let mut peaks: HashMap<u32, (f64, [f64; 2], f64)> = HashMap::new();
    for iy in 0..my {
        for ix in 0..mx {
            let i = iy * mx + ix;
            if phi[i] <= 0.0 || aux[i] == f64::NEG_INFINITY {
                continue;
            }
            let root = find(&mut parent, i as u32);
            let e = peaks
                .entry(root)
                .or_insert((f64::NEG_INFINITY, [0.0, 0.0], f64::NEG_INFINITY));
            if aux[i] > e.0 {
                e.0 = aux[i];
                e.1 = at(ix, iy);
            }
            e.2 = e.2.max(aux2[i]);
        }
    }
    let mut out: Vec<FieldRegion> = acc
        .into_iter()
        .map(|(root, mut r)| {
            if r.area > 0.0 {
                r.centroid[0] /= r.area;
                r.centroid[1] /= r.area;
            }
            if let Some((p, at, p2)) = peaks.get(&root) {
                r.peak_aux = *p;
                r.peak_at = *at;
                r.peak_aux2 = *p2;
            }
            r
        })
        .filter(|r| r.area > 0.0)
        .collect();
    out.sort_by(|a, b| b.area.total_cmp(&a.area));
    out
}

/// Area and centroid of a simple polygon.
fn poly_area_centroid(points: &[[f64; 2]]) -> (f64, [f64; 2]) {
    let n = points.len();
    if n < 3 {
        return (0.0, [0.0, 0.0]);
    }
    let mut a2 = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let (p, q) = (points[i], points[(i + 1) % n]);
        let cross = p[0] * q[1] - q[0] * p[1];
        a2 += cross;
        cx += (p[0] + q[0]) * cross;
        cy += (p[1] + q[1]) * cross;
    }
    if a2.abs() < 1e-18 {
        return (0.0, points[0]);
    }
    (a2.abs() / 2.0, [cx / (3.0 * a2), cy / (3.0 * a2)])
}

// ---------------------------------------------------------------------------
// The job description
// ---------------------------------------------------------------------------

/// The prismatic target: an outer loop minus its holes, in the stock frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartRegion {
    /// Outer boundary of the material to keep.
    pub outer: Loop2,
    /// Openings in it. Each is a closed loop lying inside `outer`.
    pub holes: Vec<Loop2>,
}

impl PartRegion {
    /// Build a region, cleaning the loops and orienting the outer one
    /// counter-clockwise and the holes clockwise.
    pub fn new(outer: Loop2, holes: Vec<Loop2>) -> Result<Self, VerifyError> {
        let mut outer = clean_loop(&outer);
        if outer.len() < 3 {
            return Err(VerifyError::DegenerateLoop(outer.len()));
        }
        if signed_area(&outer) < 0.0 {
            outer.reverse();
        }
        let mut out_holes = Vec::with_capacity(holes.len());
        for h in holes {
            let mut h = clean_loop(&h);
            if h.len() < 3 {
                return Err(VerifyError::DegenerateLoop(h.len()));
            }
            if signed_area(&h) > 0.0 {
                h.reverse();
            }
            out_holes.push(h);
        }
        Ok(Self {
            outer,
            holes: out_holes,
        })
    }

    /// Bounding box of the outer loop: `[min_x, min_y, max_x, max_y]`.
    pub fn bbox(&self) -> [f64; 4] {
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for p in &self.outer {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        b
    }

    /// Plan area of the material to keep (mm²).
    pub fn area(&self) -> f64 {
        signed_area(&self.outer) + self.holes.iter().map(|h| signed_area(h)).sum::<f64>()
    }

    /// Move the region by `(dx, dy)`; used to put a part-frame outline into the
    /// stock frame.
    pub fn translated(&self, dx: f64, dy: f64) -> Self {
        let t = |l: &Loop2| l.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
        Self {
            outer: t(&self.outer),
            holes: self.holes.iter().map(t).collect(),
        }
    }

    fn poly(&self) -> Result<Poly, VerifyError> {
        let mut loops = vec![self.outer.clone()];
        loops.extend(self.holes.iter().cloned());
        Poly::new(loops)
    }
}

/// Machine travel limits, in machine coordinates (mm).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TravelLimits {
    /// Lower corner.
    pub min: [f64; 3],
    /// Upper corner.
    pub max: [f64; 3],
}

/// A tab the job says it placed, for cross-checking against what the moves do.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeclaredTab {
    /// Metal width the tab is meant to leave (mm).
    pub width: f64,
    /// Tab height above the floor (mm).
    pub height: f64,
}

/// Everything the oracle needs beyond the moves themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// The part, in the stock frame.
    pub part: PartRegion,
    /// Stock thickness (mm). The stock top is Z0.
    pub stock_thickness: f64,
    /// Stock outline `[min_x, min_y, max_x, max_y]`. Defaults to the part
    /// bounds grown by one tool diameter.
    pub stock_bbox: Option<[f64; 4]>,
    /// Cutter diameter (mm).
    pub tool_diameter: f64,
    /// Material left under the part (mm): positive is an onion skin, negative
    /// is a deliberate break-through into whatever is under the stock.
    pub bottom_allowance: f64,
    /// A sacrificial bed is declared, so cutting past the stock underside is
    /// allowed.
    pub spoilboard: bool,
    /// Work zero in machine coordinates, for the envelope check.
    pub work_offset: Option<[f64; 3]>,
    /// Machine travel limits, in machine coordinates.
    pub travel: Option<TravelLimits>,
    /// Tabs the job says it placed.
    pub declared_tabs: Vec<DeclaredTab>,
    /// The cutter can plunge straight down (centre-cutting).
    pub centre_cutting: bool,
}

impl JobSpec {
    /// A job with no bed, no machine and no declared tabs.
    pub fn new(part: PartRegion, stock_thickness: f64, tool_diameter: f64) -> Self {
        Self {
            part,
            stock_thickness,
            stock_bbox: None,
            tool_diameter,
            bottom_allowance: 0.0,
            spoilboard: false,
            work_offset: None,
            travel: None,
            declared_tabs: Vec::new(),
            centre_cutting: true,
        }
    }

    /// Z of the floor the job is meant to reach (negative, from the stock top).
    pub fn floor_z(&self) -> f64 {
        -(self.stock_thickness - self.bottom_allowance)
    }

    /// The blank, `[min_x, min_y, max_x, max_y]`.
    ///
    /// Defaulting this to the part bounds plus one tool diameter put the stock
    /// edge exactly where an outside contour's sweep ends, so the job appeared
    /// to cut its own frame into four free corners. The default margin is
    /// three diameters, which leaves two of connected frame all the way round
    /// — enough to tell a real freed piece from the edge of a made-up blank.
    /// Set `stock_bbox` for a real job: this is a stand-in, not a measurement.
    pub fn stock(&self) -> [f64; 4] {
        self.stock_bbox.unwrap_or_else(|| {
            let b = self.part.bbox();
            let m = 3.0 * self.tool_diameter;
            [b[0] - m, b[1] - m, b[2] + m, b[3] + m]
        })
    }

    /// True when `stock_bbox` was given rather than assumed.
    pub fn stock_is_declared(&self) -> bool {
        self.stock_bbox.is_some()
    }
}

/// Thresholds and sampling settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyOptions {
    /// How far the swept cutter may enter the part before it is a gouge (mm).
    /// Polylined offsets legitimately deviate by 0.013–0.018 mm.
    pub tolerance: f64,
    /// Least metal a tab may leave (mm).
    pub min_tab_metal: f64,
    /// Height above the stock top a rapid must clear before it moves in XY.
    pub safe_rapid_z: f64,
    /// Plunge feed above which a straight entry into material is flagged.
    pub max_plunge_feed: f64,
    /// Cap on example locations kept per check.
    pub max_examples: usize,
    /// Sagitta used when an arc is sampled into segments (mm).
    pub arc_tolerance: f64,
    /// Grid pitch for the area checks (mm).
    pub grid: f64,
    /// Slack on depth comparisons (mm).
    pub depth_tolerance: f64,
    /// Regions smaller than this are grid noise, not metal (mm²).
    pub min_area: f64,
    /// How close a pass has to come to a tab's lifted stretch before it counts
    /// as running through that tab, as a multiple of the tool radius. A pass
    /// that goes below a tab's top somewhere else on the job never touched it.
    pub tab_reach: f64,
    /// Severity of a freed region that carries part material: the part itself
    /// coming loose with nothing holding it.
    pub loose_part_severity: Severity,
    /// Severity of a freed region of waste — an inside slug, a slot wedge.
    /// It will rattle and it can be thrown, but the part survives it, so
    /// whether that blocks a job is the shop's call.
    pub loose_waste_severity: Severity,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.02,
            min_tab_metal: 1.5,
            safe_rapid_z: 0.5,
            max_plunge_feed: 500.0,
            max_examples: 8,
            arc_tolerance: 0.005,
            grid: 0.05,
            depth_tolerance: 0.01,
            min_area: 0.01,
            tab_reach: 1.0,
            loose_part_severity: Severity::Error,
            loose_waste_severity: Severity::Warning,
        }
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// Whether a failed check blocks the job or only warns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// The job must not run.
    Error,
    /// Worth saying, but not a blocker.
    Warning,
}

/// One offending place, with enough detail to find it in the program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    /// G-code line number, or toolpath segment index.
    pub index: usize,
    /// Where in the stock frame (mm).
    pub xy: [f64; 2],
    /// Z there (mm).
    pub z: f64,
    /// The measured value that broke the rule (mm, or mm/min for feeds).
    pub value: f64,
    /// What went wrong, in a machinist's words.
    pub what: String,
}

/// The outcome of one check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    /// Short name, e.g. `"gouge"`.
    pub name: String,
    /// True when nothing was found.
    pub pass: bool,
    /// Does a failure block the job?
    pub severity: Severity,
    /// How many violations.
    pub violation_count: usize,
    /// Worst value seen (mm, or mm/min); 0 when clean.
    pub worst: f64,
    /// Up to `max_examples` violations, worst first.
    pub examples: Vec<Violation>,
    /// What was measured, and against what.
    pub note: String,
}

impl CheckReport {
    fn new(name: &str, severity: Severity, note: String) -> Self {
        Self {
            name: name.to_string(),
            pass: true,
            severity,
            violation_count: 0,
            worst: 0.0,
            examples: Vec::new(),
            note,
        }
    }

    fn hit(&mut self, v: Violation, max_examples: usize) {
        self.pass = false;
        self.violation_count += 1;
        self.worst = self.worst.max(v.value);
        self.examples.push(v);
        self.examples.sort_by(|a, b| b.value.total_cmp(&a.value));
        self.examples.truncate(max_examples);
    }

    fn blocks(&self) -> bool {
        !self.pass && self.severity == Severity::Error
    }
}

/// Wall the full-depth passes never reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialLeftReport {
    /// The check itself; one violation per leftover region.
    pub check: CheckReport,
    /// Total unswept area in the wall bands (mm²).
    pub unswept_area: f64,
    /// Farthest any leftover stands off its wall (mm).
    pub max_standoff: f64,
    /// Area of wall band the cutter could reach at all (mm²).
    pub reachable_band_area: f64,
    /// Walls no full-depth pass came within a tool diameter of. Not a
    /// violation — the job may not be meant to cut them — but the caller
    /// should say so out loud.
    pub untouched_walls: usize,
    /// Walls the part has that a cutter this size could follow at all.
    pub walls: usize,
    /// Metal left under the observed tabs (mm²). Excluded from
    /// `unswept_area`: a tab is metal the job meant to leave.
    pub tab_area: f64,
}

/// Depth against the stock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthReport {
    /// The check itself.
    pub check: CheckReport,
    /// Deepest Z reached (mm, negative).
    pub deepest_z: f64,
    /// Z the job is meant to reach.
    pub floor_z: f64,
    /// Material left under the part at the deepest point (mm); negative means
    /// the cutter went past the stock underside.
    pub remaining_under_part: f64,
    /// How many separate features (groups of passes) were found.
    pub features: usize,
}

/// One lifted stretch of one pass: a tab, as the moves actually cut it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabObservation {
    /// Middle of the lifted stretch (stock frame, mm).
    pub xy: [f64; 2],
    /// Where the cutter left the floor (mm).
    pub start: [f64; 2],
    /// Where it came back down (mm).
    pub end: [f64; 2],
    /// The lifted stretch itself, in order. The metal a tab leaves sits under
    /// this, and on a curved wall a chord from `start` to `end` misses it by
    /// the sagitta — enough to leave a sliver of phantom "uncut wall" behind.
    pub path: Vec<[f64; 2]>,
    /// Z the cutter was held at (mm).
    pub top_z: f64,
    /// Z the pass was cutting at (mm).
    pub pass_z: f64,
    /// Length the cutter travelled lifted (mm).
    pub lifted_run: f64,
    /// Metal actually left: the lifted run less one tool diameter (mm).
    pub metal_width: f64,
    /// Height above the floor (mm).
    pub height: f64,
    /// Chord over run of the lifted stretch; 1.0 is dead straight.
    pub straightness: f64,
    /// The stretch runs straight enough to clean off easily.
    pub straight: bool,
    /// Which pass (0-based, in program order).
    pub pass_index: usize,
}

/// Where the tabs are and what they hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabAudit {
    /// The check itself.
    pub check: CheckReport,
    /// Every lifted stretch found, over every pass.
    pub observations: Vec<TabObservation>,
    /// Distinct tab positions (clustered across passes).
    pub tab_count: usize,
    /// Passes that had to step over a tab: they go below its top *and* their
    /// path runs through it. A pass that dives deeper somewhere else on the
    /// job — another operation, another feature — is not one of these.
    pub passes_below_tabs: usize,
}

/// Swept extents, and whether the machine can reach them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeReport {
    /// The check itself.
    pub check: CheckReport,
    /// Lower corner of the tool sweep in work coordinates (mm).
    pub work_min: [f64; 3],
    /// Upper corner of the tool sweep in work coordinates (mm).
    pub work_max: [f64; 3],
    /// The same in machine coordinates, when a work offset was given.
    pub machine_min: Option<[f64; 3]>,
    /// The same in machine coordinates, when a work offset was given.
    pub machine_max: Option<[f64; 3]>,
    /// Stock the job needs around the part, `[-X, -Y, +X, +Y]` (mm).
    pub stock_margin: [f64; 4],
}

/// A piece of stock the job cuts completely free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreedPiece {
    /// Plan area (mm²).
    pub area: f64,
    /// Centroid in the stock frame (mm).
    pub centroid: [f64; 2],
    /// The piece carries part material, rather than being waste.
    pub is_part: bool,
}

/// Stock the job frees: slugs, wedges, and the part itself when nothing holds
/// it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoosePiecesReport {
    /// The check itself.
    pub check: CheckReport,
    /// Each freed region, largest first.
    pub pieces: Vec<FreedPiece>,
    /// Waste that still reaches the edge of the blank: the frame the clamps
    /// hold. Reported, never counted as loose.
    pub frame_pieces: Vec<FreedPiece>,
    /// True when the job never breaks through, so nothing can come free.
    pub skin_holds: bool,
}

/// The whole answer: one job, eight checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobVerification {
    /// False when any error-level check failed.
    pub pass: bool,
    /// Cutting moves that bring the tool into the part.
    pub gouge: CheckReport,
    /// Wall the full-depth passes never reached.
    pub material_left: MaterialLeftReport,
    /// Rapids that travel in XY below a safe height, or dive into material.
    pub rapids: CheckReport,
    /// Depth against the stock and the declared allowance.
    pub depth: DepthReport,
    /// Tabs, as cut.
    pub tabs: TabAudit,
    /// Extents and travel.
    pub envelope: EnvelopeReport,
    /// Stock the job frees completely.
    pub loose: LoosePiecesReport,
    /// Vertical entries into uncut material.
    pub plunges: CheckReport,
    /// How many moves were replayed (after arcs were sampled).
    pub moves: usize,
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// One straight move of the replay. Arcs are sampled into several of these,
/// all carrying the index of the arc they came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Move {
    /// G-code line number, or toolpath segment index.
    pub index: usize,
    /// A rapid (G0) rather than a feed move.
    pub rapid: bool,
    /// Start point (mm).
    pub from: [f64; 3],
    /// End point (mm).
    pub to: [f64; 3],
    /// Feed rate in mm/min; 0 for rapids.
    pub feed: f64,
}

impl Move {
    /// Horizontal length (mm).
    pub fn xy_len(&self) -> f64 {
        (self.to[0] - self.from[0]).hypot(self.to[1] - self.from[1])
    }
    /// Lowest Z touched (mm).
    pub fn min_z(&self) -> f64 {
        self.from[2].min(self.to[2])
    }
    fn a(&self) -> [f64; 2] {
        [self.from[0], self.from[1]]
    }
    fn b(&self) -> [f64; 2] {
        [self.to[0], self.to[1]]
    }
}

fn sample_arc(
    from: [f64; 3],
    to: [f64; 3],
    centre: [f64; 2],
    ccw: bool,
    tol: f64,
    out: &mut Vec<[f64; 3]>,
) {
    let r = (from[0] - centre[0]).hypot(from[1] - centre[1]);
    let a0 = (from[1] - centre[1]).atan2(from[0] - centre[0]);
    let a1 = (to[1] - centre[1]).atan2(to[0] - centre[0]);
    let two_pi = std::f64::consts::TAU;
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
    while sweep <= 0.0 {
        sweep += two_pi;
    }
    // A full circle arrives as identical start and end points.
    if (to[0] - from[0]).hypot(to[1] - from[1]) < 1e-12 {
        sweep = two_pi;
    }
    let step = if r > tol {
        2.0 * (1.0 - tol / r).clamp(-1.0, 1.0).acos()
    } else {
        sweep
    };
    let n = ((sweep / step.max(1e-6)).ceil() as usize).max(1);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let ang = if ccw { a0 + sweep * t } else { a0 - sweep * t };
        out.push([
            centre[0] + r * ang.cos(),
            centre[1] + r * ang.sin(),
            from[2] + (to[2] - from[2]) * t,
        ]);
    }
}

/// Flatten a [`Toolpath`] into straight moves, sampling arcs.
pub fn replay_toolpath(tp: &Toolpath, opts: &VerifyOptions) -> Result<Vec<Move>, VerifyError> {
    let mut moves = Vec::new();
    let mut at = [0.0, 0.0, 0.0];
    let mut started = false;
    for (i, seg) in tp.segments.iter().enumerate() {
        match seg {
            ToolpathSegment::Rapid { to } => {
                if started {
                    moves.push(Move {
                        index: i,
                        rapid: true,
                        from: at,
                        to: *to,
                        feed: 0.0,
                    });
                }
                at = *to;
                started = true;
            }
            ToolpathSegment::Linear { to, feed } => {
                if !started {
                    return Err(VerifyError::Unsupported(
                        "the program cuts before it has a known position".into(),
                    ));
                }
                moves.push(Move {
                    index: i,
                    rapid: false,
                    from: at,
                    to: *to,
                    feed: *feed,
                });
                at = *to;
            }
            ToolpathSegment::Arc {
                to,
                center,
                plane,
                dir,
                feed,
            } => {
                if !matches!(plane, crate::ArcPlane::Xy) {
                    return Err(VerifyError::Unsupported(format!(
                        "arc in the {plane:?} plane: this oracle is 2.5D and only replays G17"
                    )));
                }
                if !started {
                    return Err(VerifyError::Unsupported(
                        "the program cuts before it has a known position".into(),
                    ));
                }
                let centre = [at[0] + center[0], at[1] + center[1]];
                let mut pts = Vec::new();
                sample_arc(
                    at,
                    *to,
                    centre,
                    matches!(dir, crate::ArcDir::Ccw),
                    opts.arc_tolerance,
                    &mut pts,
                );
                for p in pts {
                    moves.push(Move {
                        index: i,
                        rapid: false,
                        from: at,
                        to: p,
                        feed: *feed,
                    });
                    at = p;
                }
            }
            _ => {}
        }
    }
    Ok(moves)
}

// ---------------------------------------------------------------------------
// G-code reader
// ---------------------------------------------------------------------------

/// Read a G-code program into straight moves.
///
/// Understands `G0 G1 G2 G3` with `X Y Z I J F`, `G20/G21`, `G90/G91`,
/// `G17`, `G40 G49 G53..G59 G54.1 G80 G90.1 G91.1 G94`, `M0 M1 M2 M3 M4 M5
/// M6 M7 M8 M9 M30`, `S`, `T`, `N`, `G4 P`, and `(…)` / `;` comments.
/// Anything else is an error: a line the reader skipped would be a move the
/// oracle never checked.
pub fn parse_gcode(text: &str, opts: &VerifyOptions) -> Result<Vec<Move>, VerifyError> {
    let mut moves = Vec::new();
    let mut at = [0.0f64, 0.0, 0.0];
    let mut known = false;
    let mut motion: Option<u8> = None;
    let mut scale = 1.0f64;
    let mut absolute = true;
    let mut arc_absolute = false;
    let mut feed = 0.0f64;

    for (li, raw) in text.lines().enumerate() {
        let line = li + 1;
        let mut s = String::new();
        let mut depth = 0usize;
        for ch in raw.chars() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.checked_sub(1).ok_or_else(|| VerifyError::Gcode {
                        line,
                        what: "unbalanced ')' in a comment".into(),
                    })?
                }
                ';' if depth == 0 => break,
                '%' if depth == 0 => break,
                c if depth == 0 => s.push(c),
                _ => {}
            }
        }
        if depth != 0 {
            return Err(VerifyError::Gcode {
                line,
                what: "unterminated '(' comment".into(),
            });
        }
        let s = s.trim().to_ascii_uppercase();
        if s.is_empty() {
            continue;
        }

        // Split into words: a letter followed by a number.
        let mut words: Vec<(char, f64)> = Vec::new();
        let bytes: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
        let mut i = 0;
        while i < bytes.len() {
            let letter = bytes[i];
            if !letter.is_ascii_alphabetic() {
                return Err(VerifyError::Gcode {
                    line,
                    what: format!("stray '{letter}' where a word letter was expected"),
                });
            }
            i += 1;
            let start = i;
            if i < bytes.len() && (bytes[i] == '-' || bytes[i] == '+') {
                i += 1;
            }
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
                i += 1;
            }
            let num: String = bytes[start..i].iter().collect();
            let value = num.parse::<f64>().map_err(|_| VerifyError::Gcode {
                line,
                what: format!("'{letter}{num}' is not a number"),
            })?;
            words.push((letter, value));
        }

        let mut target = [None, None, None];
        let mut ij = [None, None];
        let mut saw_r = false;
        for (letter, value) in &words {
            match letter {
                'N' | 'S' | 'T' | 'P' | 'F' => {
                    if *letter == 'F' {
                        feed = *value * scale;
                    }
                }
                'X' => target[0] = Some(*value * scale),
                'Y' => target[1] = Some(*value * scale),
                'Z' => target[2] = Some(*value * scale),
                'I' => ij[0] = Some(*value * scale),
                'J' => ij[1] = Some(*value * scale),
                // Carried so an R-form arc gets the arc's own message rather
                // than "unknown word".
                'R' => saw_r = true,
                'M' => {
                    let m = *value;
                    if !matches!(m as i32, 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 30)
                        || (m - m.round()).abs() > 1e-9
                    {
                        return Err(VerifyError::Gcode {
                            line,
                            what: format!("M{m} is not understood"),
                        });
                    }
                }
                'G' => {
                    let g = (*value * 10.0).round() as i32;
                    match g {
                        0 | 10 | 20 | 30 => {
                            motion = Some((g / 10) as u8);
                        }
                        40 => {} // G4 dwell
                        200 => scale = 25.4,
                        210 => scale = 1.0,
                        170 => {}
                        400 | 490 | 800 | 940 | 930 | 610 | 640 => {}
                        530 | 540 | 550 | 560 | 570 | 580 | 590 | 541 => {}
                        900 => absolute = true,
                        910 => absolute = false,
                        901 => arc_absolute = true,
                        911 => arc_absolute = false,
                        _ => {
                            return Err(VerifyError::Gcode {
                                line,
                                what: format!("G{} is not understood", *value),
                            })
                        }
                    }
                }
                other => {
                    return Err(VerifyError::Gcode {
                        line,
                        what: format!("word '{other}' is not understood"),
                    })
                }
            }
        }

        if saw_r && !matches!(motion, Some(2) | Some(3)) {
            return Err(VerifyError::Gcode {
                line,
                what: "word 'R' is not understood".into(),
            });
        }
        if target.iter().all(Option::is_none) {
            continue;
        }
        let Some(mode) = motion else {
            return Err(VerifyError::Gcode {
                line,
                what: "motion before any G0/G1/G2/G3 set the mode".into(),
            });
        };
        if !known && mode != 0 {
            return Err(VerifyError::Gcode {
                line,
                what: "cutting move before the position is known: no G0 came first".into(),
            });
        }
        let to = [0, 1, 2].map(|k| match target[k] {
            Some(v) if absolute => v,
            Some(v) => at[k] + v,
            None => at[k],
        });

        match mode {
            0 | 1 => {
                if known {
                    if mode == 1 && feed <= 0.0 {
                        return Err(VerifyError::Gcode {
                            line,
                            what: "a G1 with no feed rate in effect".into(),
                        });
                    }
                    moves.push(Move {
                        index: line,
                        rapid: mode == 0,
                        from: at,
                        to,
                        feed: if mode == 0 { 0.0 } else { feed },
                    });
                }
                at = to;
                known = true;
            }
            2 | 3 => {
                let (Some(i), Some(j)) = (ij[0], ij[1]) else {
                    return Err(VerifyError::Gcode {
                        line,
                        what: "an arc without both I and J (R-form arcs are not read)".into(),
                    });
                };
                if feed <= 0.0 {
                    return Err(VerifyError::Gcode {
                        line,
                        what: "an arc with no feed rate in effect".into(),
                    });
                }
                let centre = if arc_absolute {
                    [i, j]
                } else {
                    [at[0] + i, at[1] + j]
                };
                let r0 = (at[0] - centre[0]).hypot(at[1] - centre[1]);
                let r1 = (to[0] - centre[0]).hypot(to[1] - centre[1]);
                if (r0 - r1).abs() > 1e-3 * r0.max(1.0) + 1e-3 {
                    return Err(VerifyError::Gcode {
                        line,
                        what: format!(
                            "arc end points disagree on the radius: {r0:.4} mm at the start, \
                             {r1:.4} mm at the end"
                        ),
                    });
                }
                let mut pts = Vec::new();
                sample_arc(at, to, centre, mode == 3, opts.arc_tolerance, &mut pts);
                for p in pts {
                    moves.push(Move {
                        index: line,
                        rapid: false,
                        from: at,
                        to: p,
                        feed,
                    });
                    at = p;
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(moves)
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

/// Replay a toolpath against the part it is meant to make.
pub fn verify_toolpath(
    tp: &Toolpath,
    spec: &JobSpec,
    opts: &VerifyOptions,
) -> Result<JobVerification, VerifyError> {
    let moves = replay_toolpath(tp, opts)?;
    verify_moves(&moves, spec, opts)
}

/// Replay a G-code program against the part it is meant to make.
pub fn verify_gcode(
    text: &str,
    spec: &JobSpec,
    opts: &VerifyOptions,
) -> Result<JobVerification, VerifyError> {
    let moves = parse_gcode(text, opts)?;
    verify_moves(&moves, spec, opts)
}

/// Replay an already-flattened move list.
pub fn verify_moves(
    moves: &[Move],
    spec: &JobSpec,
    opts: &VerifyOptions,
) -> Result<JobVerification, VerifyError> {
    if spec.tool_diameter <= 0.0 {
        return Err(VerifyError::BadInput(format!(
            "tool diameter {} must be positive",
            spec.tool_diameter
        )));
    }
    if spec.stock_thickness <= 0.0 {
        return Err(VerifyError::BadInput(format!(
            "stock thickness {} must be positive",
            spec.stock_thickness
        )));
    }
    let part = spec.part.poly()?;
    let r = spec.tool_diameter / 2.0;

    let gouge = check_gouge(moves, &part, r, opts);
    let rapids = check_rapids(moves, spec, opts);
    let depth = check_depth(moves, spec, opts);
    let tabs = check_tabs(moves, spec, opts);
    let envelope = check_envelope(moves, spec, opts);
    let plunges = check_plunges(moves, spec, opts);
    let material_left = check_material_left(moves, spec, &tabs.observations, opts)?;
    let loose = check_loose(moves, spec, opts)?;

    let pass = !gouge.blocks()
        && !rapids.blocks()
        && !depth.check.blocks()
        && !tabs.check.blocks()
        && !envelope.check.blocks()
        && !plunges.blocks()
        && !material_left.check.blocks()
        && !loose.check.blocks();

    Ok(JobVerification {
        pass,
        gouge,
        material_left,
        rapids,
        depth,
        tabs,
        envelope,
        loose,
        plunges,
        moves: moves.len(),
    })
}

/// 1. Any cutting move below the stock top whose swept disc enters the part.
fn check_gouge(moves: &[Move], part: &Poly, r: f64, opts: &VerifyOptions) -> CheckReport {
    let mut rep = CheckReport::new(
        "gouge",
        Severity::Error,
        format!(
            "swept cutter (radius {r:.4} mm) against the part, tolerance {:.4} mm",
            opts.tolerance
        ),
    );
    for m in moves {
        if m.rapid || m.min_z() >= 0.0 {
            continue;
        }
        // Clearance is signed: a tool centre that is itself inside the part is
        // a whole radius worse than one that just grazes the wall, and saying
        // so is what tells "the offset was a hair too small" from "the cut ran
        // on the wrong side of the line" (friction item 32, where the worst
        // reading is one full tool diameter).
        let (clear, at) = segment_clearance(part, m.a(), m.b(), opts);
        let into = r - clear;
        if into > opts.tolerance {
            rep.hit(
                Violation {
                    index: m.index,
                    xy: at,
                    z: m.min_z(),
                    value: into,
                    what: format!("cutter {into:.4} mm inside the part"),
                },
                opts.max_examples,
            );
        }
    }
    rep
}

/// Least signed clearance between the segment `a`–`b` and the part, and where
/// it is: positive outside the part, negative for a centre that is inside it.
fn segment_clearance(
    part: &Poly,
    a: [f64; 2],
    b: [f64; 2],
    opts: &VerifyOptions,
) -> (f64, [f64; 2]) {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let step = opts.tolerance.clamp(0.05, 0.5);
    let n = ((len / step).ceil() as usize).max(1);
    let mut worst = (f64::INFINITY, a);
    let mut inside = false;
    for k in 0..=n {
        let t = k as f64 / n as f64;
        let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let sd = -part.signed_distance(p);
        inside |= sd < 0.0;
        if sd < worst.0 {
            worst = (sd, p);
        }
    }
    if !inside {
        // Between the samples the segment may still clip a corner; the exact
        // segment distance catches that.
        let d = part.distance_to_segment(a, b);
        if d < worst.0 {
            worst = (d, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
    }
    worst
}

/// 3. Rapids that travel in XY too low over the stock, or dive into it.
fn check_rapids(moves: &[Move], spec: &JobSpec, opts: &VerifyOptions) -> CheckReport {
    let mut rep = CheckReport::new(
        "rapids",
        Severity::Error,
        format!(
            "G0 with XY motion must stay above Z{:.2}; G0 must not descend below the stock top",
            opts.safe_rapid_z
        ),
    );
    let stock = spec.stock();
    let over_stock =
        |p: [f64; 3]| p[0] >= stock[0] && p[0] <= stock[2] && p[1] >= stock[1] && p[1] <= stock[3];
    for m in moves {
        if !m.rapid {
            continue;
        }
        if m.xy_len() > 1e-9
            && m.min_z() < opts.safe_rapid_z
            && (over_stock(m.from) || over_stock(m.to))
        {
            rep.hit(
                Violation {
                    index: m.index,
                    xy: m.b(),
                    z: m.min_z(),
                    value: opts.safe_rapid_z - m.min_z(),
                    what: format!(
                        "rapid travels {:.2} mm in XY at Z{:.3}, below the safe height",
                        m.xy_len(),
                        m.min_z()
                    ),
                },
                opts.max_examples,
            );
        } else if m.to[2] < m.from[2] && m.to[2] < 0.0 && over_stock(m.to) {
            rep.hit(
                Violation {
                    index: m.index,
                    xy: m.b(),
                    z: m.to[2],
                    value: -m.to[2],
                    what: format!("rapid descends to Z{:.3}, below the stock top", m.to[2]),
                },
                opts.max_examples,
            );
        }
    }
    rep
}

/// Contiguous runs of cutting moves, split by rapids: one pass each.
fn passes(moves: &[Move]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    for (i, m) in moves.iter().enumerate() {
        if m.rapid {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(i);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 4. Depth against the stock thickness and the declared bottom allowance.
fn check_depth(moves: &[Move], spec: &JobSpec, opts: &VerifyOptions) -> DepthReport {
    let floor = spec.floor_z();
    let mut rep = CheckReport::new(
        "depth",
        Severity::Error,
        format!(
            "floor Z{floor:.3} = {:.2} mm stock less a {:.2} mm bottom allowance",
            spec.stock_thickness, spec.bottom_allowance
        ),
    );
    let deepest = moves
        .iter()
        .filter(|m| !m.rapid)
        .map(|m| m.min_z())
        .fold(0.0f64, f64::min);

    if deepest < floor - opts.depth_tolerance {
        let past = floor - deepest;
        let through = deepest < -spec.stock_thickness + opts.depth_tolerance;
        let what = if through && !spec.spoilboard {
            format!(
                "cuts {:.3} mm past the stock underside with no spoilboard declared",
                -spec.stock_thickness - deepest
            )
        } else {
            format!("cuts {past:.3} mm past the declared floor")
        };
        if !(through && spec.spoilboard && spec.bottom_allowance <= 0.0) {
            let m = moves
                .iter()
                .filter(|m| !m.rapid)
                .min_by(|a, b| a.min_z().total_cmp(&b.min_z()))
                .unwrap();
            rep.hit(
                Violation {
                    index: m.index,
                    xy: m.b(),
                    z: deepest,
                    value: past,
                    what,
                },
                opts.max_examples,
            );
        }
    }

    // Every feature must reach the floor: group passes whose XY bounds overlap.
    let groups = passes(moves);
    let mut boxes: Vec<([f64; 4], f64)> = Vec::new();
    for g in &groups {
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut z = 0.0f64;
        for &i in g {
            let m = &moves[i];
            for p in [m.a(), m.b()] {
                b[0] = b[0].min(p[0]);
                b[1] = b[1].min(p[1]);
                b[2] = b[2].max(p[0]);
                b[3] = b[3].max(p[1]);
            }
            z = z.min(m.min_z());
        }
        boxes.push((b, z));
    }
    let mut feature: Vec<usize> = (0..boxes.len()).collect();
    fn root(f: &mut [usize], mut i: usize) -> usize {
        while f[i] != i {
            f[i] = f[f[i]];
            i = f[i];
        }
        i
    }
    for i in 0..boxes.len() {
        for j in 0..i {
            let (a, b) = (boxes[i].0, boxes[j].0);
            if a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3] {
                let (ri, rj) = (root(&mut feature, i), root(&mut feature, j));
                feature[rj] = ri;
            }
        }
    }
    let mut per_feature: HashMap<usize, (f64, [f64; 4])> = HashMap::new();
    for (i, (b, z)) in boxes.iter().enumerate() {
        let rt = root(&mut feature, i);
        let e = per_feature.entry(rt).or_insert((0.0, *b));
        e.0 = e.0.min(*z);
    }
    let features = per_feature.len();
    for (deep, b) in per_feature.values() {
        // A feature that only skims the surface is not a feature that failed
        // to reach depth; one that goes below the stock top but stops short is.
        if *deep < -opts.depth_tolerance && *deep > floor + opts.depth_tolerance {
            rep.hit(
                Violation {
                    index: 0,
                    xy: [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0],
                    z: *deep,
                    value: deep - floor,
                    what: format!(
                        "a feature stops at Z{deep:.3}, {:.3} mm short of the floor",
                        deep - floor
                    ),
                },
                opts.max_examples,
            );
        }
    }

    DepthReport {
        check: rep,
        deepest_z: deepest,
        floor_z: floor,
        remaining_under_part: spec.stock_thickness + deepest,
        features,
    }
}

/// A stretch of one pass where the cutter rode above that pass's depth.
struct LiftedRun {
    len: f64,
    start: [f64; 2],
    end: [f64; 2],
    mid: [f64; 2],
    top: f64,
    path: Vec<[f64; 2]>,
}

/// 5. Tabs, as the moves actually cut them.
fn check_tabs(moves: &[Move], spec: &JobSpec, opts: &VerifyOptions) -> TabAudit {
    let mut rep = CheckReport::new(
        "tabs",
        Severity::Error,
        format!(
            "metal left under each tab must be at least {:.2} mm, on every pass that goes below it",
            opts.min_tab_metal
        ),
    );
    let d = spec.tool_diameter;
    let groups = passes(moves);
    let floor = moves
        .iter()
        .filter(|m| !m.rapid)
        .map(|m| m.min_z())
        .fold(0.0f64, f64::min);

    let mut obs: Vec<TabObservation> = Vec::new();
    let mut pass_depth: Vec<f64> = Vec::new();
    for (pi, g) in groups.iter().enumerate() {
        let depth = g.iter().map(|&i| moves[i].min_z()).fold(0.0f64, f64::min);
        pass_depth.push(depth);
        if depth >= 0.0 {
            continue;
        }
        // A lifted stretch: level XY motion held above this pass's depth, and
        // still inside the stock.
        let mut run: Option<LiftedRun> = None;
        let flush = |run: &mut Option<LiftedRun>, obs: &mut Vec<TabObservation>| {
            if let Some(LiftedRun {
                len,
                start,
                end,
                mid,
                top,
                path,
            }) = run.take()
            {
                let chord = (end[0] - start[0]).hypot(end[1] - start[1]);
                let straightness = if len > 0.0 { chord / len } else { 1.0 };
                obs.push(TabObservation {
                    xy: mid,
                    start,
                    end,
                    path,
                    top_z: top,
                    pass_z: 0.0,
                    lifted_run: len,
                    metal_width: len - d,
                    height: 0.0,
                    straightness,
                    straight: straightness >= 0.98,
                    pass_index: 0,
                });
            }
        };
        for &i in g {
            let m = &moves[i];
            let level = (m.to[2] - m.from[2]).abs() < 1e-9;
            let lifted = level && m.to[2] > depth + 1e-9 && m.to[2] < 0.0;
            if lifted && m.xy_len() > 1e-12 {
                match run.as_mut() {
                    Some(r) => {
                        r.len += m.xy_len();
                        r.end = m.b();
                        r.mid = [(r.start[0] + m.to[0]) / 2.0, (r.start[1] + m.to[1]) / 2.0];
                        r.path.push(m.b());
                    }
                    None => {
                        run = Some(LiftedRun {
                            len: m.xy_len(),
                            start: m.a(),
                            end: m.b(),
                            mid: [(m.from[0] + m.to[0]) / 2.0, (m.from[1] + m.to[1]) / 2.0],
                            top: m.to[2],
                            path: vec![m.a(), m.b()],
                        })
                    }
                }
            } else if m.xy_len() > 1e-12 {
                flush(&mut run, &mut obs);
            }
        }
        flush(&mut run, &mut obs);
        for o in obs.iter_mut().filter(|o| o.pass_z == 0.0) {
            o.pass_z = depth;
            o.pass_index = pi;
            o.height = o.top_z - floor;
        }
    }

    // Cluster the observations by position: one cluster is one tab.
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for (i, o) in obs.iter().enumerate() {
        match clusters.iter_mut().find(|c| {
            let q = obs[c[0]].xy;
            (q[0] - o.xy[0]).hypot(q[1] - o.xy[1]) < d.max(1.0) * 2.0
        }) {
            Some(c) => c.push(i),
            None => clusters.push(vec![i]),
        }
    }

    // A pass matters to a tab only if its path actually runs through that
    // tab's stretch. Depth alone attributed every deep pass of every other
    // operation to every tab: on the copper stator that was 3 tabs x 12
    // foreign passes of false alarms on a job that cut a good part.
    let reach = opts.tab_reach * spec.tool_diameter / 2.0;
    let runs_through = |pi: usize, o: &TabObservation| {
        groups[pi].iter().any(|&i| {
            let m = &moves[i];
            m.min_z() < 0.0
                && o.path
                    .windows(2)
                    .any(|w| segment_segment_distance(m.a(), m.b(), w[0], w[1]) <= reach)
        })
    };

    let mut stepped_over: Vec<bool> = vec![false; groups.len()];
    for c in &clusters {
        let top = obs[c[0]].top_z;
        let seen: Vec<usize> = c.iter().map(|&i| obs[i].pass_index).collect();
        for (pi, depth) in pass_depth.iter().enumerate() {
            if *depth >= top - 1e-9 || *depth >= 0.0 {
                continue;
            }
            if !runs_through(pi, &obs[c[0]]) {
                continue;
            }
            stepped_over[pi] = true;
            if !seen.contains(&pi) {
                let o = &obs[c[0]];
                rep.hit(
                    Violation {
                        index: 0,
                        xy: o.xy,
                        z: *depth,
                        value: top - depth,
                        what: format!(
                            "the pass at Z{depth:.3} cuts straight through the tab at \
                             ({:.2}, {:.2}), whose top is Z{top:.3}",
                            o.xy[0], o.xy[1]
                        ),
                    },
                    opts.max_examples,
                );
            }
        }
        for &i in c {
            let o = &obs[i];
            if o.metal_width < opts.min_tab_metal {
                rep.hit(
                    Violation {
                        index: 0,
                        xy: o.xy,
                        z: o.top_z,
                        value: opts.min_tab_metal - o.metal_width,
                        what: format!(
                            "tab at ({:.2}, {:.2}) leaves {:.2} mm of metal: the cutter is \
                             lifted over {:.2} mm and eats {:.2} mm of it",
                            o.xy[0], o.xy[1], o.metal_width, o.lifted_run, d
                        ),
                    },
                    opts.max_examples,
                );
            }
        }
    }

    let passes_below_tabs = stepped_over.iter().filter(|b| **b).count();

    if !spec.declared_tabs.is_empty() && clusters.len() != spec.declared_tabs.len() {
        rep.hit(
            Violation {
                index: 0,
                xy: [0.0, 0.0],
                z: 0.0,
                value: (spec.declared_tabs.len() as f64 - clusters.len() as f64).abs(),
                what: format!(
                    "{} tabs were declared but {} are cut",
                    spec.declared_tabs.len(),
                    clusters.len()
                ),
            },
            opts.max_examples,
        );
    }

    TabAudit {
        check: rep,
        tab_count: clusters.len(),
        passes_below_tabs,
        observations: obs,
    }
}

/// 6. Swept extents, travel limits, stock margin.
fn check_envelope(moves: &[Move], spec: &JobSpec, opts: &VerifyOptions) -> EnvelopeReport {
    let r = spec.tool_diameter / 2.0;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for m in moves {
        for p in [m.from, m.to] {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    if moves.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    // The cutter sweeps a radius around the centre path in XY.
    for k in 0..2 {
        min[k] -= r;
        max[k] += r;
    }
    let mut rep = CheckReport::new(
        "envelope",
        Severity::Error,
        "tool sweep against the machine's travel".to_string(),
    );
    let machine = spec.work_offset.map(|o| {
        (
            [min[0] + o[0], min[1] + o[1], min[2] + o[2]],
            [max[0] + o[0], max[1] + o[1], max[2] + o[2]],
        )
    });
    if let (Some((mmin, mmax)), Some(t)) = (machine, spec.travel) {
        for k in 0..3 {
            let axis = ["X", "Y", "Z"][k];
            if mmin[k] < t.min[k] - 1e-9 {
                rep.hit(
                    Violation {
                        index: 0,
                        xy: [mmin[0], mmin[1]],
                        z: mmin[2],
                        value: t.min[k] - mmin[k],
                        what: format!(
                            "the sweep reaches {axis}{:.3} in machine coordinates, {:.3} mm \
                             past the {axis} travel limit {:.3}",
                            mmin[k],
                            t.min[k] - mmin[k],
                            t.min[k]
                        ),
                    },
                    opts.max_examples,
                );
            }
            if mmax[k] > t.max[k] + 1e-9 {
                rep.hit(
                    Violation {
                        index: 0,
                        xy: [mmax[0], mmax[1]],
                        z: mmax[2],
                        value: mmax[k] - t.max[k],
                        what: format!(
                            "the sweep reaches {axis}{:.3} in machine coordinates, {:.3} mm \
                             past the {axis} travel limit {:.3}",
                            mmax[k],
                            mmax[k] - t.max[k],
                            t.max[k]
                        ),
                    },
                    opts.max_examples,
                );
            }
        }
    }
    let b = spec.part.bbox();
    EnvelopeReport {
        check: rep,
        work_min: min,
        work_max: max,
        machine_min: machine.map(|m| m.0),
        machine_max: machine.map(|m| m.1),
        stock_margin: [b[0] - min[0], b[1] - min[1], max[0] - b[2], max[1] - b[3]],
    }
}

/// 8. Straight vertical entries into uncut material.
fn check_plunges(moves: &[Move], spec: &JobSpec, opts: &VerifyOptions) -> CheckReport {
    let severity = if spec.centre_cutting {
        Severity::Warning
    } else {
        Severity::Error
    };
    let mut rep = CheckReport::new(
        "plunges",
        severity,
        if spec.centre_cutting {
            format!(
                "vertical entries into uncut material above {:.0} mm/min",
                opts.max_plunge_feed
            )
        } else {
            "the tool is not centre-cutting: no vertical entry into material is allowed".to_string()
        },
    );
    let r = spec.tool_diameter / 2.0;
    let mut cut: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
    for m in moves {
        if m.rapid {
            continue;
        }
        let vertical = m.xy_len() < 1e-9;
        if vertical && m.to[2] < m.from[2] && m.to[2] < 0.0 {
            let cleared = cut
                .iter()
                .any(|(a, b, z)| *z <= m.to[2] + 1e-9 && point_segment_distance(m.b(), *a, *b) < r);
            if !cleared && (!spec.centre_cutting || m.feed > opts.max_plunge_feed) {
                rep.hit(
                    Violation {
                        index: m.index,
                        xy: m.b(),
                        z: m.to[2],
                        value: if spec.centre_cutting {
                            m.feed - opts.max_plunge_feed
                        } else {
                            m.from[2] - m.to[2]
                        },
                        what: if spec.centre_cutting {
                            format!(
                                "plunges {:.2} mm into uncut material at F{:.0}",
                                m.from[2].min(0.0) - m.to[2],
                                m.feed
                            )
                        } else {
                            "a non-centre-cutting tool plunges into uncut material".to_string()
                        },
                    },
                    opts.max_examples,
                );
            }
        }
        if m.min_z() < 0.0 {
            cut.push((m.a(), m.b(), m.min_z()));
        }
    }
    rep
}

/// Segments the cutter swept at or below `z_at_most`.
fn swept_segments(moves: &[Move], z_at_most: f64, tol: f64) -> Vec<([f64; 2], [f64; 2])> {
    moves
        .iter()
        .filter(|m| !m.rapid && m.min_z() <= z_at_most + tol && m.xy_len() > 1e-12)
        .map(|m| (m.a(), m.b()))
        .collect()
}

/// 2. Wall band the full-depth passes never reached, ignoring what the cutter
///    could never have reached anyway.
///
/// Only walls the job *worked on* are graded. A wall no full-depth pass came
/// within a tool diameter of was not attempted — the blank may already be the
/// right size there, or the part may be staying in the sheet — and that is a
/// decision about scope, not a defect. The count of them is reported so the
/// caller can say "this job machines 3 of your 5 walls" without the oracle
/// having to guess which answer was meant.
fn check_material_left(
    moves: &[Move],
    spec: &JobSpec,
    tabs: &[TabObservation],
    opts: &VerifyOptions,
) -> Result<MaterialLeftReport, VerifyError> {
    let r = spec.tool_diameter / 2.0;
    let d = spec.tool_diameter;
    let floor = spec.floor_z();
    let segs = swept_segments(moves, floor, opts.depth_tolerance);
    let swept = Poly::from_segments(segs.clone());
    // Metal under a tab is metal the job meant to leave. The footprint is the
    // stretch the cutter rode over, grown by its radius (plus a couple of grid
    // cells, so the fringe the grid leaves round it does not come back as a
    // sliver of its own).
    let tab_reach = r + 2.0 * opts.grid;
    let tab_segments: Vec<([f64; 2], [f64; 2])> = tabs
        .iter()
        .flat_map(|t| t.path.windows(2).map(|w| (w[0], w[1])))
        .collect();
    let tab_paths = (!tab_segments.is_empty()).then(|| Poly::from_segments(tab_segments));
    let not_a_tab = |p: [f64; 2]| match &tab_paths {
        Some(t) => t.distance(p) - tab_reach,
        None => f64::INFINITY,
    };
    let mut tab_area = 0.0;
    let mut rep = CheckReport::new(
        "material_left",
        Severity::Error,
        "wall the full-depth passes never reached, against what the cutter could reach".to_string(),
    );
    let mut total = 0.0;
    let mut worst_standoff: f64 = 0.0;
    let mut band_area = 0.0;
    let mut untouched = 0usize;
    let mut walls = 0usize;

    let s = spec.stock();
    let stock = Poly::new(vec![vec![
        [s[0], s[1]],
        [s[2], s[1]],
        [s[2], s[3]],
        [s[0], s[3]],
    ]])?;
    let oo = OffsetOptions::default();
    let attempted = |lp: &Poly| segs.iter().any(|(a, b)| lp.distance_to_segment(*a, *b) < d);

    // Each opening: the waste is inside it and the tool centre lives in its
    // erosion. An opening narrower than the cutter is not a wall to follow.
    for hole in &spec.part.holes {
        let hp = Poly::new(vec![hole.clone()])?;
        let b = hp.bbox();
        if (b[2] - b[0]).min(b[3] - b[1]) <= d {
            continue;
        }
        walls += 1;
        if !attempted(&hp) {
            untouched += 1;
            continue;
        }
        let e = Poly::new(offset_loop(hole, -r, &oo))?;
        let bbox = [
            b[0] - opts.grid,
            b[1] - opts.grid,
            b[2] + opts.grid,
            b[3] + opts.grid,
        ];
        let band = |p: [f64; 2]| {
            let dh = hp.signed_distance(p);
            let de = -e.signed_distance(p); // distance outside the centre region
            (dh.min(d - dh).min(r - de), dh)
        };
        let unswept = |p: [f64; 2]| {
            let (b, aux) = band(p);
            (b.min(swept.distance(p) - r), aux)
        };
        for region in march(bbox, opts.grid, |p| {
            let (b, aux) = unswept(p);
            (b.min(not_a_tab(p)), aux)
        }) {
            if region.area < opts.min_area {
                continue;
            }
            total += region.area;
            worst_standoff = worst_standoff.max(region.peak_aux);
            rep.hit(
                Violation {
                    index: 0,
                    xy: region.centroid,
                    z: floor,
                    value: region.area,
                    what: format!(
                        "{:.3} mm\u{b2} of wall left standing, up to {:.3} mm proud",
                        region.area, region.peak_aux
                    ),
                },
                opts.max_examples,
            );
        }
        if tab_paths.is_some() {
            tab_area += march(bbox, opts.grid, |p| {
                let (b, aux) = unswept(p);
                (b.min(-not_a_tab(p)), aux)
            })
            .iter()
            .map(|c| c.area)
            .sum::<f64>();
        }
        band_area += march(bbox, opts.grid, band)
            .iter()
            .map(|c| c.area)
            .sum::<f64>();
    }

    // Outside the part: the tool centre lives outside the grown outline, and
    // only where there is stock to remove in the first place.
    {
        let op = Poly::new(vec![spec.part.outer.clone()])?;
        walls += 1;
        if attempted(&op) {
            let grown = Poly::new(offset_loop(&spec.part.outer, r, &oo))?;
            let b = op.bbox();
            let bbox = [
                b[0] - d - opts.grid,
                b[1] - d - opts.grid,
                b[2] + d + opts.grid,
                b[3] + d + opts.grid,
            ];
            let band = |p: [f64; 2]| {
                let dp = -op.signed_distance(p); // positive outside the part
                let dg = grown.signed_distance(p); // positive inside the grown outline
                (dp.min(d - dp).min(r - dg).min(stock.signed_distance(p)), dp)
            };
            let unswept = |p: [f64; 2]| {
                let (b, aux) = band(p);
                (b.min(swept.distance(p) - r), aux)
            };
            for region in march(bbox, opts.grid, |p| {
                let (b, aux) = unswept(p);
                (b.min(not_a_tab(p)), aux)
            }) {
                if region.area < opts.min_area {
                    continue;
                }
                total += region.area;
                worst_standoff = worst_standoff.max(region.peak_aux);
                rep.hit(
                    Violation {
                        index: 0,
                        xy: region.centroid,
                        z: floor,
                        value: region.area,
                        what: format!(
                            "{:.3} mm\u{b2} of stock left on the outside wall, up to {:.3} mm proud",
                            region.area, region.peak_aux
                        ),
                    },
                    opts.max_examples,
                );
            }
            if tab_paths.is_some() {
                tab_area += march(bbox, opts.grid, |p| {
                    let (b, aux) = unswept(p);
                    (b.min(-not_a_tab(p)), aux)
                })
                .iter()
                .map(|c| c.area)
                .sum::<f64>();
            }
            band_area += march(bbox, opts.grid, band)
                .iter()
                .map(|c| c.area)
                .sum::<f64>();
        } else {
            untouched += 1;
        }
    }

    rep.note = format!(
        "{} of {walls} walls machined; measured against what the cutter could reach, so a \
         corner it cannot enter is fit's answer, not a violation{}",
        walls - untouched,
        if tab_paths.is_some() {
            format!(
                "; {tab_area:.2} mm\u{b2} under {} tab stretches left on purpose",
                tabs.len()
            )
        } else {
            String::new()
        }
    );
    Ok(MaterialLeftReport {
        check: rep,
        unswept_area: total,
        max_standoff: worst_standoff,
        reachable_band_area: band_area,
        untouched_walls: untouched,
        walls,
        tab_area,
    })
}

/// 7. Stock the job frees completely.
///
/// Two things are not loose pieces. The frame is whatever still reaches the
/// edge of the blank: it is what the clamps hold, and a job that trims all
/// four sides of its stock is not thereby throwing four corners across the
/// shop. And a piece that carries no part material is waste — an inside slug,
/// a slot wedge — which rattles and can be thrown but leaves the part whole,
/// so whether it blocks the job is the shop's policy, not the oracle's.
fn check_loose(
    moves: &[Move],
    spec: &JobSpec,
    opts: &VerifyOptions,
) -> Result<LoosePiecesReport, VerifyError> {
    let r = spec.tool_diameter / 2.0;
    let mut rep = CheckReport::new(
        "loose_pieces",
        opts.loose_waste_severity,
        "stock the job cuts free with no tab and no skin holding it".to_string(),
    );
    // A positive bottom allowance means nothing is ever cut through.
    let through_z = -spec.stock_thickness;
    let breaks_through = moves
        .iter()
        .any(|m| !m.rapid && m.min_z() <= through_z + opts.depth_tolerance);
    if !breaks_through {
        rep.note = format!(
            "nothing is cut through: the deepest pass stops {:.3} mm above the stock underside",
            spec.stock_thickness
                + moves
                    .iter()
                    .filter(|m| !m.rapid)
                    .map(|m| m.min_z())
                    .fold(0.0f64, f64::min)
        );
        return Ok(LoosePiecesReport {
            check: rep,
            pieces: Vec::new(),
            frame_pieces: Vec::new(),
            skin_holds: true,
        });
    }
    let segs = swept_segments(moves, through_z, opts.depth_tolerance);
    let swept = Poly::from_segments(segs);
    let s = spec.stock();
    let stock = Poly::new(vec![vec![
        [s[0], s[1]],
        [s[2], s[1]],
        [s[2], s[3]],
        [s[0], s[3]],
    ]])?;
    let part = spec.part.poly()?;
    let bbox = [
        s[0] - opts.grid * 2.0,
        s[1] - opts.grid * 2.0,
        s[2] + opts.grid * 2.0,
        s[3] + opts.grid * 2.0,
    ];
    // `aux` carries how close the region gets to the edge of the blank, so a
    // piece that reaches it can be told from one that floats inside.
    let phi = |p: [f64; 2]| {
        let ds = stock.signed_distance(p);
        (ds.min(swept.distance(p) - r), -ds, part.signed_distance(p))
    };
    let edge = 1.5 * opts.grid;
    let mut pieces = Vec::new();
    let mut frame_pieces = Vec::new();
    for c in march2(bbox, opts.grid, phi) {
        if c.area < opts.min_area {
            continue;
        }
        // Some point of the region lies inside the part.
        let is_part = c.peak_aux2 > 0.0;
        // Reaches the edge of the blank: that is the frame, whatever it
        // carries. A part still joined to it by a tab is held, not loose.
        if c.peak_aux > -edge {
            frame_pieces.push(FreedPiece {
                area: c.area,
                centroid: c.centroid,
                is_part,
            });
            continue;
        }
        pieces.push(FreedPiece {
            area: c.area,
            centroid: c.centroid,
            is_part,
        });
        rep.hit(
            Violation {
                index: 0,
                xy: c.centroid,
                z: through_z,
                value: c.area,
                what: format!(
                    "{:.2} mm\u{b2} of {} at ({:.2}, {:.2}) comes free: no tab, no skin",
                    c.area,
                    if is_part { "the part" } else { "waste" },
                    c.centroid[0],
                    c.centroid[1]
                ),
            },
            opts.max_examples,
        );
    }
    if pieces.iter().any(|p| p.is_part) {
        rep.severity = opts.loose_part_severity;
    }
    rep.note = format!(
        "{} freed piece(s), {} of them carrying part material; {} frame piece(s) still reaching \
         the edge of the {} blank",
        pieces.len(),
        pieces.iter().filter(|p| p.is_part).count(),
        frame_pieces.len(),
        if spec.stock_is_declared() {
            "declared"
        } else {
            "assumed"
        }
    );
    Ok(LoosePiecesReport {
        check: rep,
        pieces,
        frame_pieces,
        skin_holds: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::fixture::stator_in_stock_frame;
    use crate::operation::{Contour, Point2D};
    use crate::{CamSettings, Contour2D, Tool};

    fn contour_of(points: &[[f64; 2]]) -> Contour {
        let mut c = Contour::new(Point2D::new(points[0][0], points[0][1]));
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            c.line_to(Point2D::new(p[0], p[1]));
        }
        c
    }

    fn mill(d: f64) -> Tool {
        Tool::FlatEndMill {
            diameter: d,
            flute_length: 25.0,
            flutes: 2,
        }
    }

    /// The part region the fixture DXF describes, in the stock frame, and the
    /// job that actually cut it in copper on 2026-09-17.
    fn copper_job() -> (PartRegion, String) {
        let loops = stator_in_stock_frame();
        let part = PartRegion::new(loops[0].clone(), loops[1..].to_vec()).unwrap();
        let nc = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-copper-d2.nc"
        ));
        (part, nc.to_string())
    }

    /// The whole point of the package, in one test: the job that came off the
    /// machine with a good part must come back clean. Five operations, three
    /// tabs, a 0.15 mm skin. Before the passes were attributed to tabs
    /// geometrically this failed 36 times over, and the tab metal itself was
    /// reported three times as wall left standing.
    #[test]
    fn the_copper_stator_job_that_worked_passes() {
        let (part, nc) = copper_job();
        let spec = JobSpec {
            bottom_allowance: 0.15,
            ..JobSpec::new(part, 1.0, 2.0)
        };
        let rep = verify_gcode(&nc, &spec, &VerifyOptions::default()).unwrap();
        assert!(
            rep.pass,
            "gouge {:?}\nmaterial_left {:?}\ntabs {:?}\nrapids {:?}\ndepth {:?}\nloose {:?}",
            rep.gouge.examples,
            rep.material_left.check.examples,
            rep.tabs.check.examples,
            rep.rapids.examples,
            rep.depth.check.examples,
            rep.loose.check.examples,
        );
        assert_eq!(rep.tabs.tab_count, 3);
        assert_eq!(rep.tabs.passes_below_tabs, 3, "only the outside op's own");
        for o in &rep.tabs.observations {
            assert!(
                (o.metal_width - 4.0).abs() < 0.05,
                "tab metal {:.3} mm, the job asked for 4",
                o.metal_width
            );
            assert!(
                (o.height - 0.425).abs() < 0.01,
                "tab height {:.3} mm",
                o.height
            );
        }
        assert!(
            rep.material_left.tab_area > 20.0,
            "tab metal excluded: {:.2} mm²",
            rep.material_left.tab_area
        );
        assert_eq!(rep.material_left.untouched_walls, 0, "all five walls cut");
        assert!(rep.loose.skin_holds, "0.15 mm of skin under everything");
        assert!((rep.depth.deepest_z + 0.85).abs() < 1e-6);
    }

    /// Multi-operation jobs put passes of one feature below the tabs of
    /// another. A pass only cuts a tab away if it goes through it.
    #[test]
    fn a_deep_pass_elsewhere_on_the_job_is_not_a_cut_tab() {
        let (part, nc) = copper_job();
        let spec = JobSpec {
            bottom_allowance: 0.15,
            ..JobSpec::new(part, 1.0, 2.0)
        };
        let rep = verify_gcode(&nc, &spec, &VerifyOptions::default()).unwrap();
        assert!(rep.tabs.check.pass);

        // Widen the reach until every pass on the job counts as running
        // through every tab, and the false alarms come back: 3 tabs x the 12
        // passes of the other four operations.
        let wide = VerifyOptions {
            tab_reach: 200.0,
            ..VerifyOptions::default()
        };
        let rep = verify_gcode(&nc, &spec, &wide).unwrap();
        assert_eq!(rep.tabs.check.violation_count, 36, "{:?}", rep.tabs.check);
    }

    /// The ramped, tabbed, two-operation job the kernel generates today: entry
    /// is a ramp along the contour, not a plunge, so a pass no longer begins
    /// with a vertical move. The tabs must still be found on every pass that
    /// reaches them — the Python prototype read zero here.
    fn ramped_job() -> (PartRegion, Toolpath) {
        let loops = stator_in_stock_frame();
        let part = PartRegion::new(loops[0].clone(), vec![loops[1].clone()]).unwrap();
        let tool = mill(3.175);
        let settings = CamSettings {
            stepdown: 0.5,
            ..CamSettings::default()
        };
        let mut tp = Contour2D::inside(contour_of(&loops[1]), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let outside = Contour2D::outside(contour_of(&loops[0]), 6.0)
            .with_tabs(3, 4.0, 1.0)
            .generate(&tool, &settings)
            .unwrap();
        tp.segments.extend(outside.segments);
        (part, tp)
    }

    #[test]
    fn ramp_entries_do_not_hide_the_tabs() {
        let (part, tp) = ramped_job();
        let spec = JobSpec::new(part, 6.0, 3.175);
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();

        // No pass starts with a vertical plunge: the entry ramps.
        let moves = replay_toolpath(&tp, &VerifyOptions::default()).unwrap();
        let groups = passes(&moves);
        // The pass drops vertically only as far as the last pass's floor —
        // air — and reaches its own depth along the contour.
        let depth = groups[1]
            .iter()
            .map(|&i| moves[i].min_z())
            .fold(0.0f64, f64::min);
        let entry = &moves[groups[1][0]];
        assert!(
            entry.xy_len() < 1e-9 && (entry.to[2] + 0.5).abs() < 1e-9,
            "the entry should stop at the last pass's floor: {entry:?}"
        );
        let reaches_depth = groups[1]
            .iter()
            .map(|&i| &moves[i])
            .find(|m| (m.to[2] - depth).abs() < 1e-9)
            .unwrap();
        assert!(
            reaches_depth.xy_len() > 0.01 && reaches_depth.to[2] < reaches_depth.from[2],
            "depth should be reached on a ramp, not a plunge: {reaches_depth:?}"
        );

        assert_eq!(rep.tabs.tab_count, 3);
        assert_eq!(
            rep.tabs.observations.len(),
            6,
            "3 tabs on each of the 2 passes that reach them"
        );
        assert_eq!(rep.tabs.passes_below_tabs, 2);
        for o in &rep.tabs.observations {
            assert!(
                (o.metal_width - 4.0).abs() < 1e-6,
                "metal {:.4} mm",
                o.metal_width
            );
            assert!((o.lifted_run - 7.175).abs() < 1e-6);
            assert!(o.straight, "straightness {:.4}", o.straightness);
        }
        assert!(rep.tabs.check.pass, "{:?}", rep.tabs.check.examples);
        assert!(
            rep.material_left.check.pass,
            "{:?}",
            rep.material_left.check.examples
        );
    }

    /// The same job's loose pieces: the bore slug really does come free, and
    /// the corners of the assumed blank really do not.
    #[test]
    fn the_frame_is_not_a_loose_piece_but_the_slug_is() {
        let (part, tp) = ramped_job();
        let spec = JobSpec::new(part, 6.0, 3.175);
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        assert_eq!(
            rep.loose.pieces.len(),
            1,
            "{:?}",
            rep.loose.pieces.iter().map(|p| p.area).collect::<Vec<_>>()
        );
        let slug = &rep.loose.pieces[0];
        assert!((slug.area - 607.0).abs() < 5.0, "slug {:.2} mm²", slug.area);
        assert!(!slug.is_part, "the bore slug is waste, not part");
        assert_eq!(
            rep.loose.check.severity,
            Severity::Warning,
            "loose waste warns by default"
        );
        assert!(
            rep.pass,
            "a rattling slug does not block the job by default"
        );

        // A blank barely bigger than the part: the outside sweep runs off its
        // edge and trims corners off it. Those used to be reported as stock
        // coming free; they are the edge of a blank nobody measured. (The
        // margin is kept clear of exactly one tool diameter — there the sweep
        // only kisses the edge and whether the frame pinches in two depends on
        // the last micron of the offsetter.)
        let b = spec.part.bbox();
        let m = 2.5;
        let tight = JobSpec {
            stock_bbox: Some([b[0] - m, b[1] - m, b[2] + m, b[3] + m]),
            ..JobSpec::new(spec.part.clone(), 6.0, 3.175)
        };
        let rep = verify_toolpath(&tp, &tight, &VerifyOptions::default()).unwrap();
        assert_eq!(rep.loose.pieces.len(), 1, "still only the slug");
        let corners: Vec<_> = rep
            .loose
            .frame_pieces
            .iter()
            .filter(|f| !f.is_part)
            .collect();
        assert!(corners.len() >= 2, "{:?}", rep.loose.frame_pieces);
        for corner in &corners {
            // Each trimmed corner sits out by the blank's edge, well clear of
            // the part's own bounds shrunk by a little.
            let [x, y] = corner.centroid;
            let inside = x > b[0] + 5.0 && x < b[2] - 5.0 && y > b[1] + 5.0 && y < b[3] - 5.0;
            assert!(!inside, "{corner:?} is not at the edge of the blank");
            assert!(corner.area > 1.0, "{corner:?}");
        }
        // The part is still tabbed to the rest of the frame, so it reaches the
        // blank's edge too, and is held.
        assert!(rep.loose.frame_pieces.iter().any(|f| f.is_part));
    }

    /// A part with nothing holding it is a different matter from a slug.
    #[test]
    fn a_part_cut_free_with_no_tab_is_an_error() {
        let part = PartRegion::new(
            vec![[10.0, 10.0], [40.0, 10.0], [40.0, 30.0], [10.0, 30.0]],
            vec![],
        )
        .unwrap();
        let tool = mill(3.0);
        let tp = Contour2D::outside(
            contour_of(&[[10.0, 10.0], [40.0, 10.0], [40.0, 30.0], [10.0, 30.0]]),
            6.0,
        )
        .generate(
            &tool,
            &CamSettings {
                stepdown: 3.0,
                ..CamSettings::default()
            },
        )
        .unwrap();
        let spec = JobSpec::new(part, 6.0, 3.0);
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        assert_eq!(rep.loose.pieces.len(), 1, "{:?}", rep.loose.pieces);
        assert!(rep.loose.pieces[0].is_part);
        assert!((rep.loose.pieces[0].area - 600.0).abs() < 4.0);
        assert_eq!(rep.loose.check.severity, Severity::Error);
        assert!(!rep.pass, "a part with no tabs and no skin blocks the job");
        assert!(rep.loose.check.examples[0].what.contains("the part"));
    }

    /// Helical entries and arc-fitted paths are arcs with a Z change and arcs
    /// in plan. Both have to replay as the circles they are: wave 2 routes the
    /// pilots through `HelicalBore`.
    #[test]
    fn helical_and_planar_arcs_replay_as_the_circles_they_are() {
        use crate::arcfit::{fit_arcs, ArcFitOptions};
        use crate::operation::HelicalBore;
        use crate::ToolGeometry;

        let part = PartRegion::new(
            vec![[0.0, 0.0], [60.0, 0.0], [60.0, 60.0], [0.0, 60.0]],
            vec![
                // The Ø2.5 pilot the bore is meant to leave behind.
                (0..64)
                    .map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / 64.0;
                        [30.0 + 1.25 * a.cos(), 30.0 + 1.25 * a.sin()]
                    })
                    .collect(),
            ],
        )
        .unwrap();
        let tool = mill(2.0);
        let settings = CamSettings {
            stepdown: 1.0,
            plunge_rate: 100.0,
            ..CamSettings::default()
        };
        let bore = HelicalBore::new(30.0, 30.0, 2.5, 6.0, 0.3);
        let tp = bore
            .generate(&tool, &ToolGeometry::default(), &settings)
            .unwrap();
        assert!(
            tp.segments
                .iter()
                .any(|s| matches!(s, ToolpathSegment::Arc { .. })),
            "a helical bore is made of arcs"
        );
        let opts = VerifyOptions::default();
        let moves = replay_toolpath(&tp, &opts).unwrap();
        // Every cutting move sits on the helix: radius (2.5 - 2.0)/2 = 0.25.
        let cutting: Vec<&Move> = moves
            .iter()
            .filter(|m| !m.rapid && m.min_z() < 0.0 && m.xy_len() > 1e-9)
            .collect();
        assert!(cutting.len() > 50, "{} sampled moves", cutting.len());
        for m in &cutting {
            let r = (m.to[0] - 30.0).hypot(m.to[1] - 30.0);
            assert!(r <= 0.25 + 1e-6, "tool centre at radius {r:.4}");
        }
        // The wall it leaves is the hole it was asked for, and it does not
        // touch the part around it.
        let wall = 2.0
            * (cutting
                .iter()
                .map(|m| (m.to[0] - 30.0).hypot(m.to[1] - 30.0))
                .fold(0.0f64, f64::max)
                + 1.0);
        assert!((wall - 2.5).abs() < 0.01, "bore leaves Ø{wall:.4}");
        let spec = JobSpec {
            bottom_allowance: 0.0,
            ..JobSpec::new(part.clone(), 6.0, 2.0)
        };
        let rep = verify_toolpath(&tp, &spec, &opts).unwrap();
        assert!(rep.gouge.pass, "{:?}", rep.gouge.examples);

        // Arc fitting a contour must not move the cut: same gouge answer,
        // same envelope, through the same replay.
        let (part2, _) = copper_job();
        let loops = stator_in_stock_frame();
        let linear = Contour2D::inside(contour_of(&loops[1]), 1.0)
            .generate(&mill(2.0), &settings)
            .unwrap();
        let fitted = fit_arcs(&linear, &ArcFitOptions::default());
        assert!(
            fitted
                .segments
                .iter()
                .any(|s| matches!(s, ToolpathSegment::Arc { .. })),
            "the fitter found no arcs to fit"
        );
        let spec2 = JobSpec::new(part2, 1.0, 2.0);
        let a = verify_toolpath(&linear, &spec2, &opts).unwrap();
        let b = verify_toolpath(&fitted, &spec2, &opts).unwrap();
        assert_eq!(a.gouge.pass, b.gouge.pass);
        assert!(
            (a.gouge.worst - b.gouge.worst).abs() < 0.01,
            "{} vs {}",
            a.gouge.worst,
            b.gouge.worst
        );
        for k in 0..3 {
            assert!(
                (a.envelope.work_min[k] - b.envelope.work_min[k]).abs() < 0.01
                    && (a.envelope.work_max[k] - b.envelope.work_max[k]).abs() < 0.01,
                "axis {k}: {:?} vs {:?}",
                a.envelope.work_min,
                b.envelope.work_min
            );
        }
    }

    /// A plate with a rectangular window in it.
    fn windowed_plate() -> (PartRegion, Vec<[f64; 2]>) {
        let window = vec![[10.0, 10.0], [50.0, 10.0], [50.0, 30.0], [10.0, 30.0]];
        let part = PartRegion::new(
            vec![[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]],
            vec![window.clone()],
        )
        .unwrap();
        (part, window)
    }

    // ---- 1. gouge ------------------------------------------------------

    /// The kernel's inside contour stays inside the window; the same operation
    /// with the offset the other way — which is exactly what shipped until
    /// friction item 32 — drives the cutter a full diameter into the part.
    #[test]
    fn gouge_tells_an_inside_cut_from_the_one_that_ran_outward() {
        let (part, window) = windowed_plate();
        let tool = mill(6.0);
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        let spec = JobSpec::new(part, 6.0, 6.0);
        let opts = VerifyOptions::default();

        let right = Contour2D::inside(contour_of(&window), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let rep = verify_toolpath(&right, &spec, &opts).unwrap();
        assert!(
            rep.gouge.pass,
            "worst {:.4} mm at {:?}",
            rep.gouge.worst,
            rep.gouge.examples.first()
        );

        // The old behaviour: `inside()` was byte-identical to `outside()`.
        let wrong = Contour2D::outside(contour_of(&window), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let rep = verify_toolpath(&wrong, &spec, &opts).unwrap();
        assert!(!rep.gouge.pass);
        assert!(!rep.pass, "the job must not be runnable");
        assert!(
            (rep.gouge.worst - 6.0).abs() < 0.05,
            "worst gouge {:.4} mm; the cutter is one diameter into the part",
            rep.gouge.worst
        );
        assert!(!rep.gouge.examples.is_empty());
    }

    /// The tolerance exists because a polylined offset legitimately wanders by
    /// a hundredth of a millimetre. It must not swallow a real gouge.
    #[test]
    fn gouge_tolerance_covers_polyline_slop_and_nothing_more() {
        let (part, _) = windowed_plate();
        let spec = JobSpec::new(part, 6.0, 6.0);
        let opts = VerifyOptions::default();
        // A path 3 mm inside the window wall is exactly flush; nudge it in.
        for (inset, want_pass) in [(0.015, true), (0.1, false)] {
            let path = vec![
                [13.0 - inset, 13.0 - inset],
                [47.0 + inset, 13.0 - inset],
                [47.0 + inset, 27.0 + inset],
                [13.0 - inset, 27.0 + inset],
            ];
            let mut tp = Toolpath::new();
            tp.push(ToolpathSegment::rapid(path[0][0], path[0][1], 5.0));
            tp.push(ToolpathSegment::linear(path[0][0], path[0][1], -6.0, 100.0));
            for p in path.iter().skip(1).chain(std::iter::once(&path[0])) {
                tp.push(ToolpathSegment::linear(p[0], p[1], -6.0, 400.0));
            }
            let rep = verify_toolpath(&tp, &spec, &opts).unwrap();
            assert_eq!(
                rep.gouge.pass, want_pass,
                "inset {inset}: worst {:.4}",
                rep.gouge.worst
            );
        }
    }

    // ---- 5. tabs -------------------------------------------------------

    /// Build one contour pass at `z`, lifting to `top` over `tabs` stretches
    /// of `run` mm measured along the tool centre path.
    fn pass_with_tabs(tp: &mut Toolpath, rect: [f64; 4], z: f64, lifts: &[(f64, f64)], top: f64) {
        let corners = [
            [rect[0], rect[1]],
            [rect[2], rect[1]],
            [rect[2], rect[3]],
            [rect[0], rect[3]],
        ];
        tp.push(ToolpathSegment::rapid(corners[0][0], corners[0][1], 5.0));
        tp.push(ToolpathSegment::linear(
            corners[0][0],
            corners[0][1],
            z,
            100.0,
        ));
        // Walk the bottom edge, lifting where asked; then the rest of the loop.
        for (at, run) in lifts {
            tp.push(ToolpathSegment::linear(*at, rect[1], z, 400.0));
            tp.push(ToolpathSegment::linear(*at, rect[1], top, 400.0));
            tp.push(ToolpathSegment::linear(at + run, rect[1], top, 400.0));
            tp.push(ToolpathSegment::linear(at + run, rect[1], z, 400.0));
        }
        for c in corners.iter().skip(1).chain(std::iter::once(&corners[0])) {
            tp.push(ToolpathSegment::linear(c[0], c[1], z, 400.0));
        }
        tp.push(ToolpathSegment::rapid(corners[0][0], corners[0][1], 5.0));
    }

    /// Friction item 33, first half: tabs honoured on the final pass only. At
    /// 0.5 mm stepdown the pass before it cuts the tab away, and the audit has
    /// to say so pass by pass.
    #[test]
    fn tabs_only_on_the_final_pass_are_no_tabs_at_all() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let rect = [-3.175, -3.175, 53.175, 43.175];
        let lifts = [(10.0, 4.0 + 3.175), (35.0, 4.0 + 3.175)];
        let spec = JobSpec::new(part, 3.0, 3.175);
        let opts = VerifyOptions::default();

        // Every pass below the tab top steps over it: clean.
        let mut good = Toolpath::new();
        for k in 1..=6 {
            let z = -0.5 * k as f64;
            let lifts: &[(f64, f64)] = if z < -2.0 + 1e-9 { &lifts } else { &[] };
            pass_with_tabs(&mut good, rect, z, lifts, -2.0);
        }
        let rep = verify_toolpath(&good, &spec, &opts).unwrap();
        assert_eq!(rep.tabs.tab_count, 2);
        assert_eq!(rep.tabs.passes_below_tabs, 2, "the passes at -2.5 and -3.0");
        assert!(rep.tabs.check.pass, "{:?}", rep.tabs.check.examples);
        for o in &rep.tabs.observations {
            assert!(
                (o.metal_width - 4.0).abs() < 1e-6,
                "metal {:.4} mm, asked for 4",
                o.metal_width
            );
            assert!((o.height - 1.0).abs() < 1e-6, "height {:.4}", o.height);
            assert!(o.straight);
        }

        // Tabs on the last pass only: the pass at -2.5 cut straight through.
        let mut late = Toolpath::new();
        for k in 1..=6 {
            let z = -0.5 * k as f64;
            let lifts: &[(f64, f64)] = if k == 6 { &lifts } else { &[] };
            pass_with_tabs(&mut late, rect, z, lifts, -2.0);
        }
        let rep = verify_toolpath(&late, &spec, &opts).unwrap();
        assert!(!rep.tabs.check.pass);
        assert!(!rep.pass);
        assert_eq!(
            rep.tabs.check.violation_count, 2,
            "one per tab: {:?}",
            rep.tabs.check.examples
        );
        assert!(rep.tabs.check.examples[0]
            .what
            .contains("cuts straight through"));

        // And what the job claims is checked against what it cuts.
        let declared = JobSpec {
            declared_tabs: vec![
                DeclaredTab {
                    width: 4.0,
                    height: 1.0
                };
                3
            ],
            ..JobSpec::new(spec.part.clone(), 3.0, 3.175)
        };
        let rep = verify_toolpath(&good, &declared, &opts).unwrap();
        assert!(!rep.tabs.check.pass);
        assert!(
            rep.tabs
                .check
                .examples
                .iter()
                .any(|v| v.what.contains("3 tabs were declared but 2 are cut")),
            "{:?}",
            rep.tabs.check.examples
        );
    }

    /// Friction item 33, second half: a "4 mm" tab whose lifted run is 4 mm
    /// leaves 4 - 3.175 = 0.825 mm of metal, because the cutter takes a radius
    /// out of each end.
    #[test]
    fn a_tab_measured_along_the_tool_path_leaves_a_sliver() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let rect = [-3.175, -3.175, 53.175, 43.175];
        let spec = JobSpec::new(part, 3.0, 3.175);
        let mut tp = Toolpath::new();
        for k in 1..=6 {
            let z = -0.5 * k as f64;
            let lifts: &[(f64, f64)] = if z < -2.0 + 1e-9 { &[(10.0, 4.0)] } else { &[] };
            pass_with_tabs(&mut tp, rect, z, lifts, -2.0);
        }
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        assert_eq!(rep.tabs.tab_count, 1);
        for o in &rep.tabs.observations {
            assert!(
                (o.metal_width - 0.825).abs() < 1e-6,
                "metal {:.4} mm, not the 4 mm the job asked for",
                o.metal_width
            );
        }
        assert!(!rep.tabs.check.pass);
        assert!(
            rep.tabs.check.examples[0].what.contains("mm of metal"),
            "{}",
            rep.tabs.check.examples[0].what
        );
    }

    // ---- 3. rapids, 4. depth, 8. plunges --------------------------------

    /// A G0 that travels in XY below the safe height is a cutter dragged
    /// through whatever is in the way.
    #[test]
    fn a_rapid_that_travels_low_over_the_stock_is_flagged() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let spec = JobSpec::new(part, 6.0, 3.0);
        let opts = VerifyOptions::default();

        let mut safe = Toolpath::new();
        safe.push(ToolpathSegment::rapid(5.0, 5.0, 5.0));
        safe.push(ToolpathSegment::linear(5.0, 5.0, -6.0, 100.0));
        safe.push(ToolpathSegment::linear(6.0, 5.0, -6.0, 400.0));
        safe.push(ToolpathSegment::rapid(6.0, 5.0, 5.0));
        safe.push(ToolpathSegment::rapid(40.0, 30.0, 5.0));
        assert!(verify_toolpath(&safe, &spec, &opts).unwrap().rapids.pass);

        let mut low = safe.clone();
        low.push(ToolpathSegment::rapid(40.0, 30.0, 0.2));
        low.push(ToolpathSegment::rapid(10.0, 10.0, 0.2));
        let rep = verify_toolpath(&low, &spec, &opts).unwrap();
        assert!(!rep.rapids.pass);
        assert_eq!(rep.rapids.violation_count, 1);
        assert!((rep.rapids.worst - 0.3).abs() < 1e-9, "{:?}", rep.rapids);

        // And a rapid that dives into the stock instead of feeding.
        let mut dive = safe.clone();
        dive.push(ToolpathSegment::rapid(40.0, 30.0, -2.0));
        let rep = verify_toolpath(&dive, &spec, &opts).unwrap();
        assert!(!rep.rapids.pass);
        assert!(rep.rapids.examples[0].what.contains("below the stock top"));
    }

    /// Friction item 40 and 50: 10 mm of cut in 6 mm of stock is a cut into
    /// whatever the stock is sitting on. It is only allowed when the job says
    /// what that is.
    #[test]
    fn cutting_past_the_stock_needs_a_declared_bed() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(5.0, 5.0, 5.0));
        tp.push(ToolpathSegment::linear(5.0, 5.0, -10.0, 100.0));
        tp.push(ToolpathSegment::linear(45.0, 5.0, -10.0, 400.0));
        tp.push(ToolpathSegment::rapid(45.0, 5.0, 5.0));
        let opts = VerifyOptions::default();

        let bare = JobSpec::new(part.clone(), 6.0, 3.0);
        let rep = verify_toolpath(&tp, &bare, &opts).unwrap();
        assert!(!rep.depth.check.pass);
        assert!((rep.depth.deepest_z + 10.0).abs() < 1e-9);
        assert!((rep.depth.remaining_under_part + 4.0).abs() < 1e-9);
        assert!(
            rep.depth.check.examples[0]
                .what
                .contains("no spoilboard declared"),
            "{:?}",
            rep.depth.check.examples[0].what
        );

        // Declared 4 mm break-through into a spoilboard: the same moves pass.
        let declared = JobSpec {
            bottom_allowance: -4.0,
            spoilboard: true,
            ..JobSpec::new(part.clone(), 6.0, 3.0)
        };
        let rep = verify_toolpath(&tp, &declared, &opts).unwrap();
        assert!((rep.depth.floor_z + 10.0).abs() < 1e-9);
        assert!(rep.depth.check.pass, "{:?}", rep.depth.check.examples);

        // A 0.15 mm onion skin the job never leaves: it cut too deep.
        let skin = JobSpec {
            bottom_allowance: 0.15,
            ..JobSpec::new(part, 6.0, 3.0)
        };
        assert!(!verify_toolpath(&tp, &skin, &opts).unwrap().depth.check.pass);
    }

    /// A feature that stops above the floor is a hole that is not a hole.
    #[test]
    fn a_feature_that_stops_short_of_the_floor_is_flagged() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let spec = JobSpec::new(part, 6.0, 3.0);
        let mut tp = Toolpath::new();
        for (x, depth) in [(5.0, 6.0), (30.0, 4.0)] {
            tp.push(ToolpathSegment::rapid(x, 5.0, 5.0));
            tp.push(ToolpathSegment::linear(x, 5.0, -depth, 100.0));
            tp.push(ToolpathSegment::linear(x + 10.0, 5.0, -depth, 400.0));
            tp.push(ToolpathSegment::rapid(x + 10.0, 5.0, 5.0));
        }
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        assert_eq!(rep.depth.features, 2);
        assert!(!rep.depth.check.pass);
        assert!((rep.depth.check.worst - 2.0).abs() < 1e-9);
        assert!(rep.depth.check.examples[0]
            .what
            .contains("short of the floor"));
    }

    /// A tool that cannot cut at its centre cannot make its own hole.
    #[test]
    fn a_non_centre_cutting_tool_may_not_plunge() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [50.0, 0.0], [50.0, 40.0], [0.0, 40.0]],
            vec![],
        )
        .unwrap();
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(25.0, 20.0, 5.0));
        tp.push(ToolpathSegment::linear(25.0, 20.0, -3.0, 600.0));
        tp.push(ToolpathSegment::linear(30.0, 20.0, -3.0, 400.0));
        tp.push(ToolpathSegment::rapid(30.0, 20.0, 5.0));

        let opts = VerifyOptions::default();
        let centre = JobSpec::new(part.clone(), 6.0, 3.0);
        let rep = verify_toolpath(&tp, &centre, &opts).unwrap();
        assert!(!rep.plunges.pass, "F600 is above the 500 mm/min limit");
        assert_eq!(
            rep.plunges.severity,
            Severity::Warning,
            "a fast plunge is worth saying; it does not block a centre-cutting tool"
        );

        let insert = JobSpec {
            centre_cutting: false,
            ..JobSpec::new(part, 6.0, 3.0)
        };
        let rep = verify_toolpath(&tp, &insert, &opts).unwrap();
        assert!(!rep.plunges.pass);
        assert_eq!(rep.plunges.severity, Severity::Error);
        assert!(
            !rep.pass,
            "an insert tool boring its own hole blocks the job"
        );
    }

    // ---- 6. envelope ----------------------------------------------------

    /// Friction item 41: for the stator with a Ø2 cutter the sweep runs
    /// -2.00 to 65.15 mm in both X and Y, which is the blank the job needs and
    /// where work zero sits on it.
    #[test]
    fn stator_sweep_and_the_blank_it_needs() {
        let loops = stator_in_stock_frame();
        let part = PartRegion::new(loops[0].clone(), loops[1..].to_vec()).unwrap();
        let tool = mill(2.0);
        let settings = CamSettings {
            stepdown: 3.0,
            ..CamSettings::default()
        };
        let tp = Contour2D::outside(contour_of(&loops[0]), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let spec = JobSpec {
            work_offset: Some([10.0, 10.0, -50.0]),
            travel: Some(TravelLimits {
                min: [0.0, 0.0, -60.0],
                max: [400.0, 400.0, 0.0],
            }),
            ..JobSpec::new(part, 6.0, 2.0)
        };
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        let (lo, hi) = (rep.envelope.work_min, rep.envelope.work_max);
        for k in 0..2 {
            assert!(
                (lo[k] + 2.0).abs() < 0.02 && (hi[k] - 65.15).abs() < 0.02,
                "axis {k}: {:.3}..{:.3}, expected -2.00..65.15",
                lo[k],
                hi[k]
            );
        }
        for m in rep.envelope.stock_margin {
            assert!((m - 2.0).abs() < 0.02, "margin {m:.3}");
        }
        assert!(rep.envelope.check.pass);
        assert_eq!(rep.envelope.machine_min.unwrap()[2], -56.0);

        // The same job on a machine that cannot reach it.
        let small = JobSpec {
            travel: Some(TravelLimits {
                min: [0.0, 0.0, -60.0],
                max: [70.0, 70.0, 0.0],
            }),
            ..spec
        };
        let rep = verify_toolpath(&tp, &small, &VerifyOptions::default()).unwrap();
        assert!(!rep.envelope.check.pass);
        assert!(
            (rep.envelope.check.worst - 5.15).abs() < 0.02,
            "{:?}",
            rep.envelope.check
        );
    }

    // ---- 7. loose pieces -------------------------------------------------

    /// Friction item 39: the bore-and-slots contour with a Ø2 cutter frees the
    /// centre slug *and* twelve wedges, because at each 3.87 mm slot mouth the
    /// two passes overlap and sever the tip. A 0.15 mm skin holds all of it.
    #[test]
    fn stator_frees_a_slug_and_twelve_wedges_unless_a_skin_holds_them() {
        let loops = stator_in_stock_frame();
        let part = PartRegion::new(loops[0].clone(), vec![loops[1].clone()]).unwrap();
        let tool = mill(2.0);
        let opts = VerifyOptions {
            grid: 0.03,
            ..VerifyOptions::default()
        };

        let through = Contour2D::inside(contour_of(&loops[1]), 6.0)
            .generate(
                &tool,
                &CamSettings {
                    stepdown: 3.0,
                    ..CamSettings::default()
                },
            )
            .unwrap();
        let spec = JobSpec::new(part.clone(), 6.0, 2.0);
        let rep = verify_toolpath(&through, &spec, &opts).unwrap();
        assert!(!rep.loose.skin_holds);
        assert_eq!(
            rep.loose.pieces.len(),
            13,
            "the slug and twelve wedges: {:?}",
            rep.loose.pieces.iter().map(|p| p.area).collect::<Vec<_>>()
        );
        let mut areas: Vec<f64> = rep.loose.pieces.iter().map(|p| p.area).collect();
        areas.sort_by(f64::total_cmp);
        assert!(
            (areas[12] - 721.5).abs() < 8.0,
            "slug {:.2} mm², shapely measures 721.5",
            areas[12]
        );
        for a in &areas[..12] {
            assert!(
                (a - 5.671).abs() < 0.2,
                "wedge {a:.3} mm², shapely measures 5.671"
            );
        }
        assert!(!rep.loose.check.pass);

        // 0.15 mm of skin left under the part: nothing comes free.
        let skinned = Contour2D::inside(contour_of(&loops[1]), 5.85)
            .generate(
                &tool,
                &CamSettings {
                    stepdown: 3.0,
                    ..CamSettings::default()
                },
            )
            .unwrap();
        let spec = JobSpec {
            bottom_allowance: 0.15,
            ..JobSpec::new(part, 6.0, 2.0)
        };
        let rep = verify_toolpath(&skinned, &spec, &opts).unwrap();
        assert!(rep.loose.skin_holds);
        assert!(rep.loose.pieces.is_empty());
        assert!(rep.loose.check.pass);
    }

    // ---- 2. material left -------------------------------------------------

    /// A pass that stops short of the wall leaves a band of metal, and the
    /// oracle measures it. What the cutter could never have reached — the
    /// corners of the window — is not counted: that is `fit`'s answer.
    #[test]
    fn material_left_measures_the_band_a_short_pass_leaves() {
        let (part, window) = windowed_plate();
        let tool = mill(6.0);
        let settings = CamSettings {
            stepdown: 6.0,
            ..CamSettings::default()
        };
        let opts = VerifyOptions::default();
        let spec = JobSpec {
            // The blank is the part: only the window is machined here.
            stock_bbox: Some([0.0, 0.0, 60.0, 40.0]),
            ..JobSpec::new(part, 6.0, 6.0)
        };

        let full = Contour2D::inside(contour_of(&window), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let rep = verify_toolpath(&full, &spec, &opts).unwrap();
        assert!(
            rep.material_left.unswept_area < 0.5,
            "left {:.3} mm² after a full-depth wall pass",
            rep.material_left.unswept_area
        );

        // The same loop 1 mm shy of the wall all round.
        let shy: Vec<[f64; 2]> = vec![[14.0, 14.0], [46.0, 14.0], [46.0, 26.0], [14.0, 26.0]];
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(shy[0][0], shy[0][1], 5.0));
        tp.push(ToolpathSegment::linear(shy[0][0], shy[0][1], -6.0, 100.0));
        for p in shy.iter().skip(1).chain(std::iter::once(&shy[0])) {
            tp.push(ToolpathSegment::linear(p[0], p[1], -6.0, 400.0));
        }
        let rep = verify_toolpath(&tp, &spec, &opts).unwrap();
        assert!(!rep.material_left.check.pass);
        // A 1 mm band around a 40 x 20 window, less the corners the cutter
        // could not have reached anyway.
        assert!(
            (rep.material_left.unswept_area - 116.0).abs() < 6.0,
            "left {:.3} mm², a 1 mm band round the window is ~116",
            rep.material_left.unswept_area
        );
        // Worst is not the 1 mm along the straight walls but the corner, where
        // the round cutter stops 4 - 3/sqrt(2) = 1.879 mm short on the
        // diagonal.
        assert!(
            (1.879 - rep.material_left.max_standoff).abs() < 0.06,
            "stand-off {:.4}",
            rep.material_left.max_standoff
        );
        assert_eq!(
            rep.material_left.untouched_walls, 1,
            "the outside is not cut here"
        );
    }

    // ---- the G-code reader ------------------------------------------------

    /// Posting a toolpath and reading it back must give the same moves. The
    /// reader is the oracle's other front door, and a G-code file is what
    /// actually reaches the machine.
    #[test]
    fn gcode_round_trips_through_the_post() {
        use crate::post::{GrblPost, PostProcessor};
        let (_, window) = windowed_plate();
        let tool = mill(6.0);
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        let tp = Contour2D::inside(contour_of(&window), 6.0)
            .generate(&tool, &settings)
            .unwrap();
        let text = GrblPost::default().generate("window", &tool, &tp, &settings);
        let opts = VerifyOptions::default();
        let from_path = replay_toolpath(&tp, &opts).unwrap();
        let from_text = parse_gcode(&text, &opts).unwrap();
        // The post's header and footer add their own rapids.
        assert!(from_text.len() >= from_path.len());
        let cuts_path: Vec<[f64; 3]> = from_path
            .iter()
            .filter(|m| !m.rapid)
            .map(|m| m.to)
            .collect();
        let cuts_text: Vec<[f64; 3]> = from_text
            .iter()
            .filter(|m| !m.rapid)
            .map(|m| m.to)
            .collect();
        assert_eq!(cuts_path.len(), cuts_text.len());
        for (a, b) in cuts_path.iter().zip(&cuts_text) {
            for k in 0..3 {
                // The post rounds to three decimals.
                assert!((a[k] - b[k]).abs() < 5e-4, "{a:?} vs {b:?}");
            }
        }
    }

    /// What the reader does not understand it refuses. A skipped line is a
    /// move nothing checked.
    #[test]
    fn gcode_refuses_what_it_cannot_replay() {
        let opts = VerifyOptions::default();
        let cases = [
            ("G0 X0 Y0 Z5\nG81 X10 Y10 Z-5 R2\n", "G81"),
            ("G0 X0 Y0 Z5\nG1 X10 Y0 Z-1\n", "feed"),
            ("G0 X0 Y0 Z5\nG2 X10 Y0 R5 F100\n", "I and J"),
            ("G0 X0 Y0 Z5\nG1 X10 Y0 Z-1 F100 A90\n", "'A'"),
            ("G1 X10 Y0 Z-1 F100\n", "position is known"),
            ("G0 X0 Y0 Z5\nG2 X10 Y0 I3 J0 F100\n", "radius"),
            ("G0 X0 Y0 Z5\nG1 X10 Y0 Z-1 F100 (unclosed\n", "comment"),
        ];
        for (text, needle) in cases {
            let err = parse_gcode(text, &opts).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains(needle), "{text:?} gave {msg:?}");
        }
    }

    /// Inches, incremental moves and arcs all have to land in the same frame
    /// as everything else.
    #[test]
    fn gcode_reads_units_modes_and_arcs() {
        let opts = VerifyOptions::default();
        // Every number here is inches: G20 is in effect.
        let moves = parse_gcode(
            "(t)\nG20 G90\nG0 X0 Y0 Z1\nG1 Z-0.1 F10\nG91\nG1 X1 F20\nG90\nG3 X1 Y0 I0 J-0.5 F100\n",
            &opts,
        )
        .unwrap();
        // G20: one inch of incremental X is 25.4 mm.
        let linear: Vec<&Move> = moves.iter().filter(|m| !m.rapid).collect();
        assert!((linear[0].to[2] + 2.54).abs() < 1e-9, "{:?}", linear[0]);
        assert!((linear[1].to[0] - 25.4).abs() < 1e-9, "{:?}", linear[1]);
        assert!((linear[1].feed - 508.0).abs() < 1e-9, "feed in mm/min");
        // A full circle of radius 12.7 mm, sampled: back where it started.
        let arc: Vec<&Move> = moves.iter().filter(|m| m.index == 8).collect();
        assert!(arc.len() > 20, "{} samples", arc.len());
        let last = arc.last().unwrap().to;
        assert!(
            (last[0] - 25.4).abs() < 1e-6 && last[1].abs() < 1e-6,
            "{last:?}"
        );
        let radius: Vec<f64> = arc
            .iter()
            .map(|m| (m.to[0] - 25.4).hypot(m.to[1] + 12.7))
            .collect();
        for r in radius {
            assert!((r - 12.7).abs() < 1e-6, "arc sample at radius {r}");
        }
    }

    /// The oracle must be usable on a job of the size a real part produces.
    #[test]
    fn a_thirty_thousand_move_job_is_replayed_in_seconds() {
        let loops = stator_in_stock_frame();
        let part = PartRegion::new(loops[0].clone(), vec![loops[1].clone()]).unwrap();
        let tool = mill(2.0);
        let tp = Contour2D::inside(contour_of(&loops[1]), 6.0)
            .generate(
                &tool,
                &CamSettings {
                    stepdown: 0.1,
                    ..CamSettings::default()
                },
            )
            .unwrap();
        let spec = JobSpec::new(part, 6.0, 2.0);
        let t = std::time::Instant::now();
        let rep = verify_toolpath(&tp, &spec, &VerifyOptions::default()).unwrap();
        let took = t.elapsed();
        assert!(rep.moves > 30_000, "{} moves", rep.moves);
        assert!(
            took.as_secs_f64() < 30.0,
            "{} moves took {took:?}",
            rep.moves
        );
        println!("{} moves verified in {took:?}", rep.moves);
    }

    #[test]
    fn a_report_survives_a_round_trip_through_json() {
        let part = PartRegion::new(
            vec![[0.0, 0.0], [30.0, 0.0], [30.0, 20.0], [0.0, 20.0]],
            vec![],
        )
        .unwrap();
        assert!((part.area() - 600.0).abs() < 1e-9);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(-3.0, -3.0, 5.0));
        tp.push(ToolpathSegment::linear(-3.0, -3.0, -6.0, 100.0));
        tp.push(ToolpathSegment::linear(33.0, -3.0, -6.0, 400.0));
        tp.push(ToolpathSegment::rapid(33.0, -3.0, 5.0));
        let rep = verify_toolpath(
            &tp,
            &JobSpec::new(part, 6.0, 3.0),
            &VerifyOptions::default(),
        )
        .unwrap();
        let json = serde_json::to_string(&rep).unwrap();
        let back: JobVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pass, rep.pass);
        assert_eq!(back.gouge.name, "gouge");
    }
}
