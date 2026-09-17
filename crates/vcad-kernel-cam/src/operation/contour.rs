//! 2D contour/profile machining operation.

use crate::operation::{Contour, ContourSegment, Point2D};
use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
#[cfg(not(target_arch = "wasm32"))]
use geo_clipper::Clipper;
use serde::{Deserialize, Serialize};

/// A holding tab to prevent part from moving during cutout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    /// Nominal position along the contour as a fraction (0.0 to 1.0). The
    /// tab settles on the nearest stretch that runs straight, within half a
    /// tab pitch, so it never lands in a notch or wraps a tight corner.
    pub position: f64,
    /// Width of the material left standing, in mm. The cutter is lifted over
    /// this plus one tool diameter, since it cuts a radius into each end.
    pub width: f64,
    /// Height of the tab (how much material to leave).
    pub height: f64,
}

impl Tab {
    /// Create a new tab.
    pub fn new(position: f64, width: f64, height: f64) -> Self {
        Self {
            position,
            width,
            height,
        }
    }
}

/// 2D contour/profile machining operation.
///
/// Machines along the outside or inside of a contour with optional tabs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contour2D {
    /// The contour to machine.
    pub contour: Contour,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
    /// Offset from contour (positive = outside, negative = inside).
    /// This is in addition to the tool radius compensation.
    pub offset: f64,
    /// Holding tabs to prevent part movement.
    pub tabs: Vec<Tab>,
    /// Stock to leave for finishing pass.
    pub stock_to_leave: f64,
    /// The cutter runs inside the contour (a hole or opening) instead of
    /// around it: tool-radius compensation and stock-to-leave move the path
    /// inwards.
    #[serde(default)]
    pub inside: bool,
}

impl Contour2D {
    /// Create a new contour operation.
    pub fn new(contour: Contour, depth: f64) -> Self {
        Self {
            contour,
            depth,
            offset: 0.0,
            tabs: Vec::new(),
            stock_to_leave: 0.0,
            inside: false,
        }
    }

