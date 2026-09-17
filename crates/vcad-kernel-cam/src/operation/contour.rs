//! 2D contour/profile machining operation.

use crate::operation::{Contour, ContourSegment, Point2D};
use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
use geo::algorithm::contains::Contains;
use geo::algorithm::euclidean_distance::EuclideanDistance;
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
    /// Height of the tab (how much material to leave). Measured up from the
    /// underside of the stock, which is `depth` below Z0 whatever the bottom
    /// allowance does to the depth actually cut.
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

/// Which way round the cutter travels, and so which way the chip loads.
///
/// The spindle is `M3` — clockwise seen from above. With a right-hand cutter
/// that puts the material on the **right** of the direction of travel in a
/// climb cut (the tooth enters at full chip thickness) and on the left in a
/// conventional one. Around a hole that means climb runs counter-clockwise;
/// around a part it runs clockwise. The winding of the contour the caller
/// drew has nothing to do with it: the path is re-wound to suit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CutDirection {
    /// Climb (down) milling: the usual choice on a rigid machine.
    #[default]
    Climb,
    /// Conventional (up) milling.
    Conventional,
}

/// How the cutter gets down to the depth of a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EntryStyle {
    /// Ramp along the contour at [`Contour2D::ramp_angle`], zig-zagging inside
    /// the stretch before the first tab when one ramp length does not fit.
    #[default]
    Ramp,
    /// Straight down at the seam. Only what the machine can stand: an end mill
    /// has no cutting edge at its centre.
    Plunge,
}

/// What to do when the cutter is about as wide as the opening, so the offset
/// path collapses or falls into separate pieces.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ThinSlotStrategy {
    /// Refuse the job ([`CamError::ContourSplit`]). The default: a slot the
    /// cutter does not fit is the caller's problem to solve, not the CAM's.
    #[default]
    Refuse,
    /// Follow the centre line of the reachable region where the offset
    /// collapses, accepting that the cutter cuts past the wall there.
    /// Refused if the measured overcut anywhere exceeds `tolerance` mm.
    CentreLine {
        /// Largest wall overcut that may be accepted, in mm.
        tolerance: f64,
    },
}

/// Which phase of the operation a report entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContourPhase {
    /// Roughing passes, at stepdown, leaving stock on the wall.
    Rough,
    /// Finishing passes, on the true offset.
    Finish,
    /// The spring pass: the finish pass again, same path, same depth.
    Spring,
}

/// A stretch of the path that was cut on the centre line of the opening
/// instead of on the true offset, because the cutter does not fit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentreLineStretch {
    /// Which path this is on.
    pub phase: ContourPhase,
    /// Path length along that loop where the stretch starts, in mm.
    pub from: f64,
    /// Path length along that loop where it ends, in mm.
    pub to: f64,
    /// Deepest the cutter reaches past the contour wall on this stretch, mm.
    pub max_wall_error: f64,
}

/// What a [`Contour2D`] actually did: the numbers a machinist needs to decide
/// whether to run it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContourReport {
    /// Depth actually cut below Z0, in mm (positive), after the bottom
    /// allowance.
    pub final_depth: f64,
    /// Roughing passes generated.
    pub rough_passes: usize,
    /// Finishing passes generated.
    pub finish_passes: usize,
    /// Whether a spring pass was added.
    pub spring_pass: bool,
    /// Passes entered along a ramp.
    pub ramp_entries: usize,
    /// Passes entered on a tangential lead-in arc.
    pub lead_entries: usize,
    /// Passes that could only be entered straight down.
    pub plunge_entries: usize,
    /// Stretches cut on the centre line, with the wall error on each.
    pub centre_line: Vec<CentreLineStretch>,
    /// Deepest the cutter reaches past the contour wall anywhere, in mm. Zero
    /// unless a centre-line fallback was used.
    pub max_wall_error: f64,
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
    /// Stock left on the wall by the roughing phase, in mm. Zero means no
    /// separate roughing phase: the passes go straight onto the true offset.
    pub stock_to_leave: f64,
    /// The cutter runs inside the contour (a hole or opening) instead of
    /// around it: tool-radius compensation and stock-to-leave move the path
    /// inwards.
    #[serde(default)]
    pub inside: bool,
    /// How many Z steps the finishing phase takes. `None` means one full-depth
    /// pass when there was a roughing phase, and the stepdown otherwise.
    #[serde(default)]
    pub finish_stepdowns: Option<usize>,
    /// Repeat the last finish pass once more on the same path, to cut what the
    /// tool's deflection left behind.
    #[serde(default)]
    pub spring_pass: bool,
    /// Feed for the finish and spring passes (mm/min). Falls back to the
    /// job's feed rate.
    #[serde(default)]
    pub finish_feed: Option<f64>,
    /// Climb or conventional, chosen independently of the contour's winding.
    #[serde(default)]
    pub direction: CutDirection,
    /// How a pass gets down to its depth when the waste has not been cleared.
    #[serde(default)]
    pub entry: EntryStyle,
    /// Ramp angle in degrees.
    #[serde(default = "default_ramp_angle")]
    pub ramp_angle: f64,
    /// Lead the finish pass in and out on a tangential arc when the waste side
    /// has room for one.
    #[serde(default = "default_true")]
    pub lead_in: bool,
    /// Radius of the lead arcs, in mm. Defaults to the tool radius, and is
    /// capped at tool radius + stock to leave so the arc stays in the air the
    /// roughing phase cleared.
    #[serde(default)]
    pub lead_radius: Option<f64>,
    /// Positive leaves an onion skin (the cut stops this far above `depth`);
    /// negative cuts that far past it, which needs a spoilboard.
    #[serde(default)]
    pub bottom_allowance: f64,
    /// Thickness of the sacrificial board under the stock, in mm. Required
    /// before a negative bottom allowance is allowed.
    #[serde(default)]
    pub spoilboard_thickness: Option<f64>,
    /// What to do when the cutter is as wide as the opening.
    #[serde(default)]
    pub thin_slot: ThinSlotStrategy,
}

fn default_ramp_angle() -> f64 {
    3.0
}

