//! Gear profiles and tool-centre paths as CAM [`Contour`]s.
//!
//! Arcs that are exactly arcs — the cutter's root fillet, the root circle, the
//! tip circle — are emitted as [`ContourSegment::Arc`] so the post can write
//! them as G2/G3. The involute flanks are the only thing chorded, to a
//! guaranteed maximum chordal error (default [`DEFAULT_CHORDAL_TOLERANCE`]),
//! by adaptive bisection rather than a fixed sample count: a flank near the
//! base circle is nearly a cusp and a flank near the tip is nearly straight,
//! and one sample count cannot serve both.
//!
//! The tool-centre path is **not** an offset polygon of the profile. Offsetting
//! an involute by the cutter radius gives another involute of the same base
//! circle, so the path is generated directly and is exact at every vertex —
//! which is what [`SpurGear::tool_centre_path`]'s test measures.

use super::{GearError, RootBinding, SpurGear};
use crate::operation::{Contour, Point2D};
use serde::{Deserialize, Serialize};

/// Default maximum chordal error for a sampled involute flank, mm.
pub const DEFAULT_CHORDAL_TOLERANCE: f64 = 0.002;

/// How finely the involute flanks are chorded.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FlankTolerance {
    /// Maximum distance from the true involute to the chord, mm.
    pub chordal: f64,
}

impl Default for FlankTolerance {
    fn default() -> Self {
        Self {
            chordal: DEFAULT_CHORDAL_TOLERANCE,
        }
    }
}

impl FlankTolerance {
    /// A tolerance of `chordal` mm.
    pub fn new(chordal: f64) -> Result<Self, GearError> {
        if !chordal.is_finite() || chordal <= 0.0 {
            return Err(GearError::InvalidRadius(chordal));
        }
        Ok(Self { chordal })
    }
}

/// Cap on adaptive subdivision. 2^18 chords on one flank is far past any real
/// tolerance; hitting it means the caller asked for something degenerate, and
/// stopping is better than allocating forever.
const MAX_SUBDIVISION_DEPTH: u32 = 18;

fn polar(r: f64, a: f64) -> Point2D {
    Point2D::new(r * a.cos(), r * a.sin())
}

/// Adaptive chordal sampling of a parametric curve on `[t0, t1]`.
///
/// Splits wherever the curve at the parameter midpoint is further from the
/// chord than `tol`. Returns both endpoints.
fn sample_curve<F>(f: F, t0: f64, t1: f64, tol: f64) -> Vec<Point2D>
where
    F: Fn(f64) -> Point2D,
{
    fn deviation(p: Point2D, q: Point2D, m: Point2D) -> f64 {
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let len = dx.hypot(dy);
        if len < 1e-15 {
            return m.distance_to(&p);
        }
        ((m.x - p.x) * dy - (m.y - p.y) * dx).abs() / len
    }
    /// One interval of the subdivision: parameters and their curve points.
    struct Span {
        t0: f64,
        t1: f64,
        p0: Point2D,
        p1: Point2D,
    }
    fn go<F: Fn(f64) -> Point2D>(f: &F, span: Span, tol: f64, depth: u32, out: &mut Vec<Point2D>) {
        let tm = 0.5 * (span.t0 + span.t1);
        let pm = f(tm);
        if depth >= MAX_SUBDIVISION_DEPTH || deviation(span.p0, span.p1, pm) <= tol {
            out.push(span.p1);
            return;
        }
        let (t0, t1, p0, p1) = (span.t0, span.t1, span.p0, span.p1);
        go(
            f,
            Span {
                t0,
                t1: tm,
                p0,
                p1: pm,
            },
            tol,
            depth + 1,
            out,
        );
        go(
            f,
            Span {
                t0: tm,
                t1,
                p0: pm,
                p1,
            },
            tol,
            depth + 1,
            out,
        );
    }
    let (p0, p1) = (f(t0), f(t1));
    let mut out = vec![p0];
    go(&f, Span { t0, t1, p0, p1 }, tol, 0, &mut out);
    out
}

/// Direction of travel around `centre` from `at`, given the tangent `dir`.
///
/// Using the tangent rather than the chord keeps the flag right for arcs of any
/// span — the fillet between two nearly parallel flanks can exceed 180°.
fn arc_is_ccw(at: Point2D, centre: Point2D, dir: (f64, f64)) -> bool {
    let (rx, ry) = (at.x - centre.x, at.y - centre.y);
    rx * dir.1 - ry * dir.0 > 0.0
}

