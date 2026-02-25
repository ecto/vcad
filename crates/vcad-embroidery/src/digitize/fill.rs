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

/// Append x-coordinates where horizontal line `y = scan_y` intersects the
/// polygon edges to the given output vector.
fn scan_intersections_into(rotated: &[(f64, f64)], scan_y: f64, xs: &mut Vec<f64>) {
    let n = rotated.len();
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
}

/// Generate tatami fill stitches inside a closed polygon.
///
/// The algorithm rotates the polygon by `-angle`, scans horizontally across
/// the bounding box at `row_spacing` intervals, finds entry/exit pairs, fills
/// each segment with stitches, then rotates all coordinates back.
///
/// Returns an empty list if the region is not closed or has fewer than 3 points.
pub fn fill_stitch(region: &Path2D, params: &FillParams) -> Vec<StitchCommand> {
    fill_stitch_multi(std::slice::from_ref(region), params)
}

/// Generate tatami fill stitches across multiple contours using even-odd rule.
///
/// All contours are scan-line intersected together so that inner contours
/// (holes) are automatically subtracted from outer contours. This is
/// essential for glyphs like "e", "o", "d" that have holes.
pub fn fill_stitch_multi(regions: &[Path2D], params: &FillParams) -> Vec<StitchCommand> {
    // Collect all closed contours with >= 3 points.
    let contours: Vec<&Path2D> = regions
        .iter()
        .filter(|r| r.closed && r.points.len() >= 3)
        .collect();
    if contours.is_empty() {
        return vec![];
    }

    let angle_rad = params.angle.to_radians();
    let cos_neg = angle_rad.cos();
    let sin_neg = -angle_rad.sin();

    // Rotate all contour points by -angle.
    let rotated_contours: Vec<Vec<(f64, f64)>> = contours
        .iter()
        .map(|c| {
            c.points
                .iter()
                .map(|&(x, y)| rotate(x, y, cos_neg, sin_neg))
                .collect()
        })
        .collect();

    // Global bounding box.
    let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
    for contour in &rotated_contours {
        for &(_x, y) in contour {
            if y < min_y { min_y = y; }
            if y > max_y { max_y = y; }
        }
    }

    let cos_pos = angle_rad.cos();
    let sin_pos = angle_rad.sin();

    let mut commands = Vec::new();
    let mut row_idx = 0usize;
    let mut scan_y = min_y + params.row_spacing / 2.0;

    while scan_y < max_y {
        // Collect intersections from ALL contours.
        let mut xs = Vec::new();
        for contour in &rotated_contours {
            scan_intersections_into(contour, scan_y, &mut xs);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Process pairs (even-odd rule: entry at even index, exit at odd).
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