fn default_true() -> bool {
    true
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
            finish_stepdowns: None,
            spring_pass: false,
            finish_feed: None,
            direction: CutDirection::default(),
            entry: EntryStyle::default(),
            ramp_angle: default_ramp_angle(),
            lead_in: true,
            lead_radius: None,
            bottom_allowance: 0.0,
            spoilboard_thickness: None,
            thin_slot: ThinSlotStrategy::default(),
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

    /// Set stock to leave on the wall for the finishing phase.
    pub fn with_stock_to_leave(mut self, stock: f64) -> Self {
        self.stock_to_leave = stock;
        self
    }

    /// Take the finishing phase down in `count` steps instead of one.
    pub fn with_finish_stepdowns(mut self, count: usize) -> Self {
        self.finish_stepdowns = Some(count);
        self
    }

    /// Repeat the finish pass at the same depth on the same path.
    pub fn with_spring_pass(mut self, on: bool) -> Self {
        self.spring_pass = on;
        self
    }

    /// Feed for the finish and spring passes (mm/min).
    pub fn with_finish_feed(mut self, feed: f64) -> Self {
        self.finish_feed = Some(feed);
        self
    }

    /// Climb or conventional milling.
    pub fn with_direction(mut self, direction: CutDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Ramp angle for entries, in degrees.
    pub fn with_ramp_angle(mut self, degrees: f64) -> Self {
        self.ramp_angle = degrees;
        self
    }

    /// Enter straight down instead of ramping.
    pub fn with_plunge_entry(mut self) -> Self {
        self.entry = EntryStyle::Plunge;
        self
    }

    /// Lead the finish pass in and out on a tangential arc (default on).
    pub fn with_lead_in(mut self, on: bool) -> Self {
        self.lead_in = on;
        self
    }

    /// Radius of the lead arcs, in mm.
    pub fn with_lead_radius(mut self, radius: f64) -> Self {
        self.lead_radius = Some(radius);
        self
    }

    /// Leave an onion skin (positive) or break through (negative), in mm.
    pub fn with_bottom_allowance(mut self, allowance: f64) -> Self {
        self.bottom_allowance = allowance;
        self
    }

    /// Declare the sacrificial board under the stock, in mm.
    pub fn with_spoilboard(mut self, thickness: f64) -> Self {
        self.spoilboard_thickness = Some(thickness);
        self
    }

    /// What to do when the cutter is about as wide as the opening.
    pub fn with_thin_slot(mut self, strategy: ThinSlotStrategy) -> Self {
        self.thin_slot = strategy;
        self
    }

    /// Follow the centre line where the offset collapses, refusing if the
    /// cutter would cut more than `tolerance` mm past the wall.
    pub fn with_centre_line_fallback(self, tolerance: f64) -> Self {
        self.with_thin_slot(ThinSlotStrategy::CentreLine { tolerance })
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
        self.generate_reported(tool, settings).map(|(path, _)| path)
    }

    /// Generate the toolpath and a report of what it does: depths, passes,
    /// entries, and any stretch cut on the centre line with the wall error
    /// there.
    pub fn generate_reported(
        &self,
        tool: &Tool,
        settings: &CamSettings,
    ) -> Result<(Toolpath, ContourReport), CamError> {
        let tool_radius = tool.radius();
        if tool_radius <= 0.0 {
            return Err(CamError::InvalidToolDiameter(tool.diameter()));
        }
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(settings.stepdown));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }
        if let Some(feed) = self.finish_feed {
            if feed <= 0.0 {
                return Err(CamError::InvalidFeedRate(feed));
            }
        }
        if self.ramp_angle <= 0.0 || self.ramp_angle >= 90.0 {
            return Err(CamError::InvalidRampAngle(self.ramp_angle));
        }
        if self.stock_to_leave < 0.0 {
            return Err(CamError::InvalidStockToLeave(self.stock_to_leave));
        }
        if self.finish_stepdowns == Some(0) {
            return Err(CamError::InvalidStepdown(0.0));
        }
        if !self.contour.is_closed(0.01) {
            let gap = self.contour.start.distance_to(&self.contour.end_point());
            return Err(CamError::NotClosed(gap));
        }
        for tab in &self.tabs {
            if tab.position < 0.0 || tab.position > 1.0 {
                return Err(CamError::InvalidTabPosition(tab.position));
            }
        }

        // The stock's underside stays at -depth however the allowance moves
        // the cut: a break-through eats into the spoilboard, an onion skin
        // stops short, and tab tops are measured from the underside either way.
        if self.bottom_allowance < 0.0 {
            let overcut = -self.bottom_allowance;
            let declared = self.spoilboard_thickness.unwrap_or(0.0);
            if declared < overcut - 1e-9 {
                return Err(CamError::BreakThroughWithoutSpoilboard {
                    overcut,
                    spoilboard: declared,
                });
            }
        }
        let final_depth = self.depth - self.bottom_allowance;
        if final_depth <= 1e-9 {
            return Err(CamError::BottomAllowanceExceedsDepth {
                allowance: self.bottom_allowance,
                depth: self.depth,
            });
        }

        let mut report = ContourReport {
            final_depth,
            spring_pass: self.spring_pass,
            ..ContourReport::default()
        };

        let side = if self.inside { -1.0 } else { 1.0 };
        let wall = Wall::new(self.contour.to_geo_polygon(), self.inside);
        let roughing = self.stock_to_leave > 1e-9;

        let finish_loop = self.loop_for(
            self.offset + side * tool_radius,
            tool_radius,
            ContourPhase::Finish,
            &wall,
            &mut report,
        )?;
        let rough_loop = if roughing {
            Some(self.loop_for(
                self.offset + side * (tool_radius + self.stock_to_leave),
                tool_radius,
                ContourPhase::Rough,
                &wall,
                &mut report,
            )?)
        } else {
            None
        };

        let rough_levels: Vec<f64> = match &rough_loop {
            Some(_) => levels(
                final_depth,
                (final_depth / settings.stepdown).ceil() as usize,
            ),
            None => Vec::new(),
        };
        let finish_count = self.finish_stepdowns.unwrap_or(if roughing {
            1
        } else {
            (final_depth / settings.stepdown).ceil() as usize
        });
        let finish_levels = levels(final_depth, finish_count.max(1));
        report.rough_passes = rough_levels.len();
        report.finish_passes = finish_levels.len();

        let mut toolpath = Toolpath::new();
        toolpath.push(ToolpathSegment::comment(format!(
            "Contour 2D: {} of the contour, {}, depth={:.3}mm, offset={:.3}mm, {} tabs",
            if self.inside { "inside" } else { "outside" },
            match self.direction {
                CutDirection::Climb => "climb",
                CutDirection::Conventional => "conventional",
            },
            final_depth,
            self.offset,
            self.tabs.len()
        )));
        if self.bottom_allowance > 0.0 {
            toolpath.push(ToolpathSegment::comment(format!(
                "bottom allowance: {:.3}mm of skin left below the cut",
                self.bottom_allowance
            )));
        } else if self.bottom_allowance < 0.0 {
            toolpath.push(ToolpathSegment::comment(format!(
                "break-through: {:.3}mm past the stock into a {:.3}mm spoilboard",
                -self.bottom_allowance,
                self.spoilboard_thickness.unwrap_or(0.0)
            )));
        }
        for stretch in &report.centre_line {
            toolpath.push(ToolpathSegment::comment(format!(
                "centre-line cut: {:?} path {:.2}..{:.2}mm, wall cut {:.3}mm past the contour",
                stretch.phase, stretch.from, stretch.to, stretch.max_wall_error
            )));
        }

        let finish_feed = self.finish_feed.unwrap_or(settings.feed_rate);
        let ctx = Ctx {
            wall: &wall,
            tool,
            settings,
        };

        if let Some(lp) = &rough_loop {
            toolpath.push(ToolpathSegment::comment(format!(
                "roughing: {} passes leaving {:.3}mm on the wall",
                rough_levels.len(),
                self.stock_to_leave
            )));
            let mut entry_z = 0.0;
            for z in &rough_levels {
                self.emit_pass(
                    &mut toolpath,
                    lp,
                    &ctx,
                    PassPlan {
                        cut_z: *z,
                        entry_z,
                        feed: settings.feed_rate,
                        cleared: false,
                    },
                    &mut report,
                );
                entry_z = *z;
            }
        }

        toolpath.push(ToolpathSegment::comment(format!(
            "finishing: {} pass(es) at {:.0}mm/min",
            finish_levels.len(),
            finish_feed
        )));
        let mut entry_z = 0.0;
        for z in &finish_levels {
            self.emit_pass(
                &mut toolpath,
                &finish_loop,
                &ctx,
                PassPlan {
                    cut_z: *z,
                    entry_z,
                    feed: finish_feed,
                    cleared: roughing,
                },
                &mut report,
            );
            entry_z = *z;
        }

        if self.spring_pass {
            toolpath.push(ToolpathSegment::comment("spring pass"));
            self.emit_pass(
                &mut toolpath,
                &finish_loop,
                &ctx,
                PassPlan {
                    cut_z: -final_depth,
                    entry_z: -final_depth,
                    feed: finish_feed,
                    cleared: true,
                },
                &mut report,
            );
        }

        // Final retract over the contour's own start, where the job began.
        toolpath.push(ToolpathSegment::rapid(
            self.contour.start.x,
            self.contour.start.y,
            settings.safe_z,
        ));

        Ok((toolpath, report))
    }

    /// The tool-centre loop for one phase: the offset path, wound to suit the
    /// cut direction, or the centre line of the reachable region when the
    /// cutter does not fit and the caller asked for that.
    fn loop_for(
        &self,
        offset: f64,
        tool_radius: f64,
        phase: ContourPhase,
        wall: &Wall,
        report: &mut ContourReport,
    ) -> Result<Loop, CamError> {
        let points = match self.offset_contour(offset) {
            Ok(points) => points,
            Err(err) => match (self.thin_slot, &err) {
                (
                    ThinSlotStrategy::CentreLine { tolerance },
                    CamError::ContourSplit(_) | CamError::EmptyContour,
                ) => {
                    let (points, stretches, worst) =
                        self.centre_line(offset.abs(), tool_radius, tolerance, wall, phase)?;
                    report.max_wall_error = report.max_wall_error.max(worst);
                    report.centre_line.extend(stretches);
                    points
                }
                _ => return Err(err),
            },
        };
        let points = dedup_closing(points);
        if points.len() < 3 {
            return Err(CamError::EmptyContour);
        }
        // The winding the caller drew decides nothing: climb around a hole is
        // counter-clockwise, climb around a part is clockwise.
        let want_ccw = (self.direction == CutDirection::Climb) == self.inside;
        let points = if (signed_area(&points) > 0.0) == want_ccw {
            points
        } else {
            reverse_keeping_seam(&points)
        };
        let lp = Loop::new(points);
        if lp.total() <= 0.0 {
            return Err(CamError::EmptyContour);
        }
        Ok(lp)
    }

    /// Offset the contour by the given amount (native version with geo-clipper).
    #[cfg(not(target_arch = "wasm32"))]
    fn offset_contour(&self, offset: f64) -> Result<Vec<Point2D>, CamError> {
        if offset.abs() < 0.001 {
            // No offset needed, return original points
            return Ok(self.contour_to_points(&self.contour));
        }

        let result = offset_polygons(&self.contour.to_geo_polygon(), offset);

        if result.is_empty() {
            return Err(CamError::EmptyContour);
        }
        // An inward offset that falls apart means the cutter cannot pass a
        // neck of the opening. Following only the first piece would leave the
        // rest uncut without a word.
        if result.len() > 1 {
            return Err(CamError::ContourSplit(result.len()));
        }

        // Extract points from first polygon
        if let Some(poly) = result.first() {
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
    fn raised_intervals(&self, lp: &Loop, cut_z: f64, tool_diameter: f64) -> Vec<(f64, f64, f64)> {
        let total = lp.total();
        let mut out = Vec::new();
        if total <= 0.0 {
            return out;
        }
        for tab in &self.tabs {
            // Measured from the underside of the stock, not from the depth
            // actually cut: an onion skin or a break-through must not move the
            // top of a tab.
            let top = -self.depth + tab.height;
            if top <= cut_z + 1e-9 {
                continue;
            }
            let half = ((tab.width + tool_diameter) / 2.0).min(total / 2.0);
            let centre = self.settle_tab(lp, total, tab.position, half);
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
    fn settle_tab(&self, lp: &Loop, total: f64, position: f64, half: f64) -> f64 {
        const STRAIGHT_ENOUGH: f64 = 0.98;
        const STEP: f64 = 0.5;
        let nominal = position.rem_euclid(1.0) * total;
        let straightness = |centre: f64| {
            let a = lp.at(centre - half);
            let b = lp.at(centre + half);
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
}

/// What every pass of the operation shares.
struct Ctx<'a> {
    wall: &'a Wall,
    tool: &'a Tool,
    settings: &'a CamSettings,
}

/// What one pass has to do.
struct PassPlan {
    cut_z: f64,
    entry_z: f64,
    feed: f64,
    /// The waste beside the path has already been cut away to this depth, so
    /// the cutter may go down in the air beside the wall.
    cleared: bool,
}

/// A ramp entry: where it starts on the loop and the (end, z) of each leg.
struct RampPlan {
    start_s: f64,
    legs: Vec<(f64, f64)>,
}

impl Contour2D {
    /// One pass: entry, the loop with its tabs, and the retract.
    fn emit_pass(
        &self,
        toolpath: &mut Toolpath,
        lp: &Loop,
        ctx: &Ctx,
        plan: PassPlan,
        report: &mut ContourReport,
    ) {
        let Ctx {
            wall,
            tool,
            settings,
        } = *ctx;
        let raised = self.raised_intervals(lp, plan.cut_z, tool.diameter());

        // A pass whose waste is already cleared may go down beside the wall,
        // best of all on an arc that meets the wall tangentially.
        let lead = if plan.cleared && self.lead_in {
            let radius = self
                .lead_radius
                .unwrap_or_else(|| tool.radius())
                .min(tool.radius() + self.stock_to_leave);
            straight_point(lp, &raised, radius)
                .and_then(|s| Some((s, self.lead_arcs(lp, wall, s, radius)?)))
        } else {
            None
        };

        if let Some((lead_s, (arc_in, arc_out))) = lead {
            let seam_z = height_at(&raised, plan.cut_z, lead_s);
            report.lead_entries += 1;
            toolpath.push(ToolpathSegment::comment("lead-in arc"));
            toolpath.push(ToolpathSegment::rapid(
                arc_in[0].x,
                arc_in[0].y,
                settings.safe_z,
            ));
            toolpath.push(ToolpathSegment::linear(
                arc_in[0].x,
                arc_in[0].y,
                seam_z,
                settings.plunge_rate,
            ));
            for p in &arc_in[1..] {
                toolpath.push(ToolpathSegment::linear(p.x, p.y, seam_z, plan.feed));
            }
            toolpath.push(ToolpathSegment::comment("profile"));
            self.emit_loop(toolpath, lp, lead_s, &raised, &plan, settings);
            toolpath.push(ToolpathSegment::comment("lead-out arc"));
            let mut last = arc_out[0];
            for p in &arc_out {
                toolpath.push(ToolpathSegment::linear(p.x, p.y, seam_z, plan.feed));
                last = *p;
            }
            toolpath.push(ToolpathSegment::rapid(last.x, last.y, settings.safe_z));
            return;
        }

        let ramp = if self.entry == EntryStyle::Ramp && !plan.cleared {
            self.plan_ramp(lp, &raised, plan.entry_z, plan.cut_z)
        } else {
            None
        };

        let start_s = match &ramp {
            Some(plan) => plan.start_s,
            None => 0.0,
        };
        let start = lp.at(start_s);
        toolpath.push(ToolpathSegment::comment("approach"));
        toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));

        let end_s = match ramp {
            Some(RampPlan { start_s, legs }) => {
                report.ramp_entries += 1;
                // Down to where the last pass finished — air, all of it — and
                // only then into the metal along the ramp.
                toolpath.push(ToolpathSegment::linear(
                    start.x,
                    start.y,
                    plan.entry_z,
                    settings.plunge_rate,
                ));
                toolpath.push(ToolpathSegment::comment(format!(
                    "ramp entry: {:.1} deg, {} leg(s)",
                    self.ramp_angle,
                    legs.len()
                )));
                let mut s = start_s;
                let mut z = plan.entry_z;
                for (to_s, to_z) in legs {
                    emit_run(toolpath, lp, (s, to_s), (z, to_z), plan.feed);
                    s = to_s;
                    z = to_z;
                }
                s
            }
            None => {
                report.plunge_entries += 1;
                toolpath.push(ToolpathSegment::comment("plunge entry"));
                toolpath.push(ToolpathSegment::linear(
                    start.x,
                    start.y,
                    height_at(&raised, plan.cut_z, start_s),
                    settings.plunge_rate,
                ));
                start_s
            }
        };

        toolpath.push(ToolpathSegment::comment("profile"));
        self.emit_loop(toolpath, lp, end_s, &raised, &plan, settings);
        // Straight up out of the cut, after the last pass too: the final
        // rapid below would otherwise travel in XY.
        let end = lp.at(end_s);
        toolpath.push(ToolpathSegment::rapid(end.x, end.y, settings.safe_z));
    }

    /// A ramp down to `cut_z` along the contour, zig-zagging inside the
    /// stretch that runs from the seam to the first tab when one length of
    /// ramp does not fit. `None` when there is nowhere to ramp — the caller
    /// then goes straight down.
    fn plan_ramp(
        &self,
        lp: &Loop,
        raised: &[(f64, f64, f64)],
        entry_z: f64,
        cut_z: f64,
    ) -> Option<RampPlan> {
        let drop = entry_z - cut_z;
        if drop <= 1e-9 {
            return None;
        }
        let total = lp.total();
        // A ramp may not descend through a tab: start after any tab covering
        // the seam, and stop before the next one.
        let start_s = first_free(raised, total)?;
        let window = free_window(raised, total, start_s) - 1e-6;
        if window <= 1e-6 {
            return None;
        }
        let needed = drop / self.ramp_angle.to_radians().tan();
        if needed <= window {
            return Some(RampPlan {
                start_s,
                legs: vec![(start_s + needed, cut_z)],
            });
        }
        let n = (needed / window).ceil() as usize;
        let legs = (0..n)
            .map(|i| {
                let to_s = if i % 2 == 0 {
                    start_s + window
                } else {
                    start_s
                };
                (to_s, entry_z - drop * (i + 1) as f64 / n as f64)
            })
            .collect();
        Some(RampPlan { start_s, legs })
    }

    /// One lap of the closed loop from `s_start` at `cut_z`, stepping over
    /// each raised stretch with a vertical lift at its start and a vertical
    /// plunge at its end, so a tab keeps square ends at exactly the stretch it
    /// was given.
    fn emit_loop(
        &self,
        toolpath: &mut Toolpath,
        lp: &Loop,
        s_start: f64,
        raised: &[(f64, f64, f64)],
        plan: &PassPlan,
        settings: &CamSettings,
    ) {
        let (cut_z, feed) = (plan.cut_z, plan.feed);
        let total = lp.total();
        let end = s_start + total;
        let mut marks = lp.vertices_between(s_start, end);
        for (from, to, _) in raised {
            for base in [*from, *to] {
                let mut s = base + (s_start / total).floor() * total;
                while s <= s_start + 1e-12 {
                    s += total;
                }
                if s < end - 1e-12 {
                    marks.push(s);
                }
            }
        }
        marks.push(end);
        marks.sort_by(f64::total_cmp);

        let mut z = height_at(raised, cut_z, s_start.rem_euclid(total));
        let mut from = s_start;
        for to in marks {
            if to <= from + 1e-12 {
                continue;
            }
            let want = height_at(raised, cut_z, ((from + to) / 2.0).rem_euclid(total));
            if (want - z).abs() > 1e-9 {
                let p = lp.at(from);
                let rate = if want < z { settings.plunge_rate } else { feed };
                toolpath.push(ToolpathSegment::linear(p.x, p.y, want, rate));
                z = want;
            }
            let p = lp.at(to);
            toolpath.push(ToolpathSegment::linear(p.x, p.y, z, feed));
            from = to;
        }
    }
}

/// One leg of a ramp: from `s0` to `s1` along the loop (either way round),
/// descending in step with the distance travelled.
fn emit_run(
    toolpath: &mut Toolpath,
    lp: &Loop,
    (s0, s1): (f64, f64),
    (z0, z1): (f64, f64),
    feed: f64,
) {
    {
        let span = (s1 - s0).abs();
        if span <= 1e-12 {
            return;
        }
        let mut marks = lp.vertices_between(s0.min(s1), s0.max(s1));
        if s1 < s0 {
            marks.reverse();
        }
        for s in marks.into_iter().chain(std::iter::once(s1)) {
            let t = ((s - s0).abs() / span).clamp(0.0, 1.0);
            let p = lp.at(s);
            toolpath.push(ToolpathSegment::linear(p.x, p.y, z0 + (z1 - z0) * t, feed));
        }
    }
}

impl Contour2D {
    /// A quarter-circle lead-in arriving tangentially at the seam and a
    /// lead-out leaving it the same way, both on the waste side. `None` when
    /// either arc would come nearer the wall than the path itself does — a
    /// lead-in is a convenience, never a reason to cut the part.
    fn lead_arcs(
        &self,
        lp: &Loop,
        wall: &Wall,
        at_s: f64,
        radius: f64,
    ) -> Option<(Vec<Point2D>, Vec<Point2D>)> {
        if radius <= 1e-6 {
            return None;
        }
        let p0 = lp.at(at_s);
        let t = lp.tangent(at_s);
        let clearance0 = wall.clearance(p0);
        // The waste is whichever side of the path leaves the wall behind.
        let probe = 1e-3;
        let left = (-t.1, t.0);
        let right = (t.1, -t.0);
        let toward =
            |n: (f64, f64)| wall.clearance(Point2D::new(p0.x + n.0 * probe, p0.y + n.1 * probe));
        let n_w = if toward(left) >= toward(right) {
            left
        } else {
            right
        };
        let centre = Point2D::new(p0.x + n_w.0 * radius, p0.y + n_w.1 * radius);
        let u_end = (-n_w.0, -n_w.1);
        // Which way round the arc must run to arrive heading along the path.
        let v_ccw = (-u_end.1, u_end.0);
        let sigma = if v_ccw.0 * t.0 + v_ccw.1 * t.1 > 0.0 {
            1.0
        } else {
            -1.0
        };
        let a_end = u_end.1.atan2(u_end.0);
        let steps = 12;
        let quarter = std::f64::consts::FRAC_PI_2;
        let point_at_angle =
            |a: f64| Point2D::new(centre.x + radius * a.cos(), centre.y + radius * a.sin());
        let arc_in: Vec<Point2D> = (0..=steps)
            .map(|i| {
                let f = i as f64 / steps as f64;
                point_at_angle(a_end - sigma * quarter * (1.0 - f))
            })
            .collect();
        let arc_out: Vec<Point2D> = (1..=steps)
            .map(|i| {
                let f = i as f64 / steps as f64;
                point_at_angle(a_end + sigma * quarter * f)
            })
            .collect();
        // The gate is the path's own clearance, not the tool radius: the
        // offsetter's own arc tolerance puts the path a whisker inside the
        // nominal radius, and an absolute gate would refuse every lead. What
        // matters is that the arc never comes nearer the wall than the cut it
        // is leading into.
        for p in arc_in.iter().chain(arc_out.iter()) {
            if wall.clearance(*p) < clearance0 - 1e-6 {
                return None;
            }
        }
        Some((arc_in, arc_out))
    }
}

/// The part's wall, and how much room a point has from it.
struct Wall {
    poly: geo::Polygon<f64>,
    inside: bool,
}

impl Wall {
    fn new(poly: geo::Polygon<f64>, inside: bool) -> Self {
        Self { poly, inside }
    }

    /// Distance from the contour on the side the cutter is allowed to be, in
    /// mm. Negative means the tool centre has crossed to the part's side.
    fn clearance(&self, p: Point2D) -> f64 {
        let pt = geo::Point::new(p.x, p.y);
        let d = pt.euclidean_distance(self.poly.exterior());
        let signed = if self.poly.contains(&pt) { d } else { -d };
        if self.inside {
            signed
        } else {
            -signed
        }
    }
}

/// A closed tool-centre path, measured once so a pass can be walked from any
/// point on it.
struct Loop {
    points: Vec<Point2D>,
    /// Path length from `points[0]` to each vertex; the last entry is the
    /// whole loop.
    cum: Vec<f64>,
}

impl Loop {
    fn new(points: Vec<Point2D>) -> Self {
        let mut cum = Vec::with_capacity(points.len() + 1);
        let mut s = 0.0;
        cum.push(0.0);
        for k in 0..points.len() {
            s += points[k].distance_to(&points[(k + 1) % points.len()]);
            cum.push(s);
        }
        Self { points, cum }
    }

    fn total(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// The point at path length `s` (any sign; it wraps).
    fn at(&self, s: f64) -> Point2D {
        let total = self.total();
        if total <= 0.0 {
            return self.points[0];
        }
        let s = s.rem_euclid(total);
        let k = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(k) => k.min(self.points.len() - 1),
            Err(k) => k.saturating_sub(1).min(self.points.len() - 1),
        };
        let a = self.points[k];
        let b = self.points[(k + 1) % self.points.len()];
        let len = self.cum[k + 1] - self.cum[k];
        if len <= 0.0 {
            return a;
        }
        let t = (s - self.cum[k]) / len;
        Point2D::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
    }

    /// Unit direction of travel at path length `s`.
    fn tangent(&self, s: f64) -> (f64, f64) {
        let step = (self.total() * 1e-4).clamp(1e-6, 0.05);
        let a = self.at(s - step);
        let b = self.at(s + step);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let n = dx.hypot(dy);
        if n <= 0.0 {
            (1.0, 0.0)
        } else {
            (dx / n, dy / n)
        }
    }

    /// Vertex path lengths strictly between `a` and `b` (`a < b`), wrapping as
    /// many times as the span needs.
    fn vertices_between(&self, a: f64, b: f64) -> Vec<f64> {
        let total = self.total();
        let mut out = Vec::new();
        if total <= 0.0 || b <= a {
            return out;
        }
        let first = (a / total).floor() as i64;
        let last = (b / total).floor() as i64;
        for m in first..=last {
            let base = m as f64 * total;
            for c in &self.cum[..self.cum.len() - 1] {
                let s = c + base;
                if s > a + 1e-12 && s < b - 1e-12 {
                    out.push(s);
                }
            }
        }
        out.sort_by(f64::total_cmp);
        out
    }
}

/// Height the cutter rides at `s`: the cut depth, or the top of any tab there.
fn height_at(raised: &[(f64, f64, f64)], cut_z: f64, s: f64) -> f64 {
    raised
        .iter()
        .filter(|(from, to, _)| s >= *from && s <= *to)
        .map(|(_, _, top)| *top)
        .fold(cut_z, f64::max)
}

/// First point of the loop, from the seam onwards, that no tab covers.
fn first_free(raised: &[(f64, f64, f64)], total: f64) -> Option<f64> {
    let mut s = 0.0;
    for _ in 0..raised.len() + 1 {
        match raised
            .iter()
            .filter(|(from, to, _)| s >= *from - 1e-9 && s <= *to + 1e-9)
            .map(|(_, to, _)| *to)
            .fold(None::<f64>, |acc, to| {
                Some(acc.map_or(to, |a: f64| a.max(to)))
            }) {
            None => return Some(s),
            Some(to) => s = to + 1e-6,
        }
        if s >= total {
            return None;
        }
    }
    None
}

/// Distance from `start` forward along the loop to the next tab.
fn free_window(raised: &[(f64, f64, f64)], total: f64, start: f64) -> f64 {
    raised
        .iter()
        .map(|(from, _, _)| (from - start).rem_euclid(total))
        .filter(|d| *d > 1e-9)
        .fold(total, f64::min)
}

/// Equal Z steps down to `depth`, deepest last, as negative Z values.
fn levels(depth: f64, count: usize) -> Vec<f64> {
    let n = count.max(1);
    (1..=n).map(|k| -depth * k as f64 / n as f64).collect()
}

/// Twice the signed area of the closed loop: positive when it runs
/// counter-clockwise.
fn signed_area(points: &[Point2D]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|k| {
            let (a, b) = (points[k], points[(k + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

/// Drop a repeated closing point, so winding and reversal are unambiguous.
fn dedup_closing(mut points: Vec<Point2D>) -> Vec<Point2D> {
    while points.len() > 1 {
        let first = points[0];
        let last = points[points.len() - 1];
        if first.distance_to(&last) < 1e-9 {
            points.pop();
        } else {
            break;
        }
    }
    points
}

/// Where a lead arc can meet the path: the first point, from the seam
/// onwards, with a straight run of `reach` either side of it and no tab over
/// it. An arc tangent to a corner would swing into the wall the corner turns
/// around, so the lead starts the lap somewhere else instead.
fn straight_point(lp: &Loop, raised: &[(f64, f64, f64)], reach: f64) -> Option<f64> {
    let total = lp.total();
    let step = (total / 256.0).max(0.25);
    let mut s = 0.0;
    while s < total {
        let covered = raised
            .iter()
            .any(|(from, to, _)| s >= from - reach && s <= to + reach);
        if !covered {
            let a = lp.at(s - reach);
            let b = lp.at(s + reach);
            if a.distance_to(&b) >= 2.0 * reach * 0.999 {
                return Some(s);
            }
        }
        s += step;
    }
    None
}

/// Reverse the direction of travel while keeping the same seam, so tab
/// positions and the plunge point do not move when the direction changes.
fn reverse_keeping_seam(points: &[Point2D]) -> Vec<Point2D> {
    let mut out = Vec::with_capacity(points.len());
    out.push(points[0]);
    out.extend(points[1..].iter().rev().copied());
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn offset_polygons(polygon: &geo::Polygon<f64>, offset: f64) -> Vec<geo::Polygon<f64>> {
    polygon
        .offset(
            offset, // geo-clipper applies the coordinate scale internally.
            geo_clipper::JoinType::Round(10.0),
            geo_clipper::EndType::ClosedPolygon,
            1000.0,
        )
        .0
}

/// The contour's boundary as segments, for distance and escape queries.
struct Edges {
    segs: Vec<(Point2D, Point2D)>,
}

impl Edges {
    fn new(polygon: &geo::Polygon<f64>) -> Self {
        let ring: Vec<Point2D> = polygon
            .exterior()
            .0
            .iter()
            .map(|c| Point2D::new(c.x, c.y))
            .collect();
        let ring = dedup_closing(ring);
        let segs = (0..ring.len())
            .map(|k| (ring[k], ring[(k + 1) % ring.len()]))
            .filter(|(a, b)| a.distance_to(b) > 1e-12)
            .collect();
        Self { segs }
    }

    /// Distance to the wall, and the direction that gets away from it fastest:
    /// the sum of the unit vectors away from every feature that is (nearly)
    /// the nearest one. Between two walls of a slot those cancel — that is the
    /// centre line, and there is nowhere further to go.
    fn escape(&self, p: Point2D) -> (f64, (f64, f64)) {
        let mut best = f64::INFINITY;
        let mut near: Vec<(f64, f64)> = Vec::new();
        for (a, b) in &self.segs {
            let q = nearest_on_segment(p, *a, *b);
            let d = p.distance_to(&q);
            if d < best - 1e-9 {
                best = d;
                near.clear();
            }
            if d <= best + 1e-9 && d > 1e-12 {
                near.push(((p.x - q.x) / d, (p.y - q.y) / d));
            }
        }
        // Features within a whisker of the nearest one steer as well, so a
        // corner sends the point out along the bisector and not into a wall.
        let tol = (best * 0.05).max(1e-6);
        let mut sum = (0.0, 0.0);
        for (a, b) in &self.segs {
            let q = nearest_on_segment(p, *a, *b);
            let d = p.distance_to(&q);
            if d <= best + tol && d > 1e-12 {
                sum = (sum.0 + (p.x - q.x) / d, sum.1 + (p.y - q.y) / d);
            }
        }
        let n = sum.0.hypot(sum.1);
        if n > 1e-9 {
            sum = (sum.0 / n, sum.1 / n);
        } else {
            sum = (0.0, 0.0);
        }
        (best, sum)
    }
}

fn nearest_on_segment(p: Point2D, a: Point2D, b: Point2D) -> Point2D {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 {
        return a;
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    Point2D::new(a.x + dx * t, a.y + dy * t)
}

impl Contour2D {
    /// The path to take when the cutter is about as wide as the opening: as
    /// far from the wall as the region allows, which is the true offset where
    /// there is room and the centre line where there is not.
    ///
    /// The construction is a seed loop from the offsetter, at the largest
    /// distance that still gives one piece, pushed outwards point by point
    /// along the gradient of the distance to the wall until it reaches the
    /// wanted clearance or runs out of room. What the construction claims is
    /// then *measured* against the contour, densely, and the job is refused if
    /// the cutter would take more than `tolerance` off the wall anywhere.
    #[cfg(not(target_arch = "wasm32"))]
    fn centre_line(
        &self,
        distance: f64,
        tool_radius: f64,
        tolerance: f64,
        wall: &Wall,
        phase: ContourPhase,
    ) -> Result<(Vec<Point2D>, Vec<CentreLineStretch>, f64), CamError> {
        if tolerance < 0.0 {
            return Err(CamError::InvalidCentreLineTolerance(tolerance));
        }
        let polygon = self.contour.to_geo_polygon();
        let sign = if self.inside { -1.0 } else { 1.0 };
        let edges = Edges::new(&polygon);

        // Largest single-piece offset: the seed. Anything more splits, which is
        // the very case being handled here.
        let mut lo = 0.0;
        let mut hi = distance;
        let mut seed = None;
        for _ in 0..16 {
            let mid = 0.5 * (lo + hi);
            let pieces = offset_polygons(&polygon, sign * mid);
            if pieces.len() == 1 {
                lo = mid;
                seed = pieces.into_iter().next();
            } else {
                hi = mid;
            }
        }
        let Some(seed) = seed else {
            return Err(CamError::ContourSplit(2));
        };

        let ring: Vec<Point2D> = dedup_closing(
            seed.exterior()
                .0
                .iter()
                .map(|c| Point2D::new(c.x, c.y))
                .collect(),
        );
        if ring.len() < 3 {
            return Err(CamError::EmptyContour);
        }

        // Densify before marching: the march moves points, it does not add
        // them, and a long edge would otherwise cut a corner on the way out.
        let step = (distance / 4.0).clamp(0.05, 0.5);
        let mut dense: Vec<Point2D> = Vec::new();
        for k in 0..ring.len() {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            let len = a.distance_to(&b);
            let n = (len / step).ceil().max(1.0) as usize;
            for i in 0..n {
                let t = i as f64 / n as f64;
                dense.push(Point2D::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
            }
        }

        let marched: Vec<Point2D> = dense
            .into_iter()
            .map(|p| march_to_clearance(&edges, p, distance))
            .collect();

        // Measure what was actually built, at the midpoints too: a chord
        // between two well-placed points can still cut the wall.
        let mut worst: f64 = 0.0;
        let mut samples: Vec<(f64, f64)> = Vec::new(); // (path length, overcut)
        let mut s = 0.0;
        for k in 0..marched.len() {
            let (a, b) = (marched[k], marched[(k + 1) % marched.len()]);
            let len = a.distance_to(&b);
            let n = ((len / 0.2).ceil() as usize).max(1);
            for i in 0..n {
                let t = i as f64 / n as f64;
                let q = Point2D::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                let over = (tool_radius - wall.clearance(q)).max(0.0);
                worst = worst.max(over);
                samples.push((s + len * t, over));
            }
            s += len;
        }
        if worst > tolerance + 1e-9 {
            return Err(CamError::CentreLineOvercut {
                overcut: worst,
                tolerance,
            });
        }

        // Report the stretches, not just the worst number: a machinist needs
        // to know which teeth are undersized, not only by how much.
        let mut stretches: Vec<CentreLineStretch> = Vec::new();
        let mut open: Option<(f64, f64, f64)> = None;
        for (s, over) in samples {
            match (&mut open, over > 1e-6) {
                (None, true) => open = Some((s, s, over)),
                (Some(run), true) => {
                    run.1 = s;
                    run.2 = run.2.max(over);
                }
                (Some(run), false) => {
                    stretches.push(CentreLineStretch {
                        phase,
                        from: run.0,
                        to: run.1,
                        max_wall_error: run.2,
                    });
                    open = None;
                }
                (None, false) => {}
            }
        }
        if let Some(run) = open {
            stretches.push(CentreLineStretch {
                phase,
                from: run.0,
                to: run.1,
                max_wall_error: run.2,
            });
        }

        Ok((marched, stretches, worst))
    }

    /// The centre-line fallback needs a real polygon offsetter, which this
    /// build does not have.
    #[cfg(target_arch = "wasm32")]
    fn centre_line(
        &self,
        _distance: f64,
        _tool_radius: f64,
        _tolerance: f64,
        _wall: &Wall,
        _phase: ContourPhase,
    ) -> Result<(Vec<Point2D>, Vec<CentreLineStretch>, f64), CamError> {
        Err(CamError::CentreLineUnavailable)
    }
}

/// Walk a point away from the wall until it has `target` mm of room or the
/// region runs out (the centre line), whichever comes first.
#[cfg(not(target_arch = "wasm32"))]
fn march_to_clearance(edges: &Edges, mut p: Point2D, target: f64) -> Point2D {
    for _ in 0..32 {
        let (clearance, dir) = edges.escape(p);
        if clearance >= target - 1e-7 {
            break;
        }
        if dir.0.hypot(dir.1) < 0.3 {
            break; // Equidistant from both walls: this is the centre line.
        }
        let mut step = target - clearance;
        let mut moved = false;
        for _ in 0..4 {
            let q = Point2D::new(p.x + dir.0 * step, p.y + dir.1 * step);
            if edges.escape(q).0 > clearance + 1e-9 {
                p = q;
                moved = true;
                break;
            }
            step *= 0.5;
        }
        if !moved {
            break;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mill(diameter: f64) -> Tool {
        Tool::FlatEndMill {
            diameter,
            flute_length: 25.0,
            flutes: 2,
        }
    }

    fn polyline(points: &[(f64, f64)]) -> Contour {
        let mut c = Contour::new(Point2D::new(points[0].0, points[0].1));
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            c.line_to(Point2D::new(p.0, p.1));
        }
        c
    }

    /// One motion of the toolpath, with the comment that introduced it: the
    /// comments say which part of a pass a move belongs to (`ramp entry`,
    /// `lead-in arc`, `profile`), so a test can measure each on its own.
    #[derive(Debug, Clone)]
    struct Move {
        tag: String,
        from: [f64; 3],
        to: [f64; 3],
        rapid: bool,
        feed: f64,
    }

    fn moves(toolpath: &Toolpath) -> Vec<Move> {
        let mut out = Vec::new();
        let mut tag = String::new();
        let mut at = [0.0, 0.0, 0.0];
        for seg in &toolpath.segments {
            match seg {
                ToolpathSegment::Comment { text } => tag = text.clone(),
                _ => {
                    if let Some(to) = seg.target() {
                        let feed = match seg {
                            ToolpathSegment::Linear { feed, .. } => *feed,
                            _ => 0.0,
                        };
                        out.push(Move {
                            tag: tag.clone(),
                            from: at,
                            to,
                            rapid: seg.is_rapid(),
                            feed,
                        });
                        at = to;
                    }
                }
            }
        }
        out
    }

    /// Consecutive cutting moves sharing a tag: one ramp, one lead arc, or one
    /// lap of the profile.
    fn runs(toolpath: &Toolpath, tag: &str) -> Vec<Vec<Move>> {
        let mut out: Vec<Vec<Move>> = Vec::new();
        let mut open = false;
        for m in moves(toolpath) {
            if !m.rapid && m.tag.starts_with(tag) {
                if !open {
                    out.push(Vec::new());
                }
                out.last_mut().unwrap().push(m);
                open = true;
            } else {
                open = false;
            }
        }
        out
    }

    /// Distance from a rectangle for a point outside it (zero within).
    fn outside_dist(p: [f64; 3], x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
        let dx = (x0 - p[0]).max(p[0] - x1).max(0.0);
        let dy = (y0 - p[1]).max(p[1] - y1).max(0.0);
        dx.hypot(dy)
    }

    /// Signed clearance inside a rectangle: positive within, negative outside.
    fn within(p: [f64; 3], x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
        (p[0] - x0).min(x1 - p[0]).min(p[1] - y0).min(y1 - p[1])
    }

    fn run_signed_area(run: &[Move]) -> f64 {
        let pts: Vec<[f64; 3]> = std::iter::once(run[0].from)
            .chain(run.iter().map(|m| m.to))
            .collect();
        (0..pts.len())
            .map(|k| {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            / 2.0
    }

    #[test]
    fn test_contour2d_basic() {
        let contour = Contour::rectangle(0.0, 0.0, 30.0, 20.0);
        let op = Contour2D::new(contour, 5.0);
        let toolpath = op.generate(&mill(6.0), &CamSettings::default()).unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_contour2d_with_tabs() {
        let contour = Contour::rectangle(0.0, 0.0, 50.0, 40.0);
        let op = Contour2D::new(contour, 10.0).with_tabs(4, 5.0, 2.0);
        assert_eq!(op.tabs.len(), 4);
        let settings = CamSettings {
            stepdown: 5.0,
            ..CamSettings::default()
        };
        let toolpath = op.generate(&mill(6.0), &settings).unwrap();
        assert!(!toolpath.is_empty());

        let mut unique_z: Vec<f64> = toolpath
            .segments
            .iter()
            .filter_map(|s| s.target())
            .map(|[_, _, z]| z)
            .collect();
        unique_z.sort_by(|a, b| a.total_cmp(b));
        unique_z.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        assert!(unique_z.len() >= 2);
    }

    #[test]
    fn test_contour2d_circle() {
        let contour = Contour::circle(25.0, 25.0, 15.0);
        let op = Contour2D::new(contour, 6.0);
        let toolpath = op
            .generate(&Tool::default_endmill(), &CamSettings::default())
            .unwrap();
        assert!(!toolpath.is_empty());
    }

    /// The side is the whole point of a contour cut: outside keeps the cutter
    /// off the part, inside keeps it within the opening. Either winding.
    #[test]
    fn test_contour2d_cuts_on_the_named_side() {
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
                let toolpath = op.generate(&mill(6.0), &settings).unwrap();
                let cuts: Vec<[f64; 3]> = toolpath
                    .segments
                    .iter()
                    .filter(|s| s.is_cutting())
                    .filter_map(|s| s.target())
                    .filter(|t| t[2] < 0.0)
                    .collect();
                assert!(cuts.len() >= 4);
                for t in cuts {
                    if inside {
                        assert!(
                            within(t, 0.0, 0.0, 30.0, 20.0) >= 3.0 - 0.02,
                            "inside cut at {t:?} gouges the wall"
                        );
                    } else {
                        assert!(
                            within(t, 0.0, 0.0, 30.0, 20.0) <= 0.0,
                            "outside cut at {t:?} is inside the part"
                        );
                        assert!(
                            outside_dist(t, 0.0, 0.0, 30.0, 20.0) >= 3.0 - 0.02,
                            "outside cut at {t:?} gouges the part"
                        );
                    }
                }
            }
        }
    }

    /// Two 20 mm squares joined by a 4 mm wide neck.
    fn dumbbell() -> Contour {
        polyline(&[
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
        ])
    }

    /// A cutter wider than a neck of the opening cannot follow it in one
    /// loop; that is an error, not a silently shorter path.
    #[test]
    fn test_contour2d_inside_refuses_a_neck_the_cutter_cannot_pass() {
        let result =
            Contour2D::inside(dumbbell(), 2.0).generate(&mill(6.0), &CamSettings::default());
        assert!(
            matches!(result, Err(CamError::ContourSplit(2))),
            "{result:?}"
        );
    }

    /// Metal left under each tab, measured on the toolpath: for every profile
    /// lap that goes below the tab top, the length the cutter spends lifted,
    /// less the tool diameter it cuts into the two ends.
    fn tab_metal_per_pass(toolpath: &Toolpath, tab_top: f64, tool_diameter: f64) -> Vec<Vec<f64>> {
        let mut passes: Vec<Vec<f64>> = Vec::new();
        for lap in runs(toolpath, "profile") {
            let mut widths = Vec::new();
            let mut run: Option<f64> = None;
            let mut below = false;
            for m in lap {
                let lifted = (m.from[2] - tab_top).abs() < 1e-9 && (m.to[2] - tab_top).abs() < 1e-9;
                below |= m.to[2] < tab_top - 1e-9;
                let xy = (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]);
                // Within a lap a change of height happens in place, never
                // along a ramp: a tab entered on a slope is a tab eaten away.
                assert!(
                    (m.to[2] - m.from[2]).abs() < 1e-9 || xy < 1e-9,
                    "profile ramps at {:?}",
                    m.to
                );
                if lifted {
                    *run.get_or_insert(0.0) += xy;
                } else if let Some(r) = run.take() {
                    widths.push(r - tool_diameter);
                }
            }
            if let Some(r) = run.take() {
                widths.push(r - tool_diameter);
            }
            if below && !widths.is_empty() {
                passes.push(widths);
            }
        }
        passes
    }

    #[test]
    fn test_contour2d_tabs_survive_every_pass_at_their_stated_width() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        // Depth 4 in 1 mm passes, tabs 1.5 tall: the passes at -3 and -4 both
        // reach below the tab top at -2.5.
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 50.0, 40.0), 4.0)
            .with_tabs(3, 5.0, 1.5);
        let toolpath = op.generate(&mill(6.0), &settings).unwrap();
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
        let settings = CamSettings {
            stepdown: 4.0,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 50.0, 40.0), 4.0)
            .with_tab(Tab::new(0.0, 5.0, 1.5));
        let toolpath = op.generate(&mill(6.0), &settings).unwrap();
        // The two halves of the tab add up to the tab plus one tool diameter.
        let lifted: f64 = runs(&toolpath, "profile")
            .iter()
            .flatten()
            .filter(|m| (m.from[2] + 2.5).abs() < 1e-9 && (m.to[2] + 2.5).abs() < 1e-9)
            .map(|m| (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]))
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
        let settings = CamSettings {
            stepdown: 4.0,
            ..CamSettings::default()
        };
        for count in 1..=7 {
            let op = Contour2D::outside(polyline(&notched), 4.0).with_tabs(count, 5.0, 1.5);
            let toolpath = op.generate(&mill(6.0), &settings).unwrap();
            let mut tabs: Vec<Vec<[f64; 3]>> = Vec::new();
            let mut lifted = false;
            for m in runs(&toolpath, "profile").into_iter().flatten() {
                let now = (m.to[2] + 2.5).abs() < 1e-9 && (m.from[2] + 2.5).abs() < 1e-9;
                if now && !lifted {
                    tabs.push(vec![m.from]);
                }
                if now {
                    tabs.last_mut().unwrap().push(m.to);
                }
                lifted = now;
            }
            assert_eq!(tabs.len(), count);
            for run in tabs {
                let (a, b) = (run[0], run[run.len() - 1]);
                let chord = (b[0] - a[0]).hypot(b[1] - a[1]);
                assert!(
                    chord >= 0.98 * 11.0,
                    "{count} tabs: one bends (chord {chord:.2} of 11) near {a:?}"
                );
            }
        }
    }

    /// Roughing has to stay off the wall by exactly the stock it was told to
    /// leave, and the finish pass has to be on the wall — otherwise "stock to
    /// leave" is a number that changes nothing.
    #[test]
    fn test_roughing_leaves_stock_and_the_finish_pass_cuts_the_wall() {
        let settings = CamSettings {
            stepdown: 1.5,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 30.0, 20.0), 4.0)
            .with_stock_to_leave(0.4);
        let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
        assert_eq!(report.rough_passes, 3, "{report:?}");
        assert_eq!(report.finish_passes, 1, "{report:?}");

        let laps = runs(&toolpath, "profile");
        assert_eq!(laps.len(), 4);
        for (i, lap) in laps.iter().enumerate() {
            let want = if i < 3 { 3.4 } else { 3.0 };
            for m in lap {
                let d = outside_dist(m.to, 0.0, 0.0, 30.0, 20.0);
                assert!(
                    (d - want).abs() < 0.02,
                    "lap {i} runs {d:.3} mm off the part, wanted {want}"
                );
            }
        }
        // Both phases go all the way down; the roughing steps there.
        let deepest = laps
            .iter()
            .flatten()
            .map(|m| m.to[2])
            .fold(f64::INFINITY, f64::min);
        assert!((deepest + 4.0).abs() < 1e-12, "deepest {deepest}");
        let rough_depths: Vec<f64> = laps[..3]
            .iter()
            .map(|lap| lap.iter().map(|m| m.to[2]).fold(f64::INFINITY, f64::min))
            .collect();
        for (k, z) in rough_depths.iter().enumerate() {
            let want = -4.0 * (k + 1) as f64 / 3.0;
            assert!(
                (z - want).abs() < 1e-9,
                "rough pass {k} at {z}, wanted {want}"
            );
        }
    }

    /// The finish phase can be taken in steps, repeated, and run at its own
    /// feed. Each of those has to show up in the path, not just in the struct.
    #[test]
    fn test_finish_stepdowns_spring_pass_and_finish_feed() {
        let settings = CamSettings {
            stepdown: 2.0,
            feed_rate: 1000.0,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 40.0, 30.0), 4.0)
            .with_stock_to_leave(0.3)
            .with_finish_stepdowns(2)
            .with_spring_pass(true)
            .with_finish_feed(400.0);
        let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
        assert_eq!((report.rough_passes, report.finish_passes), (2, 2));
        assert!(report.spring_pass);

        let laps = runs(&toolpath, "profile");
        assert_eq!(laps.len(), 5, "2 rough + 2 finish + spring");
        // The roughing cleared the waste, so the finish passes lead in on an
        // arc instead of ramping all the way down again.
        assert_eq!(report.lead_entries, 3, "{report:?}");
        assert_eq!(report.ramp_entries, 2, "{report:?}");
        let depths: Vec<f64> = laps
            .iter()
            .map(|lap| lap.iter().map(|m| m.to[2]).fold(f64::INFINITY, f64::min))
            .collect();
        for (got, want) in depths.iter().zip([-2.0, -4.0, -2.0, -4.0, -4.0]) {
            assert!((got - want).abs() < 1e-12, "{depths:?}");
        }
        for (i, lap) in laps.iter().enumerate() {
            let want = if i < 2 { 1000.0 } else { 400.0 };
            for m in lap {
                assert!((m.feed - want).abs() < 1e-9, "lap {i} feeds {}", m.feed);
            }
        }
        // The spring pass is the finish pass again: same path, same depth.
        let finish: Vec<[f64; 3]> = laps[3].iter().map(|m| m.to).collect();
        let spring: Vec<[f64; 3]> = laps[4].iter().map(|m| m.to).collect();
        assert_eq!(finish.len(), spring.len());
        for (a, b) in finish.iter().zip(&spring) {
            assert!(
                (a[0] - b[0]).abs() < 1e-9
                    && (a[1] - b[1]).abs() < 1e-9
                    && (a[2] - b[2]).abs() < 1e-9,
                "spring pass wandered: {a:?} vs {b:?}"
            );
        }
    }

    /// Climb or conventional is the caller's choice, not an accident of how
    /// the contour was drawn. With an M3 spindle climb runs counter-clockwise
    /// around a hole and clockwise around a part.
    #[test]
    fn test_direction_is_chosen_not_inherited_from_the_winding() {
        let ccw = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
        let cw = [(0.0, 0.0), (0.0, 30.0), (40.0, 30.0), (40.0, 0.0)];
        for points in [ccw, cw] {
            for inside in [false, true] {
                for direction in [CutDirection::Climb, CutDirection::Conventional] {
                    let contour = polyline(&points);
                    let op = if inside {
                        Contour2D::inside(contour, 2.0)
                    } else {
                        Contour2D::outside(contour, 2.0)
                    }
                    .with_direction(direction);
                    let toolpath = op.generate(&mill(6.0), &CamSettings::default()).unwrap();
                    let lap = &runs(&toolpath, "profile")[0];
                    let area = run_signed_area(lap);
                    let ccw_travel = area > 0.0;
                    let want_ccw = (direction == CutDirection::Climb) == inside;
                    assert_eq!(
                        ccw_travel,
                        want_ccw,
                        "{direction:?} {} ran {} (signed area {area:.1})",
                        if inside { "inside" } else { "outside" },
                        if ccw_travel { "CCW" } else { "CW" }
                    );
                }
            }
        }
    }

    /// A ramp entry has to be a ramp: down at no more than the angle asked
    /// for, never climbing, and on the path the pass is going to cut — a ramp
    /// that wanders off the offset cuts somewhere nothing checked.
    #[test]
    fn test_ramp_entry_descends_at_the_configured_angle_and_stays_on_the_path() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        for angle in [1.5, 3.0, 8.0] {
            let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 40.0, 30.0), 3.0)
                .with_ramp_angle(angle);
            let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
            let ramps = runs(&toolpath, "ramp entry");
            assert_eq!(ramps.len(), 3, "one ramp per pass");
            assert_eq!(report.ramp_entries, 3);
            let limit = angle.to_radians().tan();
            for (k, ramp) in ramps.iter().enumerate() {
                let mut total_xy = 0.0;
                for m in ramp {
                    let xy = (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]);
                    let drop = m.from[2] - m.to[2];
                    assert!(drop >= -1e-9, "ramp {k} climbs by {drop} at {:?}", m.to);
                    assert!(
                        drop <= xy * limit + 1e-9,
                        "ramp {k} drops {drop:.4} over {xy:.4} mm, steeper than {angle} deg"
                    );
                    assert!(
                        (outside_dist(m.to, 0.0, 0.0, 40.0, 30.0) - 3.0).abs() < 0.02,
                        "ramp {k} leaves the offset path at {:?}",
                        m.to
                    );
                    total_xy += xy;
                }
                // It starts at the previous depth and ends at this one.
                let start_z = ramp[0].from[2];
                let end_z = ramp[ramp.len() - 1].to[2];
                assert!(
                    (start_z + k as f64).abs() < 1e-9,
                    "ramp {k} starts at {start_z}"
                );
                assert!(
                    (end_z + (k + 1) as f64).abs() < 1e-9,
                    "ramp {k} ends at {end_z}"
                );
                // Shallower angle, longer ramp: the length is the drop over
                // the tangent, to within one leg of the zig-zag.
                assert!(
                    total_xy >= 1.0 / limit - 1e-6,
                    "ramp {k} is {total_xy:.2} mm for a 1 mm drop at {angle} deg"
                );
            }
        }
    }

    /// Tabs are the only thing holding the part, and a ramp is the one move
    /// that descends while travelling: it may not descend through one.
    #[test]
    fn test_ramp_never_descends_through_a_tab() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 60.0, 40.0), 4.0)
            .with_tabs(3, 5.0, 1.5);
        let toolpath = op.generate(&mill(6.0), &settings).unwrap();
        let top = -2.5;

        // Where the tabs are: the stretches the profile rides over, taken
        // from the deepest pass and trimmed so the vertical step at each end
        // is not counted as being "on" the tab.
        let mut tabs: Vec<([f64; 2], [f64; 2])> = Vec::new();
        let mut open: Option<([f64; 2], [f64; 2])> = None;
        for m in runs(&toolpath, "profile").into_iter().flatten() {
            let lifted = (m.from[2] - top).abs() < 1e-9 && (m.to[2] - top).abs() < 1e-9;
            match (&mut open, lifted) {
                (None, true) => open = Some(([m.from[0], m.from[1]], [m.to[0], m.to[1]])),
                (Some(run), true) => run.1 = [m.to[0], m.to[1]],
                (Some(run), false) => {
                    tabs.push(*run);
                    open = None;
                }
                (None, false) => {}
            }
        }
        if let Some(run) = open {
            tabs.push(run);
        }
        assert!(
            tabs.len() >= 3,
            "expected the three tabs, got {}",
            tabs.len()
        );

        for m in runs(&toolpath, "ramp entry").into_iter().flatten() {
            for (a, b) in &tabs {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len2 = dx * dx + dy * dy;
                if len2 < 1e-12 {
                    continue;
                }
                // Sample the ramp move; anything landing on the tab stretch,
                // clear of its two ends, must still be at or above the top.
                for i in 0..=20 {
                    let f = i as f64 / 20.0;
                    let p = [
                        m.from[0] + (m.to[0] - m.from[0]) * f,
                        m.from[1] + (m.to[1] - m.from[1]) * f,
                        m.from[2] + (m.to[2] - m.from[2]) * f,
                    ];
                    let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2;
                    if !(0.05..=0.95).contains(&t) {
                        continue;
                    }
                    let off = (p[0] - (a[0] + dx * t)).hypot(p[1] - (a[1] + dy * t));
                    assert!(
                        off > 1e-6 || p[2] >= top - 1e-9,
                        "the ramp cuts the tab at {p:?} (top {top})"
                    );
                }
            }
        }
    }

    /// A lead-in exists to meet the wall tangentially. It must do that from
    /// the waste side — an arc swung the other way is a bite out of the part,
    /// and the side is not something the contour's winding gets to decide.
    #[test]
    fn test_lead_arcs_stay_on_the_waste_side() {
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        let ccw = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
        let cw = [(0.0, 0.0), (0.0, 30.0), (40.0, 30.0), (40.0, 0.0)];
        for points in [ccw, cw] {
            for inside in [false, true] {
                for direction in [CutDirection::Climb, CutDirection::Conventional] {
                    let contour = polyline(&points);
                    let op = if inside {
                        Contour2D::inside(contour, 2.0)
                    } else {
                        Contour2D::outside(contour, 2.0)
                    }
                    .with_direction(direction)
                    .with_stock_to_leave(0.4);
                    let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
                    assert_eq!(
                        report.lead_entries, 1,
                        "no lead on the finish pass ({points:?} inside={inside} {direction:?})"
                    );

                    let leads: Vec<Move> = runs(&toolpath, "lead-in arc")
                        .into_iter()
                        .chain(runs(&toolpath, "lead-out arc"))
                        .flatten()
                        .collect();
                    assert!(leads.len() >= 20, "{} lead moves", leads.len());
                    for m in &leads {
                        if inside {
                            assert!(
                                within(m.to, 0.0, 0.0, 40.0, 30.0) >= 3.0 - 0.02,
                                "lead at {:?} cuts the wall of the opening",
                                m.to
                            );
                        } else {
                            assert!(
                                within(m.to, 0.0, 0.0, 40.0, 30.0) <= 0.0
                                    && outside_dist(m.to, 0.0, 0.0, 40.0, 30.0) >= 3.0 - 0.02,
                                "lead at {:?} cuts into the part",
                                m.to
                            );
                        }
                    }
                    // The lead-in hands over to the profile where the cut
                    // starts, at the same point and the same depth.
                    let lap = &runs(&toolpath, "profile")[1];
                    let hand_over = runs(&toolpath, "lead-in arc")[0].last().unwrap().to;
                    assert!(
                        (hand_over[0] - lap[0].from[0]).abs() < 1e-9
                            && (hand_over[1] - lap[0].from[1]).abs() < 1e-9
                            && (hand_over[2] - lap[0].from[2]).abs() < 1e-9,
                        "lead-in ends at {hand_over:?}, the cut starts at {:?}",
                        lap[0].from
                    );
                }
            }
        }
    }

    /// In a slot barely wider than the cutter there is no room to swing a
    /// lead-in. The pass goes straight down instead — the one thing it may
    /// not do is swing the arc anyway.
    #[test]
    fn test_lead_in_falls_back_when_the_waste_has_no_room() {
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        // An 8 mm slot with a 6 mm cutter: the tool-centre path is a 2 mm
        // wide corridor, and a 3 mm lead arc does not fit in it.
        let op = Contour2D::inside(Contour::rectangle(0.0, 0.0, 8.0, 40.0), 2.0)
            .with_stock_to_leave(0.4);
        let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
        assert_eq!(report.lead_entries, 0, "swung a lead-in with no room");
        assert!(report.plunge_entries >= 1, "{report:?}");
        assert!(runs(&toolpath, "lead-in arc").is_empty());
        for m in runs(&toolpath, "profile").into_iter().flatten() {
            assert!(
                within(m.to, 0.0, 0.0, 8.0, 40.0) >= 3.0 - 0.02,
                "cut at {:?} gouges the slot wall",
                m.to
            );
        }
    }

    /// A slug cut free next to the cutter is as dangerous as a part cut free.
    /// Tabs have to work on an inside contour too — same width of metal, on
    /// every pass that reaches them, whichever way round the cut runs.
    #[test]
    fn test_tabs_on_an_inside_contour_leave_their_stated_width() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        let ccw = [(0.0, 0.0), (50.0, 0.0), (50.0, 40.0), (0.0, 40.0)];
        let cw = [(0.0, 0.0), (0.0, 40.0), (50.0, 40.0), (50.0, 0.0)];
        for points in [ccw, cw] {
            for direction in [CutDirection::Climb, CutDirection::Conventional] {
                let op = Contour2D::inside(polyline(&points), 4.0)
                    .with_direction(direction)
                    .with_tabs(3, 5.0, 1.5);
                let toolpath = op.generate(&mill(6.0), &settings).unwrap();
                let passes = tab_metal_per_pass(&toolpath, -2.5, 6.0);
                assert_eq!(passes.len(), 2, "{direction:?}: {passes:?}");
                for pass in &passes {
                    assert_eq!(pass.len(), 3, "{pass:?}");
                    for metal in pass {
                        assert!(
                            (metal - 5.0).abs() < 1e-6,
                            "{direction:?}: tab leaves {metal} mm, asked for 5"
                        );
                    }
                }
                for m in runs(&toolpath, "profile").into_iter().flatten() {
                    assert!(
                        within(m.to, 0.0, 0.0, 50.0, 40.0) >= 3.0 - 0.02,
                        "cut at {:?} gouges the wall of the opening",
                        m.to
                    );
                }
            }
        }
    }

    /// A skin left on the bottom is the whole of what keeps the part in the
    /// stock; it has to be exactly as thick as asked. Tab tops are measured
    /// from the underside of the stock, not from the shortened cut.
    #[test]
    fn test_onion_skin_stops_exactly_above_the_bottom() {
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        let op = Contour2D::outside(Contour::rectangle(0.0, 0.0, 50.0, 40.0), 6.0)
            .with_bottom_allowance(0.15)
            .with_tabs(3, 5.0, 1.0);
        let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
        assert!((report.final_depth - 5.85).abs() < 1e-12);

        let deepest = runs(&toolpath, "profile")
            .into_iter()
            .flatten()
            .map(|m| m.to[2])
            .fold(f64::INFINITY, f64::min);
        assert!(
            (deepest + 5.85).abs() < 1e-12,
            "cut to {deepest}, wanted -5.85"
        );

        // The tab top is 1 mm off the underside of the stock at -6, so -5.0 —
        // not 1 mm off the shortened depth, which would be -4.85.
        let passes = tab_metal_per_pass(&toolpath, -5.0, 6.0);
        assert_eq!(passes.len(), 1, "{passes:?}");
        assert_eq!(passes[0].len(), 3);
        for metal in &passes[0] {
            assert!((metal - 5.0).abs() < 1e-6, "tab leaves {metal} mm");
        }
        assert!(
            !toolpath
                .segments
                .iter()
                .filter_map(|s| s.target())
                .any(|t| (t[2] + 4.85).abs() < 1e-9),
            "a tab was measured from the shortened depth"
        );
    }

    /// Cutting past the bottom of the stock is only safe if something under it
    /// can take the cut. Without that declared, refuse — quietly cutting the
    /// machine bed is not an option, and neither is quietly stopping short.
    #[test]
    fn test_break_through_needs_a_declared_spoilboard() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        let base = || Contour2D::outside(Contour::rectangle(0.0, 0.0, 30.0, 20.0), 1.0);

        let bare = base()
            .with_bottom_allowance(-0.3)
            .generate(&mill(6.0), &settings);
        assert!(
            matches!(
                bare,
                Err(CamError::BreakThroughWithoutSpoilboard { overcut, spoilboard })
                    if (overcut - 0.3).abs() < 1e-9 && spoilboard == 0.0
            ),
            "{bare:?}"
        );

        let thin = base()
            .with_bottom_allowance(-0.3)
            .with_spoilboard(0.2)
            .generate(&mill(6.0), &settings);
        assert!(
            matches!(thin, Err(CamError::BreakThroughWithoutSpoilboard { .. })),
            "{thin:?}"
        );

        let (toolpath, report) = base()
            .with_bottom_allowance(-0.3)
            .with_spoilboard(3.0)
            .generate_reported(&mill(6.0), &settings)
            .unwrap();
        assert!((report.final_depth - 1.3).abs() < 1e-12);
        let deepest = runs(&toolpath, "profile")
            .into_iter()
            .flatten()
            .map(|m| m.to[2])
            .fold(f64::INFINITY, f64::min);
        assert!(
            (deepest + 1.3).abs() < 1e-12,
            "cut to {deepest}, wanted -1.3"
        );

        let nothing = base()
            .with_bottom_allowance(1.0)
            .generate(&mill(6.0), &settings);
        assert!(
            matches!(nothing, Err(CamError::BottomAllowanceExceedsDepth { .. })),
            "{nothing:?}"
        );
    }

    /// Deepest the cutter reaches past the contour anywhere on the path,
    /// measured off the toolpath itself rather than believed from the report.
    fn worst_overcut(op: &Contour2D, toolpath: &Toolpath, tool_radius: f64) -> f64 {
        let wall = Wall::new(op.contour.to_geo_polygon(), op.inside);
        let mut worst: f64 = 0.0;
        for m in runs(toolpath, "profile").into_iter().flatten() {
            for i in 0..=8 {
                let f = i as f64 / 8.0;
                let p = Point2D::new(
                    m.from[0] + (m.to[0] - m.from[0]) * f,
                    m.from[1] + (m.to[1] - m.from[1]) * f,
                );
                worst = worst.max(tool_radius - wall.clearance(p));
            }
        }
        worst
    }

    /// The centre-line fallback is opt-in, and opting in does not mean
    /// accepting anything: the cut it builds is measured against the contour
    /// and refused if it takes more off the wall than the caller allowed.
    #[test]
    fn test_centre_line_fallback_is_opt_in_and_bounded() {
        let settings = CamSettings {
            stepdown: 2.0,
            ..CamSettings::default()
        };
        // The neck is 4 mm and the cutter 6: on the centre line it cuts
        // exactly 1 mm into each wall of the neck.
        let refused = Contour2D::inside(dumbbell(), 2.0)
            .with_centre_line_fallback(0.05)
            .generate(&mill(6.0), &settings);
        assert!(
            matches!(
                refused,
                Err(CamError::CentreLineOvercut { overcut, tolerance })
                    if (overcut - 1.0).abs() < 0.05 && tolerance == 0.05
            ),
            "{refused:?}"
        );

        let op = Contour2D::inside(dumbbell(), 2.0).with_centre_line_fallback(1.1);
        let (toolpath, report) = op.generate_reported(&mill(6.0), &settings).unwrap();
        assert!(
            (report.max_wall_error - 1.0).abs() < 0.05,
            "reported {:.3} mm of wall error",
            report.max_wall_error
        );
        assert!(!report.centre_line.is_empty(), "no stretch reported");
        for stretch in &report.centre_line {
            assert!(stretch.to > stretch.from);
            assert!(stretch.max_wall_error <= report.max_wall_error + 1e-9);
            assert_eq!(stretch.phase, ContourPhase::Finish);
        }
        // What the path actually does, measured: no worse than reported, and
        // the wide lobes still get cut to the full offset.
        let measured = worst_overcut(&op, &toolpath, 3.0);
        assert!(
            measured <= report.max_wall_error + 1e-6,
            "path cuts {measured:.3} mm past the wall, report says {:.3}",
            report.max_wall_error
        );
        let wall = Wall::new(op.contour.to_geo_polygon(), true);
        let on_the_offset = runs(&toolpath, "profile")
            .into_iter()
            .flatten()
            .filter(|m| (wall.clearance(Point2D::new(m.to[0], m.to[1])) - 3.0).abs() < 0.02)
            .count();
        assert!(
            on_the_offset > 20,
            "only {on_the_offset} moves reach the true offset in the lobes"
        );
        assert!(
            toolpath.segments.iter().any(|s| matches!(
                s,
                ToolpathSegment::Comment { text } if text.starts_with("centre-line cut")
            )),
            "the G-code does not say which stretches were centre-line cut"
        );
    }

    /// The gear case: a root space narrower than the cutter. The offset does
    /// not split, it vanishes — and the wall error is the half width the
    /// cutter cannot fit into, which is arithmetic a test can check exactly.
    #[test]
    fn test_centre_line_in_a_slot_narrower_than_the_cutter() {
        let settings = CamSettings {
            stepdown: 1.0,
            ..CamSettings::default()
        };
        // 0.44 mm wide, as narrow as the roots of gears-60-cnc.json, cut with
        // the same 1 mm end mill: 0.5 - 0.22 = 0.28 mm into each flank.
        let slot = || Contour::rectangle(0.0, 0.0, 0.44, 4.0);

        let refused = Contour2D::inside(slot(), 1.0).generate(&mill(1.0), &settings);
        assert!(
            matches!(refused, Err(CamError::EmptyContour)),
            "a slot the cutter cannot enter must be refused by default: {refused:?}"
        );

        let tight = Contour2D::inside(slot(), 1.0)
            .with_centre_line_fallback(0.05)
            .generate(&mill(1.0), &settings);
        assert!(
            matches!(
                tight,
                Err(CamError::CentreLineOvercut { overcut, .. }) if (overcut - 0.28).abs() < 0.01
            ),
            "{tight:?}"
        );

        let op = Contour2D::inside(slot(), 1.0).with_centre_line_fallback(0.3);
        let (toolpath, report) = op.generate_reported(&mill(1.0), &settings).unwrap();
        assert!(
            (report.max_wall_error - 0.28).abs() < 0.01,
            "reported {:.3} mm, expected 0.28",
            report.max_wall_error
        );
        assert!(worst_overcut(&op, &toolpath, 0.5) <= report.max_wall_error + 1e-6);
        // It runs down the middle of the slot, not along one wall.
        for m in runs(&toolpath, "profile").into_iter().flatten() {
            assert!(
                (m.to[0] - 0.22).abs() < 0.01,
                "off the centre line at {:?}",
                m.to
            );
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
