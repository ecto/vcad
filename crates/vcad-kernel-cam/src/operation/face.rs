//! Face (surface) milling operation.
//!
//! Three things were checked here against the same list the pocket was, and
//! two of them were wrong.
//!
//! - **Bounds and overshoot.** Right, and left alone. The tool centre runs from
//!   `min − radius` to `max + radius` on both axes, so the cutter fully leaves
//!   the area at each end of a pass and the swept band covers every point of
//!   it. There is no stock outline here to honour: the rectangle the caller
//!   gives *is* the area to face, and the operation's job is to cover it.
//! - **Step-over.** Right: the passes are spaced `span / ceil(span / stepover)`
//!   apart, which is at most the stepover and so at most one diameter, and
//!   there is a pass on each edge of the span. No strip is left between them.
//!   [`tests::facing_leaves_no_strip_and_overshoots_by_one_radius`] measures it
//!   rather than restating it.
//! - **Direction.** Was not honoured at all: a zig-zag alternates climb and
//!   conventional every pass, and nothing said which the first one was or let a
//!   caller ask for one of them throughout. [`Face::direction`] and
//!   [`Face::one_way`] are that.
//!
//! Two defects went with them. A zero stepdown made `depth / stepdown` infinite
//! and `ceil() as usize` zero, so the operation returned a toolpath with no
//! cutting move in it at all — silently, `Ok`. And the closing retract rapided
//! to the corner of the area *from the bottom of the cut*, travelling in XY at
//! depth: the move [`Contour2D`](crate::Contour2D) says out loud it had to have
//! fixed out of it, and one the verification oracle flags.

use crate::{CamError, CamSettings, CutDirection, Tool, Toolpath, ToolpathSegment};
use serde::{Deserialize, Serialize};

/// Face milling operation for surface machining.
///
/// Generates a raster pattern to machine a flat surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    /// Minimum X coordinate of the area to face.
    pub min_x: f64,
    /// Minimum Y coordinate of the area to face.
    pub min_y: f64,
    /// Maximum X coordinate of the area to face.
    pub max_x: f64,
    /// Maximum Y coordinate of the area to face.
    pub max_y: f64,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
    /// Which way the cutting passes run.
    ///
    /// The passes step in +Y, so the metal still to come is on the +Y side:
    /// travelling in −X puts it on the right of the direction of travel, which
    /// with an `M3` spindle and a right-hand cutter is a climb cut, and
    /// travelling in +X is conventional. In a zig-zag this picks the first
    /// pass, since the rest alternate; with [`Face::one_way`] it picks them
    /// all.
    #[serde(default)]
    pub direction: CutDirection,
    /// Cut in both directions, stepping over at the end of each pass. The
    /// default, and the faster of the two: nothing is thrown away on the return
    /// travel. Clear it to keep every pass climbing (or every pass
    /// conventional) at the cost of a rapid back across the work each time.
    #[serde(default = "default_true")]
    pub zigzag: bool,
}

fn default_true() -> bool {
    true
}

