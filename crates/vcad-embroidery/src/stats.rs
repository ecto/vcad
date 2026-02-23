//! Pattern statistics and analysis.

use crate::pattern::EmbPattern;
use crate::stitch::StitchCommand;
use serde::{Deserialize, Serialize};

/// Computed statistics for an embroidery pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternStats {
    /// Total number of stitches (needle penetrations).
    pub stitch_count: usize,
    /// Total number of jump stitches.
    pub jump_count: usize,
    /// Total number of trim commands.
    pub trim_count: usize,
    /// Number of color changes.
    pub color_changes: usize,
    /// Number of distinct thread colors used.
    pub color_count: usize,
    /// Bounding box min corner (x, y) in mm.
    pub bounds_min: (f64, f64),
    /// Bounding box max corner (x, y) in mm.
    pub bounds_max: (f64, f64),
    /// Pattern width in mm.
    pub width: f64,
    /// Pattern height in mm.
    pub height: f64,
    /// Total thread length in mm (sum of stitch distances).
    pub thread_length: f64,
    /// Estimated stitching time in seconds (at ~800 stitches/min).
    pub estimated_time_seconds: f64,
}

/// Compute statistics for a pattern.
pub fn compute_stats(pattern: &EmbPattern) -> PatternStats {
    let mut stitch_count = 0usize;
    let mut jump_count = 0usize;
    let mut trim_count = 0usize;
    let mut color_changes = 0usize;

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    let mut thread_length = 0.0f64;
    let mut last_x = 0.0f64;
    let mut last_y = 0.0f64;

    for group in &pattern.stitch_groups {
        for cmd in &group.commands {
            match *cmd {
                StitchCommand::StitchTo { x, y } => {
                    stitch_count += 1;
                    let dx = x - last_x;
                    let dy = y - last_y;
                    thread_length += (dx * dx + dy * dy).sqrt();
                    last_x = x;
                    last_y = y;
                    update_bounds(x, y, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                }
                StitchCommand::MoveTo { x, y } => {
                    last_x = x;
                    last_y = y;
                    update_bounds(x, y, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                }
                StitchCommand::Jump { x, y } => {
                    jump_count += 1;
                    last_x = x;
                    last_y = y;
                    update_bounds(x, y, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                }
                StitchCommand::Trim => trim_count += 1,
                StitchCommand::ColorChange { .. } => color_changes += 1,
                StitchCommand::Stop | StitchCommand::End => {}
            }
        }
    }

    // Handle empty pattern
    if min_x == f64::INFINITY {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 0.0;
        max_y = 0.0;
    }

    let width = max_x - min_x;
    let height = max_y - min_y;

    // Typical embroidery speed: ~800 stitches per minute
    let estimated_time_seconds = stitch_count as f64 / 800.0 * 60.0;

    PatternStats {
        stitch_count,
        jump_count,
        trim_count,
        color_changes,
        color_count: pattern.threads.len(),
        bounds_min: (min_x, min_y),
        bounds_max: (max_x, max_y),
        width,
        height,
        thread_length,
        estimated_time_seconds,
    }
}

fn update_bounds(
    x: f64,
    y: f64,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    if x < *min_x {
        *min_x = x;
    }
    if y < *min_y {
        *min_y = y;
    }
    if x > *max_x {
        *max_x = x;
    }
    if y > *max_y {
        *max_y = y;
    }
}
