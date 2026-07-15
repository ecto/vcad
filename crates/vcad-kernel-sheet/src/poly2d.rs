//! Kernel-native 2D polygon booleans on a 1 µm snap grid.
//!
//! Purpose-built for the flat-pattern pipeline: merging panel outlines with
//! bend-allowance quads into a single cut silhouette ([`union_all`]) and
//! subtracting bend-relief notches from panel outlines ([`difference`]).
//!
//! The algorithm is a snap-rounded planar arrangement + face classification:
//!
//! 1. Snap every input vertex to a 1 µm integer grid (exact arithmetic from
//!    here on — `i64` coordinates, `i128` orientation tests; segment
//!    crossings are computed as exact rationals and rounded
//!    order-independently).
//! 2. Split every segment at its intersections with every other segment
//!    (proper crossings, T-junctions, and collinear overlaps).
//! 3. Enumerate the arrangement's face cycles purely topologically (both
//!    half-edges of every sub-segment; next edge = first clockwise from the
//!    reversed incoming direction), then classify each cycle's left face
//!    with ONE probe at the midpoint of its longest edge — long edges
//!    deviate less than a grid-step from their true line, so the probe
//!    can't land on the wrong side the way per-edge probing of micron
//!    slivers can.
//! 4. Emit the edges between inside and outside faces — closed loops by
//!    construction — and walk them interior-left.
//! 5. Simplify: drop collinear vertices (cross-product tolerance 1e-6 mm²).
//!
//! Inputs are expected to be *structurally robust* — callers oversize
//! bridging geometry by a few µm (see the DXF silhouette builder) so the
//! arrangement never depends on exact tangency. This is not a general
//! clipping library; it trades generality for zero new dependencies and
//! deterministic output.

use vcad_kernel_math::Point2;

/// Snap grid pitch in mm (1 µm).
pub const GRID_MM: f64 = 1e-3;

/// Collinearity tolerance for vertex simplification (mm²).
const COLLINEAR_TOL_MM2: f64 = 1e-6;

/// A polygon with optional holes. Outer ring CCW, holes CW (the convention
/// used by [`crate::model::Panel`]); rings are not closed (first point is
/// not repeated at the end).
#[derive(Debug, Clone, PartialEq)]
pub struct Poly {
    /// CCW outer ring.
    pub outer: Vec<Point2>,
    /// CW hole rings, each fully inside `outer`.
    pub holes: Vec<Vec<Point2>>,
}

impl Poly {
    /// Polygon with no holes.
    pub fn new(outer: Vec<Point2>) -> Self {
        Self {
            outer,
            holes: Vec::new(),
        }
    }

    /// Area of the outer ring minus the holes (mm²).
    pub fn area(&self) -> f64 {
        let mut a = signed_area_f(&self.outer).abs();
        for h in &self.holes {
            a -= signed_area_f(h).abs();
        }
        a
    }

    /// Axis-aligned bounding box `((min_x, min_y), (max_x, max_y))`.
    pub fn bbox(&self) -> ((f64, f64), (f64, f64)) {
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &self.outer {
            min.0 = min.0.min(p.x);
            min.1 = min.1.min(p.y);
            max.0 = max.0.max(p.x);
            max.1 = max.1.max(p.y);
        }
        (min, max)
    }
}

/// Even-odd point-in-polygon (outer minus holes) test in mm coordinates.
/// Boundary points are half-open (an exact-boundary probe may land either
/// way) — callers probe strictly off-boundary points.
pub fn contains_point(poly: &Poly, p: Point2) -> bool {
    if !point_in_ring_f(&poly.outer, p) {
        return false;
    }
    !poly.holes.iter().any(|h| point_in_ring_f(h, p))
}

fn point_in_ring_f(ring: &[Point2], p: Point2) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if (a.y > p.y) != (b.y > p.y) {
            let t = (p.y - a.y) / (b.y - a.y);
            if p.x < a.x + t * (b.x - a.x) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Signed area (shoelace) of an f64 ring. Positive = CCW.
pub fn signed_area_f(ring: &[Point2]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        s += a.x * b.y - b.x * a.y;
    }
    0.5 * s
}

