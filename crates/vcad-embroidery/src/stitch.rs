//! Stitch commands and stitch groups.

use serde::{Deserialize, Serialize};

/// A single stitch command. All coordinates are absolute, in millimeters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StitchCommand {
    /// Move to an absolute position (needle up).
    MoveTo {
        /// X coordinate in mm.
        x: f64,
        /// Y coordinate in mm.
        y: f64,
    },
    /// Stitch to an absolute position (needle down + move).
    StitchTo {
        /// X coordinate in mm.
        x: f64,
        /// Y coordinate in mm.
        y: f64,
    },
    /// Jump to position without stitching (long move, needle up).
    Jump {
        /// X coordinate in mm.
        x: f64,
        /// Y coordinate in mm.
        y: f64,
    },
    /// Cut the thread.
    Trim,
    /// Switch to thread at the given index.
    ColorChange {
        /// Index into the pattern's thread palette.
        index: u32,
    },
    /// Pause for operator intervention.
    Stop,
    /// End of pattern.
    End,
}

/// A group of stitches sharing a single thread color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StitchGroup {
    /// Index into the pattern's thread palette.
    pub thread_index: usize,
    /// Stitch commands for this group.
    pub commands: Vec<StitchCommand>,
}

impl StitchGroup {
    /// Count only StitchTo commands (actual needle penetrations).
    pub fn stitch_count(&self) -> usize {
        self.commands
            .iter()
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count()
    }
}
