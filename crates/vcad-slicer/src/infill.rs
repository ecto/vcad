//! Infill pattern generation.

use serde::{Deserialize, Serialize};
use vcad_kernel_math::Point2;

use crate::path::{optimize_polyline_order, Polygon, Polyline};
use crate::perimeter::point_in_polygon;

/// Infill pattern types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InfillPattern {
    /// Rectilinear grid (alternating 0°/90°).
    #[default]
    Grid,
    /// Single direction lines (alternating 45°/-45°).
    Lines,
    /// Triangular pattern.
    Triangles,
    /// Hexagonal honeycomb.
    Honeycomb,
    /// Gyroid (approximated with lines).
    Gyroid,
}

/// Settings for infill generation.
#[derive(Debug, Clone, Copy)]
pub struct InfillSettings {
    /// Infill pattern.
    pub pattern: InfillPattern,
    /// Infill density (0.0 to 1.0).
    pub density: f64,
    /// Line width (mm).
    pub line_width: f64,
    /// Layer index (for alternating patterns).
    pub layer_index: usize,
}

impl Default for InfillSettings {
    fn default() -> Self {
        Self {
            pattern: InfillPattern::Grid,
            density: 0.15,
            line_width: 0.45,
            layer_index: 0,
        }
    }
}

/// Result of infill generation.
#[derive(Debug, Clone)]
pub struct InfillResult {
    /// Infill paths (open polylines).
    pub paths: Vec<Polyline>,
}

impl InfillResult {
    /// Create empty infill.
    pub fn new() -> Self {
        Self { paths: Vec::new() }
    }
}

impl Default for InfillResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate infill for a region bounded by polygons.
pub fn generate_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    if boundaries.is_empty() || settings.density <= 0.0 {
        return InfillResult::new();
    }

    match settings.pattern {
        InfillPattern::Grid => generate_grid_infill(boundaries, settings),
        InfillPattern::Lines => generate_lines_infill(boundaries, settings),
        InfillPattern::Triangles => generate_triangle_infill(boundaries, settings),
        InfillPattern::Honeycomb => generate_honeycomb_infill(boundaries, settings),
        InfillPattern::Gyroid => generate_gyroid_infill(boundaries, settings),
    }
}

/// Generate grid infill (rectilinear).
fn generate_grid_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    let spacing = settings.line_width / settings.density;
    let angle = if settings.layer_index.is_multiple_of(2) {
        0.0_f64
    } else {
        90.0_f64
    };

    generate_parallel_lines(boundaries, spacing, angle.to_radians())
}

/// Generate lines infill (45°/-45° alternating).
fn generate_lines_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    let spacing = settings.line_width / settings.density;
    let angle = if settings.layer_index.is_multiple_of(2) {
        45.0_f64
    } else {
        -45.0_f64
    };

    generate_parallel_lines(boundaries, spacing, angle.to_radians())
}

/// Generate triangular infill.
fn generate_triangle_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    let spacing = settings.line_width / settings.density;

    // Triangular uses 3 line directions: 0°, 60°, -60°
    let direction_index = settings.layer_index % 3;
    let angle: f64 = match direction_index {
        0 => 0.0,
        1 => 60.0,
        _ => -60.0,
    };

    generate_parallel_lines(boundaries, spacing, angle.to_radians())
}

/// Generate honeycomb infill (simplified as offset hex grid).
fn generate_honeycomb_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    // Honeycomb approximated as alternating angled lines
    let spacing = settings.line_width / settings.density * 1.5;
    let angle = if settings.layer_index.is_multiple_of(2) {
        30.0_f64
    } else {
        -30.0_f64
    };

    generate_parallel_lines(boundaries, spacing, angle.to_radians())
}

/// Generate gyroid infill (approximated).
fn generate_gyroid_infill(boundaries: &[Polygon], settings: &InfillSettings) -> InfillResult {
    // True gyroid is a 3D TPMS surface. For 2D slices, we approximate with
    // sinusoidal lines that shift phase each layer.
    let spacing = settings.line_width / settings.density;
    let phase = (settings.layer_index as f64 * 0.5).sin() * std::f64::consts::PI;
    let angle = 45.0_f64.to_radians() + phase * 0.1;

    generate_parallel_lines(boundaries, spacing, angle)
}

