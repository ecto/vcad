//! 3D roughing operation using drop-cutter algorithm.
//!
//! Generates parallel raster toolpaths at multiple Z levels, following
//! the part surface with a specified stock margin.

use crate::dropcutter::HeightField;
use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
use serde::{Deserialize, Serialize};

/// 3D roughing operation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roughing3D {
    /// Stepover distance between passes (mm). Overrides settings if set.
    pub stepover: Option<f64>,
    /// Stepdown distance between Z levels (mm). Overrides settings if set.
    pub stepdown: Option<f64>,
    /// Raster angle in degrees (0 = along X axis, 90 = along Y axis).
    pub direction: f64,
    /// Extra material to leave around the part (mm).
    pub stock_margin: f64,
    /// Target Z depth (bottom of roughing).
    pub target_z: f64,
    /// Top Z (typically stock top or 0).
    pub top_z: f64,
}

impl Default for Roughing3D {
    fn default() -> Self {
        Self {
            stepover: None,
            stepdown: None,
            direction: 0.0,
            stock_margin: 0.5,
            target_z: -10.0,
            top_z: 0.0,
        }
    }
}

impl Roughing3D {
    /// Create a new roughing operation.
    pub fn new(target_z: f64, top_z: f64) -> Self {
        Self {
            target_z,
            top_z,
            ..Default::default()
        }
    }

    /// Set stock margin.
    pub fn with_margin(mut self, margin: f64) -> Self {
        self.stock_margin = margin;
        self
    }

    /// Set raster direction in degrees.
    pub fn with_direction(mut self, degrees: f64) -> Self {
        self.direction = degrees;
        self
    }

    /// Set stepover (overrides settings).
    pub fn with_stepover(mut self, stepover: f64) -> Self {
        self.stepover = Some(stepover);
        self
    }

    /// Set stepdown (overrides settings).
    pub fn with_stepdown(mut self, stepdown: f64) -> Self {
        self.stepdown = Some(stepdown);
        self
    }

    /// Generate toolpath from a height field.
    ///
    /// The height field contains the minimum Z values at each sample point
    /// (from drop-cutter). The roughing operation generates parallel passes
    /// at multiple Z levels.
    pub fn generate(
        &self,
        height_field: &HeightField,
        _tool: &Tool,
        settings: &CamSettings,
    ) -> Result<Toolpath, CamError> {
        let stepover = self.stepover.unwrap_or(settings.stepover);
        let stepdown = self.stepdown.unwrap_or(settings.stepdown);

        if stepover <= 0.0 {
            return Err(CamError::InvalidStepover(stepover));
        }
        if stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(stepdown));
        }

        let mut toolpath = Toolpath::new();

        // Add spindle on
        toolpath.push(ToolpathSegment::Spindle {
            rpm: settings.spindle_rpm,
            dir: crate::SpindleDir::Cw,
        });

        // Compute Z levels
        let mut z_levels = Vec::new();
        let mut z = self.top_z - stepdown;
        while z > self.target_z {
            z_levels.push(z);
            z -= stepdown;
        }
        z_levels.push(self.target_z);

        // For each Z level, generate raster passes
        let is_along_x =
            (self.direction.abs() % 180.0) < 45.0 || (self.direction.abs() % 180.0) > 135.0;

        for (level_idx, &z_level) in z_levels.iter().enumerate() {
            let passes = self.generate_level_passes(
                height_field,
                z_level,
                stepover,
                is_along_x,
                level_idx % 2 == 1, // Alternate direction for efficiency
                settings,
            );

            for pass in passes {
                // Rapid to start
                let start = pass.first().ok_or(CamError::EmptyContour)?;
                toolpath.push(ToolpathSegment::Rapid {
                    to: [start[0], start[1], settings.safe_z],
                });
                toolpath.push(ToolpathSegment::Rapid {
                    to: [start[0], start[1], start[2].max(z_level) + 2.0],
                });

                // Plunge to first point
                toolpath.push(ToolpathSegment::Linear {
                    to: *start,
                    feed: settings.plunge_rate,
                });

                // Cut along pass
                for point in pass.iter().skip(1) {
                    toolpath.push(ToolpathSegment::Linear {
                        to: *point,
                        feed: settings.feed_rate,
                    });
                }
            }
        }

        // Retract at end
        if let Some(ToolpathSegment::Linear { to, .. }) = toolpath.segments.last() {
            toolpath.push(ToolpathSegment::Rapid {
                to: [to[0], to[1], settings.safe_z],
            });
        }

        Ok(toolpath)
    }

    /// Generate passes for a single Z level.
    fn generate_level_passes(
        &self,
        hf: &HeightField,
        z_level: f64,
        stepover: f64,
        along_x: bool,
        reverse: bool,
        _settings: &CamSettings,
    ) -> Vec<Vec<[f64; 3]>> {
        let mut passes = Vec::new();

        if along_x {
            // Passes along X, stepping in Y
            let y_start = hf.bounds[1];
            let y_end = hf.bounds[3];
            let x_start = hf.bounds[0];
            let x_end = hf.bounds[2];

            let mut y = y_start;
            let mut pass_idx = 0;
            while y <= y_end {
                let mut pass = Vec::new();

                // Sample along X
                let x_iter: Box<dyn Iterator<Item = f64>> = if (pass_idx % 2 == 0) != reverse {
                    Box::new(std::iter::successors(Some(x_start), move |&x| {
                        let next = x + hf.dx();
                        if next <= x_end + 1e-6 {
                            Some(next)
                        } else {
                            None
                        }
                    }))
                } else {
                    let dx = hf.dx();
                    Box::new(std::iter::successors(Some(x_end), move |&x| {
                        let next = x - dx;
                        if next >= x_start - 1e-6 {
                            Some(next)
                        } else {
                            None
                        }
                    }))
                };

                for x in x_iter {
                    if let Some(surface_z) = hf.interpolate(x, y) {
                        // Cut at either the Z level or above the surface (with margin)
                        let cut_z = z_level.max(surface_z + self.stock_margin);
                        pass.push([x, y, cut_z]);
                    }
                }

                // Only add non-empty passes
                if pass.len() >= 2 {
                    // Simplify pass to remove redundant points
                    let simplified = simplify_pass(&pass, 0.01);
                    if simplified.len() >= 2 {
                        passes.push(simplified);
                    }
                }

                y += stepover;
                pass_idx += 1;
            }
        } else {
            // Passes along Y, stepping in X
            let x_start = hf.bounds[0];
            let x_end = hf.bounds[2];
            let y_start = hf.bounds[1];
            let y_end = hf.bounds[3];

            let mut x = x_start;
            let mut pass_idx = 0;
            while x <= x_end {
                let mut pass = Vec::new();

                let y_iter: Box<dyn Iterator<Item = f64>> = if (pass_idx % 2 == 0) != reverse {
                    Box::new(std::iter::successors(Some(y_start), move |&y| {
                        let next = y + hf.dy();
                        if next <= y_end + 1e-6 {
                            Some(next)
                        } else {
                            None
                        }
                    }))
                } else {
                    let dy = hf.dy();
                    Box::new(std::iter::successors(Some(y_end), move |&y| {
                        let next = y - dy;
                        if next >= y_start - 1e-6 {
                            Some(next)
                        } else {
                            None
                        }
                    }))
                };

                for y in y_iter {
                    if let Some(surface_z) = hf.interpolate(x, y) {
                        let cut_z = z_level.max(surface_z + self.stock_margin);
                        pass.push([x, y, cut_z]);
                    }
                }

                if pass.len() >= 2 {
                    let simplified = simplify_pass(&pass, 0.01);
                    if simplified.len() >= 2 {
                        passes.push(simplified);
                    }
                }

                x += stepover;
                pass_idx += 1;
            }
        }

        passes
    }
}

