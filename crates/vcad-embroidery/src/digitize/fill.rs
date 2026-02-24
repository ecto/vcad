//! Tatami fill stitch generator.
//!
//! Fills a closed polygon region with scan-line rows of stitches, producing
//! the classic tatami texture used for large area fills.

use super::Path2D;
use crate::StitchCommand;

/// Parameters for the tatami fill generator.
#[derive(Debug, Clone)]
pub struct FillParams {
    /// Fill angle in degrees (0 = horizontal scan lines).
    pub angle: f64,
    /// Spacing between scan rows in mm.
    pub row_spacing: f64,
    /// Stitch length along each row in mm.
    pub stitch_length: f64,
    /// Row-to-row offset fraction (0..1) for stagger.
    pub stagger: f64,
}

impl Default for FillParams {
    fn default() -> Self {
        Self {
            angle: 0.0,
            row_spacing: 0.4,
            stitch_length: 3.0,
            stagger: 0.25,
        }
    }
}

/// Rotate a point by `angle` radians around the origin.
fn rotate(x: f64, y: f64, cos_a: f64, sin_a: f64) -> (f64, f64) {
    (x * cos_a - y * sin_a, x * sin_a + y * cos_a)
}

/// Find x-coordinates where horizontal line `y = scan_y` intersects the
/// polygon edges. Returns a sorted list of intersection x-values.
fn scan_intersections(rotated: &[(f64, f64)], scan_y: f64) -> Vec<f64> {
    let n = rotated.len();
    let mut xs = Vec::new();
    for i in 0..n {
        let (x0, y0) = rotated[i];
        let (x1, y1) = rotated[(i + 1) % n];

        // Skip horizontal edges.
        if (y1 - y0).abs() < 1e-12 {
            continue;
        }

        // Check if scan_y is within the edge's y-range (exclusive of top vertex
        // to avoid double-counting at polygon vertices).
        let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        if scan_y < lo || scan_y >= hi {
            continue;
        }

        let t = (scan_y - y0) / (y1 - y0);
        let ix = x0 + t * (x1 - x0);
        xs.push(ix);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs
}

/// Generate tatami fill stitches inside a closed polygon.
///
/// The algorithm rotates the polygon by `-angle`, scans horizontally across
/// the bounding box at `row_spacing` intervals, finds entry/exit pairs, fills
/// each segment with stitches, then rotates all coordinates back.
///
/// Returns an empty list if the region is not closed or has fewer than 3 points.
pub fn fill_stitch(region: &Path2D, params: &FillParams) -> Vec<StitchCommand> {
    let pts = &region.points;
    if !region.closed || pts.len() < 3 {
        return vec![];
    }

    let angle_rad = params.angle.to_radians();
    let cos_neg = angle_rad.cos();
    let sin_neg = -angle_rad.sin();

    // Rotate all polygon points by -angle.
    let rotated: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| rotate(x, y, cos_neg, sin_neg)).collect();

    // Bounding box.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in &rotated {
        if x < min_x { min_x = x; }
        if y < min_y { min_y = y; }
        if x > max_x { max_x = x; }
        if y > max_y { max_y = y; }
    }

    let cos_pos = angle_rad.cos();
    let sin_pos = angle_rad.sin();

    let mut commands = Vec::new();
    let mut row_idx = 0usize;
    let mut scan_y = min_y + params.row_spacing / 2.0;

    while scan_y < max_y {
        let xs = scan_intersections(&rotated, scan_y);

        // Process pairs (entry, exit).
        let mut pair_idx = 0;
        while pair_idx + 1 < xs.len() {
            let x_enter = xs[pair_idx];
            let x_exit = xs[pair_idx + 1];
            pair_idx += 2;

            let seg_len = x_exit - x_enter;
            if seg_len < 1e-12 {
                continue;
            }

            // Stagger: shift the stitch pattern along the row.
            let stagger_offset = (row_idx as f64) * params.stagger * params.stitch_length;

            // Generate stitches along this segment.
            let n_stitches = (seg_len / params.stitch_length).ceil() as usize;
            let actual_step = seg_len / n_stitches as f64;

            for j in 0..=n_stitches {
                let raw_x = x_enter + j as f64 * actual_step + stagger_offset;
                // Clamp to segment bounds.
                let sx = raw_x.max(x_enter).min(x_exit);
                let sy = scan_y;

                // Rotate back.
                let (fx, fy) = rotate(sx, sy, cos_pos, sin_pos);

                if commands.is_empty() {
                    commands.push(StitchCommand::MoveTo { x: fx, y: fy });
                } else {
                    commands.push(StitchCommand::StitchTo { x: fx, y: fy });
                }
            }
        }

        scan_y += params.row_spacing;
        row_idx += 1;
    }

    commands
}
