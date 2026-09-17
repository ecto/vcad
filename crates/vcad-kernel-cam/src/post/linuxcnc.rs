//! LinuxCNC post-processor for 3-axis CNC mills.
//!
//! LinuxCNC (formerly EMC2) is a popular open-source CNC controller.
//! This post-processor generates G-code compatible with LinuxCNC's
//! RS274NGC interpreter.
//!
//! Key differences from GRBL:
//! - O-word program numbers
//! - G43 H tool length compensation
//! - Canned cycles (G81-G89)
//! - M2 program end
//! - Dwell uses seconds (P) not milliseconds

use super::{format_coord, PostProcessor, PostState, ProgramOptions};
use crate::{ArcDir, ArcPlane, CoolantMode, SpindleDir, ToolEntry, ToolpathSegment};

/// LinuxCNC post-processor configuration.
#[derive(Debug, Clone)]
pub struct LinuxCncPost {
    /// Number of decimal places for coordinates.
    pub precision: usize,
    /// Whether to use line numbers (N codes).
    pub use_line_numbers: bool,
    /// Line number increment.
    pub line_increment: u32,
    /// Whether to use modal feed rates (only emit F when changed).
    pub modal_feed: bool,
    /// Whether to include comments in output.
    pub include_comments: bool,
    /// O-word program number.
    pub program_number: u32,
    /// Whether to use tool length compensation (G43).
    pub use_tool_length_comp: bool,
}

impl Default for LinuxCncPost {
    fn default() -> Self {
        Self {
            precision: 4,
            use_line_numbers: true,
            line_increment: 10,
            modal_feed: true,
            include_comments: true,
            program_number: 1,
            use_tool_length_comp: true,
        }
    }
}

impl LinuxCncPost {
    /// Create a new LinuxCNC post-processor with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set coordinate precision.
    pub fn with_precision(mut self, precision: usize) -> Self {
        self.precision = precision;
        self
    }

    /// Disable line numbers.
    pub fn without_line_numbers(mut self) -> Self {
        self.use_line_numbers = false;
        self
    }

    /// Disable comments in output.
    pub fn without_comments(mut self) -> Self {
        self.include_comments = false;
        self
    }

    /// Set program number for O-word.
    pub fn with_program_number(mut self, number: u32) -> Self {
        self.program_number = number;
        self
    }

    /// Disable tool length compensation.
    pub fn without_tool_length_comp(mut self) -> Self {
        self.use_tool_length_comp = false;
        self
    }

    /// Format a line with optional line number.
    fn format_line(&self, line: &str, state: &mut PostState) -> String {
        if self.use_line_numbers {
            let n = state.line_number;
            state.line_number += self.line_increment;
            format!("N{} {}\n", n, line)
        } else {
            format!("{}\n", line)
        }
    }

    /// Format coordinate value.
    fn coord(&self, value: f64) -> String {
        format_coord(value, self.precision)
    }
}

impl PostProcessor for LinuxCncPost {
    /// LinuxCNC programs end with `M2`, not `M30`.
    fn default_end(&self) -> crate::ProgramEnd {
        crate::ProgramEnd::M2
    }