/// Simplify a pass by removing points that are colinear (in Z).
fn simplify_pass(points: &[[f64; 3]], tolerance: f64) -> Vec<[f64; 3]> {
    if points.len() <= 2 {
        return points.to_vec();
    }

    let mut result = Vec::with_capacity(points.len());
    result.push(points[0]);

    for i in 1..points.len() - 1 {
        let prev = result.last().unwrap();
        let curr = &points[i];
        let next = &points[i + 1];

        // Check if curr is on the line from prev to next (in Z)
        let dx = next[0] - prev[0];
        let dy = next[1] - prev[1];
        let len = (dx * dx + dy * dy).sqrt();

        if len < 1e-10 {
            result.push(*curr);
            continue;
        }

        // Parameter along line
        let t = ((curr[0] - prev[0]) * dx + (curr[1] - prev[1]) * dy) / (len * len);
        let expected_z = prev[2] + t * (next[2] - prev[2]);

        if (curr[2] - expected_z).abs() > tolerance {
            result.push(*curr);
        }
    }

    result.push(*points.last().unwrap());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_height_field() -> HeightField {
        // 11x11 height field from 0-100mm in X and Y
        let mut hf = HeightField::new(11, 11, [0.0, 0.0, 100.0, 100.0], 0.0);

        // Create a dome shape: z = -sqrt(50^2 - (x-50)^2 - (y-50)^2) / 5
        for iy in 0..11 {
            for ix in 0..11 {
                let (x, y) = hf.xy_at(ix, iy);
                let dx = x - 50.0;
                let dy = y - 50.0;
                let r2 = dx * dx + dy * dy;
                let z = if r2 < 50.0 * 50.0 {
                    (50.0 * 50.0 - r2).sqrt() / 5.0
                } else {
                    0.0
                };
                hf.set(ix, iy, z);
            }
        }

        hf
    }

    #[test]
    fn test_roughing3d_basic() {
        let hf = make_test_height_field();
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepover: 5.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 15.0,
            retract_z: 20.0,
        };

        let op = Roughing3D::new(-5.0, 12.0).with_margin(0.5);
        let toolpath = op.generate(&hf, &tool, &settings).unwrap();

        assert!(!toolpath.is_empty());
        assert!(toolpath.len() > 10); // Should have many segments
    }

    #[test]
    fn test_roughing3d_along_y() {
        let hf = make_test_height_field();
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings::default();

        let op = Roughing3D::new(-5.0, 12.0).with_direction(90.0);
        let toolpath = op.generate(&hf, &tool, &settings).unwrap();

        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_simplify_pass() {
        // Flat pass should simplify to just start and end
        let pass = vec![
            [0.0, 0.0, 5.0],
            [10.0, 0.0, 5.0],
            [20.0, 0.0, 5.0],
            [30.0, 0.0, 5.0],
        ];
        let simplified = simplify_pass(&pass, 0.01);
        assert_eq!(simplified.len(), 2);

        // Pass with varying Z should keep intermediate points
        let pass = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 5.0], [20.0, 0.0, 0.0]];
        let simplified = simplify_pass(&pass, 0.01);
        assert_eq!(simplified.len(), 3);
    }
}
