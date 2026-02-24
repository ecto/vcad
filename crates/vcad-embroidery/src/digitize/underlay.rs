//! Underlay stitch generator.
//!
//! Produces a stabilization layer under the main fill, using the same tatami
//! algorithm but with looser spacing defaults.

use super::fill::{fill_stitch, FillParams};
use super::Path2D;
use crate::StitchCommand;

/// Parameters for the underlay stitch generator.
#[derive(Debug, Clone)]
pub struct UnderlayParams {
    /// Fill angle in degrees.
    pub angle: f64,
    /// Spacing between scan rows in mm.
    pub row_spacing: f64,
    /// Stitch length along each row in mm.
    pub stitch_length: f64,
}

impl Default for UnderlayParams {
    fn default() -> Self {
        Self {
            angle: 90.0,
            row_spacing: 2.0,
            stitch_length: 6.0,
        }
    }
}

/// Generate underlay stitches inside a closed polygon.
///
/// Uses the tatami fill algorithm with looser spacing for fabric stabilization.
/// The underlay is typically stitched before the main fill layer.
pub fn underlay_stitch(region: &Path2D, params: &UnderlayParams) -> Vec<StitchCommand> {
    let fill_params = FillParams {
        angle: params.angle,
        row_spacing: params.row_spacing,
        stitch_length: params.stitch_length,
        stagger: 0.0,
    };
    fill_stitch(region, &fill_params)
}
