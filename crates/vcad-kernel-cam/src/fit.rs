//! Will the cutter fit? A contour, a diameter and a side — no toolpath.
//!
//! The part that reached the machine on 2026-09-17 was drawn for a Ø2 cutter
//! (R1.05 inside fillets) and cut with a Ø3.175 one: 24 inside corners and 8
//! outside ones kept up to 0.29 mm of extra metal, and eight of those sit on a
//! mating surface (`docs/native-app-friction-log.md` item 38). Nothing said
//! so. [`fit_contour`] is that missing answer, and it is independent of any
//! toolpath: it is a property of the shape and the tool.
//!
//! It also answers the question that used to come back as a silently shorter
//! path (item 36): does the tool centre region fall into pieces because the
//! cutter cannot pass a neck, and what is the largest tool that still passes?
//!
//! Joins are **round**, because the tool is. A mitre join reports the stator's
//! slots as separate from its bore at every diameter — the mistake the Python
//! prototype made first.

use serde::{Deserialize, Serialize};

use crate::verify2d::{
    clean_loop, march, point_segment_distance, signed_area, Loop2, Poly, VerifyError,
};

/// Which side of the contour the cutter runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContourSide {
    /// Inside an opening: the tool centre region is the contour eroded by the
    /// tool radius.
    Inside,
    /// Around a part: the tool centre region is everything outside the contour
    /// grown by the tool radius.
    Outside,
}

/// How finely the offset curve is sampled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetOptions {
    /// Spacing of the samples along the raw offset curve (mm). Straight runs
    /// are exact; this bounds the error where the curve is trimmed.
    pub step: f64,
    /// Slack on the validity test (mm).
    pub tolerance: f64,
    /// Points the offset curve can lose without moving more than this are
    /// dropped (mm). The dense sampling is only needed to find where the curve
    /// folds back on itself; carrying it into the distance queries afterwards
    /// costs an order of magnitude for nothing.
    pub simplify: f64,
}

impl Default for OffsetOptions {
    fn default() -> Self {
        Self {
            step: 0.01,
            tolerance: 1e-9,
            simplify: 1e-4,
        }
    }
}

/// Drop points that sit within `eps` of the line through their surviving
/// neighbours.
fn decimate(points: &[[f64; 2]], eps: f64) -> Loop2 {
    if eps <= 0.0 || points.len() < 4 {
        return points.to_vec();
    }
    let mut out: Loop2 = Vec::with_capacity(points.len());
    let mut anchor = points[0];
    out.push(anchor);
    let mut i = 1;
    while i + 1 < points.len() {
        let (mid, next) = (points[i], points[i + 1]);
        if crate::verify2d::point_segment_distance(mid, anchor, next) < eps {
            i += 1; // `mid` adds nothing between `anchor` and `next`
        } else {
            out.push(mid);
            anchor = mid;
            i += 1;
        }
    }
    out.push(points[points.len() - 1]);
    out
}

/// Settings for [`fit_contour`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitOptions {
    /// Offset sampling.
    pub offset: OffsetOptions,
    /// Grid pitch for the corner areas (mm). Areas are measured by marching
    /// squares on this pitch and come out slightly *under* the truth, because
    /// the leftover metal feathers out to nothing where it meets the wall:
    /// about 0.6 % at 0.02 mm and 0.3 % at 0.01 mm on the stator.
    pub grid: f64,
    /// Corners smaller than this are noise, not metal (mm²). A speck of
    /// 0.1 × 0.1 mm is also about the size of the fragments the grid itself
    /// shaves off a real corner's tip, so this doubles as the filter that
    /// keeps the corner *count* from following the pitch.
    pub min_area: f64,
    /// Search step when hunting for the diameter at which a neck closes (mm).
    pub neck_step: f64,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            offset: OffsetOptions::default(),
            grid: 0.02,
            min_area: 0.01,
            neck_step: 0.02,
        }
    }
}

