//! Swept volume subtraction for toolpath simulation.

use vcad_kernel_cam::{Tool, Toolpath, ToolpathSegment};

use crate::Stock;

/// Cutting length assumed above the tip (or cone shoulder) for tools that
/// don't report a flute length: drills, V-bits, and face mills.
const DEFAULT_CUT_LENGTH: f64 = 50.0;

/// A swept volume representing tool motion.
pub struct SweptVolume {
    /// Tool radius.
    radius: f64,
    /// Corner radius (for bull endmills).
    corner_radius: f64,
}

impl SweptVolume {
    /// Create a swept volume for a tool.
    pub fn from_tool(tool: &Tool) -> Self {
        Self {
            radius: tool.radius(),
            corner_radius: tool.corner_radius(),
        }
    }

    /// Get the effective radius at a given height from the tool tip.
    pub fn radius_at_height(&self, height: f64) -> f64 {
        if self.corner_radius <= 0.0 || height >= self.corner_radius {
            // Above corner radius or flat endmill
            self.radius
        } else {
            // In corner region (for ball or bull endmill)
            let inner_radius = self.radius - self.corner_radius;
            let dz = self.corner_radius - height;
            let dr = (self.corner_radius * self.corner_radius - dz * dz).sqrt();
            inner_radius + dr
        }
    }
}

/// SDF of a tool's cutter envelope with the tip at the origin, axis +Z.
///
/// Each variant is a 1-Lipschitz signed distance (or a conservative lower
/// bound of one), as required by [`Stock::subtract_sdf`].
enum ToolStamp {
    /// Flat-bottomed cylinder (flat endmill, face mill).
    Flat {
        /// Cutter radius.
        radius: f64,
        /// Cutting length above the tip.
        height: f64,
    },
    /// Hemispherical tip blended into a cylinder (ball endmill).
    Ball {
        /// Cutter radius.
        radius: f64,
        /// Cutting length above the tip.
        height: f64,
    },
    /// Flat bottom with a rounded outer corner (bull endmill).
    Bull {
        /// Cutter radius.
        radius: f64,
        /// Corner radius, strictly between 0 and `radius`.
        corner: f64,
        /// Cutting length above the tip.
        height: f64,
    },
    /// Cone tip opening into a cylinder (V-bit, drill).
    Cone {
        /// Cutter radius at the shoulder.
        radius: f64,
        /// Angle between the cone side and the tool axis, radians.
        half_angle: f64,
        /// Cutting length above the tip.
        height: f64,
    },
}

impl ToolStamp {
    fn from_tool(tool: &Tool) -> Self {
        let radius = tool.radius();
        match tool {
            Tool::FlatEndMill { flute_length, .. } => ToolStamp::Flat {
                radius,
                height: *flute_length,
            },
            Tool::BallEndMill { flute_length, .. } => ToolStamp::Ball {
                radius,
                height: *flute_length,
            },
            Tool::BullEndMill {
                corner_radius,
                flute_length,
                ..
            } => {
                let corner = corner_radius.clamp(0.0, radius);
                if corner <= 0.0 {
                    ToolStamp::Flat {
                        radius,
                        height: *flute_length,
                    }
                } else if (radius - corner) < 1e-9 {
                    ToolStamp::Ball {
                        radius,
                        height: *flute_length,
                    }
                } else {
                    ToolStamp::Bull {
                        radius,
                        corner,
                        height: *flute_length,
                    }
                }
            }
            Tool::VBit { angle, .. } => {
                let half_angle = (angle.to_radians() / 2.0).clamp(0.02, 1.55);
                ToolStamp::Cone {
                    radius,
                    half_angle,
                    height: radius / half_angle.tan() + DEFAULT_CUT_LENGTH,
                }
            }
            Tool::Drill { point_angle, .. } => {
                let half_angle = (point_angle.to_radians() / 2.0).clamp(0.02, 1.55);
                ToolStamp::Cone {
                    radius,
                    half_angle,
                    height: radius / half_angle.tan() + DEFAULT_CUT_LENGTH,
                }
            }
            Tool::FaceMill { .. } => ToolStamp::Flat {
                radius,
                height: DEFAULT_CUT_LENGTH,
            },
        }
    }