impl SpurGear {
    /// Points along one flank of tooth space `i`, from radius `from` to `to`.
    ///
    /// `side` picks the flank: `+1` is the one at increasing angle. Both
    /// endpoints are included, and the base circle is always a vertex when the
    /// range crosses it — below it the flank is the radial continuation, which
    /// is straight and needs no sampling at all.
    pub fn flank_points(
        &self,
        i: u32,
        side: f64,
        from: f64,
        to: f64,
        tol: FlankTolerance,
    ) -> Result<Vec<Point2D>, GearError> {
        self.validate()?;
        if !from.is_finite() || !to.is_finite() || from <= 0.0 || to <= 0.0 {
            return Err(GearError::InvalidRadius(from.min(to)));
        }
        let centre = self.space_centre_angle(i);
        let s = side.signum();
        let point = |r: f64| -> Point2D {
            let half = self
                .space_half_angle_at(r)
                .expect("radius validated by the caller");
            polar(r, centre + s * half)
        };
        let rb = self.r_base();
        let mut breaks = vec![from, to];
        if (from - rb) * (to - rb) < 0.0 {
            breaks.insert(1, rb);
        }
        let mut out = vec![point(from)];
        for w in breaks.windows(2) {
            let (a, b) = (w[0], w[1]);
            let pts = if a.min(b) >= rb {
                sample_curve(point, a, b, tol.chordal)
            } else {
                // Radial continuation: a straight line, exactly two points.
                vec![point(a), point(b)]
            };
            out.extend(pts.into_iter().skip(1));
        }
        Ok(out)
    }

    /// The boundary of tooth space `i` as the cutter leaves it: flanks, the
    /// cutter's fillet(s), the root arc where there is one, and the tip circle
    /// closing the mouth.
    ///
    /// Counter-clockwise, so the space's interior is on the left.
    pub fn tooth_space_contour(
        &self,
        i: u32,
        cutter_diameter: f64,
        tol: FlankTolerance,
    ) -> Result<Contour, GearError> {
        // Starting on the flank that makes the closed space come out CCW: the
        // mouth is outside the root on an external gear and inside it on a ring.
        let start_side = if self.internal { -1.0 } else { 1.0 };
        let mut contour = self.space_boundary(i, start_side, cutter_diameter, tol)?;
        // Close across the mouth along the tip circle.
        let mouth = self.r_tip();
        let half = self.space_half_angle_at(mouth)?;
        let centre = self.space_centre_angle(i);
        contour.arc_to(
            polar(mouth, centre + start_side * half),
            Point2D::new(0.0, 0.0),
            start_side > 0.0,
        );
        Ok(contour)
    }

    /// The whole gear as one closed contour: every tooth's tip land, every
    /// space as the cutter leaves it. Counter-clockwise.
    ///
    /// For an internal gear this is the *bore* — the material the ring's spaces
    /// remove — which is the shape the cutter has to produce.
    pub fn full_profile_contour(
        &self,
        cutter_diameter: f64,
        tol: FlankTolerance,
    ) -> Result<Contour, GearError> {
        self.validate()?;
        let ra = self.r_tip();
        let tooth_half = self.tooth_half_angle_at(ra)?;
        let start = polar(ra, self.tooth_centre_angle(0) - tooth_half);
        let mut contour = Contour::new(start);
        for i in 0..self.teeth {
            // Tip land of tooth i, counter-clockwise about the gear centre.
            let a0 = self.tooth_centre_angle(i) - tooth_half;
            let a1 = self.tooth_centre_angle(i) + tooth_half;
            debug_assert!(contour.end_point().distance_to(&polar(ra, a0)) < 1e-9);
            contour.arc_to(polar(ra, a1), Point2D::new(0.0, 0.0), true);
            // Then space i, entered from the flank that borders tooth i.
            let space = self.space_boundary(i, -1.0, cutter_diameter, tol)?;
            contour.segments.extend(space.segments);
        }
        Ok(contour)
    }

