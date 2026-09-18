//! Arc fitting: runs of short `G1` moves become `G1`/`G2`/`G3`.
//!
//! Contours arrive from the kernel as polylines — a bore is a few hundred
//! chords, a fillet a few dozen. Grbl chokes on that (planner buffer, per-line
//! serial latency), so the machine stutters around every curve. This module
//! replays a toolpath and replaces runs of consecutive [`ToolpathSegment::Linear`]
//! moves with the smallest set of lines and XY arcs that stays inside a
//! tolerance band around the original polyline.
//!
//! The tolerance is checked **both ways**, which is the whole point:
//!
//! - every original vertex is within `tolerance` of the fitted primitive, and
//! - the fitted primitive is within `tolerance` of the original polyline
//!   *everywhere*, including between vertices.
//!
//! The second check is what stops a gouge. Any three points define a circle
//! exactly, so a vertex-only check happily turns a sharp corner into an arc
//! that cuts the corner off. The bulge is measured at each chord midpoint,
//! where an arc's departure from its chord is largest.
//!
//! A consequence worth knowing: a polyline is *inside* the curve it
//! approximates, by its own sagitta. Re-fitting the curve therefore "bulges"
//! outward by that sagitta, so a coarse polyline (chord error bigger than
//! `tolerance`) is deliberately left as lines. Feed the fitter a polyline
//! whose own chord error is comfortably below the fit tolerance.
//!
//! Nothing is fitted across a Z change, a feed change, a rapid, a spindle,
//! dwell, coolant, tool-change or comment segment, a direction reversal, or a
//! tab lift (which is a Z change). Anything that cannot be fitted stays
//! exactly as it was — the fitter never errors and never shortens a path.

use crate::{ArcDir, ArcPlane, Toolpath, ToolpathSegment};
use std::f64::consts::{PI, TAU};

/// Two positions closer than this are the same point (mm).
const POINT_EPS: f64 = 1e-9;
/// Z values within this are the same level (mm).
const Z_EPS: f64 = 1e-9;
/// Feeds within this are the same feed (mm/min).
const FEED_EPS: f64 = 1e-9;
/// Slack on the sweep limit, in radians, so an exactly-180° arc is allowed.
const SWEEP_EPS: f64 = 1e-6;

/// Options for [`fit_arcs`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcFitOptions {
    /// Maximum allowed deviation between the fitted path and the original
    /// polyline, in mm, checked in both directions (mm).
    pub tolerance: f64,
    /// Arcs smaller than this radius are refused and stay linear (mm). Grbl
    /// turns a very small arc into a fistful of tiny segments anyway.
    pub min_radius: f64,
    /// Arcs larger than this radius are refused and stay linear (mm); a huge
    /// radius is a straight line with round-off on it.
    pub max_radius: f64,
    /// Maximum sweep of a single arc, in radians. Must be less than a full
    /// turn; values at or above `2π` are clamped. The default of `π` splits a
    /// full circle into two half-circle arcs, and keeps the I/J round-off of
    /// any one arc small.
    pub max_sweep: f64,
    /// An arc must replace at least this many original linear segments. Two
    /// segments (three points) always fit some circle exactly, so replacing
    /// them buys nothing and only risks rounding a corner.
    pub min_arc_segments: usize,
}

impl Default for ArcFitOptions {
    fn default() -> Self {
        Self {
            tolerance: 0.005,
            min_radius: 0.05,
            max_radius: 10_000.0,
            max_sweep: PI,
            min_arc_segments: 3,
        }
    }
}

impl ArcFitOptions {
    /// Options with the given tolerance and everything else at its default.
    pub fn with_tolerance(tolerance: f64) -> Self {
        Self {
            tolerance,
            ..Self::default()
        }
    }

    /// The sweep limit actually used: the requested one, kept below a full
    /// turn so a fitted arc is never ambiguous to the controller.
    fn sweep_limit(&self) -> f64 {
        self.max_sweep.clamp(0.0, TAU - 1e-3)
    }
}

/// What [`fit_arcs`] did, in numbers a caller can assert on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ArcFitReport {
    /// Segments in the input toolpath.
    pub segments_in: usize,
    /// Segments in the output toolpath.
    pub segments_out: usize,
    /// Linear segments in the input (the only ones eligible for fitting).
    pub linear_in: usize,
    /// Arcs the fitter created.
    pub arcs_emitted: usize,
    /// Linear segments the fitter created (merged runs of collinear moves).
    pub linears_emitted: usize,
    /// Largest deviation measured while accepting a fit, in mm. This is the
    /// worst of the vertex and chord-midpoint checks over every primitive the
    /// fitter emitted, so it is bounded by `options.tolerance`.
    pub max_deviation: f64,
}

/// Fit arcs and merged lines into a toolpath.
///
/// See the [module documentation](self) for what is and is not fitted. The
/// returned toolpath ends at exactly the same point as the input: every
/// emitted primitive keeps the original coordinates of the vertex it lands on.
pub fn fit_arcs(toolpath: &Toolpath, options: &ArcFitOptions) -> Toolpath {
    fit_arcs_reported(toolpath, options).0
}

/// [`fit_arcs`], with the measurements.
pub fn fit_arcs_reported(toolpath: &Toolpath, options: &ArcFitOptions) -> (Toolpath, ArcFitReport) {
    let segs = &toolpath.segments;
    let mut out = Toolpath::new();
    let mut report = ArcFitReport {
        segments_in: segs.len(),
        linear_in: segs
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Linear { .. }))
            .count(),
        ..ArcFitReport::default()
    };

    let mut pos = [0.0_f64; 3];
    let mut i = 0;
    while i < segs.len() {
        // A run is consecutive Linear moves at one Z and one feed. Anything
        // else — rapid, plunge, spindle, dwell, comment, an arc that is
        // already there — ends the run and is copied through untouched.
        if let ToolpathSegment::Linear { to, feed } = &segs[i] {
            if (to[2] - pos[2]).abs() <= Z_EPS {
                let z = pos[2];
                let feed = *feed;
                let mut points: Vec<[f64; 2]> = vec![[pos[0], pos[1]]];
                let mut j = i;
                let mut run_end = [pos[0], pos[1]];
                while let Some(ToolpathSegment::Linear { to, feed: f }) = segs.get(j) {
                    if (f - feed).abs() > FEED_EPS || (to[2] - z).abs() > Z_EPS {
                        break;
                    }
                    let last = points[points.len() - 1];
                    // A move that does not move carries no geometry; dropping
                    // it keeps the fit arithmetic away from zero-length chords.
                    if (to[0] - last[0]).hypot(to[1] - last[1]) > POINT_EPS {
                        points.push([to[0], to[1]]);
                    }
                    run_end = [to[0], to[1]];
                    j += 1;
                }
                // Where the run ends is not negotiable, even if the last move
                // was a hair shorter than POINT_EPS and got dropped above: a
                // contour that closes a micron short leaves a nib on the part.
                let tail = points.len() - 1;
                points[tail] = run_end;

                let run_len = j - i;
                if run_len >= 2 && points.len() >= 3 {
                    fit_run(&points, z, feed, options, &mut out, &mut report);
                } else {
                    // One move (or a run of no-ops): copy it verbatim rather
                    // than re-deriving coordinates that are already right.
                    out.extend(segs[i..j].iter().cloned());
                }
                pos = [points[points.len() - 1][0], points[points.len() - 1][1], z];
                i = j;
                continue;
            }
        }

        out.push(segs[i].clone());
        if let Some(t) = segs[i].target() {
            pos = t;
        }
        i += 1;
    }

    report.segments_out = out.len();
    (out, report)
}