    fn preamble(&self, opts: &ProgramOptions, state: &mut PostState) -> String {
        let mut output = String::new();

        // O-word program number
        output.push_str(&format!("O{:04}\n", self.program_number));

        // Program header comment
        if self.include_comments {
            output.push_str(&format!("({})\n", opts.name));
            output.push_str("(vcad-kernel-cam: stock top Z0, XY lower-left at 0)\n");
        }

        // Safety block
        output.push_str("G21\n"); // Metric (mm)
        output.push_str("G90\n"); // Absolute positioning
        output.push_str("G94\n"); // Feed per minute
        output.push_str("G17\n"); // XY plane for arcs
        output.push_str("G40\n"); // Cancel cutter compensation
        output.push_str("G49\n"); // Cancel tool length compensation
        state.plane = Some(ArcPlane::Xy);

        // The work offset belongs to the job, not to this post.
        if let Some(wcs) = opts.wcs {
            output.push_str(&format!("{}\n", wcs.code()));
        }
        output.push_str("G80\n"); // Cancel canned cycles
        output.push_str("G64 P0.01\n"); // Path blending with tolerance

        // Move to safe height
        if let Some(park_z) = opts.park_z {
            output.push_str(&format!("G0 Z{}\n", self.coord(park_z)));
            state.z = park_z;
        }

        if let Some((rpm, dir)) = opts.spindle {
            output.push_str(&self.segment(&ToolpathSegment::Spindle { rpm, dir }, state));
        }

        output
    }

    fn tool_change(&self, tool: &ToolEntry, state: &mut PostState) -> String {
        let mut output = String::new();

        if self.include_comments {
            output.push_str(&format!(
                "(Tool {}: {} - {} mm)\n",
                tool.number,
                tool.name,
                tool.tool.diameter()
            ));
        }

        // Tool change
        output.push_str(&format!("T{} M6\n", tool.number));

        // Tool length compensation
        if self.use_tool_length_comp {
            output.push_str(&format!("G43 H{}\n", tool.number));
        }

        // Spindle on with tool's default RPM
        output.push_str(&format!("M3 S{:.0}\n", tool.default_rpm));

        state.tool_number = tool.number;
        state.spindle_rpm = tool.default_rpm;
        state.spindle_on = true;

        output
    }

