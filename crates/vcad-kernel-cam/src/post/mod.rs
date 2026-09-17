//! Post-processors for converting toolpaths to machine-specific G-code.

mod grbl;
mod linuxcnc;

pub use grbl::GrblPost;
pub use linuxcnc::LinuxCncPost;

use crate::{CamSettings, ProgramEnd, SpindleDir, Tool, ToolEntry, Toolpath, ToolpathSegment, Wcs};

/// Everything a post needs to open and close a program that is not a segment.
///
/// The work offset and the spindle live here rather than inside the post,
/// because they are the *job's* choices. A post that writes `G54` of its own
/// accord contradicts a job that asked for `G55`, and a post that starts the
/// spindle contradicts a job that starts one per tool with a spin-up dwell —
/// both of which this crate shipped before wave 2.
#[derive(Debug, Clone)]
pub struct ProgramOptions {
    /// Program name, written as the first comment.
    pub name: String,
    /// Work coordinate system to select, or `None` to leave the machine's
    /// alone.
    pub wcs: Option<Wcs>,
    /// Height to retract to at the start and end, in mm. `None` leaves Z
    /// where it is, for a program that opens with its own move.
    pub park_z: Option<f64>,
    /// How the program ends.
    pub end: ProgramEnd,
    /// Spindle to start in the preamble, for a lone toolpath that does not
    /// start one itself. A job leaves this `None`.
    pub spindle: Option<(f64, SpindleDir)>,
}

impl ProgramOptions {
    /// Options for a lone toolpath: `G54`, park at the retract height, start
    /// the spindle the settings ask for, end with `M30`.
    ///
    /// This is what [`PostProcessor::generate`] uses, and it is the historic
    /// behaviour of `header` + `M3` + `footer`.
    pub fn standalone(name: impl Into<String>, settings: &CamSettings) -> Self {
        Self {
            name: name.into(),
            wcs: Some(Wcs::G54),
            park_z: Some(settings.retract_z),
            end: ProgramEnd::M30,
            spindle: Some((settings.spindle_rpm, SpindleDir::Cw)),
        }
    }
}

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
    /// Plane the machine is in (`G17`/`G18`/`G19`), once one has been
    /// commanded. `None` means nothing has said yet, so the next arc must.
    pub plane: Option<crate::ArcPlane>,
}

/// Trait for post-processors that convert toolpaths to G-code.
pub trait PostProcessor {
    /// Everything before the first segment: the safety block, the work
    /// offset the caller asked for, the park move, and any spindle the
    /// caller asked to be started.
    fn preamble(&self, opts: &ProgramOptions, state: &mut PostState) -> String;

    /// Generate G-code for a tool change.
    fn tool_change(&self, tool: &ToolEntry, state: &mut PostState) -> String;

    /// Generate G-code for a single toolpath segment.
    fn segment(&self, seg: &ToolpathSegment, state: &mut PostState) -> String;

    /// Everything after the last segment: spindle and coolant off, retract,
    /// and the end word the caller asked for.
    fn postamble(&self, opts: &ProgramOptions, state: &mut PostState) -> String;

    /// The end word this machine's dialect uses when the caller has not
    /// asked for one: `M30` unless a post says otherwise.
    fn default_end(&self) -> ProgramEnd {
        ProgramEnd::M30
    }

    /// Post a whole program.
    fn program(&self, opts: &ProgramOptions, toolpath: &Toolpath) -> String {
        let mut output = String::new();
        let mut state = PostState::default();
        output.push_str(&self.preamble(opts, &mut state));
        for seg in &toolpath.segments {
            output.push_str(&self.segment(seg, &mut state));
        }
        output.push_str(&self.postamble(opts, &mut state));
        output
    }

    /// Generate the G-code header (units, modes, etc.) for a lone toolpath.
    fn header(&self, job_name: &str, settings: &CamSettings) -> String {
        let mut opts = ProgramOptions::standalone(job_name, settings);
        opts.spindle = None; // `generate` used to write the M3 itself.
        opts.end = self.default_end();
        self.preamble(&opts, &mut PostState::default())
    }

    /// Generate the G-code footer (program end) for a lone toolpath.
    fn footer(&self, state: &PostState) -> String {
        let mut opts = ProgramOptions::standalone("", &CamSettings::default());
        opts.end = self.default_end();
        let mut state = state.clone();
        self.postamble(&opts, &mut state)
    }

    /// Generate complete G-code for a lone toolpath, with one tool and one
    /// spindle start. A multi-tool program goes through
    /// [`Program::to_gcode`](crate::Program::to_gcode) instead.
    fn generate(
        &self,
        job_name: &str,
        _tool: &Tool,
        toolpath: &Toolpath,
        settings: &CamSettings,
    ) -> String {
        let mut opts = ProgramOptions::standalone(job_name, settings);
        opts.end = self.default_end();
        self.program(&opts, toolpath)
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
