//! Toolpath representation for CAM operations.

use serde::{Deserialize, Serialize};
use vcad_kernel_math::Point3;

/// Direction of spindle rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpindleDir {
    /// Clockwise (M3).
    Cw,
    /// Counter-clockwise (M4).
    Ccw,
}

/// Coolant mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoolantMode {
    /// Coolant off (M9).
    Off,
    /// Mist coolant (M7).
    Mist,
    /// Flood coolant (M8).
    Flood,
}

/// Plane for arc interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArcPlane {
    /// XY plane (G17).
    Xy,
    /// XZ plane (G18).
    Xz,
    /// YZ plane (G19).
    Yz,
}

/// Direction of arc movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArcDir {
    /// Clockwise (G2).
    Cw,
    /// Counter-clockwise (G3).
    Ccw,
}

/// A single segment in a toolpath.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolpathSegment {
    /// Rapid move (G0).
    Rapid {
        /// Target position.
        to: [f64; 3],
    },
    /// Linear interpolation (G1).
    Linear {
        /// Target position.
        to: [f64; 3],
        /// Feed rate in mm/min.
        feed: f64,
    },
    /// Arc interpolation (G2/G3).
    Arc {
        /// Target position.
        to: [f64; 3],
        /// Arc center (relative to start for GRBL).
        center: [f64; 3],
        /// Arc plane.
        plane: ArcPlane,
        /// Arc direction.
        dir: ArcDir,
        /// Feed rate in mm/min.
        feed: f64,
    },
    /// Dwell (G4).
    Dwell {
        /// Dwell time in seconds.
        seconds: f64,
    },
    /// Spindle control (M3/M4/M5).
    Spindle {
        /// Spindle speed in RPM (0 = off).
        rpm: f64,
        /// Rotation direction.
        dir: SpindleDir,
    },
    /// Coolant control (M7/M8/M9).
    Coolant {
        /// Coolant mode.
        mode: CoolantMode,
    },
    /// Tool change (M6).
    ToolChange {
        /// Tool number.
        tool_number: u32,
    },
    /// Comment for documentation.
    Comment {
        /// Comment text.
        text: String,
    },
    /// Stop and wait for the operator (M0).
    ///
    /// The one thing a program does that is neither motion nor a mode: it
    /// hands control back. The post writes the stop word and the message
    /// beside it, so a pause is posted like everything else instead of being
    /// spliced into the text afterwards.
    Pause {
        /// What the operator has to do before resuming. Posted as a comment
        /// next to the stop word.
        message: Option<String>,
    },
    /// Machine-specific text, emitted verbatim.
    ///
    /// **Escape hatch.** Nothing reads this: the verification oracle cannot
    /// replay it, [`Toolpath::cutting_length`] does not measure it, and the
    /// time estimators do not cost it. It exists for the one thing a segment
    /// cannot express — a probing macro the machine's own dialect defines —
    /// and every other use is a missing variant. Whatever goes in here is the
    /// caller's to have checked.
    Raw {
        /// Lines to emit, exactly as given.
        text: String,
    },
}

impl ToolpathSegment {
    /// Create a rapid move to the given position.
    pub fn rapid(x: f64, y: f64, z: f64) -> Self {
        Self::Rapid { to: [x, y, z] }
    }

    /// Create a rapid move to a Point3.
    pub fn rapid_to(p: &Point3) -> Self {
        Self::Rapid {
            to: [p.x, p.y, p.z],
        }
    }

    /// Create a linear move with feed rate.
    pub fn linear(x: f64, y: f64, z: f64, feed: f64) -> Self {
        Self::Linear {
            to: [x, y, z],
            feed,
        }
    }

    /// Create a linear move to a Point3.
    pub fn linear_to(p: &Point3, feed: f64) -> Self {
        Self::Linear {
            to: [p.x, p.y, p.z],
            feed,
        }
    }

