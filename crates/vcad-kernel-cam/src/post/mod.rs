//! Post-processors for converting toolpaths to machine-specific G-code.

mod grbl;
mod linuxcnc;

pub use grbl::GrblPost;
pub use linuxcnc::LinuxCncPost;

use crate::{CamSettings, Tool, ToolEntry, Toolpath, ToolpathSegment};

/// State tracked during post-processing.
#[derive(Debug, Clone, Default)]
pub struct PostState {
    /// Current X position.
    pub x: f64,
    /// Current Y position.
    pub y: f64,
    /// Current Z position.
    pub z: f64,
    /// Current feed rate.
    pub feed: f64,
    /// Current spindle speed.
    pub spindle_rpm: f64,
    /// Whether spindle is on.
    pub spindle_on: bool,
    /// Current tool number.
    pub tool_number: u32,
    /// Line number for G-code output.
    pub line_number: u32,
}

/// Trait for post-processors that convert toolpaths to G-code.
pub trait PostProcessor {
    /// Generate the G-code header (units, modes, etc.).
    fn header(&self, job_name: &str, settings: &CamSettings) -> String;

    /// Generate G-code for a tool change.
    fn tool_change(&self, tool: &ToolEntry, state: &mut PostState) -> String;

    /// Generate G-code for a single toolpath segment.
    fn segment(&self, seg: &ToolpathSegment, state: &mut PostState) -> String;

    /// Generate the G-code footer (program end).
    fn footer(&self, state: &PostState) -> String;

    /// Generate complete G-code for a toolpath.
    fn generate(
        &self,
        job_name: &str,
        _tool: &Tool,
        toolpath: &Toolpath,
        settings: &CamSettings,
    ) -> String {
        let mut output = String::new();
        let mut state = PostState::default();

        // Header
        output.push_str(&self.header(job_name, settings));

        // Spindle on
        output.push_str(&format!("M3 S{:.0}\n", settings.spindle_rpm));
        state.spindle_on = true;
        state.spindle_rpm = settings.spindle_rpm;

        // Process segments
        for seg in &toolpath.segments {
            output.push_str(&self.segment(seg, &mut state));
        }

        // Footer
        output.push_str(&self.footer(&state));

        output
    }
}

/// Format a floating point value with appropriate precision.
pub fn format_coord(value: f64, precision: usize) -> String {
    format!("{:.prec$}", value, prec = precision)
}

/// Calculate distance between two 3D points.
pub fn distance_3d(p1: [f64; 3], p2: [f64; 3]) -> f64 {
    let dx = p2[0] - p1[0];
    let dy = p2[1] - p1[1];
    let dz = p2[2] - p1[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_coord() {
        assert_eq!(format_coord(1.2345, 3), "1.234"); // Rust rounds to even
        assert_eq!(format_coord(1.0, 3), "1.000");
        assert_eq!(format_coord(-0.5, 2), "-0.50");
    }

    #[test]
    fn test_distance_3d() {
        let d = distance_3d([0.0, 0.0, 0.0], [3.0, 4.0, 0.0]);
        assert!((d - 5.0).abs() < 1e-6);
    }
}