/// Generate parallel lines at specified angle within boundaries.
fn generate_parallel_lines(boundaries: &[Polygon], spacing: f64, angle: f64) -> InfillResult {
    if boundaries.is_empty() {
        return InfillResult::new();
    }

    // Compute bounding box of all boundaries
    let (min, max) = compute_bounds(boundaries);

    // Generate scan lines perpendicular to angle direction
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Direction along lines
    let dir = Point2::new(cos_a, sin_a);
    // Direction perpendicular (for spacing)
    let perp = Point2::new(-sin_a, cos_a);

    // Project bounds onto perpendicular direction to find range
    let corners = [
        Point2::new(min[0], min[1]),
        Point2::new(max[0], min[1]),
        Point2::new(max[0], max[1]),
        Point2::new(min[0], max[1]),
    ];

    let mut perp_min = f64::MAX;
    let mut perp_max = f64::MIN;

    for corner in &corners {
        let proj = corner.x * perp.x + corner.y * perp.y;
        perp_min = perp_min.min(proj);
        perp_max = perp_max.max(proj);
    }

    // Generate lines
    let mut paths: Vec<Polyline> = Vec::new();
    let mut offset = perp_min + spacing / 2.0;

    while offset < perp_max {
        // Line passes through point: offset * perp
        let line_origin = Point2::new(offset * perp.x, offset * perp.y);

        // Find intersections with all boundary segments
        let mut intersections = find_line_boundary_intersections(&line_origin, &dir, boundaries);

        // Sort by position along line
        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Create line segments (pairs of intersections)
        for pair in intersections.chunks(2) {
            if pair.len() == 2 {
                let t0 = pair[0];
                let t1 = pair[1];

                let p0 = Point2::new(line_origin.x + t0 * dir.x, line_origin.y + t0 * dir.y);
                let p1 = Point2::new(line_origin.x + t1 * dir.x, line_origin.y + t1 * dir.y);

                // Verify midpoint is inside
                let mid = Point2::new((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
                if is_point_inside_boundaries(&mid, boundaries) {
                    paths.push(Polyline::new(vec![p0, p1]));
                }
            }
        }

        offset += spacing;
    }

    optimize_polyline_order(&mut paths);

    InfillResult { paths }
}

/// Compute bounding box of polygons.
fn compute_bounds(polygons: &[Polygon]) -> ([f64; 2], [f64; 2]) {
    let mut min = [f64::MAX, f64::MAX];
    let mut max = [f64::MIN, f64::MIN];

    for poly in polygons {
        for pt in &poly.points {
            min[0] = min[0].min(pt.x);
            min[1] = min[1].min(pt.y);
            max[0] = max[0].max(pt.x);
            max[1] = max[1].max(pt.y);
        }
    }

    (min, max)
}

/// Find intersections between a line and polygon boundaries.
/// Returns sorted list of t values along the line direction.
fn find_line_boundary_intersections(
    origin: &Point2,
    dir: &Point2,
    boundaries: &[Polygon],
) -> Vec<f64> {
    let mut intersections = Vec::new();
    let eps = 1e-10;

    for poly in boundaries {
        let n = poly.points.len();
        for i in 0..n {
            let j = (i + 1) % n;
            let a = &poly.points[i];
            let b = &poly.points[j];

            // Line: origin + t * dir
            // Segment: a + s * (b - a), s in [0, 1]
            // Solve: origin + t * dir = a + s * (b - a)

            let seg_dir = Point2::new(b.x - a.x, b.y - a.y);

            // Cross product of directions
            let cross = dir.x * seg_dir.y - dir.y * seg_dir.x;

            if cross.abs() < eps {
                // Parallel lines
                continue;
            }

            let diff = Point2::new(a.x - origin.x, a.y - origin.y);
            let t = (diff.x * seg_dir.y - diff.y * seg_dir.x) / cross;
            let s = (diff.x * dir.y - diff.y * dir.x) / cross;

            // Check if intersection is on segment
            if s >= -eps && s <= 1.0 + eps {
                intersections.push(t);
            }
        }
    }

    intersections
}

/// Check if a point is inside the boundary region.
/// Point must be inside an outer (CCW) contour and outside all holes (CW).
fn is_point_inside_boundaries(point: &Point2, boundaries: &[Polygon]) -> bool {
    let mut inside_outer = false;

    for poly in boundaries {
        let contains = point_in_polygon(point, poly);

        if poly.is_ccw() {
            // Outer boundary
            if contains {
                inside_outer = true;
            }
        } else {
            // Hole
            if contains {
                return false;
            }
        }
    }

    inside_outer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_infill() {
        let square = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ]);

        let settings = InfillSettings {
            pattern: InfillPattern::Grid,
            density: 0.2,
            line_width: 0.45,
            layer_index: 0,
        };

        let result = generate_infill(&[square], &settings);
        assert!(!result.paths.is_empty());
    }

    #[test]
    fn test_infill_with_hole() {
        let outer = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(20.0, 0.0),
            Point2::new(20.0, 20.0),
            Point2::new(0.0, 20.0),
        ]);

        // CW hole in center
        let hole = Polygon::new(vec![
            Point2::new(8.0, 8.0),
            Point2::new(8.0, 12.0),
            Point2::new(12.0, 12.0),
            Point2::new(12.0, 8.0),
        ]);

        let settings = InfillSettings {
            pattern: InfillPattern::Lines,
            density: 0.2,
            line_width: 0.45,
            layer_index: 0,
        };

        let result = generate_infill(&[outer, hole], &settings);
        assert!(!result.paths.is_empty());

        // Verify no line passes through center of hole
        for path in &result.paths {
            for pt in &path.points {
                // Should not be inside the hole
                assert!(!(pt.x > 8.5 && pt.x < 11.5 && pt.y > 8.5 && pt.y < 11.5));
            }
        }
    }
}