    /// Create an XY arc.
    pub fn arc_xy(to_x: f64, to_y: f64, to_z: f64, i: f64, j: f64, dir: ArcDir, feed: f64) -> Self {
        Self::Arc {
            to: [to_x, to_y, to_z],
            center: [i, j, 0.0],
            plane: ArcPlane::Xy,
            dir,
            feed,
        }
    }

    /// Create a dwell.
    pub fn dwell(seconds: f64) -> Self {
        Self::Dwell { seconds }
    }

    /// Create a spindle on command.
    pub fn spindle_on(rpm: f64, dir: SpindleDir) -> Self {
        Self::Spindle { rpm, dir }
    }

    /// Create a spindle off command.
    pub fn spindle_off() -> Self {
        Self::Spindle {
            rpm: 0.0,
            dir: SpindleDir::Cw,
        }
    }

    /// Create a coolant command.
    pub fn coolant(mode: CoolantMode) -> Self {
        Self::Coolant { mode }
    }

    /// Create a tool change command.
    pub fn tool_change(tool_number: u32) -> Self {
        Self::ToolChange { tool_number }
    }

    /// Create a comment.
    pub fn comment(text: impl Into<String>) -> Self {
        Self::Comment { text: text.into() }
    }

    /// Stop and wait for the operator, saying why.
    pub fn pause(message: impl Into<String>) -> Self {
        Self::Pause {
            message: Some(message.into()),
        }
    }

    /// Machine-specific text, emitted verbatim. See [`ToolpathSegment::Raw`]
    /// before reaching for this.
    pub fn raw(text: impl Into<String>) -> Self {
        Self::Raw { text: text.into() }
    }

    /// Get the target position if this is a motion segment.
    pub fn target(&self) -> Option<[f64; 3]> {
        match self {
            Self::Rapid { to } | Self::Linear { to, .. } | Self::Arc { to, .. } => Some(*to),
            _ => None,
        }
    }

    /// Check if this is a rapid move.
    pub fn is_rapid(&self) -> bool {
        matches!(self, Self::Rapid { .. })
    }

    /// Check if this is a cutting move (linear or arc).
    pub fn is_cutting(&self) -> bool {
        matches!(self, Self::Linear { .. } | Self::Arc { .. })
    }

    /// How far the tool travels on this move, starting from `from`, in mm.
    ///
    /// An arc is measured **along the arc**, not across its chord: a fitted
    /// half-circle of radius 5 is 15.708 mm of cutting, not the 10 mm the
    /// chord says. A helical arc adds its rise the Pythagorean way. Segments
    /// that are not motion are zero.
    pub fn length_from(&self, from: [f64; 3]) -> f64 {
        match self {
            Self::Rapid { to } | Self::Linear { to, .. } => {
                let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            }
            Self::Arc {
                to,
                center,
                plane,
                dir,
                ..
            } => arc_geometry(from, *to, *center, *plane, *dir).length,
            _ => 0.0,
        }
    }
}

/// The measured shape of an arc segment: what the length and the speed limit
/// both need.
#[derive(Debug, Clone, Copy)]
struct ArcGeometry {
    /// True path length including any helical rise (mm).
    length: f64,
    /// Radius in the plane of the arc (mm).
    radius: f64,
    /// Unit direction of travel at the start.
    tangent_in: [f64; 3],
    /// Unit direction of travel at the end.
    tangent_out: [f64; 3],
}

/// Axis indices of an arc plane: the two it sweeps in, then the helical one.
fn plane_axes(plane: ArcPlane) -> (usize, usize, usize) {
    match plane {
        ArcPlane::Xy => (0, 1, 2),
        ArcPlane::Xz => (2, 0, 1),
        ArcPlane::Yz => (1, 2, 0),
    }
}