/// Greedily cover `points` with the longest line or arc that fits, from the
/// front, emitting into `out`.
fn fit_run(
    points: &[[f64; 2]],
    z: f64,
    feed: f64,
    options: &ArcFitOptions,
    out: &mut Toolpath,
    report: &mut ArcFitReport,
) {
    let last = points.len() - 1;
    let mut a = 0;
    while a < last {
        // A line is always available: two points are always collinear.
        let line_end = grow(a + 1, last, |b| line_fit(points, a, b, options).is_some());
        let arc_end = if last - a >= options.min_arc_segments.max(2) {
            grow_arc(points, a, last, options)
        } else {
            a
        };

        // Longer coverage wins; a tie goes to the line, which is cheaper for
        // the controller and cannot bulge.
        let use_arc =
            arc_end > a && arc_end - a >= options.min_arc_segments.max(2) && arc_end > line_end;

        if use_arc {
            let fit = arc_fit(points, a, arc_end, options)
                .expect("the accepted arc was validated by the same check");
            out.push(ToolpathSegment::Arc {
                to: [points[arc_end][0], points[arc_end][1], z],
                center: [
                    fit.center[0] - points[a][0],
                    fit.center[1] - points[a][1],
                    0.0,
                ],
                plane: ArcPlane::Xy,
                dir: if fit.ccw { ArcDir::Ccw } else { ArcDir::Cw },
                feed,
            });
            report.arcs_emitted += 1;
            report.max_deviation = report.max_deviation.max(fit.deviation);
            a = arc_end;
        } else {
            let deviation = line_fit(points, a, line_end, options)
                .expect("the accepted line was validated by the same check");
            out.push(ToolpathSegment::Linear {
                to: [points[line_end][0], points[line_end][1], z],
                feed,
            });
            report.linears_emitted += 1;
            report.max_deviation = report.max_deviation.max(deviation);
            a = line_end;
        }
    }
}

/// Largest `b` in `[lo, hi]` with `fits(b)`, given `fits(lo)` holds.
///
/// Doubling then bisection, so a primitive covering `k` points costs
/// `O(log k)` checks of `O(k)` each instead of `O(k)` of them. Feasibility is
/// not guaranteed monotone in `b`, so this can stop short of the true maximum
/// — but every `b` it returns has been checked, so a fit is never accepted on
/// faith.
fn grow(lo: usize, hi: usize, mut fits: impl FnMut(usize) -> bool) -> usize {
    let mut best = lo;
    let mut step = 1;
    while best < hi {
        let candidate = (best + step).min(hi);
        if fits(candidate) {
            best = candidate;
            step *= 2;
        } else {
            // Bisect (best, candidate): the answer is below `candidate`.
            let mut low = best + 1;
            let mut high = candidate - 1;
            while low <= high {
                let mid = low + (high - low) / 2;
                if fits(mid) {
                    best = mid;
                    low = mid + 1;
                } else {
                    if mid == 0 {
                        break;
                    }
                    high = mid - 1;
                }
            }
            break;
        }
    }
    best
}

/// Largest `b` such that `points[a..=b]` fits one arc, or `a` if none does.
fn grow_arc(points: &[[f64; 2]], a: usize, hi: usize, options: &ArcFitOptions) -> usize {
    let first = a + options.min_arc_segments.max(2);
    if first > hi || arc_fit(points, a, first, options).is_none() {
        return a;
    }
    grow(first, hi, |b| arc_fit(points, a, b, options).is_some())
}