/// Union of a set of polygons-with-holes. Returns the disjoint result
/// regions (each with CCW outer + CW holes). The flat-pattern silhouette
/// path asserts `len() == 1` and reports islands otherwise.
pub fn union_all(polys: &[Poly]) -> Vec<Poly> {
    boolean(polys, &[], BoolOp::Union)
}

/// Subject minus clip.
pub fn difference(subject: &[Poly], clip: &[Poly]) -> Vec<Poly> {
    boolean(subject, clip, BoolOp::Difference)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolOp {
    Union,
    Difference,
}

/// Integer grid point.
type IPt = (i64, i64);

fn snap(p: Point2) -> IPt {
    (
        (p.x / GRID_MM).round() as i64,
        (p.y / GRID_MM).round() as i64,
    )
}

fn unsnap(p: IPt) -> Point2 {
    Point2::new(p.0 as f64 * GRID_MM, p.1 as f64 * GRID_MM)
}

/// A snapped ring with consecutive duplicates removed. `None` if degenerate.
fn snap_ring(ring: &[Point2]) -> Option<Vec<IPt>> {
    let mut out: Vec<IPt> = Vec::with_capacity(ring.len());
    for &p in ring {
        let ip = snap(p);
        if out.last() != Some(&ip) {
            out.push(ip);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    if out.len() < 3 {
        None
    } else {
        Some(out)
    }
}

/// One input polygon after snapping — rings kept together so the inside
/// predicate can apply the outer-minus-holes rule per polygon.
struct SnappedPoly {
    rings: Vec<Vec<IPt>>, // rings[0] = outer, rest = holes
    /// Per-ring bounding box (min, max), for cheap point-probe rejection.
    bboxes: Vec<(IPt, IPt)>,
}

fn ring_bbox(ring: &[IPt]) -> (IPt, IPt) {
    let mut min = (i64::MAX, i64::MAX);
    let mut max = (i64::MIN, i64::MIN);
    for &(x, y) in ring {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    }
    (min, max)
}

fn snap_poly(p: &Poly) -> Option<SnappedPoly> {
    let outer = snap_ring(&p.outer)?;
    let mut rings = vec![outer];
    for h in &p.holes {
        if let Some(r) = snap_ring(h) {
            rings.push(r);
        }
    }
    let bboxes = rings.iter().map(|r| ring_bbox(r)).collect();
    Some(SnappedPoly { rings, bboxes })
}

/// Even-odd point-in-ring test in grid space (f64 probe vs integer ring).
fn point_in_ring(ring: &[IPt], px: f64, py: f64) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (x0, y0) = (ring[i].0 as f64, ring[i].1 as f64);
        let j = (i + 1) % n;
        let (x1, y1) = (ring[j].0 as f64, ring[j].1 as f64);
        if (y0 > py) != (y1 > py) {
            let t = (py - y0) / (y1 - y0);
            let xi = x0 + t * (x1 - x0);
            if px < xi {
                inside = !inside;
            }
        }
    }
    inside
}

/// Probe point strictly outside a ring's bbox (with 1-cell slack for the
/// f64 probe offset) can't be inside the ring.
fn probe_outside_bbox(bbox: &(IPt, IPt), px: f64, py: f64) -> bool {
    px < (bbox.0 .0 - 1) as f64
        || px > (bbox.1 .0 + 1) as f64
        || py < (bbox.0 .1 - 1) as f64
        || py > (bbox.1 .1 + 1) as f64
}

/// Point inside polygon = inside outer and not inside any hole.
fn point_in_poly(p: &SnappedPoly, px: f64, py: f64) -> bool {
    if probe_outside_bbox(&p.bboxes[0], px, py) || !point_in_ring(&p.rings[0], px, py) {
        return false;
    }
    for (h, bb) in p.rings[1..].iter().zip(&p.bboxes[1..]) {
        if !probe_outside_bbox(bb, px, py) && point_in_ring(h, px, py) {
            return false;
        }
    }
    true
}

fn point_in_any(polys: &[SnappedPoly], px: f64, py: f64) -> bool {
    polys.iter().any(|p| point_in_poly(p, px, py))
}

/// Orientation of c relative to segment a→b. Exact in i128.
fn orient(a: IPt, b: IPt, c: IPt) -> i128 {
    let abx = (b.0 - a.0) as i128;
    let aby = (b.1 - a.1) as i128;
    let acx = (c.0 - a.0) as i128;
    let acy = (c.1 - a.1) as i128;
    abx * acy - aby * acx
}

fn on_segment_collinear(a: IPt, b: IPt, c: IPt) -> bool {
    // Assumes orient(a, b, c) == 0. Is c within the bbox of [a, b]?
    c.0 >= a.0.min(b.0) && c.0 <= a.0.max(b.0) && c.1 >= a.1.min(b.1) && c.1 <= a.1.max(b.1)
}

/// Intersection points contributed by segment `(c, d)` onto segment
/// `(a, b)`, snapped to grid. Handles proper crossings, T-junctions, and
/// collinear overlaps.
fn splits_from(a: IPt, b: IPt, c: IPt, d: IPt, out: &mut Vec<IPt>) {
    let d1 = orient(c, d, a);
    let d2 = orient(c, d, b);
    let d3 = orient(a, b, c);
    let d4 = orient(a, b, d);

    if d3 == 0 && d4 == 0 {
        // Collinear: endpoints of (c, d) interior to [a, b] split it.
        if on_segment_collinear(a, b, c) {
            out.push(c);
        }
        if on_segment_collinear(a, b, d) {
            out.push(d);
        }
        return;
    }
    // T-junction: c or d lies exactly on [a, b].
    if d3 == 0 && on_segment_collinear(a, b, c) {
        out.push(c);
    }
    if d4 == 0 && on_segment_collinear(a, b, d) {
        out.push(d);
    }
    // Proper crossing. Exact i128 rational intersection with deterministic
    // rounding, so BOTH segments split at the *same* grid point regardless
    // of argument order (f64 evaluation rounds differently per order and
    // leaves loops unable to close).
    if ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0)) {
        let abx = (b.0 - a.0) as i128;
        let aby = (b.1 - a.1) as i128;
        let cdx = (d.0 - c.0) as i128;
        let cdy = (d.1 - c.1) as i128;
        let denom = abx * cdy - aby * cdx;
        if denom == 0 {
            return;
        }
        // t along a→b where the lines cross: t = cross(c−a, d−c) / denom.
        let t_num = ((c.0 - a.0) as i128) * cdy - ((c.1 - a.1) as i128) * cdx;
        let ix = div_round((a.0 as i128) * denom + t_num * abx, denom);
        let iy = div_round((a.1 as i128) * denom + t_num * aby, denom);
        out.push((ix, iy));
    }
}