/// Measure an arc whose centre is given relative to its start (the I/J/K the
/// posts emit).
///
/// Coincident end points mean a full turn, as they do in G-code. The radius is
/// the mean of the two ends' radii: they agree on any arc worth posting, and
/// averaging keeps a round-off away from the length.
fn arc_geometry(
    from: [f64; 3],
    to: [f64; 3],
    center: [f64; 3],
    plane: ArcPlane,
    dir: ArcDir,
) -> ArcGeometry {
    let (a, b, c) = plane_axes(plane);
    let centre = [from[a] + center[a], from[b] + center[b]];
    let (sx, sy) = (from[a] - centre[0], from[b] - centre[1]);
    let (ex, ey) = (to[a] - centre[0], to[b] - centre[1]);
    let radius = 0.5 * (sx.hypot(sy) + ex.hypot(ey));
    let start = sy.atan2(sx);
    let end = ey.atan2(ex);
    let closed = (to[a] - from[a]).hypot(to[b] - from[b]) < 1e-9;
    let ccw = matches!(dir, ArcDir::Ccw);
    let sweep = if closed {
        std::f64::consts::TAU
    } else {
        let raw = if ccw { end - start } else { start - end };
        raw.rem_euclid(std::f64::consts::TAU)
    };
    let rise = to[c] - from[c];
    let planar = radius * sweep;
    let length = planar.hypot(rise);

    // Tangents: perpendicular to the radius, turned the way the arc runs, with
    // the helical rise folded in so a junction against a straight move sees
    // the direction the tool is really going.
    let tangent = |angle: f64| {
        let sign = if ccw { 1.0 } else { -1.0 };
        let mut t = [0.0; 3];
        t[a] = -sign * angle.sin();
        t[b] = sign * angle.cos();
        let rise_per_length = if length > 0.0 { rise / length } else { 0.0 };
        let planar_scale = (1.0 - rise_per_length * rise_per_length).max(0.0).sqrt();
        t[a] *= planar_scale;
        t[b] *= planar_scale;
        t[c] = rise_per_length;
        t
    };
    ArcGeometry {
        length,
        radius,
        tangent_in: tangent(start),
        tangent_out: tangent(end),
    }
}

/// What the machine can actually do, for [`Toolpath::estimated_time_with`].
///
/// These are the grbl settings by another name: `$120-122` (acceleration),
/// `$110-112` (max rate) and `$11` (junction deviation). Per-axis values
/// collapse to the slowest, because the estimate is one-dimensional along the
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MachineLimits {
    /// Acceleration in mm/s².
    pub max_accel: f64,
    /// Maximum rate in mm/min, which also caps rapids.
    pub max_rate: f64,
    /// Junction deviation in mm: how far off the programmed corner the
    /// planner will let the tool cut in exchange for not stopping. grbl's
    /// default is 0.01 mm.
    pub junction_deviation: f64,
}

impl MachineLimits {
    /// Limits from an acceleration and a max rate, with grbl's default
    /// junction deviation.
    pub fn new(max_accel: f64, max_rate: f64) -> Self {
        Self {
            max_accel,
            max_rate,
            junction_deviation: 0.01,
        }
    }

    /// The AnoleX Ultra 2 as measured: `$120-122 = 300` mm/s², `$110-112 =
    /// 3000` mm/min.
    pub fn anolex_ultra_2() -> Self {
        Self::new(300.0, 3000.0)
    }

    /// Set the junction deviation.
    pub fn with_junction_deviation(mut self, deviation: f64) -> Self {
        self.junction_deviation = deviation;
        self
    }

    /// Speed the planner allows through a corner where the direction of
    /// travel turns by `turn` radians, in mm/s.
    ///
    /// grbl's rule: fit a circle of the given deviation into the corner and
    /// take the speed that circle's centripetal limit allows. A straight
    /// junction is unlimited; a reversal is a full stop.
    fn junction_speed(&self, turn: f64) -> f64 {
        let half = (turn * 0.5).cos(); // sin of grbl's included half-angle
        if half <= 1e-9 {
            return 0.0;
        }
        if half >= 1.0 - 1e-12 {
            return f64::INFINITY;
        }
        (self.max_accel * self.junction_deviation * half / (1.0 - half)).sqrt()
    }
}

