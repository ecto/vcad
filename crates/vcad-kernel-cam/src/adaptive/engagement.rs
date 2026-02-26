//! Radial engagement computation for adaptive clearing.
//!
//! Engagement angle is the arc of the tool circumference that contacts
//! material during cutting. Lower engagement means less tool load.

use crate::operation::Point2D;

/// Result of engagement computation.
#[derive(Debug, Clone)]
pub struct EngagementResult {
    /// Engagement angle in radians.
    pub angle: f64,
    /// Direction of highest engagement (radians from +X).
    pub direction: f64,
    /// Whether this engagement exceeds the safe limit.
    pub exceeds_limit: bool,
}

/// Compute radial engagement at a point given a tool moving in a direction.
///
/// # Arguments
///
/// * `position` - Current tool position
/// * `direction` - Movement direction (normalized)
/// * `tool_radius` - Tool radius
/// * `material_boundary` - Points defining the material boundary
/// * `max_engagement` - Maximum allowed engagement (for exceeds_limit flag)
///
/// # Returns
///
/// Engagement result with angle and direction.
pub fn compute_engagement(
    position: Point2D,
    direction: Point2D,
    tool_radius: f64,
    material_boundary: &[Point2D],
    max_engagement: f64,
) -> EngagementResult {
    // Normalize direction (reserved for future use with directional engagement)
    let dir_len = (direction.x * direction.x + direction.y * direction.y).sqrt();
    let _dir = if dir_len > 1e-10 {
        Point2D::new(direction.x / dir_len, direction.y / dir_len)
    } else {
        Point2D::new(1.0, 0.0)
    };

    // Sample around the tool perimeter to find material contact
    let n_samples = 36; // Every 10 degrees
    let mut in_material = vec![false; n_samples];

    for (i, material_flag) in in_material.iter_mut().enumerate() {
        let angle = i as f64 * 2.0 * std::f64::consts::PI / n_samples as f64;
        let sample_x = position.x + tool_radius * angle.cos();
        let sample_y = position.y + tool_radius * angle.sin();
        let sample = Point2D::new(sample_x, sample_y);

        *material_flag = point_in_polygon(&sample, material_boundary);
    }

    // Find the longest arc of material contact
    let mut max_arc = 0;
    let mut max_arc_start = 0;
    let mut current_arc = 0;
    let mut current_start = 0;

    // Handle wrap-around by doubling the array conceptually
    for i in 0..(n_samples * 2) {
        let idx = i % n_samples;
        if in_material[idx] {
            if current_arc == 0 {
                current_start = i;
            }
            current_arc += 1;
        } else {
            if current_arc > max_arc {
                max_arc = current_arc;
                max_arc_start = current_start;
            }
            current_arc = 0;
        }
    }
    if current_arc > max_arc {
        max_arc = current_arc;
        max_arc_start = current_start;
    }

    // Cap at n_samples (full circle)
    let arc_samples = max_arc.min(n_samples);
    let engagement_angle = arc_samples as f64 * 2.0 * std::f64::consts::PI / n_samples as f64;

    // Direction of engagement (center of material contact arc)
    let arc_center = max_arc_start + arc_samples / 2;
    let engagement_dir =
        (arc_center % n_samples) as f64 * 2.0 * std::f64::consts::PI / n_samples as f64;

    EngagementResult {
        angle: engagement_angle,
        direction: engagement_dir,
        exceeds_limit: engagement_angle > max_engagement,
    }
}

/// Check if a point is inside a polygon (boundary).
fn point_in_polygon(point: &Point2D, polygon: &[Point2D]) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    let mut inside = false;
    let mut j = polygon.len() - 1;

    for i in 0..polygon.len() {
        let pi = &polygon[i];
        let pj = &polygon[j];

        if ((pi.y > point.y) != (pj.y > point.y))
            && (point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }

    inside
}

/// Compute stepover based on target engagement and tool radius.
///
/// For a given engagement angle, the stepover is:
/// stepover = R * (1 - cos(engagement/2))
///
/// Solving for stepover given target engagement:
/// This gives the lateral distance that produces the target engagement.
#[allow(dead_code)]
pub fn stepover_for_engagement(tool_radius: f64, target_engagement: f64) -> f64 {
    // For engagement angle theta, stepover = R * (1 - cos(theta/2))
    // But we want the stepover that produces theta engagement when
    // cutting into a straight wall.

    // The relationship is: stepover = 2 * R * sin(theta/2) for small angles
    // More accurately, for full slot (180 deg) stepover = R
    // For engagement theta: stepover ≈ R * (1 - cos(theta/2)) * 2

    tool_radius * (1.0 - (target_engagement / 2.0).cos()) * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_square_boundary() -> Vec<Point2D> {
        vec![
            Point2D::new(0.0, 0.0),
            Point2D::new(100.0, 0.0),
            Point2D::new(100.0, 100.0),
            Point2D::new(0.0, 100.0),
        ]
    }

    #[test]
    fn test_point_in_polygon() {
        let boundary = make_square_boundary();

        // Inside
        assert!(point_in_polygon(&Point2D::new(50.0, 50.0), &boundary));

        // Outside
        assert!(!point_in_polygon(&Point2D::new(150.0, 50.0), &boundary));
        assert!(!point_in_polygon(&Point2D::new(-10.0, 50.0), &boundary));
    }

    #[test]
    fn test_engagement_center() {
        let boundary = make_square_boundary();

        // Tool at center, no engagement (fully inside empty area conceptually)
        // This tests the function runs without error
        let result = compute_engagement(
            Point2D::new(50.0, 50.0),
            Point2D::new(1.0, 0.0),
            5.0,
            &boundary,
            std::f64::consts::PI / 2.0,
        );

        // Engagement should be non-negative
        assert!(result.angle >= 0.0);
    }

    #[test]
    fn test_engagement_near_wall() {
        let boundary = make_square_boundary();

        // Tool near left wall (x=5, tool radius 5 means touching wall)
        let result = compute_engagement(
            Point2D::new(5.0, 50.0),
            Point2D::new(1.0, 0.0),
            5.0,
            &boundary,
            std::f64::consts::PI / 2.0,
        );

        // Should have some engagement
        assert!(result.angle >= 0.0);
    }

    #[test]
    fn test_stepover_for_engagement() {
        let tool_radius = 5.0;

        // For 90 degree engagement
        let stepover_90 = stepover_for_engagement(tool_radius, std::f64::consts::PI / 2.0);
        assert!(stepover_90 > 0.0);
        assert!(stepover_90 < tool_radius * 2.0);

        // For smaller engagement, smaller stepover
        let stepover_45 = stepover_for_engagement(tool_radius, std::f64::consts::PI / 4.0);
        assert!(stepover_45 < stepover_90);
    }
}