/// Round-half-away-from-zero integer division, sign-normalised so the same
/// rational always rounds to the same integer.
fn div_round(n: i128, d: i128) -> i64 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    let q = if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    };
    q as i64
}

fn dedup_sorted_along(a: IPt, b: IPt, pts: &mut Vec<IPt>) {
    let key = |p: &IPt| -> i128 {
        let dx = (b.0 - a.0) as i128;
        let dy = (b.1 - a.1) as i128;
        dx * (p.0 - a.0) as i128 + dy * (p.1 - a.1) as i128
    };
    pts.sort_by_key(key);
    pts.dedup();
}

/// Uniform-grid broadphase over segment bounding boxes: `candidates(a, b)`
/// returns every segment index whose bbox shares a cell with `[a, b]`'s bbox.
/// A superset of all segments that can interact with `[a, b]` (any split
/// point lies on both segments, hence inside both bboxes).
struct SegGrid {
    cell: i64,
    min: IPt,
    cols: i64,
    rows: i64,
    /// cell index → segment indices whose bbox covers that cell.
    buckets: Vec<Vec<u32>>,
    /// Scratch stamp per segment to dedupe candidates across cells.
    stamp: std::cell::RefCell<(u64, Vec<u64>)>,
}

impl SegGrid {
    /// Target grid resolution per axis. 128×128 keeps bucket fan-out small
    /// while a board-length segment still only touches 128 cells.
    const RES: i64 = 128;

