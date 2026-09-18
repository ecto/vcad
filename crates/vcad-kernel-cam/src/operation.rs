//! CAM operation definitions.

use crate::{CamError, CamSettings, CutContext, Tool, Toolpath};
use serde::{Deserialize, Serialize};

mod contour;
mod drill;
mod face;
mod pocket;
mod roughing3d;

pub use crate::stock::Spoilboard;
pub use contour::{
    CentreLineStretch, Contour2D, ContourPhase, ContourReport, CutDirection, EntryStyle, Tab,
    ThinSlotStrategy,
};
pub use drill::{tip_length, BreakThrough, Drill, DrillCycle, DrillError, HelicalBore, Hole};
pub use face::Face;
pub use pocket::{Pocket2D, PocketReport, Stepover, UncutPatch};
pub use roughing3d::Roughing3D;

/// A CAM operation that can generate a toolpath.
///
/// Hole making is in here too, as of wave 2. It used to sit outside in a
/// parallel `JobOperation` enum because [`DrillError`] says things `CamError`
/// could not; [`CamError::Operation`] carries those words verbatim, so one
/// enum is enough and a caller no longer has to know which of two to reach
/// for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CamOperation {
    /// Surface facing operation.
    Face(Face),
    /// 2D pocket clearing operation.
    Pocket2D(Pocket2D),
    /// 2D contour/profile operation.
    Contour2D(Contour2D),
    /// 3D roughing operation.
    Roughing3D(Roughing3D),
    /// Drilling a list of holes.
    Drill(Drill),
    /// Boring a hole larger than the cutter, helically.
    HelicalBore(HelicalBore),
}

impl From<Face> for CamOperation {
    fn from(op: Face) -> Self {
        CamOperation::Face(op)
    }
}

impl From<Pocket2D> for CamOperation {
    fn from(op: Pocket2D) -> Self {
        CamOperation::Pocket2D(op)
    }
}

impl From<Contour2D> for CamOperation {
    fn from(op: Contour2D) -> Self {
        CamOperation::Contour2D(op)
    }
}

impl From<Roughing3D> for CamOperation {
    fn from(op: Roughing3D) -> Self {
        CamOperation::Roughing3D(op)
    }
}

impl From<Drill> for CamOperation {
    fn from(op: Drill) -> Self {
        CamOperation::Drill(op)
    }
}

impl From<HelicalBore> for CamOperation {
    fn from(op: HelicalBore) -> Self {
        CamOperation::HelicalBore(op)
    }
}

