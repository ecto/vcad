//! Satin stitch generator.
//!
//! Produces a zigzag perpendicular to a path centerline, commonly used for
//! lettering and narrow column fills.

use super::Path2D;
use crate::StitchCommand;

/// Parameters for the satin stitch generator.
#[derive(Debug, Clone)]
pub struct SatinParams {
    /// Column width in mm.
    pub width: f64,
    /// Stitch density in stitches per mm along the path.
    pub density: f64,
    /// Outward offset in mm to compensate for thread pull-in.
    pub pull_compensation: f64,
}

impl Default for SatinParams {
    fn default() -> Self {
        Self {
            width: 3.0,
            density: 4.0,
            pull_compensation: 0.0,
        }
    }
}

/// Sample a point and tangent at a given arc-length `t` along `pts`.
///
/// Returns `(point, tangent_unit_vector)`. If the path has zero length,
/// returns the first point with a zero tangent.
fn sample_at(pts: &[(f64, f64)], t: f64) -> ((f64, f64), (f64, f64)) {
    let mut accum = 0.0;
    for i in 0..pts.len() - 1 {
        let dx = pts[i + 1].0 - pts[i].0;
        let dy = pts[i + 1].1 - pts[i].1;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len < 1e-12 {
            continue;
        }
        if accum + seg_len >= t - 1e-12 {
            let frac = ((t - accum) / seg_len).clamp(0.0, 1.0);
            let px = pts[i].0 + dx * frac;
            let py = pts[i].1 + dy * frac;
            let ux = dx / seg_len;
            let uy = dy / seg_len;
            return ((px, py), (ux, uy));
        }
        accum += seg_len;
    }
    // Past the end -- clamp to last point.
    let last = pts[pts.len() - 1];
    if pts.len() >= 2 {
        let prev = pts[pts.len() - 2];
        let dx = last.0 - prev.0;
        let dy = last.1 - prev.1;
        let len = (dx * dx + dy * dy).sqrt();
        if len > 1e-12 {
            return (last, (dx / len, dy / len));
        }
    }
    (last, (0.0, 0.0))
}

/// Compute total arc length of a polyline.
fn arc_length(pts: &[(f64, f64)]) -> f64 {
    pts.windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Generate satin stitches along a path.
///
/// Samples the path at intervals of `1 / density` mm. At each sample point
/// the perpendicular direction is computed, and a stitch is placed alternately
/// on the left and right side of the centerline at `width/2 + pull_compensation`.
pub fn satin_stitch(path: &Path2D, params: &SatinParams) -> Vec<StitchCommand> {
    let pts = &path.points;
    if pts.len() < 2 {
        return vec![];
    }

    let total = arc_length(pts);
    if total < 1e-12 {
        return vec![];
    }

    let step = 1.0 / params.density;
    let half_w = params.width / 2.0 + params.pull_compensation;
    let n_samples = (total / step).floor() as usize + 1;

    let mut commands = Vec::with_capacity(n_samples + 1);
    let mut left = true;

    for i in 0..n_samples {
        let t = (i as f64) * step;
        let ((cx, cy), (tx, ty)) = sample_at(pts, t);
        // Perpendicular: rotate tangent 90 degrees.
        let (nx, ny) = (-ty, tx);
        let sign = if left { 1.0 } else { -1.0 };
        let sx = cx + nx * half_w * sign;
        let sy = cy + ny * half_w * sign;

        if i == 0 {
            commands.push(StitchCommand::MoveTo { x: sx, y: sy });
        } else {
            commands.push(StitchCommand::StitchTo { x: sx, y: sy });
        }
        left = !left;
    }

    commands
}