    fn new(segs: &[(IPt, IPt)]) -> Self {
        let mut min = (i64::MAX, i64::MAX);
        let mut max = (i64::MIN, i64::MIN);
        for &(a, b) in segs {
            min.0 = min.0.min(a.0).min(b.0);
            min.1 = min.1.min(a.1).min(b.1);
            max.0 = max.0.max(a.0).max(b.0);
            max.1 = max.1.max(a.1).max(b.1);
        }
        if segs.is_empty() {
            min = (0, 0);
            max = (0, 0);
        }
        let extent = (max.0 - min.0).max(max.1 - min.1).max(1);
        let cell = (extent / Self::RES).max(1);
        let cols = (max.0 - min.0) / cell + 1;
        let rows = (max.1 - min.1) / cell + 1;
        let mut buckets = vec![Vec::new(); (cols * rows) as usize];
        for (idx, &(a, b)) in segs.iter().enumerate() {
            let (c0, c1, r0, r1) = Self::cell_range(min, cell, cols, rows, a, b);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    buckets[(r * cols + c) as usize].push(idx as u32);
                }
            }
        }
        Self {
            cell,
            min,
            cols,
            rows,
            buckets,
            stamp: std::cell::RefCell::new((0, vec![0; segs.len()])),
        }
    }

    #[allow(clippy::type_complexity)]
    fn cell_range(
        min: IPt,
        cell: i64,
        cols: i64,
        rows: i64,
        a: IPt,
        b: IPt,
    ) -> (i64, i64, i64, i64) {
        let clamp = |v: i64, hi: i64| v.clamp(0, hi - 1);
        (
            clamp((a.0.min(b.0) - min.0) / cell, cols),
            clamp((a.0.max(b.0) - min.0) / cell, cols),
            clamp((a.1.min(b.1) - min.1) / cell, rows),
            clamp((a.1.max(b.1) - min.1) / cell, rows),
        )
    }

    /// Collect (deduplicated) indices of segments whose grid cells overlap
    /// the query segment's bbox cells.
    fn candidates(&self, a: IPt, b: IPt, out: &mut Vec<usize>) {
        out.clear();
        let mut guard = self.stamp.borrow_mut();
        let (ref mut tick, ref mut seen) = *guard;
        *tick += 1;
        let t = *tick;
        let (c0, c1, r0, r1) = Self::cell_range(self.min, self.cell, self.cols, self.rows, a, b);
        for r in r0..=r1 {
            for c in c0..=c1 {
                for &j in &self.buckets[(r * self.cols + c) as usize] {
                    let j = j as usize;
                    if seen[j] != t {
                        seen[j] = t;
                        out.push(j);
                    }
                }
            }
        }
    }
}