/// A complete toolpath consisting of multiple segments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Toolpath {
    /// The segments in order.
    pub segments: Vec<ToolpathSegment>,
}

impl Toolpath {
    /// Create a new empty toolpath.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a segment to the toolpath.
    pub fn push(&mut self, segment: ToolpathSegment) {
        self.segments.push(segment);
    }

    /// Extend with multiple segments.
    pub fn extend(&mut self, segments: impl IntoIterator<Item = ToolpathSegment>) {
        self.segments.extend(segments);
    }

    /// Get the number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Check if the toolpath is empty.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Calculate the total path length (cutting moves only).
    ///
    /// Arcs are measured along the arc; see
    /// [`ToolpathSegment::length_from`].
    pub fn cutting_length(&self) -> f64 {
        let mut length = 0.0;
        let mut last_pos = [0.0, 0.0, 0.0];

        for seg in &self.segments {
            if let Some(to) = seg.target() {
                if seg.is_cutting() {
                    length += seg.length_from(last_pos);
                }
                last_pos = to;
            }
        }

        length
    }

    /// Estimated machining time in seconds, assuming every move runs at its
    /// programmed feed from end to end.
    ///
    /// Optimistic by construction: nothing here knows that a 0.3 mm segment
    /// cannot reach F400 on a machine that accelerates at 300 mm/s². Use
    /// [`Toolpath::estimated_time_with`] when the machine's limits are known.
    pub fn estimated_time(&self) -> f64 {
        const RAPID_FEED: f64 = 5000.0; // mm/min
        let mut time = 0.0;
        let mut last_pos = [0.0, 0.0, 0.0];

        for seg in &self.segments {
            match seg {
                ToolpathSegment::Rapid { to } => {
                    time += seg.length_from(last_pos) / RAPID_FEED * 60.0;
                    last_pos = *to;
                }
                ToolpathSegment::Linear { to, feed } | ToolpathSegment::Arc { to, feed, .. } => {
                    if *feed > 0.0 {
                        time += seg.length_from(last_pos) / feed * 60.0;
                    }
                    last_pos = *to;
                }
                ToolpathSegment::Dwell { seconds } => {
                    time += seconds;
                }
                _ => {}
            }
        }

        time
    }