/// A place the tool centre region pinches: the passage's full width.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Neck {
    /// Width of the passage (mm): the largest tool that gets through it.
    pub width: f64,
    /// Where it is (mm).
    pub at: [f64; 2],
}

/// One corner the cutter cannot reach.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UnreachableCorner {
    /// Metal left there (mm²).
    pub area: f64,
    /// Centroid (mm).
    pub centroid: [f64; 2],
    /// How far it stands off the wall at its worst (mm).
    pub standoff: f64,
}

/// What a whole side's unreachable corners add up to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CornerSummary {
    /// How many corners keep metal.
    pub count: usize,
    /// Total metal left (mm²).
    pub total_area: f64,
    /// The worst single corner (mm²).
    pub largest_area: f64,
    /// Farthest any of them stands off its wall (mm).
    pub max_standoff: f64,
    /// Each corner, largest first.
    pub corners: Vec<UnreachableCorner>,
}

/// Whether the cutter fits, and what it leaves behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitReport {
    /// The diameter asked about (mm).
    pub tool_diameter: f64,
    /// Which side the cutter runs on.
    pub side: ContourSide,
    /// Pieces the tool centre region falls into. More than one means the
    /// cutter cannot follow the contour in a single loop.
    pub centre_pieces: usize,
    /// The cutter fits at all: the centre region is non-empty and whole.
    pub fits: bool,
    /// The tightest passage, when the shape has one.
    pub min_neck: Option<Neck>,
    /// Clearance each side of the cutter at that passage (mm). Negative means
    /// it does not go through.
    pub slot_clearance_per_side: Option<f64>,
    /// Largest diameter that still passes every neck (mm). `None` when nothing
    /// constrains it.
    pub largest_tool_diameter: Option<f64>,
    /// Corners this diameter cannot reach.
    pub unreachable: CornerSummary,
}

