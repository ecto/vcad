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
    pub fn cutting_length(&self) -> f64 {
        let mut length = 0.0;
        let mut last_pos = [0.0, 0.0, 0.0];

        for seg in &self.segments {
            if let Some(to) = seg.target() {
                if seg.is_cutting() {
                    let dx = to[0] - last_pos[0];
                    let dy = to[1] - last_pos[1];
                    let dz = to[2] - last_pos[2];
                    length += (dx * dx + dy * dy + dz * dz).sqrt();
                }
                last_pos = to;
            }
        }

        length
    }

    /// Calculate estimated machining time in seconds.
    pub fn estimated_time(&self) -> f64 {
        let mut time = 0.0;
        let mut last_pos = [0.0, 0.0, 0.0];
        let rapid_feed = 5000.0; // mm/min for rapids

        for seg in &self.segments {
            match seg {
                ToolpathSegment::Rapid { to } => {
                    let dx = to[0] - last_pos[0];
                    let dy = to[1] - last_pos[1];
                    let dz = to[2] - last_pos[2];
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                    time += dist / rapid_feed * 60.0;
                    last_pos = *to;
                }
                ToolpathSegment::Linear { to, feed } => {
                    let dx = to[0] - last_pos[0];
                    let dy = to[1] - last_pos[1];
                    let dz = to[2] - last_pos[2];
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                    time += dist / feed * 60.0;
                    last_pos = *to;
                }
                ToolpathSegment::Arc { to, feed, .. } => {
                    // Approximate arc length as straight line for simplicity
                    let dx = to[0] - last_pos[0];
                    let dy = to[1] - last_pos[1];
                    let dz = to[2] - last_pos[2];
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                    time += dist / feed * 60.0;
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