/// Deviation of `points[a..=b]` from the straight segment between its ends,
/// or `None` if the run does not fit a line within tolerance.
fn line_fit(points: &[[f64; 2]], a: usize, b: usize, options: &ArcFitOptions) -> Option<f64> {
    if b <= a {
        return None;
    }
    let (p0, p1) = (points[a], points[b]);
    let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let len = dx.hypot(dy);
    if len <= POINT_EPS {
        // The run comes back to where it started: a line cannot represent it.
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let mut worst: f64 = 0.0;
    let mut previous_along = f64::NEG_INFINITY;
    for p in &points[a..=b] {
        let (vx, vy) = (p[0] - p0[0], p[1] - p0[1]);
        let along = vx * ux + vy * uy;
        // A run that doubles back on itself is not one line, whatever the
        // perpendicular distances say.
        if along < previous_along - POINT_EPS {
            return None;
        }
        previous_along = along;
        if along < -options.tolerance || along > len + options.tolerance {
            return None;
        }
        let across = (vx * uy - vy * ux).abs();
        if across > options.tolerance {
            return None;
        }
        worst = worst.max(across);
    }
    Some(worst)
}

/// An accepted arc fit.
struct ArcFit {
    center: [f64; 2],
    ccw: bool,
    deviation: f64,
}

/// Fit one arc to `points[a..=b]`, or `None` if it cannot be done inside the
/// tolerance and the radius/sweep limits.
fn arc_fit(points: &[[f64; 2]], a: usize, b: usize, options: &ArcFitOptions) -> Option<ArcFit> {
    if b < a + 2 {
        return None;
    }
    // A circle through the two ends and the middle point: the ends sit on it
    // exactly, so the arc starts and ends where the polyline does and the
    // start and end radii the controller checks agree to the last bit.
    let mid = a + (b - a) / 2;
    let (center, radius) = circle_through(points[a], points[mid], points[b])?;
    if !(options.min_radius..=options.max_radius).contains(&radius) {
        return None;
    }

    // Turn direction from the middle point; every step must then advance in
    // that direction, which is what rules out an S-curve or a doubling back.
    let (ax, ay) = (points[mid][0] - points[a][0], points[mid][1] - points[a][1]);
    let (bx, by) = (points[b][0] - points[mid][0], points[b][1] - points[mid][1]);
    let ccw = ax * by - ay * bx > 0.0;

    let angle = |p: &[f64; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
    let limit = options.sweep_limit();
    let mut sweep = 0.0_f64;
    let mut previous = angle(&points[a]);
    let mut worst: f64 = 0.0;
    for k in a..=b {
        let p = points[k];
        // Vertex on the circle?
        let deviation = ((p[0] - center[0]).hypot(p[1] - center[1]) - radius).abs();
        if deviation > options.tolerance {
            return None;
        }
        worst = worst.max(deviation);

        if k > a {
            let theta = angle(&p);
            let step = if ccw {
                (theta - previous).rem_euclid(TAU)
            } else {
                (previous - theta).rem_euclid(TAU)
            };
            if step <= 0.0 || step >= PI {
                return None;
            }
            sweep += step;
            if sweep > limit + SWEEP_EPS {
                return None;
            }
            previous = theta;

            // The arc between two vertices bulges away from the chord; at the
            // chord midpoint that bulge is largest. Unchecked, this is how a
            // corner gets rounded off and a wall gets gouged.
            let previous_point = points[k - 1];
            let m = [
                (previous_point[0] + p[0]) * 0.5,
                (previous_point[1] + p[1]) * 0.5,
            ];
            let bulge = (radius - (m[0] - center[0]).hypot(m[1] - center[1])).abs();
            if bulge > options.tolerance {
                return None;
            }
            worst = worst.max(bulge);
        }
    }

    Some(ArcFit {
        center,
        ccw,
        deviation: worst,
    })
}

/// Centre and radius of the circle through three points, or `None` when they
/// are collinear (or so nearly so that the centre is not representable).
fn circle_through(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64)> {
    // Work relative to `a`: the coordinates that matter are the small ones.
    let (bx, by) = (b[0] - a[0], b[1] - a[1]);
    let (cx, cy) = (c[0] - a[0], c[1] - a[1]);
    let d = 2.0 * (bx * cy - by * cx);
    if d == 0.0 {
        return None;
    }
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    let ux = (cy * b2 - by * c2) / d;
    let uy = (bx * c2 - cx * b2) / d;
    if !ux.is_finite() || !uy.is_finite() {
        return None;
    }
    let center = [a[0] + ux, a[1] + uy];
    let radius = ux.hypot(uy);
    if !radius.is_finite() || radius == 0.0 {
        return None;
    }
    Some((center, radius))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::post::{GrblPost, PostProcessor};
    use crate::{CamSettings, Tool};

    // ---------------------------------------------------------------- helpers

    /// A primitive of a toolpath, recovered by replaying it: what the machine
    /// would actually travel.
    #[derive(Debug, Clone)]
    enum Prim {
        Line {
            a: [f64; 2],
            b: [f64; 2],
        },
        Arc {
            center: [f64; 2],
            radius: f64,
            start: f64,
            sweep: f64,
            ccw: bool,
        },
    }

    impl Prim {
        /// Analytic distance from a point to this primitive.
        fn distance(&self, p: [f64; 2]) -> f64 {
            match self {
                Prim::Line { a, b } => point_to_segment(p, *a, *b),
                Prim::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                    ccw,
                } => {
                    let theta = (p[1] - center[1]).atan2(p[0] - center[0]);
                    let along = if *ccw {
                        (theta - start).rem_euclid(TAU)
                    } else {
                        (start - theta).rem_euclid(TAU)
                    };
                    if along <= *sweep {
                        ((p[0] - center[0]).hypot(p[1] - center[1]) - radius).abs()
                    } else {
                        let ends = [self.point_at(0.0), self.point_at(1.0)];
                        ends.iter()
                            .map(|e| (p[0] - e[0]).hypot(p[1] - e[1]))
                            .fold(f64::INFINITY, f64::min)
                    }
                }
            }
        }

        /// Point at parameter `t` in `[0, 1]`.
        fn point_at(&self, t: f64) -> [f64; 2] {
            match self {
                Prim::Line { a, b } => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
                Prim::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                    ccw,
                } => {
                    let theta = if *ccw {
                        start + sweep * t
                    } else {
                        start - sweep * t
                    };
                    [
                        center[0] + radius * theta.cos(),
                        center[1] + radius * theta.sin(),
                    ]
                }
            }
        }

        fn length(&self) -> f64 {
            match self {
                Prim::Line { a, b } => (b[0] - a[0]).hypot(b[1] - a[1]),
                Prim::Arc { radius, sweep, .. } => radius * sweep,
            }
        }
    }

    fn point_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        if len2 <= 0.0 {
            return (p[0] - a[0]).hypot(p[1] - a[1]);
        }
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
        (p[0] - (a[0] + dx * t)).hypot(p[1] - (a[1] + dy * t))
    }

    /// Replay a toolpath into XY cutting primitives at a given Z.
    fn cutting_prims(tp: &Toolpath, z: f64) -> Vec<Prim> {
        cutting_prims_with_feed(tp, z)
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    /// The same, keeping the feed each primitive is cut at.
    fn cutting_prims_with_feed(tp: &Toolpath, z: f64) -> Vec<(Prim, f64)> {
        let mut prims = Vec::new();
        let mut pos = [0.0_f64; 3];
        for seg in &tp.segments {
            match seg {
                ToolpathSegment::Linear { to, feed } => {
                    if (to[2] - z).abs() < 1e-9 && (pos[2] - z).abs() < 1e-9 {
                        prims.push((
                            Prim::Line {
                                a: [pos[0], pos[1]],
                                b: [to[0], to[1]],
                            },
                            *feed,
                        ));
                    }
                    pos = *to;
                }
                ToolpathSegment::Arc {
                    to,
                    center,
                    dir,
                    feed,
                    ..
                } => {
                    if (to[2] - z).abs() < 1e-9 && (pos[2] - z).abs() < 1e-9 {
                        let c = [pos[0] + center[0], pos[1] + center[1]];
                        let radius = center[0].hypot(center[1]);
                        let start = (pos[1] - c[1]).atan2(pos[0] - c[0]);
                        let end = (to[1] - c[1]).atan2(to[0] - c[0]);
                        let ccw = matches!(dir, ArcDir::Ccw);
                        let sweep = if ccw {
                            (end - start).rem_euclid(TAU)
                        } else {
                            (start - end).rem_euclid(TAU)
                        };
                        prims.push((
                            Prim::Arc {
                                center: c,
                                radius,
                                start,
                                sweep,
                                ccw,
                            },
                            *feed,
                        ));
                    }
                    pos = *to;
                }
                other => {
                    if let Some(t) = other.target() {
                        pos = t;
                    }
                }
            }
        }
        prims
    }

    /// Worst distance from the fitted path to the original polyline, sampled
    /// densely along every fitted primitive, and back the other way from every
    /// original vertex to the fitted path. Both directions matter: the first
    /// catches a bulge between vertices, the second a vertex left behind.
    fn two_way_deviation(fitted: &[Prim], polyline: &[[f64; 2]]) -> (f64, f64) {
        let mut forward: f64 = 0.0;
        for prim in fitted {
            let samples = ((prim.length() / 0.02).ceil() as usize).clamp(8, 4000);
            for s in 0..=samples {
                let p = prim.point_at(s as f64 / samples as f64);
                let mut best = f64::INFINITY;
                for w in polyline.windows(2) {
                    best = best.min(point_to_segment(p, w[0], w[1]));
                    if best == 0.0 {
                        break;
                    }
                }
                forward = forward.max(best);
            }
        }
        let mut backward: f64 = 0.0;
        for p in polyline {
            let best = fitted
                .iter()
                .map(|prim| prim.distance(*p))
                .fold(f64::INFINITY, f64::min);
            backward = backward.max(best);
        }
        (forward, backward)
    }

    /// Build a toolpath that plunges and then walks a polyline at `z`.
    fn polyline_path(points: &[[f64; 2]], z: f64, feed: f64) -> Toolpath {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(points[0][0], points[0][1], 5.0));
        tp.push(ToolpathSegment::linear(
            points[0][0],
            points[0][1],
            z,
            200.0,
        ));
        for p in &points[1..] {
            tp.push(ToolpathSegment::linear(p[0], p[1], z, feed));
        }
        tp
    }

    /// Sample a circular arc with a chord sagitta no larger than `sag`.
    fn sample_arc(
        center: [f64; 2],
        radius: f64,
        start: f64,
        sweep: f64,
        ccw: bool,
        sag: f64,
        out: &mut Vec<[f64; 2]>,
    ) {
        // sagitta ≈ chord² / 8r  ⇒  chord = sqrt(8 r sag)
        let chord = (8.0 * radius * sag).sqrt();
        let steps = ((radius * sweep / chord).ceil() as usize).max(2);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let theta = if ccw {
                start + sweep * t
            } else {
                start - sweep * t
            };
            push_point(
                out,
                [
                    center[0] + radius * theta.cos(),
                    center[1] + radius * theta.sin(),
                ],
            );
        }
    }

    fn push_point(out: &mut Vec<[f64; 2]>, p: [f64; 2]) {
        if let Some(last) = out.last() {
            if (last[0] - p[0]).hypot(last[1] - p[1]) < 1e-9 {
                return;
            }
        }
        out.push(p);
    }

    /// A circle as `steps` chords, closing exactly on its first point. The
    /// step count is explicit (and even) because the fitter's sweep limit is
    /// half a turn: an even circle is two arcs, an odd one is two arcs and a
    /// leftover chord, and a test that cannot say which is testing nothing.
    fn circle_polyline(center: [f64; 2], radius: f64, steps: usize) -> Vec<[f64; 2]> {
        assert!(steps.is_multiple_of(2));
        let mut pts: Vec<[f64; 2]> = (0..steps)
            .map(|k| {
                let theta = TAU * k as f64 / steps as f64;
                [
                    center[0] + radius * theta.cos(),
                    center[1] + radius * theta.sin(),
                ]
            })
            .collect();
        pts.push(pts[0]);
        // The polyline is inscribed: it misses the true circle by this much
        // between vertices, and that error is part of the fit's budget.
        let sagitta = radius * (1.0 - (PI / steps as f64).cos());
        assert!(
            sagitta < 0.001,
            "fixture sagitta {sagitta:.6} eats the tolerance"
        );
        pts
    }

    fn polyline_length(points: &[[f64; 2]]) -> f64 {
        points
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum()
    }

    // ------------------------------------------------------------------ tests

    /// A polylined circle comes back as two half-circle arcs, not 700 lines,
    /// and the metal it would cut is the same to better than the tolerance.
    #[test]
    fn test_polylined_circle_becomes_two_arcs_within_tolerance() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([30.0, 30.0], 20.0, 720);
        assert!(pts.len() > 300, "fixture too coarse: {} points", pts.len());
        let tp = polyline_path(&pts, -1.0, 800.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);

        assert_eq!(
            report.arcs_emitted, 2,
            "{} arcs + {} lines from {} moves",
            report.arcs_emitted, report.linears_emitted, report.linear_in
        );
        assert_eq!(report.linears_emitted, 0);

        let prims = cutting_prims(&fitted, -1.0);
        assert_eq!(prims.len(), 2);
        for prim in &prims {
            match prim {
                Prim::Arc { radius, sweep, .. } => {
                    assert!((radius - 20.0).abs() < 1e-6, "radius {radius}");
                    assert!((sweep - PI).abs() < 1e-6, "sweep {sweep}");
                }
                other => panic!("expected an arc, got {other:?}"),
            }
        }

        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(
            forward <= options.tolerance && backward <= options.tolerance,
            "deviation fitted->polyline {forward:.6}, polyline->fitted {backward:.6}"
        );
        assert!(report.max_deviation <= options.tolerance);

        // Circumference: the polyline is inscribed, so the arcs are a hair
        // longer — by much less than the fit tolerance.
        let fitted_len: f64 = prims.iter().map(Prim::length).sum();
        let original_len = polyline_length(&pts);
        assert!(
            (fitted_len - original_len).abs() <= options.tolerance,
            "length {fitted_len:.6} vs {original_len:.6}"
        );
    }

    /// A rounded rectangle: four straight sides and four corner arcs, out of a
    /// polyline that knows nothing about either.
    #[test]
    fn test_polylined_rounded_rectangle_becomes_four_lines_and_four_arcs() {
        let options = ArcFitOptions::default();
        let (w, h, r) = (60.0, 40.0, 8.0);
        let mut pts: Vec<[f64; 2]> = Vec::new();
        // Start on the bottom edge, run counter-clockwise.
        push_point(&mut pts, [r, 0.0]);
        push_point(&mut pts, [w - r, 0.0]);
        sample_arc([w - r, r], r, -PI / 2.0, PI / 2.0, true, 0.0005, &mut pts);
        push_point(&mut pts, [w, h - r]);
        sample_arc([w - r, h - r], r, 0.0, PI / 2.0, true, 0.0005, &mut pts);
        push_point(&mut pts, [r, h]);
        sample_arc([r, h - r], r, PI / 2.0, PI / 2.0, true, 0.0005, &mut pts);
        push_point(&mut pts, [0.0, r]);
        sample_arc([r, r], r, PI, PI / 2.0, true, 0.0005, &mut pts);
        push_point(&mut pts, [r, 0.0]);

        let tp = polyline_path(&pts, -2.0, 900.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        let prims = cutting_prims(&fitted, -2.0);

        let arcs = prims
            .iter()
            .filter(|p| matches!(p, Prim::Arc { .. }))
            .count();
        let lines = prims.len() - arcs;
        assert_eq!(
            (arcs, lines),
            (4, 4),
            "{} primitives from {} moves: {prims:#?}",
            prims.len(),
            report.linear_in
        );
        // The corner radius is recovered exactly. The sweep can fall a sample
        // or two short of 90°: right at the tangent point the first corner
        // vertex is within tolerance of the straight side, so the greedy fit
        // hands it to the line. That is a different split, not a worse path —
        // the two-way deviation below is what says the path is right.
        let step = r * PI / 2.0 / 71.0;
        for prim in &prims {
            if let Prim::Arc { radius, sweep, .. } = prim {
                assert!((radius - r).abs() < 1e-9, "corner radius {radius}");
                assert!(
                    *sweep <= PI / 2.0 + 1e-9 && *sweep >= PI / 2.0 - 3.0 * step / r,
                    "corner sweep {sweep} (quarter turn is {})",
                    PI / 2.0
                );
            }
        }

        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(
            forward <= options.tolerance && backward <= options.tolerance,
            "deviation {forward:.6} / {backward:.6}"
        );

        let fitted_len: f64 = prims.iter().map(Prim::length).sum();
        let original_len = polyline_length(&pts);
        assert!(
            (fitted_len - original_len).abs() <= options.tolerance,
            "perimeter {fitted_len:.6} vs {original_len:.6}"
        );
    }

    /// A square corner is not an arc, however well three points fit a circle.
    /// This is the midpoint check earning its keep.
    #[test]
    fn test_square_corner_survives_as_two_lines() {
        let options = ArcFitOptions::default();
        let pts = [
            [0.0, 0.0],
            [10.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [20.0, 20.0],
        ];
        let tp = polyline_path(&pts, -1.0, 700.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        assert_eq!(report.arcs_emitted, 0, "{fitted:#?}");
        assert_eq!(report.linears_emitted, 2, "{fitted:#?}");
        let prims = cutting_prims(&fitted, -1.0);
        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(forward < 1e-9 && backward < 1e-9, "{forward} / {backward}");
    }

    /// A retrace doubles back along its own line. Every vertex is on that
    /// line, so a perpendicular-distance check alone would merge the lot into
    /// one move and throw away half the cutting — and, on a slot, half the
    /// metal that was supposed to come out.
    #[test]
    fn test_a_retrace_is_not_merged_away() {
        let options = ArcFitOptions::default();
        let pts = [
            [0.0, 0.0],
            [10.0, 0.0],
            [20.0, 0.0],
            [10.0, 0.0],
            [0.0, 0.0],
        ];
        let tp = polyline_path(&pts, -1.0, 700.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        assert_eq!(report.arcs_emitted, 0, "{fitted:#?}");
        let prims = cutting_prims(&fitted, -1.0);
        let travelled: f64 = prims.iter().map(Prim::length).sum();
        assert!(
            (travelled - 40.0).abs() < 1e-9,
            "travelled {travelled} mm of 40"
        );
    }

    /// An S-curve reverses curvature at the inflection. One arc cannot span
    /// it; two must, and the join must sit at the inflection point.
    #[test]
    fn test_s_curve_splits_at_the_inflection() {
        let options = ArcFitOptions::default();
        let r = 10.0;
        let mut pts = Vec::new();
        // Left half: CCW around (0, r), from the bottom to the top.
        sample_arc([0.0, r], r, -PI / 2.0, PI, true, 0.0005, &mut pts);
        let inflection = [0.0, 2.0 * r];
        // Right half: CW around (0, 3r), continuing tangentially.
        sample_arc([0.0, 3.0 * r], r, -PI / 2.0, PI, false, 0.0005, &mut pts);

        let tp = polyline_path(&pts, -1.0, 600.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        let prims = cutting_prims(&fitted, -1.0);

        assert_eq!(report.arcs_emitted, 2, "{prims:#?}");
        assert_eq!(report.linears_emitted, 0, "{prims:#?}");
        match (&prims[0], &prims[1]) {
            (Prim::Arc { ccw: first, .. }, Prim::Arc { ccw: second, .. }) => {
                assert!(*first && !*second, "curvature sign not split");
            }
            other => panic!("expected two arcs, got {other:?}"),
        }
        // The join lands on the inflection, to within the sample spacing: the
        // two circles are tangent there, so the first vertex past the
        // inflection is still within tolerance of the first circle and the
        // greedy fit may keep it. It cannot keep two — by the second the
        // curvature difference is four times the tolerance.
        let step = (pts[1][0] - pts[0][0]).hypot(pts[1][1] - pts[0][1]);
        let join = prims[0].point_at(1.0);
        let slip = (join[0] - inflection[0]).hypot(join[1] - inflection[1]);
        assert!(
            slip <= 1.5 * step,
            "join at {join:?} is {slip:.4} mm past the inflection {inflection:?} \
             (sample spacing {step:.4})"
        );

        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(
            forward <= options.tolerance && backward <= options.tolerance,
            "deviation {forward:.6} / {backward:.6}"
        );
    }

    /// Two depth passes of the same circle are two pairs of arcs, never one
    /// arc that spirals through the Z step.
    #[test]
    fn test_nothing_is_fitted_across_a_z_step() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([0.0, 0.0], 12.0, 600);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(pts[0][0], pts[0][1], 5.0));
        for z in [-1.0, -2.0] {
            tp.push(ToolpathSegment::linear(pts[0][0], pts[0][1], z, 200.0));
            for p in &pts[1..] {
                tp.push(ToolpathSegment::linear(p[0], p[1], z, 800.0));
            }
        }
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        assert_eq!(report.arcs_emitted, 4, "{report:?}");

        // Every arc stays on one level, and the plunges are still there.
        let mut pos = [0.0, 0.0, 5.0];
        let mut plunges = Vec::new();
        for seg in &fitted.segments {
            if let ToolpathSegment::Arc { to, .. } = seg {
                assert!(
                    (to[2] - pos[2]).abs() < 1e-12,
                    "arc climbs from {} to {}",
                    pos[2],
                    to[2]
                );
            }
            if let ToolpathSegment::Linear { to, .. } = seg {
                if (to[2] - pos[2]).abs() > 1e-12 {
                    plunges.push(to[2]);
                }
            }
            if let Some(t) = seg.target() {
                pos = t;
            }
        }
        assert_eq!(plunges, vec![-1.0, -2.0], "plunges lost or merged");

        for z in [-1.0, -2.0] {
            let prims = cutting_prims(&fitted, z);
            assert_eq!(prims.len(), 2);
            let (forward, backward) = two_way_deviation(&prims, &pts);
            assert!(forward <= options.tolerance && backward <= options.tolerance);
        }
    }

    /// A tab lifts the cutter mid-contour. The lift, the travel over the tab
    /// and the drop back down must survive, and no arc may span them.
    #[test]
    fn test_nothing_is_fitted_across_a_tab_lift() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([0.0, 0.0], 15.0, 640);
        let split = pts.len() / 2;
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(pts[0][0], pts[0][1], 5.0));
        tp.push(ToolpathSegment::linear(pts[0][0], pts[0][1], -3.0, 200.0));
        for p in &pts[1..split] {
            tp.push(ToolpathSegment::linear(p[0], p[1], -3.0, 800.0));
        }
        // Tab: up to -1, across at -1, back down to -3.
        let tab_entry = pts[split - 1];
        let tab_exit = pts[split];
        tp.push(ToolpathSegment::linear(
            tab_entry[0],
            tab_entry[1],
            -1.0,
            200.0,
        ));
        tp.push(ToolpathSegment::linear(
            tab_exit[0],
            tab_exit[1],
            -1.0,
            800.0,
        ));
        tp.push(ToolpathSegment::linear(
            tab_exit[0],
            tab_exit[1],
            -3.0,
            200.0,
        ));
        for p in &pts[split + 1..] {
            tp.push(ToolpathSegment::linear(p[0], p[1], -3.0, 800.0));
        }

        let (fitted, _) = fit_arcs_reported(&tp, &options);

        // The lift, the move at tab height and the drop are all still there,
        // in order, at their original coordinates.
        let lift: Vec<[f64; 3]> = fitted
            .segments
            .iter()
            .filter_map(|s| s.target())
            .filter(|t| (t[2] + 1.0).abs() < 1e-12)
            .collect();
        assert_eq!(
            lift,
            vec![
                [tab_entry[0], tab_entry[1], -1.0],
                [tab_exit[0], tab_exit[1], -1.0]
            ],
            "tab lift altered"
        );

        // No fitted arc touches the tab height, and the cutting arcs on each
        // side of the tab stop at the tab.
        let mut pos = [0.0_f64, 0.0, 5.0];
        for seg in &fitted.segments {
            if let ToolpathSegment::Arc { to, .. } = seg {
                assert!((to[2] + 3.0).abs() < 1e-12 && (pos[2] + 3.0).abs() < 1e-12);
            }
            if let Some(t) = seg.target() {
                pos = t;
            }
        }
        let prims = cutting_prims(&fitted, -3.0);
        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(
            forward <= options.tolerance && backward <= options.tolerance,
            "{forward} / {backward}"
        );
    }

    /// A feed change inside a run is a machining decision (a lead-in, a
    /// corner slow-down). Fitting across it would throw the change away.
    #[test]
    fn test_nothing_is_fitted_across_a_feed_change() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([0.0, 0.0], 18.0, 700);
        let half = pts.len() / 2;
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(pts[0][0], pts[0][1], 5.0));
        tp.push(ToolpathSegment::linear(pts[0][0], pts[0][1], -1.0, 200.0));
        for (k, p) in pts[1..].iter().enumerate() {
            let feed = if k < half { 400.0 } else { 900.0 };
            tp.push(ToolpathSegment::linear(p[0], p[1], -1.0, feed));
        }
        let (fitted, _) = fit_arcs_reported(&tp, &options);
        for seg in &fitted.segments {
            if let ToolpathSegment::Arc { feed, .. } = seg {
                assert!(
                    (*feed - 400.0).abs() < 1e-9 || (*feed - 900.0).abs() < 1e-9,
                    "arc at feed {feed}"
                );
            }
        }
        // The slow stretch and the fast stretch each still cover their own
        // share of the circumference: the feed boundary did not move, and no
        // arc smeared one feed over the other's metal.
        let cut = cutting_prims_with_feed(&fitted, -1.0);
        let slow: f64 = cut
            .iter()
            .filter(|(_, feed)| (feed - 400.0).abs() < 1e-9)
            .map(|(p, _)| p.length())
            .sum();
        let fast: f64 = cut
            .iter()
            .filter(|(_, feed)| (feed - 900.0).abs() < 1e-9)
            .map(|(p, _)| p.length())
            .sum();
        let circumference = TAU * 18.0;
        let expected_slow = circumference * half as f64 / (pts.len() - 1) as f64;
        assert!(
            (slow - expected_slow).abs() < 0.01,
            "slow stretch {slow:.4}, expected {expected_slow:.4}"
        );
        assert!(
            (slow + fast - circumference).abs() < 0.01,
            "{slow:.4} + {fast:.4} vs circumference {circumference:.4}"
        );
    }

    /// The fitted path must end exactly where the original did — bit for bit,
    /// not within a tolerance. A contour that closes 3 µm short leaves a nib.
    #[test]
    fn test_end_point_is_bit_identical() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([7.3, -11.9], 9.75, 500);
        let tp = polyline_path(&pts, -1.5, 850.0);
        let (fitted, _) = fit_arcs_reported(&tp, &options);
        let original_end = tp.segments.last().unwrap().target().unwrap();
        let fitted_end = fitted.segments.last().unwrap().target().unwrap();
        assert_eq!(fitted_end, original_end);
    }

    /// Radius bounds are a refusal, not a clamp: a bore smaller than the
    /// minimum stays linear rather than being fitted to some other circle.
    #[test]
    fn test_radius_bounds_refuse_rather_than_clamp() {
        let options = ArcFitOptions {
            min_radius: 2.0,
            ..ArcFitOptions::default()
        };
        let pts = circle_polyline([0.0, 0.0], 1.0, 200);
        let tp = polyline_path(&pts, -1.0, 400.0);
        let (fitted, report) = fit_arcs_reported(&tp, &options);
        assert_eq!(report.arcs_emitted, 0);
        // The bore is still a bore: chords may be merged where the polyline
        // runs straight to within tolerance, but the path still goes all the
        // way round and stays inside the tolerance band.
        let prims = cutting_prims(&fitted, -1.0);
        let (forward, backward) = two_way_deviation(&prims, &pts);
        assert!(
            forward <= options.tolerance && backward <= options.tolerance,
            "{forward} / {backward}"
        );
        let fitted_len: f64 = prims.iter().map(Prim::length).sum();
        // An absolute bound, not one that grows with however many primitives
        // the fit happened to emit: a closed curve re-drawn inside a band of
        // half-width `t` changes its length by at most `2πt` (the length of a
        // curve offset by `t` is `2πt` longer), so that is the geometry's own
        // answer and it does not move when the fitter does. Measured here:
        // 0.0090 mm against a 0.0314 mm bound.
        assert!(
            (fitted_len - polyline_length(&pts)).abs() <= TAU * options.tolerance,
            "circumference {fitted_len:.6} vs {:.6}",
            polyline_length(&pts)
        );
    }

    /// A coarse polyline is *inside* the curve it approximates. Re-fitting the
    /// curve would cut outside it by the polyline's own sagitta, so it is
    /// refused: a fit is only allowed when it cannot gouge.
    #[test]
    fn test_a_coarse_polyline_is_left_alone() {
        let options = ArcFitOptions::default();
        // 32 chords on r = 20: sagitta ≈ 20 * (1 - cos(π/32)) = 0.096 mm,
        // twenty times the tolerance.
        let mut pts = Vec::new();
        for k in 0..=32 {
            let theta = TAU * k as f64 / 32.0;
            pts.push([20.0 * theta.cos(), 20.0 * theta.sin()]);
        }
        let sagitta = 20.0 * (1.0 - (PI / 32.0).cos());
        assert!(sagitta > 0.09, "fixture sagitta {sagitta}");
        let tp = polyline_path(&pts, -1.0, 800.0);
        let (_, report) = fit_arcs_reported(&tp, &options);
        assert_eq!(report.arcs_emitted, 0, "gouged by {sagitta:.4} mm");
    }

    /// Rapids, spindle, dwell, coolant and comments are copied through in
    /// order, and a run never reaches across one.
    #[test]
    fn test_non_motion_segments_are_preserved_in_place() {
        let options = ArcFitOptions::default();
        let pts = circle_polyline([0.0, 0.0], 10.0, 520);
        let half = pts.len() / 2;
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::comment("contour"));
        tp.push(ToolpathSegment::spindle_on(12000.0, crate::SpindleDir::Cw));
        tp.push(ToolpathSegment::dwell(1.0));
        tp.push(ToolpathSegment::rapid(pts[0][0], pts[0][1], 5.0));
        tp.push(ToolpathSegment::linear(pts[0][0], pts[0][1], -1.0, 200.0));
        for p in &pts[1..half] {
            tp.push(ToolpathSegment::linear(p[0], p[1], -1.0, 800.0));
        }
        tp.push(ToolpathSegment::comment("half way"));
        for p in &pts[half..] {
            tp.push(ToolpathSegment::linear(p[0], p[1], -1.0, 800.0));
        }
        let (fitted, _) = fit_arcs_reported(&tp, &options);

        let order: Vec<String> = fitted
            .segments
            .iter()
            .filter_map(|s| match s {
                ToolpathSegment::Comment { text } => Some(format!("c:{text}")),
                ToolpathSegment::Spindle { rpm, .. } => Some(format!("s:{rpm}")),
                ToolpathSegment::Dwell { seconds } => Some(format!("d:{seconds}")),
                ToolpathSegment::Rapid { .. } => Some("r".to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            order,
            vec!["c:contour", "s:12000", "d:1", "r", "c:half way"],
            "non-motion segments moved"
        );
        // The comment splits the circle, so neither arc spans it.
        let comment_at = fitted
            .segments
            .iter()
            .position(|s| matches!(s, ToolpathSegment::Comment { text } if text == "half way"))
            .unwrap();
        let arcs_before = fitted.segments[..comment_at]
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Arc { .. }))
            .count();
        assert!((1..=2).contains(&arcs_before), "{arcs_before} arcs");
    }

    // ------------------------------------------------------------- DXF fixture

    /// Minimal DXF reader: LWPOLYLINE vertices (codes 10/20) and the closed
    /// flag (code 70). Bulges are not read — the fixture stores plain
    /// polylines, which is what the fitter is for.
    fn parse_lwpolylines(dxf: &str) -> Vec<(Vec<[f64; 2]>, bool)> {
        let mut loops = Vec::new();
        let mut current: Option<(Vec<[f64; 2]>, bool)> = None;
        let mut pending_x: Option<f64> = None;
        let mut lines = dxf.lines().map(str::trim);
        while let (Some(code), Some(value)) = (lines.next(), lines.next()) {
            match code.parse::<i32>() {
                Ok(0) => {
                    if let Some(entity) = current.take() {
                        loops.push(entity);
                    }
                    if value.eq_ignore_ascii_case("LWPOLYLINE") {
                        current = Some((Vec::new(), false));
                        pending_x = None;
                    }
                }
                Ok(10) => pending_x = value.parse::<f64>().ok(),
                Ok(20) => {
                    if let (Some(entity), Some(x), Ok(y)) =
                        (current.as_mut(), pending_x.take(), value.parse::<f64>())
                    {
                        entity.0.push([x, y]);
                    }
                }
                Ok(70) => {
                    if let (Some(entity), Ok(flags)) = (current.as_mut(), value.parse::<i32>()) {
                        entity.1 = flags & 1 == 1;
                    }
                }
                _ => {}
            }
        }
        if let Some(entity) = current.take() {
            loops.push(entity);
        }
        loops.retain(|(pts, _)| pts.len() >= 3);
        loops
    }

    /// The stator outline named by the CAM roadmap, as DXF text.
    ///
    /// `docs/cam-fixtures/stator-outline.dxf` is used when it is present. It
    /// is not on this branch yet (the roadmap commit shipped only the gear
    /// fixture), so this stands in for it: same shape family — Ø50 outer, four
    /// Ø5 bolt holes, and a bore with twelve slots whose mouths are 3.87 mm
    /// wide with R1.05 fillets — polylined the way the kernel polylines a
    /// contour, at a 1 µm sagitta.
    fn stator_dxf() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-outline.dxf"
        );
        // No stand-in: numbers measured on a look-alike say nothing about the
        // part. (The file is force-added: `*.dxf` is gitignored.)
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The real thing: every loop of the stator outline, cut at one depth,
    /// fitted, and measured. Numbers are printed so a change in them is
    /// visible in the test log, not just a pass or a fail.
    #[test]
    fn test_stator_outline_fixture_fits_within_tolerance() {
        let options = ArcFitOptions::default();
        let loops = parse_lwpolylines(&stator_dxf());
        assert_eq!(
            loops.len(),
            5,
            "expected the outer loop, the bore-and-slots loop and 3 pilots"
        );

        let (mut total_in, mut total_out, mut total_arcs) = (0usize, 0usize, 0usize);
        let mut worst: f64 = 0.0;
        for (points, closed) in &loops {
            let mut pts = points.clone();
            if *closed && pts[0] != pts[pts.len() - 1] {
                pts.push(pts[0]);
            }
            let tp = polyline_path(&pts, -1.0, 800.0);
            let (fitted, report) = fit_arcs_reported(&tp, &options);
            let prims = cutting_prims(&fitted, -1.0);
            let (forward, backward) = two_way_deviation(&prims, &pts);
            assert!(
                forward <= options.tolerance && backward <= options.tolerance,
                "loop of {} points: deviation {forward:.6} / {backward:.6}",
                pts.len()
            );
            // Length is metal: an arc that cuts a different distance cuts a
            // different part.
            let fitted_len: f64 = prims.iter().map(Prim::length).sum();
            let original_len = polyline_length(&pts);
            // The same absolute bound as the bore test: `2πt` for a closed
            // loop re-drawn inside a band of half-width `t`. Scaling it by
            // `prims.len()` made it looser exactly when the fitter emitted
            // more, which is the thing under test. Worst measured on this
            // fixture: 0.0147 mm against 0.0314 mm.
            assert!(
                (fitted_len - original_len).abs() <= TAU * options.tolerance,
                "length {fitted_len:.6} vs {original_len:.6} over {} primitives",
                prims.len()
            );
            assert_eq!(
                fitted.segments.last().unwrap().target().unwrap(),
                tp.segments.last().unwrap().target().unwrap()
            );

            total_in += report.linear_in;
            total_out += report.arcs_emitted + report.linears_emitted;
            total_arcs += report.arcs_emitted;
            worst = worst.max(forward.max(backward));
        }

        println!(
            "stator-outline: {total_in} linear moves -> {total_out} primitives \
             ({total_arcs} arcs), max deviation {worst:.6} mm"
        );
        assert!(total_in > 1000, "fixture too small: {total_in} moves");
        // The real outline is a faceted CSG (32-gon R1.05 fillets, a 256-gon
        // rim): its own sagitta is about the fit tolerance, and a two-way
        // tolerance will not bulge an arc past the polyline it came from. So
        // the honest reduction here is ~2.7x, not the 17x a finely sampled
        // curve gives. Contours taken from the solid with true arcs do better.
        assert!(
            total_out * 2 < total_in,
            "only {total_in} -> {total_out}: less than a 2x reduction"
        );
        assert!(worst <= options.tolerance, "max deviation {worst:.6}");
    }

    /// Post the fitted path through Grbl and read the arcs back the way the
    /// controller does. Grbl computes the radius at the start from I/J and at
    /// the end from the target, and throws error:33 when they disagree — so
    /// the rounding in the G-code, not just the f64 in memory, has to hold up.
    #[test]
    fn test_grbl_round_trip_start_and_end_radii_agree() {
        let options = ArcFitOptions::default();
        let loops = parse_lwpolylines(&stator_dxf());
        let mut tp = Toolpath::new();
        for (points, _) in &loops {
            let mut pts = points.clone();
            if pts[0] != pts[pts.len() - 1] {
                pts.push(pts[0]);
            }
            tp.push(ToolpathSegment::rapid(pts[0][0], pts[0][1], 5.0));
            tp.push(ToolpathSegment::linear(pts[0][0], pts[0][1], -1.0, 200.0));
            for p in &pts[1..] {
                tp.push(ToolpathSegment::linear(p[0], p[1], -1.0, 800.0));
            }
        }
        let fitted = fit_arcs(&tp, &options);

        let post = GrblPost::default();
        let gcode = post.generate(
            "arcfit",
            &Tool::default_endmill(),
            &fitted,
            &CamSettings::default(),
        );

        let word = |line: &str, letter: char| -> Option<f64> {
            line.split_whitespace()
                .find(|w| w.starts_with(letter))
                .and_then(|w| w[1..].parse::<f64>().ok())
        };

        let (mut x, mut y) = (0.0_f64, 0.0_f64);
        let mut arcs = 0;
        let mut worst: f64 = 0.0;
        for line in gcode.lines() {
            let line = line.trim();
            // The motion word is the first one on the line: "G21" and "G17"
            // are not rapids and arcs, whatever they start with.
            let motion = line.split_whitespace().next().unwrap_or("");
            if motion == "G0" || motion == "G1" {
                x = word(line, 'X').unwrap_or(x);
                y = word(line, 'Y').unwrap_or(y);
            } else if motion == "G2" || motion == "G3" {
                let (i, j) = (word(line, 'I').unwrap(), word(line, 'J').unwrap());
                let (tx, ty) = (word(line, 'X').unwrap(), word(line, 'Y').unwrap());
                let (cx, cy) = (x + i, y + j);
                let start_radius = i.hypot(j);
                let end_radius = (tx - cx).hypot(ty - cy);
                worst = worst.max((start_radius - end_radius).abs());
                arcs += 1;
                x = tx;
                y = ty;
            }
        }
        println!("grbl round trip: {arcs} arcs, worst radius mismatch {worst:.6} mm");
        // Pinned on the real fixture; a change here is a change in fitting.
        assert_eq!(arcs, 39, "{arcs} arcs posted");
        assert!(
            worst < 0.002,
            "start/end radius disagree by {worst:.6} mm — grbl throws error:33"
        );
    }

    /// Determinism and cost: the same input gives the same output, and a
    /// 30k-move job is fitted in one pass over the data.
    #[test]
    fn test_deterministic_and_linearithmic() {
        let options = ArcFitOptions::default();
        let mut pts = Vec::new();
        for k in 0..=30_000 {
            let theta = TAU * 3.0 * k as f64 / 30_000.0;
            pts.push([40.0 * theta.cos(), 40.0 * theta.sin()]);
        }
        let tp = polyline_path(&pts, -1.0, 800.0);
        let (first, report_a) = fit_arcs_reported(&tp, &options);
        let (second, report_b) = fit_arcs_reported(&tp, &options);
        assert_eq!(report_a, report_b);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        // Three laps at 180° per arc.
        assert_eq!(report_a.arcs_emitted, 6, "{report_a:?}");

        // The name says linearithmic; it used to print the elapsed time and
        // assert nothing at all, which would have let an O(n²) rewrite through
        // silently. Timed against four times the input: `grow`'s doubling
        // makes a primitive covering k points cost O(log k) checks of O(k),
        // so 4n should cost about 4x, and certainly not 16x.
        let timed = |n: usize| {
            let mut pts = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let theta = TAU * 3.0 * k as f64 / n as f64;
                pts.push([40.0 * theta.cos(), 40.0 * theta.sin()]);
            }
            let tp = polyline_path(&pts, -1.0, 800.0);
            // One warm run, then the measured one: the first touches the
            // allocator for every buffer this will use.
            let _ = fit_arcs_reported(&tp, &options);
            let started = std::time::Instant::now();
            let (_, report) = fit_arcs_reported(&tp, &options);
            assert_eq!(report.arcs_emitted, 6, "{report:?}");
            started.elapsed().as_secs_f64()
        };
        let small = timed(20_000);
        let large = timed(80_000);
        // A generous ceiling on purpose: this is a guard against a change of
        // complexity class, not a benchmark, and it runs on shared CI. 4x the
        // input at quadratic cost would be 16x the time; the floor is there so
        // a timer that reports nothing cannot pass it either.
        let ratio = large / small.max(1e-9);
        assert!(
            (1.0..=10.0).contains(&ratio),
            "4x the input took {ratio:.1}x the time ({small:.4}s -> {large:.4}s): \
             quadratic is 16x, and a ratio under 1 means the timer measured nothing"
        );
    }
}