    /// The path the cutter's **centre** follows to cut tooth space `i`.
    ///
    /// Open, starting and ending at the mouth. Every vertex is exactly the
    /// cutter radius from the flank, because the offset of an involute is an
    /// involute of the same base circle rather than a polygon offset.
    ///
    /// When the space is no wider than the cutter the two offset flanks
    /// collapse onto the centreline: the path degenerates to a single plunge
    /// line down the middle of the space, and that is what comes back.
    pub fn tool_centre_path(
        &self,
        i: u32,
        cutter_diameter: f64,
        tol: FlankTolerance,
    ) -> Result<Contour, GearError> {
        let space = self.tooth_space(cutter_diameter)?;
        let rc = space.cutter_radius;
        let centre_angle = self.space_centre_angle(i);
        let mouth = self.r_tip();
        let bottom = space.form_radius;

        let centre_point = |side: f64, foot: f64| -> Result<Point2D, GearError> {
            let c = self.cutter_centre_at_foot(rc, foot)?;
            Ok(polar(c.radius, centre_angle + side.signum() * c.angle))
        };

        // Degenerate: the admissible centres never leave the centreline by more
        // than the tolerance, so the "path" is a plunge down the middle.
        let widest = self.cutter_centre_at_foot(rc, mouth)?;
        if widest.radius * widest.angle.sin() <= tol.chordal {
            let deep = self.cutter_centre_at_foot(rc, bottom)?;
            // Straight in from the mouth, down the centreline, to the deepest
            // the cutter reaches: there is no room to move sideways.
            let mut c = Contour::new(polar(mouth, centre_angle));
            c.line_to(polar(deep.radius, centre_angle));
            return Ok(c);
        }

        let start_side = 1.0;
        let down = sample_curve(
            |t| centre_point(start_side, t).expect("foot inside the flank range"),
            mouth,
            bottom,
            tol.chordal,
        );
        let mut contour = Contour::new(down[0]);
        for p in down.iter().skip(1) {
            contour.line_to(*p);
        }
        match space.binding {
            RootBinding::Flanks => {
                // The offset flanks meet: one point, no arc across.
            }
            RootBinding::RootCircle => {
                let locus = if self.internal {
                    self.r_root() - rc
                } else {
                    self.r_root() + rc
                };
                contour.arc_to(
                    polar(locus, centre_angle - start_side * space.fillet_centre_angle),
                    Point2D::new(0.0, 0.0),
                    start_side < 0.0,
                );
            }
        }
        let up = sample_curve(
            |t| centre_point(-start_side, t).expect("foot inside the flank range"),
            bottom,
            mouth,
            tol.chordal,
        );
        for p in up.iter().skip(1) {
            contour.line_to(*p);
        }
        Ok(contour)
    }