impl CamOperation {
    /// Generate a toolpath for this operation.
    ///
    /// Note: For Roughing3D, use `generate_with_height_field` instead.
    ///
    /// The hole operations ask the tool whether it cuts across its own centre
    /// and how long its flutes are. Nothing here declares that, so this
    /// refuses — in the tool's own words. Use
    /// [`CamOperation::generate_with_geometry`] with the tool library's entry
    /// when the answer is known.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        self.generate_with_geometry(tool, &crate::ToolGeometry::default(), settings)
    }

    /// Generate a toolpath, with what is known about the tool's geometry.
    pub fn generate_with_geometry(
        &self,
        tool: &Tool,
        geometry: &crate::ToolGeometry,
        settings: &CamSettings,
    ) -> Result<Toolpath, CamError> {
        match self {
            CamOperation::Face(op) => op.generate(tool, settings),
            CamOperation::Pocket2D(op) => op.generate(tool, settings),
            CamOperation::Contour2D(op) => op.generate(tool, settings),
            CamOperation::Roughing3D(_) => Err(CamError::EmptyContour), // Need height field
            CamOperation::Drill(op) => Ok(op.generate(tool, geometry, settings)?),
            CamOperation::HelicalBore(op) => Ok(op.generate(tool, geometry, settings)?),
        }
    }

    /// What this operation asks of its tool, for the tool-geometry checks.
    ///
    /// The depth reported is the one the flutes actually reach, not the one
    /// the operation was asked for: contour and pocket both cut
    /// `depth - bottom_allowance`, and a break-through allowance is negative.
    /// Reporting `op.depth` let `with_bottom_allowance(-0.5)` on a 6.0 mm cut
    /// pass a 6.2 mm flute check and then bury 6.5 mm of tool — the shank in
    /// the work, which is the very thing this gate exists to stop.
    pub fn cut_context(&self, tool: &Tool) -> Result<CutContext, CamError> {
        Ok(match self {
            // Facing has no bottom allowance: the depth asked for is the depth.
            CamOperation::Face(op) => CutContext::new(op.depth),
            CamOperation::Pocket2D(op) => CutContext::new(op.reached_depth()),
            CamOperation::Contour2D(op) => CutContext::new(op.reached_depth()),
            // The depth a 3D roughing pass reaches is the height field's to
            // say, and this does not carry one.
            CamOperation::Roughing3D(_) => {
                return Err(CamError::Operation(
                    "3D roughing needs a height field before its depth is known".into(),
                ))
            }
            CamOperation::Drill(op) => op.cut_context(tool)?,
            CamOperation::HelicalBore(op) => op.cut_context(tool),
        })
    }

    /// Generate a toolpath for Roughing3D operation with a height field.
    pub fn generate_with_height_field(
        &self,
        height_field: &crate::dropcutter::HeightField,
        tool: &Tool,
        settings: &CamSettings,
    ) -> Result<Toolpath, CamError> {
        match self {
            CamOperation::Roughing3D(op) => op.generate(height_field, tool, settings),
            _ => self.generate(tool, settings),
        }
    }

    /// Get a descriptive name for this operation type.
    pub fn name(&self) -> &'static str {
        match self {
            CamOperation::Face(_) => "Face",
            CamOperation::Pocket2D(_) => "Pocket 2D",
            CamOperation::Contour2D(_) => "Contour 2D",
            CamOperation::Roughing3D(_) => "Roughing 3D",
            CamOperation::Drill(_) => "Drill",
            CamOperation::HelicalBore(_) => "Helical bore",
        }
    }

    /// Check if this operation requires a height field.
    pub fn requires_height_field(&self) -> bool {
        matches!(self, CamOperation::Roughing3D(_))
    }

    /// Where this kind of operation sits in the order a job runs things.
    pub fn default_role(&self) -> crate::OpRole {
        match self {
            CamOperation::Face(_) => crate::OpRole::Facing,
            // An outside contour is the cut that frees the part; everything
            // else works inside it.
            CamOperation::Contour2D(op) if !op.inside => crate::OpRole::OutsideProfile,
            _ => crate::OpRole::InsideFeature,
        }
    }
}

/// Chord sag a linearised arc may have, in mm.
///
/// Both linearisers in this crate used a fixed 5° step, which is a *constant
/// angle* and therefore a sag that grows with the radius: 0.047 mm at R50, on
/// an inside contour, straight into the part. 0.005 mm is the same figure
/// `ArcFitOptions::tolerance` and `VerifyOptions::arc_tolerance` already use,
/// so the three agree about what "the same curve" means.
pub const ARC_CHORD_TOLERANCE: f64 = 0.005;

/// How far apart two readings of an arc's radius may be before the arc is not
/// an arc, in mm.
///
/// Absolute rather than relative: it is the coordinate precision a posted
/// program carries (three decimals on the Grbl post, so ±0.0005 mm on each
/// word), with an order of magnitude of headroom. An arc whose ends disagree
/// by more than this is not round-off — it is a wrong `I`/`J`, and the answer
/// depends on which end you believe.
pub const ARC_RADIUS_TOLERANCE: f64 = 0.01;