    /// Estimated machining time in seconds, with the machine's acceleration
    /// and corner handling taken into account.
    ///
    /// Each move is a trapezoid — accelerate, run, decelerate — bounded at
    /// both ends by the speed the planner will carry through that corner, and
    /// the whole path is swept backwards then forwards so a move can only
    /// enter a corner as fast as it can still stop for it. That is what makes
    /// the difference on a path of thousands of short segments: the naive
    /// estimate charges them at the programmed feed, and the machine never
    /// gets there.
    ///
    /// Dwells count. A [`ToolpathSegment::Pause`] does not — how long an
    /// operator takes is not the machine's business — and neither does
    /// [`ToolpathSegment::Raw`], whose contents nothing here can read.
    pub fn estimated_time_with(&self, limits: &MachineLimits) -> f64 {
        let accel = limits.max_accel.max(1e-9);
        let ceiling = (limits.max_rate / 60.0).max(1e-9);

        // Motion first, as (length, speed cap, entry tangent, exit tangent).
        struct Leg {
            length: f64,
            cap: f64,
            tangent_in: [f64; 3],
            tangent_out: [f64; 3],
        }
        let mut legs: Vec<Leg> = Vec::new();
        let mut dwell = 0.0;
        let mut at = [0.0, 0.0, 0.0];
        for seg in &self.segments {
            match seg {
                ToolpathSegment::Dwell { seconds } => dwell += seconds,
                ToolpathSegment::Rapid { to } | ToolpathSegment::Linear { to, .. } => {
                    let length = seg.length_from(at);
                    let feed = match seg {
                        ToolpathSegment::Linear { feed, .. } => *feed / 60.0,
                        _ => f64::INFINITY,
                    };
                    if length > 0.0 {
                        let unit = [0, 1, 2].map(|k| (to[k] - at[k]) / length);
                        legs.push(Leg {
                            length,
                            cap: feed.min(ceiling),
                            tangent_in: unit,
                            tangent_out: unit,
                        });
                    }
                    at = *to;
                }
                ToolpathSegment::Arc {
                    to,
                    center,
                    plane,
                    dir,
                    feed,
                } => {
                    let arc = arc_geometry(at, *to, *center, *plane, *dir);
                    if arc.length > 0.0 {
                        // Rounding a corner of radius r at v pulls v²/r; the
                        // planner will not pull harder than it accelerates.
                        let centripetal = (accel * arc.radius).sqrt();
                        legs.push(Leg {
                            length: arc.length,
                            cap: (feed / 60.0).min(ceiling).min(centripetal),
                            tangent_in: arc.tangent_in,
                            tangent_out: arc.tangent_out,
                        });
                    }
                    at = *to;
                }
                _ => {}
            }
        }
        if legs.is_empty() {
            return dwell;
        }

        // Corner speeds, then the two sweeps that make them reachable.
        let mut corner = vec![0.0; legs.len() + 1];
        for k in 1..legs.len() {
            let previous = legs[k - 1].tangent_out;
            let next = legs[k].tangent_in;
            let dot = (0..3).map(|i| previous[i] * next[i]).sum::<f64>();
            let turn = dot.clamp(-1.0, 1.0).acos();
            corner[k] = limits
                .junction_speed(turn)
                .min(legs[k - 1].cap)
                .min(legs[k].cap);
        }
        for k in (0..legs.len()).rev() {
            let reachable = (corner[k + 1] * corner[k + 1] + 2.0 * accel * legs[k].length).sqrt();
            corner[k] = corner[k].min(reachable);
        }
        for k in 0..legs.len() {
            let reachable = (corner[k] * corner[k] + 2.0 * accel * legs[k].length).sqrt();
            corner[k + 1] = corner[k + 1].min(reachable);
        }

        let mut time = dwell;
        for (k, leg) in legs.iter().enumerate() {
            let (entry, exit) = (corner[k], corner[k + 1]);
            // Peak of the trapezoid, or of the triangle when the move is too
            // short to reach the feed.
            let peak = (((2.0 * accel * leg.length + entry * entry + exit * exit) / 2.0).sqrt())
                .min(leg.cap)
                .max(entry.max(exit));
            let ramp_up = (peak * peak - entry * entry) / (2.0 * accel);
            let ramp_down = (peak * peak - exit * exit) / (2.0 * accel);
            let cruise = leg.length - ramp_up - ramp_down;
            time += (peak - entry) / accel + (peak - exit) / accel;
            if cruise > 0.0 && peak > 0.0 {
                time += cruise / peak;
            }
        }
        time
    }

    /// Get the bounding box of the toolpath.
    pub fn bounding_box(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut has_points = false;

        for seg in &self.segments {
            if let Some(to) = seg.target() {
                has_points = true;
                for i in 0..3 {
                    min[i] = min[i].min(to[i]);
                    max[i] = max[i].max(to[i]);
                }
            }
        }

        if has_points {
            Some((min, max))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolpath_segment_creation() {
        let rapid = ToolpathSegment::rapid(10.0, 20.0, 5.0);
        assert!(rapid.is_rapid());
        assert!(!rapid.is_cutting());
        assert_eq!(rapid.target(), Some([10.0, 20.0, 5.0]));

        let linear = ToolpathSegment::linear(10.0, 20.0, 0.0, 1000.0);
        assert!(!linear.is_rapid());
        assert!(linear.is_cutting());
    }

    #[test]
    fn test_toolpath_length() {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 5.0));
        tp.push(ToolpathSegment::linear(0.0, 0.0, 0.0, 300.0));
        tp.push(ToolpathSegment::linear(10.0, 0.0, 0.0, 1000.0));
        tp.push(ToolpathSegment::linear(10.0, 10.0, 0.0, 1000.0));

        // Cutting length should be 5 + 10 + 10 = 25
        let length = tp.cutting_length();
        assert!((length - 25.0).abs() < 1e-6);
    }