    /// Set the offset from contour.
    pub fn with_offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    /// Add a tab.
    pub fn with_tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab);
        self
    }

    /// Add multiple evenly-spaced tabs.
    pub fn with_tabs(mut self, count: usize, width: f64, height: f64) -> Self {
        for i in 0..count {
            // Half a pitch in, so no tab sits on the seam where each pass plunges.
            let position = (i as f64 + 0.5) / count as f64;
            self.tabs.push(Tab::new(position, width, height));
        }
        self
    }

    /// Set stock to leave.
    pub fn with_stock_to_leave(mut self, stock: f64) -> Self {
        self.stock_to_leave = stock;
        self
    }

    /// Create an outside contour for cutting out a part.
    pub fn outside(contour: Contour, depth: f64) -> Self {
        Self::new(contour, depth)
    }

    /// Create an inside contour for cutting a hole.
    pub fn inside(contour: Contour, depth: f64) -> Self {
        Self {
            inside: true,
            ..Self::new(contour, depth)
        }
    }

    /// Generate the toolpath for this contour operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        // Validate inputs
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(settings.stepdown));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }
        if !self.contour.is_closed(0.01) {
            let gap = self.contour.start.distance_to(&self.contour.end_point());
            return Err(CamError::NotClosed(gap));
        }

        // Validate tab positions
        for tab in &self.tabs {
            if tab.position < 0.0 || tab.position > 1.0 {
                return Err(CamError::InvalidTabPosition(tab.position));
            }
        }

        let mut toolpath = Toolpath::new();

        toolpath.push(ToolpathSegment::comment(format!(
            "Contour 2D: depth={:.2}mm, offset={:.2}mm, {} tabs",
            self.depth,
            self.offset,
            self.tabs.len()
        )));

        // Calculate offset path
        let tool_radius = tool.radius();
        let compensation = tool_radius + self.stock_to_leave;
        let total_offset = self.offset
            + if self.inside {
                -compensation
            } else {
                compensation
            };
        let offset_contour = self.offset_contour(total_offset)?;

        // Calculate Z levels
        let num_z_passes = (self.depth / settings.stepdown).ceil() as usize;
        let z_step = self.depth / num_z_passes as f64;

        for z_pass in 0..num_z_passes {
            let z = -((z_pass + 1) as f64) * z_step;

            toolpath.push(ToolpathSegment::comment(format!("Z level: {:.3}", z)));

            let points = &offset_contour;
            if points.is_empty() {
                continue;
            }
            // Every pass that reaches below a tab's top steps over it — not
            // only the last one, or the passes before it cut the tab away.
            let raised = self.raised_intervals(points, z, tool.diameter());
            self.follow(&mut toolpath, points, z, &raised, settings);
            // Straight up out of the cut, after the last pass too: the final
            // rapid below travels in XY.
            let start = &points[0];
            toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));
        }

        // Final retract
        toolpath.push(ToolpathSegment::rapid(
            self.contour.start.x,
            self.contour.start.y,
            settings.safe_z,
        ));

        Ok(toolpath)
    }

    /// Offset the contour by the given amount (native version with geo-clipper).
    #[cfg(not(target_arch = "wasm32"))]
    fn offset_contour(&self, offset: f64) -> Result<Vec<Point2D>, CamError> {
        if offset.abs() < 0.001 {
            // No offset needed, return original points
            return Ok(self.contour_to_points(&self.contour));
        }

        // Use geo-clipper for offset
        let polygon = self.contour.to_geo_polygon();
        let scale = 1000.0;

        let result = polygon.offset(
            offset, // geo-clipper applies the coordinate scale internally.
            geo_clipper::JoinType::Round(10.0),
            geo_clipper::EndType::ClosedPolygon,
            scale,
        );

        if result.0.is_empty() {
            return Err(CamError::EmptyContour);
        }
        // An inward offset that falls apart means the cutter cannot pass a
        // neck of the opening. Following only the first piece would leave the
        // rest uncut without a word.
        if result.0.len() > 1 {
            return Err(CamError::ContourSplit(result.0.len()));
        }

        // Extract points from first polygon
        if let Some(poly) = result.0.first() {
            let exterior = poly.exterior();
            Ok(exterior.0.iter().map(|c| Point2D::new(c.x, c.y)).collect())
        } else {
            Err(CamError::EmptyContour)
        }
    }

    /// Offset the contour by the given amount (WASM version with simple offset).
    ///
    /// This is a simplified implementation for rectangular and circular contours.
    #[cfg(target_arch = "wasm32")]
    fn offset_contour(&self, offset: f64) -> Result<Vec<Point2D>, CamError> {
        use geo::BoundingRect;

        if offset.abs() < 0.001 {
            return Ok(self.contour_to_points(&self.contour));
        }

        let polygon = self.contour.to_geo_polygon();
        let Some(bbox) = polygon.bounding_rect() else {
            return Err(CamError::EmptyContour);
        };

        let width = bbox.width();
        let height = bbox.height();
        let cx = bbox.min().x + width / 2.0;
        let cy = bbox.min().y + height / 2.0;

        // For simple offset, expand/contract the bounding rectangle
        let is_circular = self.contour.is_circular();

        if is_circular {
            // Circular offset
            let radius = width.min(height) / 2.0 + offset;
            if radius <= 0.0 {
                return Err(CamError::EmptyContour);
            }

            let segments = 36;
            let points: Vec<Point2D> = (0..=segments)
                .map(|i| {
                    let angle = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                    Point2D::new(cx + radius * angle.cos(), cy + radius * angle.sin())
                })
                .collect();
            Ok(points)
        } else {
            // Rectangular offset
            let half_w = width / 2.0 + offset;
            let half_h = height / 2.0 + offset;

            if half_w <= 0.0 || half_h <= 0.0 {
                return Err(CamError::EmptyContour);
            }

            Ok(vec![
                Point2D::new(cx - half_w, cy - half_h),
                Point2D::new(cx + half_w, cy - half_h),
                Point2D::new(cx + half_w, cy + half_h),
                Point2D::new(cx - half_w, cy + half_h),
                Point2D::new(cx - half_w, cy - half_h),
            ])
        }
    }

    /// Convert contour to a list of points.
    fn contour_to_points(&self, contour: &Contour) -> Vec<Point2D> {
        let mut points = vec![contour.start];

        for seg in &contour.segments {
            match seg {
                ContourSegment::Line { to } => {
                    points.push(*to);
                }
                ContourSegment::Arc { to, center, ccw } => {
                    // Linearize arc
                    let current = points.last().unwrap();
                    let r =
                        ((center.x - current.x).powi(2) + (center.y - current.y).powi(2)).sqrt();
                    let start_angle = (current.y - center.y).atan2(current.x - center.x);
                    let end_angle = (to.y - center.y).atan2(to.x - center.x);

                    let mut delta = if *ccw {
                        end_angle - start_angle
                    } else {
                        start_angle - end_angle
                    };
                    if delta < 0.0 {
                        delta += 2.0 * std::f64::consts::PI;
                    }

                    let segments = ((delta.abs() / 0.087).ceil() as usize).max(1);
                    let step = delta / segments as f64;

                    for i in 1..=segments {
                        let angle = if *ccw {
                            start_angle + step * i as f64
                        } else {
                            start_angle - step * i as f64
                        };
                        points.push(Point2D::new(
                            center.x + r * angle.cos(),
                            center.y + r * angle.sin(),
                        ));
                    }
                }
            }
        }

        points
    }

    /// Stretches of the closed loop where a pass at `cut_z` must ride over a
    /// tab: `(from, to, top_z)` in path length from the loop's first point.
    /// A tab across the seam comes back as two stretches.
    fn raised_intervals(
        &self,
        points: &[Point2D],
        cut_z: f64,
        tool_diameter: f64,
    ) -> Vec<(f64, f64, f64)> {
        let total = loop_length(points);
        let mut out = Vec::new();
        if total <= 0.0 {
            return out;
        }
        for tab in &self.tabs {
            let top = -self.depth + tab.height;
            if top <= cut_z + 1e-9 {
                continue;
            }
            let half = ((tab.width + tool_diameter) / 2.0).min(total / 2.0);
            let centre = self.settle_tab(points, total, tab.position, half);
            let (from, to) = (centre - half, centre + half);
            if from < 0.0 {
                out.push((from + total, total, top));
                out.push((0.0, to, top));
            } else if to > total {
                out.push((from, total, top));
                out.push((0.0, to - total, top));
            } else {
                out.push((from, to, top));
            }
        }
        out
    }

    /// Where a tab nominally at `position` actually goes: the nearest stretch
    /// (within half a tab pitch) that runs straight enough, so a tab never
    /// lands in a notch or wraps a tight corner, where it would hold little
    /// and be hard to clean off. Straightness is the chord across the lifted
    /// stretch over its path length.
    fn settle_tab(&self, points: &[Point2D], total: f64, position: f64, half: f64) -> f64 {
        const STRAIGHT_ENOUGH: f64 = 0.98;
        const STEP: f64 = 0.5;
        let nominal = position.rem_euclid(1.0) * total;
        let straightness = |centre: f64| {
            let a = point_at(points, total, centre - half);
            let b = point_at(points, total, centre + half);
            a.distance_to(&b) / (2.0 * half)
        };
        let reach = total / (2.0 * self.tabs.len().max(1) as f64) - half;
        let mut best = (straightness(nominal), nominal);
        let mut d = STEP;
        while best.0 < STRAIGHT_ENOUGH && d <= reach {
            for centre in [nominal + d, nominal - d] {
                let q = straightness(centre);
                if q > best.0 && (q >= STRAIGHT_ENOUGH || best.0 < STRAIGHT_ENOUGH) {
                    best = (q, centre);
                }
            }
            d += STEP;
        }
        best.1.rem_euclid(total)
    }

    /// One pass around the closed loop at `cut_z`: plunge at the seam, follow
    /// the loop, and step over each raised stretch with a vertical lift at its
    /// start and a vertical plunge at its end, so a tab keeps square ends at
    /// exactly the stretch it was given.
    fn follow(
        &self,
        toolpath: &mut Toolpath,
        points: &[Point2D],
        cut_z: f64,
        raised: &[(f64, f64, f64)],
        settings: &CamSettings,
    ) {
        let height_at = |s: f64| {
            raised
                .iter()
                .filter(|(from, to, _)| s >= *from && s <= *to)
                .map(|(_, _, top)| *top)
                .fold(cut_z, f64::max)
        };
        let start = &points[0];
        let mut z = height_at(0.0);
        toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));
        toolpath.push(ToolpathSegment::linear(
            start.x,
            start.y,
            z,
            settings.plunge_rate,
        ));

        let mut s0 = 0.0;
        for k in 0..points.len() {
            let (a, b) = (&points[k], &points[(k + 1) % points.len()]);
            let len = a.distance_to(b);
            if len <= 0.0 {
                continue;
            }
            let s1 = s0 + len;
            // Where the height changes inside this segment.
            let mut cuts: Vec<f64> = raised
                .iter()
                .flat_map(|(from, to, _)| [*from, *to])
                .filter(|s| *s > s0 + 1e-9 && *s < s1 - 1e-9)
                .collect();
            cuts.push(s1);
            cuts.sort_by(f64::total_cmp);
            let mut from = s0;
            for to in cuts {
                let want = height_at((from + to) / 2.0);
                if (want - z).abs() > 1e-9 {
                    let t = (from - s0) / len;
                    let (x, y) = (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                    let rate = if want < z {
                        settings.plunge_rate
                    } else {
                        settings.feed_rate
                    };
                    toolpath.push(ToolpathSegment::linear(x, y, want, rate));
                    z = want;
                }
                let t = (to - s0) / len;
                toolpath.push(ToolpathSegment::linear(
                    a.x + (b.x - a.x) * t,
                    a.y + (b.y - a.y) * t,
                    z,
                    settings.feed_rate,
                ));
                from = to;
            }
            s0 = s1;
        }
    }
}

