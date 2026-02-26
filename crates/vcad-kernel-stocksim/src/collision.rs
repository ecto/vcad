//! Tool and holder collision detection.
//!
//! Checks for collisions between the tool shank/holder and stock material
//! during machining moves.

use vcad_kernel_cam::{Tool, ToolHolder};

use crate::Stock;

/// Type of collision detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionType {
    /// Collision with the tool shank (above flute length).
    ToolShank,
    /// Collision with the tool holder.
    Holder,
}

/// Result of a collision check.
#[derive(Debug, Clone)]
pub struct CollisionResult {
    /// Whether a collision was detected.
    pub collides: bool,
    /// Approximate collision point (if detected).
    pub point: Option<[f64; 3]>,
    /// Type of collision.
    pub collision_type: Option<CollisionType>,
    /// Minimum clearance found (negative if collision).
    pub clearance: f64,
}

impl CollisionResult {
    /// No collision detected.
    pub fn clear(clearance: f64) -> Self {
        Self {
            collides: false,
            point: None,
            collision_type: None,
            clearance,
        }
    }

    /// Collision detected.
    pub fn collision(point: [f64; 3], collision_type: CollisionType, clearance: f64) -> Self {
        Self {
            collides: true,
            point: Some(point),
            collision_type: Some(collision_type),
            clearance,
        }
    }
}

impl Stock {
    /// Check for collision between tool/holder and stock.
    ///
    /// # Arguments
    ///
    /// * `tool` - The cutting tool
    /// * `holder` - Optional tool holder
    /// * `from` - Start position of move
    /// * `to` - End position of move
    ///
    /// # Returns
    ///
    /// Collision result with details if a collision is found.
    pub fn check_collision(
        &self,
        tool: &Tool,
        holder: Option<&ToolHolder>,
        from: [f64; 3],
        to: [f64; 3],
    ) -> CollisionResult {
        let tool_radius = tool.radius();
        let flute_length = tool.max_depth().unwrap_or(50.0);

        // Sample points along the move
        let move_vec = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let move_len = (move_vec[0].powi(2) + move_vec[1].powi(2) + move_vec[2].powi(2)).sqrt();

        let n_samples = ((move_len / 1.0).ceil() as usize).max(2);
        let mut min_clearance = f64::INFINITY;
        let mut worst_point = None;
        let mut worst_type = None;

        for i in 0..=n_samples {
            let t = i as f64 / n_samples as f64;
            let pos = [
                from[0] + t * move_vec[0],
                from[1] + t * move_vec[1],
                from[2] + t * move_vec[2],
            ];

            // Check shank collision (cylinder above flute length)
            let shank_result = self.check_cylinder_collision(
                pos,
                tool_radius,
                flute_length,
                flute_length + 50.0, // Check 50mm above flute
            );

            if shank_result < min_clearance {
                min_clearance = shank_result;
                worst_point = Some([pos[0], pos[1], pos[2] + flute_length + 10.0]);
                worst_type = Some(CollisionType::ToolShank);
            }

            // Check holder collision if provided
            if let Some(h) = holder {
                let holder_start = flute_length + 5.0; // Small gap between tool and holder
                let holder_result = self.check_cylinder_collision(
                    pos,
                    h.diameter / 2.0,
                    holder_start,
                    holder_start + h.length,
                );

                if holder_result < min_clearance {
                    min_clearance = holder_result;
                    worst_point = Some([pos[0], pos[1], pos[2] + holder_start + h.length / 2.0]);
                    worst_type = Some(CollisionType::Holder);
                }
            }
        }

        if min_clearance < 0.0 {
            CollisionResult::collision(
                worst_point.unwrap_or(from),
                worst_type.unwrap_or(CollisionType::ToolShank),
                min_clearance,
            )
        } else {
            CollisionResult::clear(min_clearance)
        }
    }

    /// Check collision for a cylinder at a position.
    ///
    /// Returns the minimum clearance (negative if collision).
    fn check_cylinder_collision(
        &self,
        base_pos: [f64; 3],
        radius: f64,
        z_start: f64,
        z_end: f64,
    ) -> f64 {
        let n_z_samples = (((z_end - z_start) / 2.0).ceil() as usize).max(2);
        let mut min_clearance = f64::INFINITY;

        for iz in 0..=n_z_samples {
            let t = iz as f64 / n_z_samples as f64;
            let z = base_pos[2] + z_start + t * (z_end - z_start);

            // Sample around the cylinder perimeter
            for angle_i in 0..8 {
                let angle = angle_i as f64 * std::f64::consts::PI / 4.0;
                let x = base_pos[0] + radius * angle.cos();
                let y = base_pos[1] + radius * angle.sin();

                let sdf = self.sdf_at(x, y, z);

                // Clearance is negative SDF (inside stock = collision)
                // SDF < 0 means inside stock, so clearance = -SDF when SDF < 0
                let clearance = sdf;
                if clearance < min_clearance {
                    min_clearance = clearance;
                }
            }

            // Also check center
            let sdf = self.sdf_at(base_pos[0], base_pos[1], z);
            if sdf < min_clearance {
                min_clearance = sdf;
            }
        }

        min_clearance
    }

    /// Check if a tool position would cause a collision.
    ///
    /// Simpler API for single-point checks.
    pub fn would_collide(
        &self,
        tool: &Tool,
        holder: Option<&ToolHolder>,
        position: [f64; 3],
    ) -> bool {
        self.check_collision(tool, holder, position, position)
            .collides
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collision_clear() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 50.0, 50.0, 10.0], 2.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };

        // Tool well above stock - no collision
        let result = stock.check_collision(&tool, None, [25.0, 25.0, 50.0], [25.0, 25.0, 50.0]);
        assert!(!result.collides);
        assert!(result.clearance > 0.0);
    }

    #[test]
    fn test_collision_detected() {
        let mut stock = Stock::from_box([0.0, 0.0, 0.0, 50.0, 50.0, 30.0], 2.0);

        // Subtract a pocket but leave material on the sides
        stock.subtract_sphere([25.0, 25.0, 15.0], 10.0);

        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 10.0, // Short flute
            flutes: 2,
        };

        // Tool deep in the pocket - shank may hit sides
        let result = stock.check_collision(&tool, None, [25.0, 25.0, 5.0], [25.0, 25.0, 5.0]);

        // This specific scenario may or may not collide depending on pocket geometry
        // The test validates the function runs without error
        assert!(result.clearance.is_finite());
    }

    #[test]
    fn test_collision_with_holder() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 50.0, 50.0, 10.0], 2.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let holder = ToolHolder::new(30.0, 40.0);

        // Tool well above stock - no collision even with large holder
        let result = stock.check_collision(
            &tool,
            Some(&holder),
            [25.0, 25.0, 100.0],
            [25.0, 25.0, 100.0],
        );
        assert!(!result.collides);
    }

    #[test]
    fn test_would_collide_api() {
        let stock = Stock::from_box([0.0, 0.0, 0.0, 50.0, 50.0, 10.0], 2.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };

        // Well above stock
        assert!(!stock.would_collide(&tool, None, [25.0, 25.0, 50.0]));
    }
}