fn boolean(subject: &[Poly], clip: &[Poly], op: BoolOp) -> Vec<Poly> {
    let subj: Vec<SnappedPoly> = subject.iter().filter_map(snap_poly).collect();
    let clp: Vec<SnappedPoly> = clip.iter().filter_map(snap_poly).collect();
    if subj.is_empty() {
        return Vec::new();
    }

    // Gather all segments of the arrangement.
    let mut segs: Vec<(IPt, IPt)> = Vec::new();
    for sp in subj.iter().chain(clp.iter()) {
        for ring in &sp.rings {
            let n = ring.len();
            for i in 0..n {
                let a = ring[i];
                let b = ring[(i + 1) % n];
                if a != b {
                    segs.push((a, b));
                }
            }
        }
    }

    // Split every segment at every interaction with every other segment.
    //
    // Broadphase: a split point always lies on both segments, so a pair whose
    // bounding boxes are disjoint contributes nothing — bucket segments into a
    // uniform grid and only test pairs sharing a cell. Exact same splits as
    // the all-pairs loop, without the O(S²) pair sweep that made dense PCB
    // pours (thousands of clearance capsules) take minutes.
    let seg_grid = SegGrid::new(&segs);
    let mut candidates: Vec<usize> = Vec::new();
    let mut sub_edges: std::collections::BTreeSet<(IPt, IPt)> = std::collections::BTreeSet::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let mut cuts: Vec<IPt> = vec![a, b];
        seg_grid.candidates(a, b, &mut candidates);
        for &j in &candidates {
            if i == j {
                continue;
            }
            let (c, d) = segs[j];
            // Cheap exact bbox rejection (candidates from shared cells can
            // still be disjoint within the cell).
            if a.0.max(b.0) < c.0.min(d.0)
                || c.0.max(d.0) < a.0.min(b.0)
                || a.1.max(b.1) < c.1.min(d.1)
                || c.1.max(d.1) < a.1.min(b.1)
            {
                continue;
            }
            splits_from(a, b, c, d, &mut cuts);
        }
        dedup_sorted_along(a, b, &mut cuts);
        for w in cuts.windows(2) {
            let (p, q) = (w[0], w[1]);
            if p != q {
                let key = if p < q { (p, q) } else { (q, p) };
                sub_edges.insert(key);
            }
        }
    }

    // Region predicate in grid space.
    let inside = |px: f64, py: f64| -> bool {
        match op {
            BoolOp::Union => point_in_any(&subj, px, py) || point_in_any(&clp, px, py),
            BoolOp::Difference => point_in_any(&subj, px, py) && !point_in_any(&clp, px, py),
        }
    };

    // ── Face-cycle enumeration ───────────────────────────────────────────
    //
    // Insert every sub-edge in BOTH directions and trace the planar
    // arrangement's face cycles purely topologically: from edge (u, v) the
    // next edge of the same (left-hand) face is the outgoing edge at `v`
    // first encountered clockwise from the reversed edge. Every directed
    // edge belongs to exactly one cycle, every cycle closes by
    // construction — no per-edge geometric probing that could leave gaps.
    let mut directed: Vec<(IPt, IPt)> = Vec::with_capacity(sub_edges.len() * 2);
    for &(p, q) in &sub_edges {
        directed.push((p, q));
        directed.push((q, p));
    }
    // Lookup-only (never iterated): HashMap, not BTreeMap — the arrangement's
    // determinism comes from `sub_edges`/`boundary` ordering, not these maps.
    let mut twin: std::collections::HashMap<(IPt, IPt), usize> =
        std::collections::HashMap::with_capacity(directed.len());
    for (idx, &(u, v)) in directed.iter().enumerate() {
        twin.insert((u, v), idx);
    }
    let mut out_edges: std::collections::HashMap<IPt, Vec<usize>> =
        std::collections::HashMap::with_capacity(directed.len());
    for (idx, &(u, _)) in directed.iter().enumerate() {
        out_edges.entry(u).or_default().push(idx);
    }
    let angle_of = |u: IPt, v: IPt| -> f64 { ((v.1 - u.1) as f64).atan2((v.0 - u.0) as f64) };
    let next_edge = |cur: usize| -> usize {
        let (u, v) = directed[cur];
        let rev_angle = angle_of(v, u);
        let candidates = &out_edges[&v];
        let mut best: Option<(f64, usize)> = None;
        for &e in candidates {
            let (eu, ev) = directed[e];
            // Clockwise distance from the reversed incoming direction,
            // in (0, 2π]; the twin itself sits at distance 2π so spurs
            // bounce back only when nothing else leaves the vertex.
            let ang = angle_of(eu, ev);
            let mut delta = rev_angle - ang;
            while delta <= 1e-12 {
                delta += std::f64::consts::TAU;
            }
            while delta > std::f64::consts::TAU {
                delta -= std::f64::consts::TAU;
            }
            if best.is_none() || delta < best.unwrap().0 {
                best = Some((delta, e));
            }
        }
        best.expect("vertex with an incoming edge has an outgoing edge")
            .1
    };

    let mut cycle_of = vec![usize::MAX; directed.len()];
    let mut cycles: Vec<Vec<usize>> = Vec::new();
    for start in 0..directed.len() {
        if cycle_of[start] != usize::MAX {
            continue;
        }
        let id = cycles.len();
        let mut cycle = Vec::new();
        let mut cur = start;
        loop {
            cycle_of[cur] = id;
            cycle.push(cur);
            cur = next_edge(cur);
            if cur == start {
                break;
            }
            // A directed edge can appear in only one cycle; revisiting a
            // labelled edge that isn't the start would mean the next-edge
            // rule is inconsistent — bail rather than loop forever.
            if cycle_of[cur] != usize::MAX {
                break;
            }
        }
        cycles.push(cycle);
    }

    // ── Classify each cycle's left-hand face ─────────────────────────────
    //
    // One probe per cycle, taken at the midpoint of the cycle's LONGEST
    // edge: long edges deviate < 1 grid-step from their true line, so a
    // 2-step offset probes the correct side. (Per-edge probing dies on
    // micron slivers; per-cycle probing on the longest edge does not.)
    const DELTA: f64 = 2.0;
    let cycle_inside: Vec<bool> = cycles
        .iter()
        .map(|cycle| {
            let longest = cycle
                .iter()
                .max_by_key(|&&e| {
                    let (u, v) = directed[e];
                    let dx = (v.0 - u.0) as i128;
                    let dy = (v.1 - u.1) as i128;
                    dx * dx + dy * dy
                })
                .copied()
                .expect("cycles are non-empty");
            let (u, v) = directed[longest];
            let (px, py) = ((u.0 + v.0) as f64 * 0.5, (u.1 + v.1) as f64 * 0.5);
            let (dx, dy) = ((v.0 - u.0) as f64, (v.1 - u.1) as f64);
            let len = (dx * dx + dy * dy).sqrt();
            if len <= 0.0 {
                return false;
            }
            // Left normal of u→v — the cycle's face side.
            inside(px - dy / len * DELTA, py + dx / len * DELTA)
        })
        .collect();

    // ── Emit boundary edges between inside and outside faces ─────────────
    let mut boundary: Vec<(IPt, IPt)> = Vec::new();
    for &(p, q) in &sub_edges {
        let fwd = twin[&(p, q)];
        let rev = twin[&(q, p)];
        let left_in = cycle_inside[cycle_of[fwd]];
        let right_in = cycle_inside[cycle_of[rev]];
        match (left_in, right_in) {
            (true, false) => boundary.push((p, q)),
            (false, true) => boundary.push((q, p)),
            _ => {}
        }
    }

    if std::env::var("POLY2D_DEBUG").is_ok() {
        eprintln!(
            "[poly2d] segs={} sub_edges={} cycles={} boundary={}",
            segs.len(),
            sub_edges.len(),
            cycles.len(),
            boundary.len()
        );
    }

    // ── Walk boundary loops ──────────────────────────────────────────────
    // The boundary of a union of faces is vertex-balanced, so loops always
    // close under the same next-edge rule.
    let mut bout: std::collections::HashMap<IPt, Vec<usize>> =
        std::collections::HashMap::with_capacity(boundary.len());
    for (idx, &(u, _)) in boundary.iter().enumerate() {
        bout.entry(u).or_default().push(idx);
    }
    let mut used = vec![false; boundary.len()];
    let mut loops: Vec<Vec<IPt>> = Vec::new();
    for start in 0..boundary.len() {
        if used[start] {
            continue;
        }
        let mut ring: Vec<IPt> = Vec::new();
        let mut cur = start;
        loop {
            used[cur] = true;
            let (u, v) = boundary[cur];
            ring.push(u);
            let rev_angle = angle_of(v, u);
            let Some(candidates) = bout.get(&v) else {
                break;
            };
            let mut best: Option<(f64, usize)> = None;
            for &e in candidates {
                if used[e] && e != start {
                    continue;
                }
                let ang = angle_of(boundary[e].0, boundary[e].1);
                let mut delta = rev_angle - ang;
                while delta <= 1e-12 {
                    delta += std::f64::consts::TAU;
                }
                while delta > std::f64::consts::TAU {
                    delta -= std::f64::consts::TAU;
                }
                if best.is_none() || delta < best.unwrap().0 {
                    best = Some((delta, e));
                }
            }
            let next = match best {
                Some((_, e)) => e,
                None => break,
            };
            if next == start {
                loops.push(std::mem::take(&mut ring));
                break;
            }
            cur = next;
        }
    }

    // Simplify + convert to mm.
    let loops_mm: Vec<Vec<Point2>> = loops
        .iter()
        .map(|l| simplify_ring(l))
        .filter(|l| l.len() >= 3)
        .collect();

    // Partition into exteriors (CCW) and holes (CW); assign each hole to the
    // smallest containing exterior.
    let mut exteriors: Vec<(Vec<Point2>, f64)> = Vec::new();
    let mut holes: Vec<Vec<Point2>> = Vec::new();
    for l in loops_mm {
        let a = signed_area_f(&l);
        if a > 0.0 {
            exteriors.push((l, a));
        } else if a < 0.0 {
            holes.push(l);
        }
    }
    let mut result: Vec<Poly> = exteriors
        .iter()
        .map(|(l, _)| Poly::new(l.clone()))
        .collect();
    for h in holes {
        let probe = ring_interior_probe(&h);
        let mut best: Option<(usize, f64)> = None;
        for (i, (ext, area)) in exteriors.iter().enumerate() {
            let snapped: Vec<IPt> = ext.iter().map(|&p| snap(p)).collect();
            if point_in_ring(&snapped, probe.x / GRID_MM, probe.y / GRID_MM)
                && (best.is_none() || *area < best.unwrap().1)
            {
                best = Some((i, *area));
            }
        }
        if let Some((i, _)) = best {
            result[i].holes.push(h);
        }
        // Orphan holes (no containing exterior) are dropped — they can only
        // arise from inputs that violate the ring conventions.
    }
    result
}