/// The point at path length `s` (any sign; it wraps) along the closed loop.
fn point_at(points: &[Point2D], total: f64, s: f64) -> Point2D {
    let mut s = s.rem_euclid(total);
    for k in 0..points.len() {
        let (a, b) = (&points[k], &points[(k + 1) % points.len()]);
        let len = a.distance_to(b);
        if s <= len && len > 0.0 {
            let t = s / len;
            return Point2D::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        }
        s -= len;
    }
    Point2D::new(points[0].x, points[0].y)
}

/// Length of the closed loop through `points`.
fn loop_length(points: &[Point2D]) -> f64 {
    (0..points.len())
        .map(|k| points[k].distance_to(&points[(k + 1) % points.len()]))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contour2d_basic() {
        let contour = Contour::rectangle(0.0, 0.0, 30.0, 20.0);
        let op = Contour2D::new(contour, 5.0);

        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings::default();

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_contour2d_with_tabs() {
        let contour = Contour::rectangle(0.0, 0.0, 50.0, 40.0);
        let op = Contour2D::new(contour, 10.0).with_tabs(4, 5.0, 2.0);

        assert_eq!(op.tabs.len(), 4);

        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 25.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepdown: 5.0,
            ..CamSettings::default()
        };

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());

        // Check that we have Z variations in final pass (tabs)
        let z_values: Vec<f64> = toolpath
            .segments
            .iter()
            .filter_map(|s| s.target())
            .map(|[_, _, z]| z)
            .collect();

        // Should have at least two distinct Z values (cut depth and tab height)
        let mut unique_z: Vec<f64> = z_values.clone();
        unique_z.sort_by(|a, b| a.total_cmp(b));
        unique_z.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        assert!(unique_z.len() >= 2);
    }

    #[test]
    fn test_contour2d_circle() {
        let contour = Contour::circle(25.0, 25.0, 15.0);
        let op = Contour2D::new(contour, 6.0);

        let tool = Tool::default_endmill();
        let settings = CamSettings::default();

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
    }

    fn polyline(points: &[(f64, f64)]) -> Contour {
        let mut c = Contour::new(Point2D::new(points[0].0, points[0].1));
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            c.line_to(Point2D::new(p.0, p.1));
        }
        c
    }

    /// The side is the whole point of a contour cut: outside keeps the cutter
    /// off the part, inside keeps it within the opening. Either winding.
    #[test]
    fn test_contour2d_cuts_on_the_named_side() {
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings::default();
        let ccw = [(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)];
        let cw = [(0.0, 0.0), (0.0, 20.0), (30.0, 20.0), (30.0, 0.0)];
        for loop_points in [ccw, cw] {
            for inside in [false, true] {
                let contour = polyline(&loop_points);
                let op = if inside {
                    Contour2D::inside(contour, 2.0)
                } else {
                    Contour2D::outside(contour, 2.0)
                };
                let toolpath = op.generate(&tool, &settings).unwrap();
                let cuts: Vec<[f64; 3]> = toolpath
                    .segments
                    .iter()
                    .filter(|s| s.is_cutting())
                    .filter_map(|s| s.target())
                    .filter(|t| t[2] < 0.0)
                    .collect();
                assert!(cuts.len() >= 4);
                for t in cuts {
                    // Signed clearance of the tool centre from the rectangle's
                    // boundary: positive inside the rectangle.
                    let within = t[0].min(30.0 - t[0]).min(t[1]).min(20.0 - t[1]);
                    // Distance from the rectangle for a point outside it
                    // (round joins put the corners on an arc).
                    let dx = (-t[0]).max(t[0] - 30.0).max(0.0);
                    let dy = (-t[1]).max(t[1] - 20.0).max(0.0);
                    if inside {
                        assert!(within >= 3.0 - 0.02, "inside cut at {t:?} gouges the wall");
                    } else {
                        assert!(within <= 0.0, "outside cut at {t:?} is inside the part");
                        assert!(
                            dx.hypot(dy) >= 3.0 - 0.02,
                            "outside cut at {t:?} gouges the part"
                        );
                    }
                }
            }
        }
    }

    /// A cutter wider than a neck of the opening cannot follow it in one
    /// loop; that is an error, not a silently shorter path.
    #[test]
    fn test_contour2d_inside_refuses_a_neck_the_cutter_cannot_pass() {
        // Two 20 mm squares joined by a 4 mm wide neck.
        let dumbbell = polyline(&[
            (0.0, 0.0),
            (20.0, 0.0),
            (20.0, 8.0),
            (30.0, 8.0),
            (30.0, 0.0),
            (50.0, 0.0),
            (50.0, 20.0),
            (30.0, 20.0),
            (30.0, 12.0),
            (20.0, 12.0),
            (20.0, 20.0),
            (0.0, 20.0),
        ]);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let result = Contour2D::inside(dumbbell, 2.0).generate(&tool, &CamSettings::default());
        assert!(
            matches!(result, Err(CamError::ContourSplit(2))),
            "{result:?}"
        );
    }

    /// Metal left under each tab, measured on the toolpath: for every pass
    /// that goes below the tab top, the length the cutter spends lifted, less
    /// the tool diameter it cuts into the two ends.
    fn tab_metal_per_pass(toolpath: &Toolpath, tab_top: f64, tool_diameter: f64) -> Vec<Vec<f64>> {
        let mut passes: Vec<Vec<f64>> = Vec::new();
        let mut at = [0.0, 0.0, 10.0];
        let mut run: Option<f64> = None;
        let mut below = false;
        for seg in &toolpath.segments {
            let Some(to) = seg.target() else { continue };
            if seg.is_rapid() {
                if let Some(r) = run.take() {
                    passes.last_mut().unwrap().push(r - tool_diameter);
                }
                if below {
                    below = false;
                } else if passes.last().is_some_and(Vec::is_empty) {
                    passes.pop();
                }
                passes.push(Vec::new());
            } else {
                let lifted = (at[2] - tab_top).abs() < 1e-9 && (to[2] - tab_top).abs() < 1e-9;
                below |= to[2] < tab_top - 1e-9;
                let xy = (to[0] - at[0]).hypot(to[1] - at[1]);
                // A change of height happens in place, never along a ramp.
                assert!(
                    (to[2] - at[2]).abs() < 1e-9 || xy < 1e-9 || at[2] > 0.0,
                    "ramp at {to:?}"
                );
                if lifted {
                    *run.get_or_insert(0.0) += xy;
                } else if let Some(r) = run.take() {
                    passes.last_mut().unwrap().push(r - tool_diameter);
                }
            }
            at = to;
        }
        passes.retain(|p| !p.is_empty());
        passes
    }

    #[test]
    fn test_contour2d_tabs_survive_every_pass_at_their_stated_width() {
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 25.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        // Depth 4 in 1 mm passes, tabs 1.5 tall: the passes at -3 and -4 both
        // reach below the tab top at -2.5.
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 50.0, 40.0), 4.0)
            .with_tabs(3, 5.0, 1.5);
        let toolpath = op.generate(&tool, &settings).unwrap();
        let passes = tab_metal_per_pass(&toolpath, -2.5, 6.0);
        assert_eq!(passes.len(), 2, "{passes:?}");
        for pass in passes {
            assert_eq!(pass.len(), 3, "{pass:?}");
            for metal in pass {
                assert!(
                    (metal - 5.0).abs() < 1e-6,
                    "tab leaves {metal} mm, asked for 5"
                );
            }
        }
    }

    /// A tab on the seam (where each pass starts and ends) is one tab, whole.
    #[test]
    fn test_contour2d_tab_across_the_seam_is_whole() {
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 25.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepdown: 4.0,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 50.0, 40.0), 4.0)
            .with_tab(Tab::new(0.0, 5.0, 1.5));
        let toolpath = op.generate(&tool, &settings).unwrap();
        // The pass starts lifted (no plunge through the tab) and the two
        // halves add up to the tab plus one tool diameter.
        let first_cut = toolpath
            .segments
            .iter()
            .find(|s| s.is_cutting())
            .and_then(|s| s.target())
            .unwrap();
        assert!(
            (first_cut[2] + 2.5).abs() < 1e-9,
            "plunged to {first_cut:?}"
        );
        let lifted: f64 = toolpath
            .segments
            .windows(2)
            .filter_map(|w| Some((w[0].target()?, w[1].target()?, w[1].is_cutting())))
            .filter(|(a, b, cutting)| {
                *cutting && (a[2] + 2.5).abs() < 1e-9 && (b[2] + 2.5).abs() < 1e-9
            })
            .map(|(a, b, _)| (b[0] - a[0]).hypot(b[1] - a[1]))
            .sum();
        assert!((lifted - 11.0).abs() < 1e-6, "lifted over {lifted} mm");
    }

    /// Evenly spaced tabs land wherever the arithmetic puts them — in a notch,
    /// around a corner. Each must settle on a stretch that runs straight.
    #[test]
    fn test_contour2d_tabs_settle_on_straight_stretches() {
        // 60 x 40 with an 8 mm wide, 6 mm deep notch in the bottom edge.
        let notched = [
            (0.0, 0.0),
            (26.0, 0.0),
            (26.0, 6.0),
            (34.0, 6.0),
            (34.0, 0.0),
            (60.0, 0.0),
            (60.0, 40.0),
            (0.0, 40.0),
        ];
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 25.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepdown: 4.0,
            ..CamSettings::default()
        };
        for count in 1..=7 {
            let op = Contour2D::outside(polyline(&notched), 4.0).with_tabs(count, 5.0, 1.5);
            let toolpath = op.generate(&tool, &settings).unwrap();
            let mut runs: Vec<Vec<[f64; 3]>> = Vec::new();
            let mut lifted = false;
            for t in toolpath
                .segments
                .iter()
                .filter(|s| s.is_cutting())
                .filter_map(|s| s.target())
            {
                let now = (t[2] + 2.5).abs() < 1e-9;
                if now && !lifted {
                    runs.push(Vec::new());
                }
                if now {
                    runs.last_mut().unwrap().push(t);
                }
                lifted = now;
            }
            assert_eq!(runs.len(), count);
            for run in runs {
                let (a, b) = (run[0], run[run.len() - 1]);
                let chord = (b[0] - a[0]).hypot(b[1] - a[1]);
                assert!(
                    chord >= 0.98 * 11.0,
                    "{count} tabs: one bends (chord {chord:.2} of 11) near {a:?}"
                );
            }
        }
    }

    #[test]
    fn test_tab_creation() {
        let tab = Tab::new(0.25, 5.0, 2.0);
        assert!((tab.position - 0.25).abs() < 1e-6);
        assert!((tab.width - 5.0).abs() < 1e-6);
        assert!((tab.height - 2.0).abs() < 1e-6);
    }
}