    #[test]
    fn test_toolpath_bounding_box() {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 5.0));
        tp.push(ToolpathSegment::linear(10.0, 20.0, -5.0, 1000.0));

        let (min, max) = tp.bounding_box().unwrap();
        assert!((min[0] - 0.0).abs() < 1e-6);
        assert!((min[1] - 0.0).abs() < 1e-6);
        assert!((min[2] - -5.0).abs() < 1e-6);
        assert!((max[0] - 10.0).abs() < 1e-6);
        assert!((max[1] - 20.0).abs() < 1e-6);
        assert!((max[2] - 5.0).abs() < 1e-6);
    }

    /// A circle posted as two half-arcs measures 2πr, not the 4r its chords
    /// add up to. Before arcs were measured along the arc, fitting a circle
    /// made the job look 36 % shorter than it is.
    #[test]
    fn a_fitted_circle_is_two_pi_r_long() {
        let r = 5.0;
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(r, 0.0, 0.0));
        tp.push(ToolpathSegment::arc_xy(
            -r,
            0.0,
            0.0,
            -r,
            0.0,
            ArcDir::Ccw,
            500.0,
        ));
        tp.push(ToolpathSegment::arc_xy(
            r,
            0.0,
            0.0,
            r,
            0.0,
            ArcDir::Ccw,
            500.0,
        ));

        let want = std::f64::consts::TAU * r;
        assert!(
            (tp.cutting_length() - want).abs() < 1e-6,
            "{} vs {want}",
            tp.cutting_length()
        );
        // The chord measure would have said 4r = 20 mm.
        assert!(tp.cutting_length() > 4.0 * r);
        // Time follows length: 31.4159 mm at 500 mm/min, plus the 5 mm
        // rapid that gets to the start.
        let rapid = 5.0 / 5000.0 * 60.0;
        assert!((tp.estimated_time() - rapid - want / 500.0 * 60.0).abs() < 1e-9);
    }

    /// One turn of a helical bore is the hypotenuse of its circumference and
    /// its pitch, whether it is posted as one arc or as four quarters.
    #[test]
    fn a_helical_turn_is_the_helix_formula() {
        let (r, pitch) = (3.0, 1.2);
        let rise = pitch / 4.0;
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(r, 0.0, 0.0));
        let quarters = [
            ([0.0, r], [-r, 0.0]),
            ([-r, 0.0], [0.0, -r]),
            ([0.0, -r], [r, 0.0]),
            ([r, 0.0], [0.0, r]),
        ];
        for (k, (to, centre)) in quarters.iter().enumerate() {
            tp.push(ToolpathSegment::arc_xy(
                to[0],
                to[1],
                -rise * (k as f64 + 1.0),
                centre[0],
                centre[1],
                ArcDir::Ccw,
                300.0,
            ));
        }

        let want = (std::f64::consts::TAU * r).hypot(pitch);
        assert!(
            (tp.cutting_length() - want).abs() < 1e-9,
            "{} vs {want}",
            tp.cutting_length()
        );
    }

    /// Thousands of short moves never reach their feed. A staircase of
    /// 0.25 mm segments at F1200 asks for 20 mm/s; the machine turns 90°
    /// every segment, and at 300 mm/s² and 0.01 mm of junction deviation it
    /// leaves each corner at 2.7 mm/s and has 0.25 mm to recover in.
    #[test]
    fn short_segments_never_reach_their_feed() {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 0.0));
        let step = 0.25;
        let mut at = [0.0, 0.0];
        for k in 0..800 {
            if k % 2 == 0 {
                at[0] += step;
            } else {
                at[1] += step;
            }
            tp.push(ToolpathSegment::linear(at[0], at[1], 0.0, 1200.0));
        }

        let limits = MachineLimits::anolex_ultra_2();
        let naive = tp.estimated_time();
        let honest = tp.estimated_time_with(&limits);
        let length = tp.cutting_length();
        assert!((length - 200.0).abs() < 1e-9, "{length}");
        // The naive estimate charges the whole 200 mm at 20 mm/s.
        assert!((naive - 10.0).abs() < 1e-9, "{naive}");
        assert!(
            honest > naive * 1.8,
            "accel-aware {honest:.2}s should dwarf the naive {naive:.2}s"
        );
        // Mean speed under half the programmed feed.
        assert!(length / honest < 10.0, "{} mm/s", length / honest);
    }

    /// The one job this crate has watched run: the stator's copper contour,
    /// started 15:15:30 and off the machine by 15:29:48 — 850 s of an AnoleX
    /// Ultra 2 (`$120-122 = 300` mm/s², `$110-112 = 3000` mm/min).
    ///
    /// The app showed 14:05 from [`Toolpath::estimated_time`]. Both estimates
    /// are printed so the gap is on the record.
    #[test]
    fn the_copper_fixture_takes_the_time_the_machine_took() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-copper-d2.nc"
        );
        let text = std::fs::read_to_string(path).expect("docs/cam-fixtures/stator-copper-d2.nc");
        let moves = crate::verify2d::parse_gcode(&text, &crate::verify2d::VerifyOptions::default())
            .expect("the fixture parses");

        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::Rapid { to: moves[0].from });
        for m in &moves {
            if m.rapid {
                tp.push(ToolpathSegment::Rapid { to: m.to });
            } else {
                tp.push(ToolpathSegment::Linear {
                    to: m.to,
                    feed: m.feed,
                });
            }
        }

        let measured = 850.0; // 15:15:30 → 15:29:48, minus the 18 s of setup
        let naive = tp.estimated_time();
        let honest = tp.estimated_time_with(&MachineLimits::anolex_ultra_2());
        let err = |t: f64| (t - measured) / measured * 100.0;
        println!(
            "copper fixture: measured {measured:.0}s, naive {naive:.1}s ({:+.2}%), \
             accel-aware {honest:.1}s ({:+.2}%)",
            err(naive),
            err(honest)
        );
        assert!(
            err(honest).abs() <= 5.0,
            "accel-aware estimate {honest:.1}s is {:+.2}% off the measured {measured:.0}s",
            err(honest)
        );
    }

    /// A long straight move does reach its feed, so the two estimates agree
    /// to the ramp time and no more.
    #[test]
    fn a_long_move_costs_the_same_either_way() {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 0.0));
        tp.push(ToolpathSegment::linear(300.0, 0.0, 0.0, 600.0));

        let limits = MachineLimits::anolex_ultra_2();
        let naive = tp.estimated_time(); // 30 s at 10 mm/s
        let honest = tp.estimated_time_with(&limits);
        // Starting and stopping at 300 mm/s² costs 10/300 s each way, half of
        // it recovered because the ramp still covers ground.
        assert!((naive - 30.0).abs() < 1e-9);
        assert!(
            honest > naive && honest - naive < 0.05,
            "{honest} vs {naive}"
        );
    }

    #[test]
    fn test_toolpath_serialization() {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(10.0, 20.0, 5.0));
        tp.push(ToolpathSegment::linear(10.0, 20.0, 0.0, 1000.0));

        let json = serde_json::to_string(&tp).unwrap();
        let parsed: Toolpath = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
    }
}