    /// Evaluate the stamp SDF at world point `p` for a tool tip at `tip`.
    fn sdf(&self, p: [f64; 3], tip: [f64; 3]) -> f64 {
        let dx = p[0] - tip[0];
        let dy = p[1] - tip[1];
        let z = p[2] - tip[2];
        let q = (dx * dx + dy * dy).sqrt();
        match *self {
            ToolStamp::Flat { radius, height } => flat_cylinder_sdf(q, z, radius, height),
            ToolStamp::Ball { radius, height } => {
                let sphere = (q * q + (z - radius).powi(2)).sqrt() - radius;
                let barrel = flat_cylinder_sdf(q, z - radius, radius, (height - radius).max(0.0));
                sphere.min(barrel)
            }
            ToolStamp::Bull {
                radius,
                corner,
                height,
            } => {
                let uq = q - (radius - corner);
                let uz = z - corner;
                if uq > 0.0 && uz < 0.0 {
                    // Rounded outer corner: distance to the torus arc.
                    (uq * uq + uz * uz).sqrt() - corner
                } else {
                    flat_cylinder_sdf(q, z, radius, height)
                }
            }
            ToolStamp::Cone {
                radius,
                half_angle,
                height,
            } => {
                // Signed distance to the cone side through the apex, capped
                // by the cylinder wall and the top.
                let side = q * half_angle.cos() - z * half_angle.sin();
                side.max(q - radius).max(z - height)
            }
        }
    }
}

/// Exact SDF of a finite cylinder with radius `r` spanning z ∈ [0, h], in
/// radial/axial coordinates.
fn flat_cylinder_sdf(q: f64, z: f64, r: f64, h: f64) -> f64 {
    let dq = q - r;
    let dz = (z - h / 2.0).abs() - h / 2.0;
    let outside = (dq.max(0.0).powi(2) + dz.max(0.0).powi(2)).sqrt();
    let inside = dq.max(dz).min(0.0);
    outside + inside
}

impl Stock {
    /// Subtract a toolpath from the stock.
    ///
    /// Motion segments (rapids included — a rapid through material is a
    /// crash, and simulating it as removal is what lets verification catch
    /// it) are sampled at sub-cell spacing, and the tool's cutter envelope —
    /// flat, ball, bull, or cone bottom depending on the tool type — is
    /// subtracted at each sample. Toolpath coordinates are tool-tip
    /// positions; the cutter extends upward (+Z) from the tip.
    ///
    /// # Arguments
    ///
    /// * `tool` - The cutting tool
    /// * `toolpath` - The toolpath to subtract
    pub fn subtract_toolpath(&mut self, tool: &Tool, toolpath: &Toolpath) {
        let stamp = ToolStamp::from_tool(tool);
        let spacing = self.stamp_spacing();
        // Start above the stock so the approach move cuts nothing.
        let mut current = [0.0, 0.0, self.bounds()[5] + 10.0];

        for segment in &toolpath.segments {
            match segment {
                ToolpathSegment::Rapid { to } | ToolpathSegment::Linear { to, .. } => {
                    self.stamp_segment(&stamp, current, *to, spacing);
                    current = *to;
                }
                ToolpathSegment::Arc {
                    to, center, dir, ..
                } => {
                    let mut prev = current;
                    for pt in linearize_arc(current, *to, *center, *dir) {
                        self.stamp_segment(&stamp, prev, pt, spacing);
                        prev = pt;
                    }
                    current = *to;
                }
                _ => {} // Ignore non-motion segments
            }
        }
    }

    /// Stamp spacing: half the finest leaf cell, so the scallop left
    /// between consecutive stamps stays well below the sim resolution.
    fn stamp_spacing(&self) -> f64 {
        let b = self.bounds();
        let cells = (1u32 << self.max_depth()) as f64;
        let cell = ((b[3] - b[0]) / cells)
            .max((b[4] - b[1]) / cells)
            .max((b[5] - b[2]) / cells);
        (cell * 0.5).max(1e-3)
    }

    /// Subtract the cutter envelope at samples along a linear move.
    fn stamp_segment(&mut self, stamp: &ToolStamp, from: [f64; 3], to: [f64; 3], spacing: f64) {
        // The cutter extends upward from the tip: a move whose tip stays
        // above the stock top can't touch material.
        let top = self.bounds()[5];
        if from[2] > top && to[2] > top {
            return;
        }

        let len =
            ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2))
                .sqrt();
        let n = ((len / spacing).ceil() as usize).max(1);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let tip = [
                from[0] + t * (to[0] - from[0]),
                from[1] + t * (to[1] - from[1]),
                from[2] + t * (to[2] - from[2]),
            ];
            self.subtract_sdf(|p| stamp.sdf(p, tip));
        }
    }
}