    /// Flank → fillet → (root arc → fillet) → flank, from the mouth on
    /// `start_side` to the mouth on the other side. Shared by the standalone
    /// space contour and the full profile so the two cannot drift apart.
    fn space_boundary(
        &self,
        i: u32,
        start_side: f64,
        cutter_diameter: f64,
        tol: FlankTolerance,
    ) -> Result<Contour, GearError> {
        let space = self.tooth_space(cutter_diameter)?;
        let s = start_side.signum();
        let centre = self.space_centre_angle(i);
        let mouth = self.r_tip();
        let form = space.form_radius;

        let down = self.flank_points(i, s, mouth, form, tol)?;
        let mut contour = Contour::new(down[0]);
        for p in down.iter().skip(1) {
            contour.line_to(*p);
        }
        let dir = {
            let n = down.len();
            let (a, b) = (down[n - 2], down[n - 1]);
            let d = (b.x - a.x, b.y - a.y);
            let l = d.0.hypot(d.1);
            (d.0 / l, d.1 / l)
        };
        let fillet_centre = polar(
            space.fillet_centre_radius,
            centre + s * space.fillet_centre_angle,
        );
        let ccw = arc_is_ccw(contour.end_point(), fillet_centre, dir);
        let far_flank = polar(form, centre - s * self.space_half_angle_at(form)?);
        match space.binding {
            RootBinding::Flanks => {
                // One circle tangent to both flanks: a single arc across.
                contour.arc_to(far_flank, fillet_centre, ccw);
            }
            RootBinding::RootCircle => {
                let deep = space.effective_root_radius;
                let a0 = centre + s * space.root_arc_half_angle;
                let a1 = centre - s * space.root_arc_half_angle;
                contour.arc_to(polar(deep, a0), fillet_centre, ccw);
                contour.arc_to(polar(deep, a1), Point2D::new(0.0, 0.0), s < 0.0);
                let other_centre = polar(
                    space.fillet_centre_radius,
                    centre - s * space.fillet_centre_angle,
                );
                // Leaving the root arc, travel is tangent to it: perpendicular
                // to the radius, in the direction the arc was running.
                let r = polar(deep, a1);
                let t = if s < 0.0 { (-r.y, r.x) } else { (r.y, -r.x) };
                let l = t.0.hypot(t.1);
                let ccw2 = arc_is_ccw(r, other_centre, (t.0 / l, t.1 / l));
                contour.arc_to(far_flank, other_centre, ccw2);
            }
        }
        let up = self.flank_points(i, -s, form, mouth, tol)?;
        for p in up.iter().skip(1) {
            contour.line_to(*p);
        }
        Ok(contour)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::*;
    use crate::operation::ContourSegment;
    use std::f64::consts::PI;

    /// Signed area of a contour, arcs linearised. Positive is counter-clockwise.
    fn signed_area(c: &Contour) -> f64 {
        let poly = c.to_geo_polygon();
        let coords: Vec<_> = poly.exterior().0.clone();
        let mut a = 0.0;
        for w in coords.windows(2) {
            a += w[0].x * w[1].y - w[1].x * w[0].y;
        }
        a / 2.0
    }

    /// The true involute point on a flank, for checking the chords against.
    fn true_flank_point(g: &SpurGear, i: u32, side: f64, r: f64) -> Point2D {
        let a = g.space_centre_angle(i) + side * g.space_half_angle_at(r).unwrap();
        Point2D::new(r * a.cos(), r * a.sin())
    }

    /// Distance from a point to a polyline.
    fn distance_to_polyline(p: Point2D, pts: &[Point2D]) -> f64 {
        let mut best = f64::INFINITY;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy));
            best = best.min(d);
        }
        best
    }

    /// The chordal guarantee, measured the way it is meant: every point of the
    /// *true* involute is within tolerance of the polyline. (The reverse — the
    /// polyline's points lying on the involute — is true by construction and
    /// would pass however coarse the sampling was.)
    #[test]
    fn flank_polyline_holds_its_chordal_tolerance() {
        for tol in [0.002, 0.0002, 0.00002] {
            let t = FlankTolerance::new(tol).unwrap();
            for g in [sun(), planet(), ring()] {
                let space = g.tooth_space(1.0).unwrap();
                let (lo, hi) = if g.internal {
                    (g.r_tip(), space.form_radius)
                } else {
                    (space.form_radius, g.r_tip())
                };
                let pts = g.flank_points(0, 1.0, lo, hi, t).unwrap();
                assert!(pts.len() >= 2);
                let mut worst = 0.0_f64;
                for k in 0..=4000 {
                    let r = lo + (hi - lo) * k as f64 / 4000.0;
                    let p = true_flank_point(&g, 0, 1.0, r);
                    worst = worst.max(distance_to_polyline(p, &pts));
                }
                assert!(
                    worst <= tol,
                    "z{} tol {tol}: worst chordal error {worst} over {} points",
                    g.teeth,
                    pts.len()
                );
            }
        }
    }

    /// Tighter tolerance, more points — and the extra points are on the same
    /// curve, so the profile does not move.
    #[test]
    fn tolerance_densifies_without_moving_the_flank() {
        let g = planet();
        let coarse = g
            .flank_points(0, 1.0, 9.3, 10.89, FlankTolerance::new(0.01).unwrap())
            .unwrap();
        let fine = g
            .flank_points(0, 1.0, 9.3, 10.89, FlankTolerance::new(0.0001).unwrap())
            .unwrap();
        assert!(
            fine.len() > coarse.len() * 4,
            "{} vs {}",
            fine.len(),
            coarse.len()
        );
        for p in [coarse.first().unwrap(), coarse.last().unwrap()] {
            assert!(distance_to_polyline(*p, &fine) < 1e-12);
        }
    }

    /// The space contour closes, winds the right way, and its arcs really are
    /// the cutter's: every arc segment's radius is either the cutter radius
    /// (fillet), the effective root, or the tip.
    #[test]
    fn tooth_space_contour_closes_and_winds_ccw() {
        for g in [sun(), planet(), ring()] {
            let space = g.tooth_space(1.0).unwrap();
            let c = g
                .tooth_space_contour(0, 1.0, FlankTolerance::default())
                .unwrap();
            assert!(c.is_closed(1e-9), "z{} space contour not closed", g.teeth);
            assert!(signed_area(&c) > 0.0, "z{} space contour winds CW", g.teeth);
            let mut from = c.start;
            let mut arcs = 0;
            for seg in &c.segments {
                if let ContourSegment::Arc { to, center, .. } = seg {
                    let r = center.distance_to(&from);
                    let r2 = center.distance_to(to);
                    assert!((r - r2).abs() < 1e-9, "arc radius mismatch {r} {r2}");
                    let known = [space.cutter_radius, space.effective_root_radius, g.r_tip()];
                    assert!(
                        known.iter().any(|k| (r - k).abs() < 1e-9),
                        "z{}: arc radius {r} is none of {known:?}",
                        g.teeth
                    );
                    arcs += 1;
                    from = *to;
                } else if let ContourSegment::Line { to } = seg {
                    from = *to;
                }
            }
            // Two fillets + a root arc + the mouth, or one fillet + the mouth.
            assert!(arcs >= 2, "z{}: {arcs} arcs", g.teeth);
        }
    }

    /// The fillet meets both flanks tangentially: no corner for the cutter to
    /// jump. Measured two ways at the join — against the *true* flank tangent,
    /// which is pure geometry and holds to 1e-9, and against the emitted chord,
    /// which can only be as good as the sampling (≈2·√(2·tol/r_fillet)).
    #[test]
    fn fillet_is_tangent_to_both_flanks() {
        for g in [sun(), planet(), ring()] {
            let space = g.tooth_space(1.0).unwrap();
            let chordal = 1e-6;
            let c = g
                .tooth_space_contour(0, 1.0, FlankTolerance::new(chordal).unwrap())
                .unwrap();
            let start_side = if g.internal { -1.0 } else { 1.0 };
            // True flank tangent at the form radius, by central difference.
            let h = 1e-7;
            let a = true_flank_point(&g, 0, start_side, space.form_radius - h);
            let b = true_flank_point(&g, 0, start_side, space.form_radius + h);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l = dx.hypot(dy);
            let tangent = (dx / l, dy / l);

            let mut from = c.start;
            let mut prev_dir = None;
            let mut checked = false;
            for seg in &c.segments {
                match seg {
                    ContourSegment::Line { to } => {
                        let d = (to.x - from.x, to.y - from.y);
                        let l = d.0.hypot(d.1);
                        if l > 0.0 {
                            prev_dir = Some((d.0 / l, d.1 / l));
                        }
                        from = *to;
                    }
                    ContourSegment::Arc { to, center, .. } => {
                        let r = center.distance_to(&from);
                        if (r - space.cutter_radius).abs() < 1e-9 {
                            let (rx, ry) = ((from.x - center.x) / r, (from.y - center.y) / r);
                            // The fillet's tangent at the join is perpendicular
                            // to its radius, so the radius must be perpendicular
                            // to the flank there.
                            let exact = rx * tangent.0 + ry * tangent.1;
                            assert!(
                                exact.abs() < 1e-7,
                                "z{}: fillet meets the true flank tangent at cos {exact}",
                                g.teeth
                            );
                            let dir = prev_dir.expect("a flank runs into the fillet");
                            let chorded = rx * dir.0 + ry * dir.1;
                            let bound = 2.0 * (2.0 * chordal / space.cutter_radius).sqrt();
                            assert!(
                                chorded.abs() < bound,
                                "z{}: emitted chord meets the fillet at cos {chorded} (bound {bound})",
                                g.teeth
                            );
                            checked = true;
                            break;
                        }
                        from = *to;
                    }
                }
            }
            assert!(checked, "z{}: no fillet arc found", g.teeth);
        }
    }

    /// The whole gear: one closed contour, z tip lands, and an enclosed area
    /// between the root and tip circles.
    #[test]
    fn full_profile_closes_with_the_right_area() {
        for g in [sun(), planet(), ring()] {
            let c = g
                .full_profile_contour(1.0, FlankTolerance::new(0.0005).unwrap())
                .unwrap();
            assert!(c.is_closed(1e-9), "z{} profile not closed", g.teeth);
            let area = signed_area(&c);
            let space = g.tooth_space(1.0).unwrap();
            let (inner, outer) = if g.internal {
                (g.r_tip(), space.effective_root_radius)
            } else {
                (space.effective_root_radius, g.r_tip())
            };
            assert!(
                area > PI * inner * inner && area < PI * outer * outer,
                "z{}: area {area} outside [{}, {}]",
                g.teeth,
                PI * inner * inner,
                PI * outer * outer
            );
        }
    }

    /// The tool-centre path is exactly the cutter radius from the flank — at
    /// every vertex, not on average. This is the parallel-involute property
    /// doing its job; a polygon offset of a chorded profile would fail it by
    /// the chordal error.
    #[test]
    fn tool_centre_path_keeps_the_cutter_radius() {
        for g in [sun(), planet(), ring()] {
            let space = g.tooth_space(1.0).unwrap();
            let path = g
                .tool_centre_path(0, 1.0, FlankTolerance::new(0.001).unwrap())
                .unwrap();
            let mut pts = vec![path.start];
            for seg in &path.segments {
                match seg {
                    ContourSegment::Line { to } | ContourSegment::Arc { to, .. } => pts.push(*to),
                }
            }
            // Dense true flank, both sides, over the cut range.
            let (lo, hi) = if g.internal {
                (g.r_tip(), space.form_radius)
            } else {
                (space.form_radius, g.r_tip())
            };
            let mut flank = Vec::new();
            for side in [-1.0, 1.0] {
                for k in 0..=20000 {
                    let r = lo + (hi - lo) * k as f64 / 20000.0;
                    flank.push(true_flank_point(&g, 0, side, r));
                }
            }
            for p in &pts {
                let d = flank
                    .iter()
                    .map(|q| p.distance_to(q))
                    .fold(f64::INFINITY, f64::min);
                // Points on the root arc are further from the flank than rc;
                // never closer, which is the thing that would gouge.
                assert!(
                    d > space.cutter_radius - 2e-5,
                    "z{}: tool centre {d} from the flank, cutter radius {}",
                    g.teeth,
                    space.cutter_radius
                );
            }
            // And the flank-following vertices sit exactly on the offset.
            let on_offset = pts
                .iter()
                .filter(|p| {
                    let d = flank
                        .iter()
                        .map(|q| p.distance_to(q))
                        .fold(f64::INFINITY, f64::min);
                    (d - space.cutter_radius).abs() < 2e-5
                })
                .count();
            assert!(
                on_offset >= pts.len() / 2,
                "z{}: only {on_offset} of {} vertices ride the flank",
                g.teeth,
                pts.len()
            );
        }
    }

    /// A cutter as wide as the space collapses the centre path to one line down
    /// the middle — the case a naive offset would turn into a self-crossing
    /// loop.
    #[test]
    fn tool_centre_path_degenerates_to_a_plunge() {
        let g = SpurGear::external(2.0, 30);
        let tol = FlankTolerance::new(0.001).unwrap();
        // The largest cutter that fits at all: past it the space refuses it.
        let (mut fits, mut jams) = (0.1, 10.0);
        for _ in 0..80 {
            let mid = 0.5 * (fits + jams);
            if g.tooth_space(mid).is_ok() {
                fits = mid;
            } else {
                jams = mid;
            }
        }
        let path = g.tool_centre_path(0, fits, tol).unwrap();
        assert_eq!(path.segments.len(), 1, "expected a single plunge line");
        assert!(matches!(path.segments[0], ContourSegment::Line { .. }));
        // It really is a plunge: straight down the space centreline.
        let centre_angle = g.space_centre_angle(0);
        for p in [path.start, path.end_point()] {
            let a = p.y.atan2(p.x);
            assert!((a - centre_angle).abs() < 1e-9, "off the centreline at {a}");
        }
        assert!(
            path.start.distance_to(&path.end_point()) > 0.1,
            "zero plunge"
        );
        // A hair wider is refused rather than silently shortened.
        assert!(matches!(
            g.tool_centre_path(0, jams * 1.001, tol),
            Err(GearError::CutterTooLarge { .. })
        ));
        // And a normal cutter is not mistaken for the degenerate case.
        assert!(g.tool_centre_path(0, 1.0, tol).unwrap().segments.len() > 8);
    }
}