impl Face {
    /// Create a new face operation.
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64, depth: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
            depth,
            direction: CutDirection::default(),
            zigzag: true,
        }
    }

    /// Create a face operation from width and height at origin.
    pub fn from_size(width: f64, height: f64, depth: f64) -> Self {
        Self::new(0.0, 0.0, width, height, depth)
    }

    /// Climb or conventional milling.
    pub fn with_direction(mut self, direction: CutDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Cut every pass in the same direction, rapiding back between them, so the
    /// whole operation climbs (or the whole of it is conventional).
    pub fn one_way(mut self) -> Self {
        self.zigzag = false;
        self
    }

    /// Generate the toolpath for this face operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        let width = self.max_x - self.min_x;
        let height = self.max_y - self.min_y;

        if width <= 0.0 || height <= 0.0 {
            return Err(CamError::DegenerateBounds(width, height));
        }
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepover <= 0.0 || settings.stepover > tool.diameter() {
            return Err(CamError::InvalidStepover(settings.stepover));
        }
        // Without this the pass count is `ceil(inf) as usize` — zero — and the
        // operation hands back a toolpath that cuts nothing, with no error.
        if settings.stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(settings.stepdown));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }

        let mut toolpath = Toolpath::new();
        toolpath.push(ToolpathSegment::comment(format!(
            "Face: {:.1}x{:.1}mm, depth={:.2}mm, {}, {}",
            width,
            height,
            self.depth,
            match self.direction {
                CutDirection::Climb => "climb",
                CutDirection::Conventional => "conventional",
            },
            if self.zigzag { "zig-zag" } else { "one way" }
        )));

        let tool_radius = tool.radius();
        let stepover = settings.stepover;

        // The cutter has to leave the area at each end of a pass and at each
        // edge of the raster, so the tool centre runs a radius beyond it.
        let start_x = self.min_x - tool_radius;
        let end_x = self.max_x + tool_radius;
        let start_y = self.min_y - tool_radius;
        let end_y = self.max_y + tool_radius;

        let num_z_passes = (self.depth / settings.stepdown).ceil() as usize;
        let z_step = self.depth / num_z_passes.max(1) as f64;

        // Travelling in −X leaves the metal still to come, which is on the +Y
        // side, on the right of the direction of travel: the climb cut.
        let first_runs_minus_x = self.direction == CutDirection::Climb;

        for z_pass in 0..num_z_passes {
            let z = -((z_pass + 1) as f64) * z_step;

            let y_span = end_y - start_y;
            let num_passes = (y_span / stepover).ceil() as usize;
            let actual_stepover = y_span / num_passes as f64;

            toolpath.push(ToolpathSegment::comment(format!("Z level: {z:.3}")));

            for pass in 0..=num_passes {
                let y = (start_y + pass as f64 * actual_stepover).min(end_y);

                let minus_x = if self.zigzag {
                    first_runs_minus_x == (pass % 2 == 0)
                } else {
                    first_runs_minus_x
                };
                let (x_start, x_end) = if minus_x {
                    (end_x, start_x)
                } else {
                    (start_x, end_x)
                };

                if pass == 0 || !self.zigzag {
                    // Come in from above: the first pass of the level, and
                    // every pass of a one-way raster, starts clear of the work.
                    // Straight up where the cutter stands, and only then across.
                    if let Some([cx, cy, cz]) = toolpath.segments.last().and_then(|s| s.target()) {
                        if cz < settings.safe_z {
                            toolpath.push(ToolpathSegment::rapid(cx, cy, settings.safe_z));
                        }
                    }
                    toolpath.push(ToolpathSegment::rapid(x_start, y, settings.safe_z));
                    toolpath.push(ToolpathSegment::linear(x_start, y, z, settings.plunge_rate));
                } else {
                    // Step over to the next row at depth: the cutter is already
                    // at this end of it.
                    toolpath.push(ToolpathSegment::linear(x_start, y, z, settings.feed_rate));
                }

                toolpath.push(ToolpathSegment::linear(x_end, y, z, settings.feed_rate));
            }

            // Straight up out of the cut before anything travels in XY.
            if let Some([x, y, _]) = toolpath.segments.last().and_then(|s| s.target()) {
                toolpath.push(ToolpathSegment::rapid(x, y, settings.safe_z));
            }
        }

        // And only then back over the corner the job started from.
        toolpath.push(ToolpathSegment::rapid(
            self.min_x,
            self.min_y,
            settings.safe_z,
        ));

        Ok(toolpath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mill(diameter: f64) -> Tool {
        Tool::FlatEndMill {
            diameter,
            flute_length: 20.0,
            flutes: 2,
        }
    }

    fn settings() -> CamSettings {
        CamSettings {
            stepover: 5.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        }
    }

    /// One motion of the toolpath, with where it came from.
    #[derive(Debug, Clone, Copy)]
    struct Move {
        from: [f64; 3],
        to: [f64; 3],
        rapid: bool,
    }

    fn moves(toolpath: &Toolpath) -> Vec<Move> {
        let mut out = Vec::new();
        let mut at = [0.0, 0.0, 0.0];
        for seg in &toolpath.segments {
            if let Some(to) = seg.target() {
                out.push(Move {
                    from: at,
                    to,
                    rapid: seg.is_rapid(),
                });
                at = to;
            }
        }
        out
    }

    /// Distance from `p` to a segment.
    fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        crate::geom2d::point_segment_distance(p, a, b)
    }

    #[test]
    fn test_face_basic() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 2.0);
        let toolpath = face.generate(&mill(10.0), &settings()).unwrap();
        assert!(!toolpath.is_empty());

        let cutting = toolpath.segments.iter().filter(|s| s.is_cutting()).count();
        assert!(cutting > 0);

        let min_z = toolpath
            .segments
            .iter()
            .filter_map(|s| s.target())
            .map(|[_, _, z]| z)
            .fold(f64::INFINITY, f64::min);
        assert!((min_z + 2.0).abs() < 0.01);
    }

    #[test]
    fn test_face_from_size() {
        let face = Face::from_size(100.0, 50.0, 1.0);
        assert!((face.min_x - 0.0).abs() < 1e-6);
        assert!((face.max_x - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_face_invalid_bounds() {
        let face = Face::new(10.0, 10.0, 5.0, 5.0, 1.0);
        let result = face.generate(&Tool::default_endmill(), &CamSettings::default());
        assert!(matches!(result, Err(CamError::DegenerateBounds(_, _))));
    }

    #[test]
    fn test_face_invalid_stepover() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 1.0);
        let settings = CamSettings {
            stepover: 10.0, // Larger than the tool diameter.
            ..CamSettings::default()
        };
        let result = face.generate(&mill(6.0), &settings);
        assert!(matches!(result, Err(CamError::InvalidStepover(_))));
    }

    /// The step-over guarantee and the overshoot, measured rather than
    /// restated: every point of the faced area is within a tool radius of some
    /// cutting move at the final depth, and the swept band reaches exactly one
    /// radius past each edge.
    ///
    /// Mutation check: raising the stepover past the diameter is refused
    /// outright (`test_face_invalid_stepover`); forcing the spacing by hand to
    /// 1.4 diameters — the mutation below — leaves a 2 mm strip uncut, which
    /// this assertion catches at 0.25 mm sampling.
    #[test]
    fn facing_leaves_no_strip_and_overshoots_by_one_radius() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 2.0);
        let tool = mill(10.0);
        let radius = tool.radius();
        let toolpath = face.generate(&tool, &settings()).unwrap();

        let floor = -2.0;
        let cuts: Vec<([f64; 2], [f64; 2])> = moves(&toolpath)
            .iter()
            .filter(|m| !m.rapid && m.from[2] <= floor + 1e-9 && m.to[2] <= floor + 1e-9)
            .map(|m| ([m.from[0], m.from[1]], [m.to[0], m.to[1]]))
            .collect();
        assert!(!cuts.is_empty());

        let worst = |cuts: &[([f64; 2], [f64; 2])]| {
            let mut worst: f64 = 0.0;
            let mut y = 0.0;
            while y <= 30.0 + 1e-9 {
                let mut x = 0.0;
                while x <= 50.0 + 1e-9 {
                    let d = cuts
                        .iter()
                        .map(|(a, b)| to_segment([x, y], *a, *b))
                        .fold(f64::INFINITY, f64::min);
                    worst = worst.max(d);
                    x += 0.25;
                }
                y += 0.25;
            }
            worst
        };
        let uncovered = worst(&cuts);
        assert!(
            uncovered <= radius + 1e-9,
            "a point of the area is {uncovered:.3} mm from the nearest pass, the cutter reaches \
             {radius:.3}"
        );
        // Not vacuous: the worst point sits half a row spacing from its two
        // neighbours, which is what the guarantee is made of.
        assert!(
            (uncovered - settings().stepover / 2.0).abs() < 0.02,
            "worst gap {uncovered:.3} mm, half a row spacing is {:.3}",
            settings().stepover / 2.0
        );

        // Overshoot: the tool centre runs exactly a radius past each edge.
        let xs: Vec<f64> = cuts.iter().flat_map(|(a, b)| [a[0], b[0]]).collect();
        let ys: Vec<f64> = cuts.iter().flat_map(|(a, b)| [a[1], b[1]]).collect();
        let lo_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let lo_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!((lo_x + radius).abs() < 1e-9, "{lo_x}");
        assert!((hi_x - (50.0 + radius)).abs() < 1e-9, "{hi_x}");
        assert!((lo_y + radius).abs() < 1e-9, "{lo_y}");
        assert!((hi_y - (30.0 + radius)).abs() < 1e-9, "{hi_y}");

        // The mutation: the same measurement over rows 1.4 diameters apart.
        let wide: Vec<([f64; 2], [f64; 2])> = (0..4)
            .map(|k| {
                let y = -radius + k as f64 * 14.0;
                ([-radius, y], [50.0 + radius, y])
            })
            .collect();
        assert!(
            worst(&wide) > radius,
            "a 14 mm spacing has to leave a strip a 10 mm cutter cannot reach"
        );
    }

    /// A stepdown of zero used to come back `Ok` with a toolpath that cut
    /// nothing: `depth / 0` is infinite and `ceil() as usize` saturates to
    /// zero, so the Z loop never ran.
    #[test]
    fn a_stepdown_of_zero_is_refused_instead_of_cutting_nothing() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 2.0);
        let settings = CamSettings {
            stepdown: 0.0,
            ..settings()
        };
        assert!(matches!(
            face.generate(&mill(10.0), &settings),
            Err(CamError::InvalidStepdown(_))
        ));
    }

    /// No rapid travels in XY below the safe height. The closing retract used
    /// to go from the bottom of the last cut straight to the corner of the
    /// area, dragging the cutter across the work at depth.
    #[test]
    fn no_rapid_travels_across_the_work_at_depth() {
        for one_way in [false, true] {
            let mut face = Face::new(0.0, 0.0, 50.0, 30.0, 4.0);
            face.zigzag = !one_way;
            let toolpath = face.generate(&mill(10.0), &settings()).unwrap();
            // The first move is measured from the replay's made-up origin: the
            // machine is really wherever the last operation left it, which is
            // at the safe height. Every operation in this crate opens with it.
            for m in moves(&toolpath).into_iter().skip(1) {
                if !m.rapid {
                    continue;
                }
                let travels = (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]) > 1e-9;
                let low = m.from[2].min(m.to[2]) < 5.0 - 1e-9;
                assert!(
                    !(travels && low),
                    "a rapid travels in XY at Z {:.3}..{:.3} (one_way={one_way})",
                    m.from[2],
                    m.to[2]
                );
            }
        }
    }

    /// Climb puts the metal still to come on the right of the direction of
    /// travel. The passes step in +Y, so a climbing pass runs −X, and a one-way
    /// raster keeps every pass that way instead of alternating.
    #[test]
    fn one_way_facing_keeps_every_pass_on_the_same_side() {
        for (direction, want_minus_x) in [
            (CutDirection::Climb, true),
            (CutDirection::Conventional, false),
        ] {
            let face = Face::new(0.0, 0.0, 50.0, 30.0, 2.0)
                .with_direction(direction)
                .one_way();
            let toolpath = face.generate(&mill(10.0), &settings()).unwrap();
            let long_cuts: Vec<Move> = moves(&toolpath)
                .into_iter()
                .filter(|m| !m.rapid && (m.to[0] - m.from[0]).abs() > 1.0)
                .collect();
            assert!(long_cuts.len() >= 5, "{}", long_cuts.len());
            for m in &long_cuts {
                assert_eq!(
                    m.to[0] < m.from[0],
                    want_minus_x,
                    "a {direction:?} pass runs from {:.1} to {:.1}",
                    m.from[0],
                    m.to[0]
                );
            }
            // And the zig-zag really does alternate, which is why it cannot
            // honour a direction for more than its first pass.
            let zig = Face::new(0.0, 0.0, 50.0, 30.0, 2.0).with_direction(direction);
            let zig_cuts: Vec<Move> = moves(&zig.generate(&mill(10.0), &settings()).unwrap())
                .into_iter()
                .filter(|m| !m.rapid && (m.to[0] - m.from[0]).abs() > 1.0)
                .collect();
            assert_eq!(zig_cuts[0].to[0] < zig_cuts[0].from[0], want_minus_x);
            assert_ne!(zig_cuts[1].to[0] < zig_cuts[1].from[0], want_minus_x);
        }
    }
}