/// Largest angular step, in radians, whose chord sags no more than
/// `tolerance` from an arc of radius `radius`.
///
/// The sagitta of a chord subtending `θ` on radius `r` is `r(1 − cos(θ/2))`,
/// so the step that sags exactly `tolerance` is `2·acos(1 − tolerance/r)`. A
/// radius at or below the tolerance cannot sag more than it, and takes one
/// step.
pub fn arc_step(radius: f64, tolerance: f64) -> f64 {
    if radius.is_nan() || radius <= tolerance || !tolerance.is_finite() || tolerance <= 0.0 {
        return std::f64::consts::TAU;
    }
    2.0 * (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos()
}

/// Sweep of the arc from `from` to `to` about `center`, in radians, always
/// positive and in the direction `ccw` says.
fn arc_sweep(from: Point2D, to: Point2D, center: Point2D, ccw: bool) -> f64 {
    let start = (from.y - center.y).atan2(from.x - center.x);
    let end = (to.y - center.y).atan2(to.x - center.x);
    let mut delta = if ccw { end - start } else { start - end };
    if delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    delta
}

/// Linearise one arc into `out`, appending the points **after** `from` and
/// ending exactly on `to`.
///
/// The step follows the radius, so the chord sag is bounded by `tolerance`
/// whatever the arc's size. The radius used is the mean of the two ends'; an
/// arc whose ends disagree about it is refused by
/// [`Contour::check_arcs`] before any of this runs.
pub fn linearize_arc(
    from: Point2D,
    to: Point2D,
    center: Point2D,
    ccw: bool,
    tolerance: f64,
    out: &mut Vec<Point2D>,
) {
    let r0 = center.distance_to(&from);
    let r1 = center.distance_to(&to);
    let radius = 0.5 * (r0 + r1);
    let sweep = arc_sweep(from, to, center, ccw);
    let start = (from.y - center.y).atan2(from.x - center.x);
    let step = arc_step(radius, tolerance);
    let n = ((sweep / step.max(1e-9)).ceil() as usize).max(1);
    for i in 1..n {
        let t = i as f64 / n as f64;
        let angle = if ccw {
            start + sweep * t
        } else {
            start - sweep * t
        };
        out.push(Point2D::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }
    // The last point is the arc's own end point, not a sample of it: a
    // contour that closes a micron short leaves a nib on the part.
    out.push(to);
}

/// A 2D point for contour definitions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2D {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl Point2D {
    /// Create a new 2D point.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Distance to another point.
    pub fn distance_to(&self, other: &Point2D) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl From<(f64, f64)> for Point2D {
    fn from((x, y): (f64, f64)) -> Self {
        Self::new(x, y)
    }
}

/// A 2D contour segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContourSegment {
    /// Line segment.
    Line {
        /// End point.
        to: Point2D,
    },
    /// Arc segment.
    Arc {
        /// End point.
        to: Point2D,
        /// Arc center.
        center: Point2D,
        /// Counter-clockwise direction.
        ccw: bool,
    },
}

/// A closed 2D contour made of line and arc segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contour {
    /// Starting point.
    pub start: Point2D,
    /// Segments forming the contour.
    pub segments: Vec<ContourSegment>,
}

impl Contour {
    /// Create a new contour starting at the given point.
    pub fn new(start: Point2D) -> Self {
        Self {
            start,
            segments: Vec::new(),
        }
    }

    /// Add a line segment to the contour.
    pub fn line_to(&mut self, to: Point2D) {
        self.segments.push(ContourSegment::Line { to });
    }

    /// Add an arc segment to the contour.
    pub fn arc_to(&mut self, to: Point2D, center: Point2D, ccw: bool) {
        self.segments.push(ContourSegment::Arc { to, center, ccw });
    }

    /// Create a rectangular contour.
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Self {
        let mut contour = Self::new(Point2D::new(x, y));
        contour.line_to(Point2D::new(x + width, y));
        contour.line_to(Point2D::new(x + width, y + height));
        contour.line_to(Point2D::new(x, y + height));
        contour.line_to(Point2D::new(x, y));
        contour
    }

    /// Create a circular contour.
    pub fn circle(cx: f64, cy: f64, radius: f64) -> Self {
        let mut contour = Self::new(Point2D::new(cx + radius, cy));
        // Two semicircles
        contour.arc_to(
            Point2D::new(cx - radius, cy),
            Point2D::new(cx, cy),
            true, // CCW
        );
        contour.arc_to(
            Point2D::new(cx + radius, cy),
            Point2D::new(cx, cy),
            true, // CCW
        );
        contour
    }