/// A point strictly inside a ring: the midpoint of the segment from a convex
/// vertex slightly inward. Falls back to the first vertex for degenerate
/// rings (only used for hole→exterior assignment, where the hole's own
/// vertices already lie inside the exterior).
fn ring_interior_probe(ring: &[Point2]) -> Point2 {
    if ring.len() < 3 {
        return ring
            .first()
            .copied()
            .unwrap_or_else(|| Point2::new(0.0, 0.0));
    }
    ring[0]
}

/// Drop collinear and duplicate vertices from an integer ring; return mm.
fn simplify_ring(ring: &[IPt]) -> Vec<Point2> {
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    let mut keep: Vec<IPt> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = ring[(i + n - 1) % n];
        let cur = ring[i];
        let next = ring[(i + 1) % n];
        if cur == prev {
            continue;
        }
        let cross = orient(prev, cur, next);
        // Grid cross-product 1 unit² == 1e-6 mm² — the spec tolerance.
        if (cross.unsigned_abs() as f64) * GRID_MM * GRID_MM <= COLLINEAR_TOL_MM2 {
            continue;
        }
        keep.push(cur);
    }
    keep.iter().map(|&p| unsnap(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Poly {
        Poly::new(vec![
            Point2::new(x0, y0),
            Point2::new(x1, y0),
            Point2::new(x1, y1),
            Point2::new(x0, y1),
        ])
    }

    fn total_area(polys: &[Poly]) -> f64 {
        polys.iter().map(|p| p.area()).sum()
    }

    #[test]
    fn union_of_disjoint_rects_keeps_two_regions() {
        let out = union_all(&[rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 0.0, 30.0, 10.0)]);
        assert_eq!(out.len(), 2);
        assert!((total_area(&out) - 200.0).abs() < 1e-6);
    }

    #[test]
    fn union_of_overlapping_rects_is_one_region() {
        let out = union_all(&[rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 0.0, 15.0, 10.0)]);
        assert_eq!(out.len(), 1);
        assert!(out[0].holes.is_empty());
        assert!((out[0].area() - 150.0).abs() < 1e-6);
        // Simplified to the 4 corners of the merged rectangle.
        assert_eq!(out[0].outer.len(), 4);
    }

    #[test]
    fn union_bridged_by_thin_strip() {
        // Two panels separated by a 2 mm gap, bridged by an oversized strip —
        // the flat-pattern silhouette case.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(12.0, 0.0, 22.0, 10.0);
        let eps = 0.005;
        let strip = rect(10.0 - eps, 0.0, 12.0 + eps, 10.0);
        let out = union_all(&[a, b, strip]);
        assert_eq!(out.len(), 1, "expected a single bridged region");
        let expected = 100.0 + 100.0 + 2.0 * 10.0;
        assert!(
            (out[0].area() - expected).abs() < 0.01,
            "area {} vs {expected}",
            out[0].area()
        );
    }

    #[test]
    fn union_preserves_holes_outside_overlap() {
        let mut a = rect(0.0, 0.0, 20.0, 20.0);
        a.holes.push(vec![
            Point2::new(5.0, 5.0),
            Point2::new(5.0, 8.0),
            Point2::new(8.0, 8.0),
            Point2::new(8.0, 5.0),
        ]);
        let b = rect(15.0, 0.0, 30.0, 20.0);
        let out = union_all(&[a, b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].holes.len(), 1);
        let expected = 20.0 * 20.0 + 15.0 * 20.0 - 5.0 * 20.0 - 9.0;
        assert!((out[0].area() - expected).abs() < 1e-6);
    }

    #[test]
    fn union_fills_hole_covered_by_other_poly() {
        let mut a = rect(0.0, 0.0, 20.0, 20.0);
        a.holes.push(vec![
            Point2::new(5.0, 5.0),
            Point2::new(5.0, 8.0),
            Point2::new(8.0, 8.0),
            Point2::new(8.0, 5.0),
        ]);
        let b = rect(4.0, 4.0, 9.0, 9.0); // covers the hole entirely
        let out = union_all(&[a, b]);
        assert_eq!(out.len(), 1);
        assert!(out[0].holes.is_empty());
        assert!((out[0].area() - 400.0).abs() < 1e-6);
    }

    #[test]
    fn difference_notch_on_edge() {
        // A 2×3 notch cut into the bottom edge of a 20×10 panel.
        let panel = rect(0.0, 0.0, 20.0, 10.0);
        let notch = rect(8.0, -0.5, 10.0, 3.0);
        let out = difference(&[panel], &[notch]);
        assert_eq!(out.len(), 1);
        assert!(out[0].holes.is_empty());
        assert!((out[0].area() - (200.0 - 2.0 * 3.0)).abs() < 1e-6);
        assert_eq!(out[0].outer.len(), 8);
    }

    #[test]
    fn difference_interior_clip_creates_hole() {
        let panel = rect(0.0, 0.0, 20.0, 10.0);
        let cut = rect(8.0, 3.0, 12.0, 7.0);
        let out = difference(&[panel], &[cut]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].holes.len(), 1);
        assert!((out[0].area() - (200.0 - 16.0)).abs() < 1e-6);
    }

    #[test]
    fn difference_can_split_subject() {
        let panel = rect(0.0, 0.0, 20.0, 10.0);
        let slot = rect(9.0, -1.0, 11.0, 11.0); // full-height slot
        let out = difference(&[panel], &[slot]);
        assert_eq!(out.len(), 2);
        assert!((total_area(&out) - (200.0 - 2.0 * 10.0)).abs() < 1e-6);
    }

    #[test]
    fn collinear_shared_edges_merge_cleanly() {
        // Exactly abutting rectangles (shared edge, no overlap) — the
        // T-junction / collinear-overlap path.
        let out = union_all(&[rect(0.0, 0.0, 10.0, 10.0), rect(10.0, 0.0, 20.0, 10.0)]);
        assert_eq!(out.len(), 1);
        assert!((out[0].area() - 200.0).abs() < 1e-6);
        assert_eq!(out[0].outer.len(), 4, "collinear seam vertices dropped");
    }

    #[test]
    fn snap_grid_merges_submicron_slivers() {
        // A strip whose edge is 0.3 µm away from the panel edge — under the
        // 1 µm grid they snap together.
        let out = union_all(&[
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0000003, 0.0, 12.0, 10.0),
        ]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn union_result_orientation_conventions() {
        let mut a = rect(0.0, 0.0, 20.0, 20.0);
        a.holes.push(vec![
            Point2::new(5.0, 5.0),
            Point2::new(5.0, 10.0),
            Point2::new(10.0, 10.0),
            Point2::new(10.0, 5.0),
        ]);
        let out = union_all(&[a]);
        assert_eq!(out.len(), 1);
        assert!(signed_area_f(&out[0].outer) > 0.0, "outer must be CCW");
        assert!(signed_area_f(&out[0].holes[0]) < 0.0, "hole must be CW");
    }
}