/// Offset a closed loop by `delta` — positive grows the polygon, negative
/// shrinks it — with round joins, returning the pieces it falls into.
///
/// The raw offset curve is sampled, the samples that end up nearer the
/// original than `|delta|` are dropped (those are the self-intersection lobes
/// that every offsetter has to trim), and the surviving runs are stitched back
/// together at the crossings. That is what lets a shrink return *several*
/// loops: the stator's slots leaving its bore.
pub(crate) fn offset_loop(points: &[[f64; 2]], delta: f64, opts: &OffsetOptions) -> Vec<Loop2> {
    let mut src = clean_loop(points);
    if src.len() < 3 {
        return Vec::new();
    }
    if signed_area(&src) < 0.0 {
        src.reverse();
    }
    if delta.abs() < 1e-12 {
        return vec![src];
    }
    let Ok(poly) = Poly::new(vec![src.clone()]) else {
        return Vec::new();
    };
    let n = src.len();
    let step = opts.step.max(1e-6);

    // Outward normal of edge i (the polygon is counter-clockwise, so the
    // interior is on the left and (dy, -dx) points out).
    let normal = |i: usize| {
        let (a, b) = (src[i], src[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        if l <= 0.0 {
            [0.0, 0.0]
        } else {
            [dy / l, -dx / l]
        }
    };

    let mut raw: Vec<[f64; 2]> = Vec::new();
    for i in 0..n {
        let prev = (i + n - 1) % n;
        let (np, ni) = (normal(prev), normal(i));
        let p = src[i];
        // Turn direction at this vertex.
        let (ax, ay) = (p[0] - src[prev][0], p[1] - src[prev][1]);
        let (bx, by) = (src[(i + 1) % n][0] - p[0], src[(i + 1) % n][1] - p[1]);
        let cross = ax * by - ay * bx;
        if cross * delta > 0.0 {
            // The offset edges diverge here: the tool rolls around the vertex.
            let a0 = np[1].atan2(np[0]);
            let a1 = ni[1].atan2(ni[0]);
            let mut sweep = a1 - a0;
            while sweep > std::f64::consts::PI {
                sweep -= std::f64::consts::TAU;
            }
            while sweep < -std::f64::consts::PI {
                sweep += std::f64::consts::TAU;
            }
            let steps = ((sweep.abs() * delta.abs() / step).ceil() as usize).max(1);
            for k in 0..=steps {
                let a = a0 + sweep * k as f64 / steps as f64;
                raw.push([p[0] + delta * a.cos(), p[1] + delta * a.sin()]);
            }
        } else {
            raw.push([p[0] + delta * np[0], p[1] + delta * np[1]]);
            raw.push([p[0] + delta * ni[0], p[1] + delta * ni[1]]);
        }
        // Walk the offset edge itself.
        let q = src[(i + 1) % n];
        let len = (q[0] - p[0]).hypot(q[1] - p[1]);
        let steps = ((len / step).ceil() as usize).max(1);
        for k in 1..steps {
            let t = k as f64 / steps as f64;
            raw.push([
                p[0] + (q[0] - p[0]) * t + delta * ni[0],
                p[1] + (q[1] - p[1]) * t + delta * ni[1],
            ]);
        }
    }

    // Trim: every point of a true offset curve is exactly |delta| from the
    // original and on the offset's own side of it. The self-intersection lobes
    // fail the first test — some other edge is nearer than the one that
    // generated them — and a raw offset that has crossed the shape entirely
    // fails the second.
    let keep: Vec<bool> = raw
        .iter()
        .map(|p| {
            let sd = poly.signed_distance(*p);
            sd.abs() >= delta.abs() - opts.tolerance
                && if delta > 0.0 {
                    sd <= opts.tolerance
                } else {
                    sd >= -opts.tolerance
                }
        })
        .collect();
    if keep.iter().all(|k| !k) {
        return Vec::new();
    }
    if keep.iter().all(|k| *k) {
        return vec![decimate(&clean_loop(&raw), opts.simplify)];
    }

    // Cyclic runs of surviving samples.
    let m = raw.len();
    let start = (0..m).find(|&i| keep[i] && !keep[(i + m - 1) % m]).unwrap();
    let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    for k in 0..m {
        let i = (start + k) % m;
        if keep[i] {
            cur.push(raw[i]);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs.retain(|r| r.len() >= 2);
    if runs.is_empty() {
        return Vec::new();
    }

    // Stitch each run's end to a run start: at a self-crossing the curve
    // leaves one branch and rejoins another, and the two ends meet there.
    // Shortest pairs first, globally — taking each run's nearest free start in
    // program order instead lets an early run steal a partner and stitch a
    // chord across a corner, which is how the stator's R1.05 fillets lost
    // 0.9 % of their leftover metal before this was written this way.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(runs.len() * runs.len());
    for (i, run) in runs.iter().enumerate() {
        let end = *run.last().unwrap();
        for (j, other) in runs.iter().enumerate() {
            let s = other[0];
            pairs.push(((s[0] - end[0]).hypot(s[1] - end[1]), i, j));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut next = vec![usize::MAX; runs.len()];
    let mut used = vec![false; runs.len()];
    for (_, i, j) in pairs {
        if next[i] == usize::MAX && !used[j] {
            next[i] = j;
            used[j] = true;
        }
    }

    // The two branches meet at a point neither run contains: the last sample
    // before the crossing and the first one after it both stop short of it.
    // Joining them with a chord rounds the corner off — 0.008 mm at the
    // stator's fillets, which is 0.9 % of the metal they leave — so the
    // crossing is solved for and put back. A point that is not at the offset
    // distance is not a crossing and is dropped.
    let sagitta = step * step / (2.0 * delta.abs().max(step));
    let corner = |i: usize, j: usize| -> Option<[f64; 2]> {
        let a = &runs[i];
        let b = &runs[j];
        let (p1, p2) = (a[a.len() - 2], a[a.len() - 1]);
        let (q1, q2) = (b[1], b[0]);
        let (r1, r2) = (
            [p2[0] - p1[0], p2[1] - p1[1]],
            [q2[0] - q1[0], q2[1] - q1[1]],
        );
        let den = r1[0] * r2[1] - r1[1] * r2[0];
        if den.abs() < 1e-12 {
            return None;
        }
        let t = ((q2[0] - p2[0]) * r2[1] - (q2[1] - p2[1]) * r2[0]) / den;
        if !(0.0..=10.0 * delta.abs() / step.max(1e-9)).contains(&t) {
            return None;
        }
        let x = [p2[0] + r1[0] * t, p2[1] + r1[1] * t];
        let sd = poly.signed_distance(x);
        (sd.abs() >= delta.abs() - sagitta - opts.tolerance).then_some(x)
    };

    let mut seen = vec![false; runs.len()];
    let mut out = Vec::new();
    for i in 0..runs.len() {
        if seen[i] {
            continue;
        }
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let mut j = i;
        while j != usize::MAX && !seen[j] {
            seen[j] = true;
            pts.extend_from_slice(&runs[j]);
            let k = next[j];
            if k != usize::MAX && runs[j].len() >= 2 && runs[k].len() >= 2 {
                if let Some(x) = corner(j, k) {
                    pts.push(x);
                }
            }
            j = k;
        }
        let l = decimate(&clean_loop(&pts), opts.simplify);
        if l.len() >= 3 && signed_area(&l).abs() > 1e-9 {
            out.push(l);
        }
    }
    out
}

/// How many pieces the tool centre region falls into at radius `r`, and the
/// pieces themselves.
fn centre_region(
    loop_: &[[f64; 2]],
    r: f64,
    side: ContourSide,
    opts: &OffsetOptions,
) -> Vec<Loop2> {
    match side {
        ContourSide::Inside => offset_loop(loop_, -r, opts),
        ContourSide::Outside => offset_loop(loop_, r, opts),
    }
}

/// Cutter-fit report for one contour, one diameter and one side.
///
/// `loop_` is a closed polyline. For [`ContourSide::Inside`] it is an opening
/// and the cutter works within it; for [`ContourSide::Outside`] it is the part
/// and the cutter works around it.
pub fn fit_contour(
    loop_: &[[f64; 2]],
    tool_diameter: f64,
    side: ContourSide,
    opts: &FitOptions,
) -> Result<FitReport, VerifyError> {
    if tool_diameter <= 0.0 {
        return Err(VerifyError::BadInput(format!(
            "tool diameter {tool_diameter} must be positive"
        )));
    }
    let src = clean_loop(loop_);
    if src.len() < 3 {
        return Err(VerifyError::DegenerateLoop(src.len()));
    }
    let r = tool_diameter / 2.0;
    let wall = Poly::new(vec![src.clone()])?;
    let bbox = wall.bbox();

    let pieces = centre_region(&src, r, side, &opts.offset);
    let centre_pieces = pieces.len();
    let fits = centre_pieces == 1;

    // Unreachable corners: the wall metal no disc of this radius can touch
    // without leaving the region the tool centre is allowed in.
    let (grid_box, unreachable) = match side {
        ContourSide::Inside => {
            let g = [
                bbox[0] - opts.grid,
                bbox[1] - opts.grid,
                bbox[2] + opts.grid,
                bbox[3] + opts.grid,
            ];
            (g, pieces.clone())
        }
        ContourSide::Outside => {
            let m = r + 2.0 * opts.grid;
            let g = [bbox[0] - m, bbox[1] - m, bbox[2] + m, bbox[3] + m];
            (g, pieces.clone())
        }
    };
    let centre = if unreachable.is_empty() {
        None
    } else {
        Some(Poly::new(unreachable)?)
    };

    let mut corners: Vec<UnreachableCorner> = Vec::new();
    if let Some(centre) = &centre {
        let regions = match side {
            // Inside: metal at p survives if no allowed tool centre comes
            // within r of it.
            ContourSide::Inside => march(grid_box, opts.grid, |p| {
                let dw = wall.signed_distance(p);
                let de = -centre.signed_distance(p);
                (dw.min(de - r), dw)
            }),
            // Outside: the closing of the part — what a disc rolling around
            // the outside cannot sweep out of the concave corners.
            ContourSide::Outside => march(grid_box, opts.grid, |p| {
                let dw = -wall.signed_distance(p);
                let dg = centre.signed_distance(p);
                (dw.min(dg - r), dw)
            }),
        };
        for c in regions {
            if c.area < opts.min_area {
                continue;
            }
            corners.push(UnreachableCorner {
                area: c.area,
                centroid: c.centroid,
                standoff: c.peak_aux,
            });
        }
    }
    corners.sort_by(|a, b| b.area.total_cmp(&a.area));
    let summary = CornerSummary {
        count: corners.len(),
        total_area: corners.iter().map(|c| c.area).sum(),
        largest_area: corners.first().map(|c| c.area).unwrap_or(0.0),
        max_standoff: corners.iter().map(|c| c.standoff).fold(0.0, f64::max),
        corners,
    };

    // The largest tool that still passes every neck: the radius at which the
    // centre region stops being one piece. Only an inside contour has necks —
    // outside, the cutter works in open air.
    let (largest, neck) = match side {
        ContourSide::Outside => (None, None),
        ContourSide::Inside => {
            let limit = ((bbox[2] - bbox[0]).min(bbox[3] - bbox[1]) / 2.0).max(opts.neck_step);
            // A coarse sampling is enough to count pieces; the bisection that
            // follows is where the precision goes.
            let coarse = OffsetOptions {
                step: (opts.offset.step * 4.0).min(0.05),
                ..opts.offset.clone()
            };
            let count = |rr: f64| offset_loop(&src, -rr, &coarse).len();
            let mut lo = opts.neck_step.min(limit / 2.0);
            let mut hi = None;
            let mut rr = lo;
            while rr < limit {
                if count(rr) != 1 {
                    hi = Some(rr);
                    break;
                }
                lo = rr;
                rr += opts.neck_step;
            }
            match hi {
                None => (None, None),
                Some(mut hi) => {
                    for _ in 0..40 {
                        let mid = (lo + hi) / 2.0;
                        if count(mid) == 1 {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                        if hi - lo < 1e-6 {
                            break;
                        }
                    }
                    let split = offset_loop(&src, -hi, &coarse);
                    let at = closest_between_pieces(&split)
                        .unwrap_or([(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0]);
                    (
                        Some(2.0 * lo),
                        Some(Neck {
                            width: 2.0 * lo,
                            at,
                        }),
                    )
                }
            }
        }
    };

    Ok(FitReport {
        tool_diameter,
        side,
        centre_pieces,
        fits: fits && !pieces_empty(&pieces),
        min_neck: neck,
        slot_clearance_per_side: neck.map(|n| (n.width - tool_diameter) / 2.0),
        largest_tool_diameter: largest,
        unreachable: summary,
    })
}

/// The stator outline the first real job was cut from, and the loader for it.
#[cfg(test)]
pub(crate) mod fixture {
    use super::Loop2;

    /// Closed `LWPOLYLINE` loops from a DXF, largest area first.
    ///
    /// Group codes 10/20 are the vertex coordinates and 70 bit 1 marks the
    /// polyline closed; anything else in the file is ignored, which is all the
    /// fixture writer emits.
    pub(crate) fn dxf_loops(text: &str) -> Vec<Loop2> {
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let mut out: Vec<Loop2> = Vec::new();
        let mut cur: Option<Loop2> = None;
        let mut i = 0;
        while i + 1 < lines.len() {
            let (code, value) = (lines[i], lines[i + 1]);
            match code {
                "0" => {
                    if let Some(l) = cur.take() {
                        out.push(l);
                    }
                    if value == "LWPOLYLINE" {
                        cur = Some(Vec::new());
                    }
                }
                "10" => {
                    if let Some(l) = cur.as_mut() {
                        l.push([value.parse().unwrap(), f64::NAN]);
                    }
                }
                "20" => {
                    if let Some(l) = cur.as_mut() {
                        l.last_mut().unwrap()[1] = value.parse().unwrap();
                    }
                }
                _ => {}
            }
            i += 2;
        }
        if let Some(l) = cur {
            out.push(l);
        }
        for l in &out {
            assert!(l.iter().all(|p| p[1].is_finite()), "a vertex lost its Y");
        }
        out.sort_by(|a, b| {
            super::signed_area(b)
                .abs()
                .total_cmp(&super::signed_area(a).abs())
        });
        out
    }

    /// The 1 outer + 4 holes of `docs/cam-fixtures/stator-outline.dxf`, in the
    /// part frame the DXF was written in.
    pub(crate) fn stator() -> Vec<Loop2> {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-outline.dxf"
        ));
        let loops = dxf_loops(text);
        assert_eq!(loops.len(), 5, "the stator fixture is 1 outer + 4 holes");
        loops
    }

    /// The same, moved so the outline's lower-left corner is the stock origin.
    pub(crate) fn stator_in_stock_frame() -> Vec<Loop2> {
        let loops = stator();
        let (ox, oy) = loops[0]
            .iter()
            .fold((f64::INFINITY, f64::INFINITY), |(x, y), p| {
                (x.min(p[0]), y.min(p[1]))
            });
        loops
            .iter()
            .map(|l| l.iter().map(|p| [p[0] - ox, p[1] - oy]).collect())
            .collect()
    }
}

fn pieces_empty(pieces: &[Loop2]) -> bool {
    pieces.is_empty()
}

/// Midpoint of the closest pair of points taken from two different pieces.
fn closest_between_pieces(pieces: &[Loop2]) -> Option<[f64; 2]> {
    if pieces.len() < 2 {
        return None;
    }
    let mut best = (f64::INFINITY, [0.0, 0.0]);
    for i in 0..pieces.len() {
        for j in (i + 1)..pieces.len() {
            for p in &pieces[i] {
                for k in 0..pieces[j].len() {
                    let (a, b) = (pieces[j][k], pieces[j][(k + 1) % pieces[j].len()]);
                    let d = point_segment_distance(*p, a, b);
                    if d < best.0 {
                        // Good enough: the pinch is where the two pieces almost
                        // touch, so either point is within a sample of it.
                        best = (d, [(p[0] + a[0]) / 2.0, (p[1] + a[1]) / 2.0]);
                    }
                }
            }
        }
    }
    Some(best.1)
}

#[cfg(test)]
mod tests {
    use super::fixture::{stator, stator_in_stock_frame};
    use super::*;

    fn square(size: f64) -> Loop2 {
        vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]]
    }

    fn area_of(loops: &[Loop2]) -> f64 {
        loops.iter().map(|l| signed_area(l)).sum()
    }

    /// Growing a square by r adds a band of perimeter*r plus one full disc of
    /// corner arcs — the round-join identity, and the cheapest proof that the
    /// offsetter is not quietly mitring.
    #[test]
    fn grow_a_square_by_round_joins() {
        let s = square(20.0);
        let r = 3.0;
        let out = offset_loop(&s, r, &OffsetOptions::default());
        assert_eq!(out.len(), 1);
        let want = 400.0 + 4.0 * 20.0 * r + std::f64::consts::PI * r * r;
        assert!(
            (area_of(&out) - want).abs() < 0.01,
            "grown area {} vs {want}",
            area_of(&out)
        );
    }

    /// Shrinking is the same identity in reverse, and a shrink past the
    /// inradius must come back empty rather than inside-out.
    #[test]
    fn shrink_a_square_to_nothing() {
        let s = square(20.0);
        let out = offset_loop(&s, -4.0, &OffsetOptions::default());
        assert_eq!(out.len(), 1);
        assert!((area_of(&out) - 144.0).abs() < 0.01, "{}", area_of(&out));
        assert!(offset_loop(&s, -10.5, &OffsetOptions::default()).is_empty());
    }

    /// The defect behind friction item 36: the cutter does not fit through the
    /// neck, so the tool-centre region falls in two. A mitre join would report
    /// this for every diameter; a round one only past the real width.
    #[test]
    fn a_neck_splits_the_centre_region_at_its_own_width() {
        // Two 20 mm squares joined by a 4 mm neck.
        let dumbbell = vec![
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 8.0],
            [30.0, 8.0],
            [30.0, 0.0],
            [50.0, 0.0],
            [50.0, 20.0],
            [30.0, 20.0],
            [30.0, 12.0],
            [20.0, 12.0],
            [20.0, 20.0],
            [0.0, 20.0],
        ];
        let opts = FitOptions::default();
        let thin = fit_contour(&dumbbell, 3.0, ContourSide::Inside, &opts).unwrap();
        assert_eq!(thin.centre_pieces, 1, "a D3 cutter passes a 4 mm neck");
        assert!(thin.fits);
        let fat = fit_contour(&dumbbell, 5.0, ContourSide::Inside, &opts).unwrap();
        assert_eq!(fat.centre_pieces, 2, "a D5 cutter does not");
        assert!(!fat.fits);
        let neck = thin.min_neck.expect("the dumbbell has a neck");
        assert!(
            (neck.width - 4.0).abs() < 0.02,
            "neck measured {} mm, the gap is 4",
            neck.width
        );
        assert!(
            (neck.at[0] - 25.0).abs() < 1.0 && (neck.at[1] - 10.0).abs() < 1.0,
            "neck at {:?}, the gap is at (25, 10)",
            neck.at
        );
        assert!((thin.largest_tool_diameter.unwrap() - 4.0).abs() < 0.02);
        assert!((thin.slot_clearance_per_side.unwrap() - 0.5).abs() < 0.02);
    }

    /// A tool that fits exactly reaches everything; a square has no corner a
    /// disc cannot reach from outside.
    #[test]
    fn a_convex_shape_has_no_unreachable_corners() {
        let rep = fit_contour(
            &square(20.0),
            6.0,
            ContourSide::Outside,
            &FitOptions::default(),
        )
        .unwrap();
        assert_eq!(rep.unreachable.count, 0, "{:?}", rep.unreachable);
        assert_eq!(rep.largest_tool_diameter, None);
        let rep = fit_contour(
            &square(20.0),
            6.0,
            ContourSide::Inside,
            &FitOptions::default(),
        )
        .unwrap();
        // Four corners, each a square minus a quarter disc of radius 3.
        let want = 4.0 * (9.0 - std::f64::consts::PI * 9.0 / 4.0);
        assert_eq!(rep.unreachable.count, 4);
        assert!(
            (rep.unreachable.total_area - want).abs() < 0.02,
            "{} vs {want}",
            rep.unreachable.total_area
        );
        // The deepest the corner metal reaches from the wall is on the
        // diagonal, where the cutter's centre can get no closer than r: at
        // r - r/sqrt(2). A grid this pitch reads it a node short.
        let standoff = 3.0 - 3.0 / std::f64::consts::SQRT_2;
        assert!(
            (standoff - rep.unreachable.max_standoff).abs() < 0.03,
            "{} vs {standoff}",
            rep.unreachable.max_standoff
        );
    }

    /// Outside a part, the metal that stays is in the concave corners, and a
    /// right-angled notch has an answer that can be written down: a square of
    /// side r less a quarter disc of radius r, at each of its two floor
    /// corners.
    #[test]
    fn a_notch_keeps_metal_in_its_two_floor_corners() {
        let notched = vec![
            [0.0, 0.0],
            [26.0, 0.0],
            [26.0, 6.0],
            [34.0, 6.0],
            [34.0, 0.0],
            [60.0, 0.0],
            [60.0, 40.0],
            [0.0, 40.0],
        ];
        let rep = fit_contour(&notched, 6.0, ContourSide::Outside, &FitOptions::default()).unwrap();
        assert_eq!(rep.unreachable.count, 2, "{:?}", rep.unreachable);
        let want = 9.0 - std::f64::consts::PI * 9.0 / 4.0;
        for c in &rep.unreachable.corners {
            assert!(
                (want - c.area).abs() < 0.03,
                "corner at {:?} keeps {:.4} mm², the closed form is {want:.4}",
                c.centroid,
                c.area
            );
        }
        // Both corners, not one: the first draft of the offsetter kept the
        // fold-in lobes and reported neither.
        let xs: Vec<f64> = rep
            .unreachable
            .corners
            .iter()
            .map(|c| c.centroid[0])
            .collect();
        assert!(
            xs.iter().any(|x| (*x - 26.7).abs() < 0.5)
                && xs.iter().any(|x| (*x - 33.3).abs() < 0.5),
            "{xs:?}"
        );
    }

    /// The real part, the real cutter, the numbers the Python prototype
    /// printed: 24 inside corners, ~10.7 mm² of metal, 0.29 mm at worst.
    #[test]
    fn stator_inside_corners_a_d3175_cutter_cannot_reach() {
        let loops = stator();
        let opts = FitOptions::default();
        let rep = fit_contour(&loops[1], 3.175, ContourSide::Inside, &opts).unwrap();
        assert_eq!(
            rep.centre_pieces, 1,
            "a D3.175 cutter passes the slot mouths"
        );
        assert_eq!(rep.unreachable.count, 24, "one per slot corner pair");
        assert!(
            (10.55..10.75).contains(&rep.unreachable.total_area),
            "left {:.4} mm², shapely's converged answer is 10.65",
            rep.unreachable.total_area
        );
        assert!(
            (0.28..0.30).contains(&rep.unreachable.max_standoff),
            "stand-off {:.4} mm, the prototype measured 0.294",
            rep.unreachable.max_standoff
        );
        assert!(
            (0.43..0.46).contains(&rep.unreachable.largest_area),
            "largest corner {:.4} mm², the prototype measured 0.4447",
            rep.unreachable.largest_area
        );
    }

    /// The eight outside corners are where the clocking tabs meet the ring —
    /// a mating surface, which is why they are worth a line of their own.
    #[test]
    fn stator_outside_corners_sit_on_the_clocking_tabs() {
        let loops = stator();
        let rep = fit_contour(
            &loops[0],
            3.175,
            ContourSide::Outside,
            &FitOptions::default(),
        )
        .unwrap();
        assert_eq!(rep.unreachable.count, 8);
        assert!(
            (1.80..1.90).contains(&rep.unreachable.total_area),
            "outside corners {:.4} mm², the prototype measured 1.845",
            rep.unreachable.total_area
        );
    }

    /// Friction item 36's number, from the shape rather than from the log:
    /// the slot mouths are 3.87 mm, so a D3.175 passes with 0.35 mm a side and
    /// anything from D3.9 up does not.
    #[test]
    fn stator_largest_tool_that_still_passes_the_slot_mouths() {
        let loops = stator_in_stock_frame();
        let rep = fit_contour(
            &loops[1],
            3.175,
            ContourSide::Inside,
            &FitOptions::default(),
        )
        .unwrap();
        let d = rep.largest_tool_diameter.expect("the slots are necks");
        assert!(
            (3.85..3.90).contains(&d),
            "largest passing tool D{d:.4}, the mouths are 3.87 mm"
        );
        assert!(
            (rep.slot_clearance_per_side.unwrap() - 0.35).abs() < 0.02,
            "clearance {:?}",
            rep.slot_clearance_per_side
        );
        let fat = fit_contour(&loops[1], 3.9, ContourSide::Inside, &FitOptions::default()).unwrap();
        assert!(fat.centre_pieces > 1, "D3.9 should not pass the mouths");
    }
}