/// Linearize an arc into line segments.
fn linearize_arc(
    from: [f64; 3],
    to: [f64; 3],
    center: [f64; 3],
    dir: vcad_kernel_cam::ArcDir,
) -> Vec<[f64; 3]> {
    let mut points = Vec::new();

    // Compute arc parameters
    let r = ((from[0] - center[0]).powi(2) + (from[1] - center[1]).powi(2)).sqrt();
    let start_angle = (from[1] - center[1]).atan2(from[0] - center[0]);
    let end_angle = (to[1] - center[1]).atan2(to[0] - center[0]);

    let delta = match dir {
        vcad_kernel_cam::ArcDir::Ccw => {
            let mut d = end_angle - start_angle;
            if d <= 0.0 {
                d += 2.0 * std::f64::consts::PI;
            }
            d
        }
        vcad_kernel_cam::ArcDir::Cw => {
            let mut d = start_angle - end_angle;
            if d <= 0.0 {
                d += 2.0 * std::f64::consts::PI;
            }
            -d
        }
    };

    // Number of segments (approximately 5 degree steps)
    let n_segments = ((delta.abs() / 0.087).ceil() as usize).max(1);
    let angle_step = delta / n_segments as f64;
    let z_step = (to[2] - from[2]) / n_segments as f64;

    for i in 1..=n_segments {
        let angle = start_angle + angle_step * i as f64;
        let z = from[2] + z_step * i as f64;
        points.push([center[0] + r * angle.cos(), center[1] + r * angle.sin(), z]);
    }

    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swept_volume_flat() {
        let tool = Tool::FlatEndMill {
            diameter: 10.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let sv = SweptVolume::from_tool(&tool);

        assert!((sv.radius_at_height(0.0) - 5.0).abs() < 1e-6);
        assert!((sv.radius_at_height(10.0) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_swept_volume_ball() {
        let tool = Tool::BallEndMill {
            diameter: 10.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let sv = SweptVolume::from_tool(&tool);

        // At tip, radius should be 0
        assert!(sv.radius_at_height(0.0) < 0.1);

        // At top of ball, radius should be full
        assert!((sv.radius_at_height(5.0) - 5.0).abs() < 0.1);
    }

    #[test]
    fn test_linearize_arc_ccw() {
        let from = [10.0, 0.0, 0.0];
        let to = [0.0, 10.0, 0.0];
        let center = [0.0, 0.0, 0.0];

        let points = linearize_arc(from, to, center, vcad_kernel_cam::ArcDir::Ccw);

        assert!(!points.is_empty());
        // Last point should be close to 'to'
        let last = points.last().unwrap();
        assert!((last[0] - to[0]).abs() < 0.1);
        assert!((last[1] - to[1]).abs() < 0.1);
    }

    #[test]
    fn test_flat_stamp_cuts_flat_floor() {
        let stamp = ToolStamp::from_tool(&Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        });
        let tip = [0.0, 0.0, 0.0];

        // Inside the cutter, just above the tip.
        assert!(stamp.sdf([0.0, 0.0, 0.5], tip) < 0.0);
        assert!(stamp.sdf([2.9, 0.0, 0.5], tip) < 0.0);
        // Below the tip: a flat endmill does NOT cut under its floor.
        assert!(stamp.sdf([0.0, 0.0, -0.5], tip) > 0.0);
        // Outside the radius.
        assert!(stamp.sdf([3.5, 0.0, 5.0], tip) > 0.0);
    }

    #[test]
    fn test_ball_stamp_tip_geometry() {
        let stamp = ToolStamp::from_tool(&Tool::BallEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        });
        let tip = [0.0, 0.0, 0.0];

        // Sphere center sits at z = +r, so the tip itself is on the surface.
        assert!(stamp.sdf([0.0, 0.0, 0.0], tip).abs() < 1e-9);
        assert!(stamp.sdf([0.0, 0.0, 3.0], tip) < 0.0);
        // At tip height the ball has zero radius: a point at q=2 is outside.
        assert!(stamp.sdf([2.0, 0.0, 0.01], tip) > 0.0);
        // Nothing below the tip.
        assert!(stamp.sdf([0.0, 0.0, -0.5], tip) > 0.0);
    }

    #[test]
    fn test_stock_subtract_toolpath() {
        use vcad_kernel_cam::{CamSettings, Face};

        // CAM convention: stock top at Z=0, material below.
        let mut stock = Stock::from_box([0.0, 0.0, -10.0, 50.0, 50.0, 0.0], 1.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };

        // Face off the top 2mm.
        let face = Face::new(0.0, 0.0, 50.0, 50.0, 2.0);
        let settings = CamSettings {
            stepover: 4.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        };
        let toolpath = face.generate(&tool, &settings).unwrap();

        stock.subtract_toolpath(&tool, &toolpath);

        // The faced layer is gone...
        assert!(
            stock.sdf_at(25.0, 25.0, -1.0) > 0.0,
            "faced layer should be removed at the center"
        );
        assert!(
            stock.sdf_at(5.0, 5.0, -0.5) > 0.0,
            "faced layer should be removed near the edges"
        );
        // ...and the floor below survives: a flat endmill leaves a flat
        // floor instead of gouging a ball radius below the tip.
        assert!(
            stock.sdf_at(25.0, 25.0, -2.5) < 0.0,
            "flat endmill must not gouge below the floor"
        );
        assert!(
            stock.sdf_at(25.0, 25.0, -5.0) < 0.0,
            "material well below the floor must remain"
        );
    }
}