    /// Every arc's end has to sit at the same radius from its centre as its
    /// start does.
    ///
    /// Nothing checked this. An arc whose `to` is at a different radius than
    /// its `from` is not an arc, and every reader in the crate quietly picked
    /// a different answer: the linearisers swept from the start radius, the
    /// toolpath's own `arc_geometry` averaged the two, and `verify2d`'s
    /// sampler took the start radius and landed somewhere other than `to`.
    /// Three readings of one bad number, none of them a refusal.
    pub fn check_arcs(&self) -> Result<(), CamError> {
        let mut at = self.start;
        for seg in &self.segments {
            match seg {
                ContourSegment::Line { to } => at = *to,
                ContourSegment::Arc { to, center, .. } => {
                    let start_radius = center.distance_to(&at);
                    let end_radius = center.distance_to(to);
                    if (end_radius - start_radius).abs() > ARC_RADIUS_TOLERANCE {
                        return Err(CamError::ArcRadiusMismatch {
                            start_radius,
                            end_radius,
                            center: [center.x, center.y],
                        });
                    }
                    at = *to;
                }
            }
        }
        Ok(())
    }

    /// Check if the contour is closed (within tolerance).
    pub fn is_closed(&self, tolerance: f64) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        let end = self.end_point();
        self.start.distance_to(&end) < tolerance
    }

    /// Get the end point of the contour.
    pub fn end_point(&self) -> Point2D {
        match self.segments.last() {
            Some(ContourSegment::Line { to }) => *to,
            Some(ContourSegment::Arc { to, .. }) => *to,
            None => self.start,
        }
    }

    /// Calculate the approximate perimeter length.
    pub fn perimeter(&self) -> f64 {
        let mut length = 0.0;
        let mut current = self.start;

        for seg in &self.segments {
            match seg {
                ContourSegment::Line { to } => {
                    length += current.distance_to(to);
                    current = *to;
                }
                ContourSegment::Arc { to, center, .. } => {
                    // Approximate arc length
                    let r = center.distance_to(&current);
                    let dx1 = current.x - center.x;
                    let dy1 = current.y - center.y;
                    let dx2 = to.x - center.x;
                    let dy2 = to.y - center.y;
                    let angle1 = dy1.atan2(dx1);
                    let angle2 = dy2.atan2(dx2);
                    let mut delta = angle2 - angle1;
                    if delta < 0.0 {
                        delta += 2.0 * std::f64::consts::PI;
                    }
                    length += r * delta;
                    current = *to;
                }
            }
        }

        length
    }

    /// Check if the contour is roughly circular (made of arc segments only).
    pub fn is_circular(&self) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        self.segments
            .iter()
            .all(|s| matches!(s, ContourSegment::Arc { .. }))
    }

    /// Convert to geo crate LineString for offset operations.
    pub fn to_geo_polygon(&self) -> geo::Polygon<f64> {
        let points = self.to_points(ARC_CHORD_TOLERANCE);
        let coords: Vec<geo::Coord<f64>> = points
            .into_iter()
            .map(|p| geo::Coord { x: p.x, y: p.y })
            .collect();
        geo::Polygon::new(geo::LineString::from(coords), vec![])
    }

    /// The contour as a polyline, arcs linearised to a chord sag of at most
    /// `tolerance`.
    ///
    /// The one linearisation in the crate. There were two, both stepping a
    /// fixed 5°, which is the same *angle* at every radius and so a sag that
    /// grows with it: 0.047 mm at R50, cut into the part on an inside
    /// contour.
    pub fn to_points(&self, tolerance: f64) -> Vec<Point2D> {
        let mut points = vec![self.start];
        for seg in &self.segments {
            match seg {
                ContourSegment::Line { to } => points.push(*to),
                ContourSegment::Arc { to, center, ccw } => {
                    let from = *points.last().expect("seeded with start");
                    linearize_arc(from, *to, *center, *ccw, tolerance, &mut points);
                }
            }
        }
        points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point2d() {
        let p1 = Point2D::new(0.0, 0.0);
        let p2 = Point2D::new(3.0, 4.0);
        assert!((p1.distance_to(&p2) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_contour_rectangle() {
        let rect = Contour::rectangle(0.0, 0.0, 10.0, 5.0);
        assert_eq!(rect.segments.len(), 4);
        assert!(rect.is_closed(1e-6));
        assert!((rect.perimeter() - 30.0).abs() < 1e-6);
    }

    #[test]
    fn test_contour_circle() {
        let circle = Contour::circle(0.0, 0.0, 10.0);
        assert_eq!(circle.segments.len(), 2);
        assert!(circle.is_closed(1e-6));
        // Perimeter should be approximately 2*PI*r = 62.83
        let expected = 2.0 * std::f64::consts::PI * 10.0;
        assert!((circle.perimeter() - expected).abs() < 0.1);
    }

    /// A fixed 5° step is a fixed *angle*, so the chord sag grows with the
    /// radius: 0.047 mm at R50, cut straight into the part on an inside
    /// contour. The step now follows the radius, and the sag is bounded
    /// wherever the arc is measured — including between the samples, at the
    /// chord midpoints, which is where it is largest.
    #[test]
    fn a_linearised_arc_sags_no_more_than_the_tolerance_at_any_radius() {
        for radius in [1.0, 5.0, 50.0, 500.0] {
            let circle = Contour::circle(0.0, 0.0, radius);
            let points = circle.to_points(ARC_CHORD_TOLERANCE);
            let worst = points
                .windows(2)
                .map(|w| {
                    let m = Point2D::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
                    // How far the true circle sits outside the chord's middle.
                    radius - m.x.hypot(m.y)
                })
                .fold(0.0f64, f64::max);
            assert!(
                worst <= ARC_CHORD_TOLERANCE + 1e-12,
                "R{radius}: sag {worst:.6} mm over {} points",
                points.len()
            );
            // …and it is not simply over-sampling: the sag is within a factor
            // of four of the tolerance, so the step really does follow the
            // radius rather than being pinned small.
            assert!(
                worst > ARC_CHORD_TOLERANCE / 4.0,
                "R{radius}: sag {worst:.6} mm is far below the tolerance, so the step is not \
                 adaptive"
            );
            // Every point is on the circle, and the last one is its end.
            for p in &points {
                assert!((p.x.hypot(p.y) - radius).abs() < 1e-9);
            }
            assert!(points.last().unwrap().distance_to(&circle.start) < 1e-12);

            // The old fixed 5° step, for the record: at R50 it sagged 0.048 mm.
            let fixed_step_sag = radius * (1.0 - (0.087_f64 / 2.0).cos());
            if radius >= 50.0 {
                assert!(fixed_step_sag > ARC_CHORD_TOLERANCE, "{fixed_step_sag}");
            }
        }
    }

    /// An arc whose end is not the same distance from the centre as its start
    /// is not an arc, and three readers of it used to pick three different
    /// curves. It is refused.
    #[test]
    fn an_arc_whose_ends_disagree_about_its_radius_is_refused() {
        let mut bad = Contour::new(Point2D::new(10.0, 0.0));
        // Start is R10 from the origin, end is R12.
        bad.arc_to(Point2D::new(0.0, 12.0), Point2D::new(0.0, 0.0), true);
        bad.line_to(Point2D::new(10.0, 0.0));
        let err = bad.check_arcs().unwrap_err();
        assert!(
            matches!(
                err,
                CamError::ArcRadiusMismatch { start_radius, end_radius, .. }
                    if (start_radius - 10.0).abs() < 1e-9 && (end_radius - 12.0).abs() < 1e-9
            ),
            "{err:?}"
        );

        // And the operation that would cut it refuses rather than sweeping
        // whichever radius it happened to read first.
        let op = Contour2D::inside(bad, 1.0);
        let tool = crate::Tool::FlatEndMill {
            diameter: 2.0,
            flute_length: 20.0,
            flutes: 2,
        };
        assert!(matches!(
            op.generate(&tool, &CamSettings::default()),
            Err(CamError::ArcRadiusMismatch { .. })
        ));

        // A round-off of a few microns is still one arc.
        let mut ok = Contour::new(Point2D::new(10.0, 0.0));
        ok.arc_to(Point2D::new(0.0, 10.002), Point2D::new(0.0, 0.0), true);
        ok.line_to(Point2D::new(10.0, 0.0));
        assert!(ok.check_arcs().is_ok());
    }
}
