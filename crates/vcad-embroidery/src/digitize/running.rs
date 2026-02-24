//! Running stitch generator.
//!
//! Walks a path placing stitches at regular arc-length intervals.

use super::Path2D;
use crate::StitchCommand;

/// Parameters for the running stitch generator.
#[derive(Debug, Clone)]
pub struct RunningStitchParams {
    /// Stitch length in mm.
    pub stitch_length: f64,
}

impl Default for RunningStitchParams {
    fn default() -> Self {
        Self {
            stitch_length: 2.5,
        }
    }
}

/// Generate a running stitch along a path.
///
/// Walks each segment accumulating arc length. When the accumulated distance
/// reaches `stitch_length`, a `StitchTo` command is emitted and the
/// accumulator resets. Closed paths stitch back to their starting point.
pub fn running_stitch(path: &Path2D, params: &RunningStitchParams) -> Vec<StitchCommand> {
    let pts = &path.points;
    if pts.is_empty() {
        return vec![];
    }

    let mut commands = vec![StitchCommand::MoveTo {
        x: pts[0].0,
        y: pts[0].1,
    }];

    // Build the full edge list (append closing edge if closed).
    let n = pts.len();
    let edge_count = if path.closed && n > 1 { n } else { n - 1 };
    if edge_count == 0 {
        return commands;
    }

    let mut accum = 0.0;

    for i in 0..edge_count {
        let start = pts[i % n];
        let end = pts[(i + 1) % n];
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len < 1e-12 {
            continue;
        }
        let ux = dx / seg_len;
        let uy = dy / seg_len;

        let mut remaining = seg_len;

        loop {
            let need = params.stitch_length - accum;
            if remaining < need {
                // Not enough left in this segment to complete a stitch.
                accum += remaining;
                break;
            }
            // Advance along the segment by `need`.
            remaining -= need;
            let px = end.0 - ux * remaining;
            let py = end.1 - uy * remaining;
            commands.push(StitchCommand::StitchTo { x: px, y: py });
            accum = 0.0;
        }
    }

    // If there is leftover accumulation, place a final stitch at the end point.
    let end_pt = if path.closed { pts[0] } else { pts[n - 1] };
    if accum > 1e-12 {
        commands.push(StitchCommand::StitchTo {
            x: end_pt.0,
            y: end_pt.1,
        });
    }

    commands
}