    fn segment(&self, seg: &ToolpathSegment, state: &mut PostState) -> String {
        match seg {
            ToolpathSegment::Rapid { to } => {
                let mut parts = vec!["G0".to_string()];

                // Only output changed coordinates
                if (to[0] - state.x).abs() > 1e-6 {
                    parts.push(format!("X{}", self.coord(to[0])));
                }
                if (to[1] - state.y).abs() > 1e-6 {
                    parts.push(format!("Y{}", self.coord(to[1])));
                }
                if (to[2] - state.z).abs() > 1e-6 {
                    parts.push(format!("Z{}", self.coord(to[2])));
                }

                state.x = to[0];
                state.y = to[1];
                state.z = to[2];

                if parts.len() > 1 {
                    self.format_line(&parts.join(" "), state)
                } else {
                    String::new()
                }
            }

            ToolpathSegment::Linear { to, feed } => {
                let mut parts = vec!["G1".to_string()];

                if (to[0] - state.x).abs() > 1e-6 {
                    parts.push(format!("X{}", self.coord(to[0])));
                }
                if (to[1] - state.y).abs() > 1e-6 {
                    parts.push(format!("Y{}", self.coord(to[1])));
                }
                if (to[2] - state.z).abs() > 1e-6 {
                    parts.push(format!("Z{}", self.coord(to[2])));
                }

                // Feed rate (modal or always)
                if !self.modal_feed || (*feed - state.feed).abs() > 1e-6 {
                    parts.push(format!("F{:.1}", feed));
                    state.feed = *feed;
                }

                state.x = to[0];
                state.y = to[1];
                state.z = to[2];

                if parts.len() > 1 {
                    self.format_line(&parts.join(" "), state)
                } else {
                    String::new()
                }
            }

            ToolpathSegment::Arc {
                to,
                center,
                plane,
                dir,
                feed,
            } => {
                let arc_code = match dir {
                    ArcDir::Cw => "G2",
                    ArcDir::Ccw => "G3",
                };

                let mut output = String::new();

                // Say which plane, whenever it is not the plane the machine
                // is already in. Assuming G17 is how an XZ arc gets cut in XY.
                if state.plane != Some(*plane) {
                    let plane_code = match plane {
                        ArcPlane::Xy => "G17",
                        ArcPlane::Xz => "G18",
                        ArcPlane::Yz => "G19",
                    };
                    output.push_str(&self.format_line(plane_code, state));
                    state.plane = Some(*plane);
                }

                // LinuxCNC uses absolute IJ by default with G90.1
                // or incremental with G91.1. We use incremental (relative to start).
                let i = center[0];
                let j = center[1];

                let mut parts = vec![arc_code.to_string()];

                parts.push(format!("X{}", self.coord(to[0])));
                parts.push(format!("Y{}", self.coord(to[1])));

                if (to[2] - state.z).abs() > 1e-6 {
                    parts.push(format!("Z{}", self.coord(to[2])));
                }

                parts.push(format!("I{}", self.coord(i)));
                parts.push(format!("J{}", self.coord(j)));

                if !self.modal_feed || (*feed - state.feed).abs() > 1e-6 {
                    parts.push(format!("F{:.1}", feed));
                    state.feed = *feed;
                }

                state.x = to[0];
                state.y = to[1];
                state.z = to[2];

                output.push_str(&self.format_line(&parts.join(" "), state));
                output
            }

            ToolpathSegment::Dwell { seconds } => {
                // LinuxCNC uses seconds for dwell with P
                self.format_line(&format!("G4 P{:.3}", seconds), state)
            }

            ToolpathSegment::Spindle { rpm, dir } => {
                if *rpm <= 0.0 {
                    state.spindle_on = false;
                    self.format_line("M5", state)
                } else {
                    let m_code = match dir {
                        SpindleDir::Cw => "M3",
                        SpindleDir::Ccw => "M4",
                    };
                    state.spindle_on = true;
                    state.spindle_rpm = *rpm;
                    self.format_line(&format!("{} S{:.0}", m_code, rpm), state)
                }
            }

            ToolpathSegment::Coolant { mode } => {
                let code = match mode {
                    CoolantMode::Off => "M9",
                    CoolantMode::Mist => "M7",
                    CoolantMode::Flood => "M8",
                };
                self.format_line(code, state)
            }

            ToolpathSegment::ToolChange { tool_number } => {
                let mut output = String::new();
                output.push_str(&self.format_line(&format!("T{} M6", tool_number), state));
                if self.use_tool_length_comp {
                    output.push_str(&self.format_line(&format!("G43 H{}", tool_number), state));
                }
                state.tool_number = *tool_number;
                output
            }

            ToolpathSegment::Comment { text } => {
                if self.include_comments {
                    format!("({})\n", text)
                } else {
                    String::new()
                }
            }

            ToolpathSegment::Pause { message } => {
                // The reason before the stop, so the operator reads it while
                // the machine is still streaming towards the pause.
                let mut output = String::new();
                if let (Some(text), true) = (message, self.include_comments) {
                    output.push_str(&format!("({})\n", text));
                }
                output.push_str(&self.format_line("M0", state));
                output
            }

            ToolpathSegment::Raw { text } => {
                let text = text.trim_end();
                if text.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", text)
                }
            }
        }
    }

    fn postamble(&self, opts: &ProgramOptions, state: &mut PostState) -> String {
        let mut output = String::new();

        output.push_str("M5\n"); // Spindle off
        state.spindle_on = false;
        output.push_str("M9\n"); // Coolant off
        output.push_str("G49\n"); // Cancel tool length compensation
        if let Some(park_z) = opts.park_z {
            output.push_str(&format!("G0 Z{}\n", self.coord(park_z)));
            state.z = park_z;
        }
        if self.include_comments {
            output.push_str("(End of program)\n");
        }
        output.push_str(&format!("{}\n", opts.end.code()));

        output.push('%');

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CamSettings, Tool, Toolpath};

    fn create_test_toolpath() -> Toolpath {
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::comment("Test operation"));
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 5.0));
        tp.push(ToolpathSegment::linear(0.0, 0.0, -1.0, 300.0));
        tp.push(ToolpathSegment::linear(50.0, 0.0, -1.0, 1000.0));
        tp.push(ToolpathSegment::linear(50.0, 30.0, -1.0, 1000.0));
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 5.0));
        tp
    }

    #[test]
    fn test_linuxcnc_basic() {
        let post = LinuxCncPost::default();
        let tool = Tool::default_endmill();
        let settings = CamSettings::default();
        let toolpath = create_test_toolpath();

        let gcode = post.generate("test_job", &tool, &toolpath, &settings);

        // Check for expected G-codes
        assert!(gcode.contains("O0001")); // Program number
        assert!(gcode.contains("G21")); // Metric
        assert!(gcode.contains("G90")); // Absolute
        assert!(gcode.contains("G64")); // Path blending
        assert!(gcode.contains("G0")); // Rapid
        assert!(gcode.contains("G1")); // Linear
        assert!(gcode.contains("M3")); // Spindle on
        assert!(gcode.contains("M5")); // Spindle off
        assert!(gcode.contains("M2")); // Program end (not M30)
    }

    #[test]
    fn test_linuxcnc_line_numbers() {
        let post = LinuxCncPost::default();
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(10.0, 0.0, 0.0));
        tp.push(ToolpathSegment::rapid(20.0, 0.0, 0.0));

        let tool = Tool::default_endmill();
        let settings = CamSettings::default();

        let gcode = post.generate("test", &tool, &tp, &settings);

        // Should have line numbers by default
        assert!(gcode.contains("N0 ") || gcode.contains("N10 "));
    }

    #[test]
    fn test_linuxcnc_tool_change() {
        let post = LinuxCncPost::default();
        let tool_entry = ToolEntry::new(3, "10mm Endmill", Tool::default_endmill());
        let mut state = PostState::default();

        let output = post.tool_change(&tool_entry, &mut state);

        assert!(output.contains("T3 M6")); // Tool change
        assert!(output.contains("G43 H3")); // Tool length comp
        assert!(output.contains("M3 S")); // Spindle on
    }

    #[test]
    fn test_linuxcnc_dwell() {
        let post = LinuxCncPost::default();
        let mut state = PostState::default();

        let out = post.segment(&ToolpathSegment::dwell(1.5), &mut state);

        // LinuxCNC uses seconds, not milliseconds
        assert!(out.contains("G4 P1.5"));
    }

    #[test]
    fn test_linuxcnc_precision() {
        let post = LinuxCncPost::default(); // Default 4 decimal places
        let mut state = PostState::default();

        let out = post.segment(
            &ToolpathSegment::rapid(10.123456, 20.987654, 5.555555),
            &mut state,
        );

        // Should have 4 decimal places
        assert!(out.contains("X10.1235")); // Rounded
        assert!(out.contains("Y20.9877")); // Rounded
        assert!(out.contains("Z5.5556")); // Rounded
    }

    #[test]
    fn test_linuxcnc_program_number() {
        let post = LinuxCncPost::default().with_program_number(1234);
        let tool = Tool::default_endmill();
        let settings = CamSettings::default();
        let toolpath = create_test_toolpath();

        let gcode = post.generate("test", &tool, &toolpath, &settings);

        assert!(gcode.contains("O1234"));
    }

    #[test]
    fn test_linuxcnc_footer() {
        let post = LinuxCncPost::default();
        let state = PostState::default();

        let footer = post.footer(&state);

        assert!(footer.contains("M5")); // Spindle off
        assert!(footer.contains("M9")); // Coolant off
        assert!(footer.contains("G49")); // Cancel TLC
        assert!(footer.contains("M2")); // Program end
                                        // No return to machine zero: the postamble is shared with a whole
                                        // job, which has already parked over its last cut, and a traverse
                                        // after the spindle has stopped is the operator's move to make.
        assert!(!footer.contains("G53"));
        assert!(footer.contains('%')); // End character
    }
}
