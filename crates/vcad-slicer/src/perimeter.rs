//! Perimeter (wall) generation from slice contours.

use crate::path::{optimize_polygon_order, Polygon};
use crate::slice::SliceLayer;

/// Settings for perimeter generation.
#[derive(Debug, Clone, Copy)]
pub struct PerimeterSettings {
    /// Number of perimeter walls.
    pub wall_count: u32,
    /// Extrusion line width (mm).
    pub line_width: f64,
    /// External perimeter speed factor (0.0-1.0, relative to base speed).
    pub external_speed_factor: f64,
}

impl Default for PerimeterSettings {
    fn default() -> Self {
        Self {
            wall_count: 3,
            line_width: 0.45,
            external_speed_factor: 0.8,
        }
    }
}

/// Result of perimeter generation for a layer.
#[derive(Debug, Clone)]
pub struct LayerPerimeters {
    /// Outer perimeters (printed slow, visible surface).
    pub outer: Vec<Polygon>,
    /// Inner perimeters (printed faster).
    pub inner: Vec<Polygon>,
    /// Innermost boundary for infill region.
    pub infill_boundary: Vec<Polygon>,
}

impl LayerPerimeters {
    /// Create empty perimeters.
    pub fn new() -> Self {
        Self {
            outer: Vec::new(),
            inner: Vec::new(),
            infill_boundary: Vec::new(),
        }
    }
}

impl Default for LayerPerimeters {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate perimeters from slice layer contours.
///
/// For each contour:
/// 1. The original contour becomes the outer perimeter
/// 2. Offset inward by line_width for each inner perimeter
/// 3. The innermost offset becomes the infill boundary
pub fn generate_perimeters(layer: &SliceLayer, settings: &PerimeterSettings) -> LayerPerimeters {
    let mut result = LayerPerimeters::new();

    if settings.wall_count == 0 {
        // No walls - entire contour is infill boundary
        result.infill_boundary = layer.contours.clone();
        return result;
    }

    for contour in &layer.contours {
        let _is_hole = !contour.is_ccw();

        // Generate perimeters from outside to inside
        let mut current = contour.clone();

        // First perimeter is the outer wall (offset inward by half line width for extrusion center)
        if let Some(outer) = current.offset(settings.line_width / 2.0) {
            result.outer.push(outer.clone());
            current = outer;
        } else {
            // Contour too small for perimeter
            continue;
        }

        // Inner perimeters
        for i in 1..settings.wall_count {
            if let Some(inner) = current.offset(settings.line_width) {
                if i == settings.wall_count - 1 {
                    // Last perimeter - its inside edge is the infill boundary
                    if let Some(infill) = inner.offset(settings.line_width / 2.0) {
                        result.infill_boundary.push(infill);
                    }
                }
                result.inner.push(inner.clone());
                current = inner;
            } else {
                // Collapsed - remaining area is solid
                break;
            }
        }

        // If only one wall, the inner edge of outer perimeter is the infill boundary
        if settings.wall_count == 1 {
            if let Some(infill) = result
                .outer
                .last()
                .and_then(|p| p.offset(settings.line_width / 2.0))
            {
                result.infill_boundary.push(infill);
            }
        }
    }

    // Optimize print order to minimize travel
    optimize_polygon_order(&mut result.outer);
    optimize_polygon_order(&mut result.inner);

    result
}

/// Classify contours into outer boundaries and holes.
/// Returns (outer_contours, hole_contours).
pub fn classify_contours(contours: &[Polygon]) -> (Vec<&Polygon>, Vec<&Polygon>) {
    let mut outers = Vec::new();
    let mut holes = Vec::new();

    for contour in contours {
        if contour.is_ccw() {
            outers.push(contour);
        } else {
            holes.push(contour);
        }
    }

    (outers, holes)
}

/// Check if a point is inside a polygon (2D).
pub fn point_in_polygon(point: &vcad_kernel_math::Point2, polygon: &Polygon) -> bool {
    let n = polygon.points.len();
    if n < 3 {
        return false;
    }

    let mut inside = false;
    let mut j = n - 1;

    for i in 0..n {
        let pi = &polygon.points[i];
        let pj = &polygon.points[j];

        if ((pi.y > point.y) != (pj.y > point.y))
            && (point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }

    inside
}

/// Associate holes with their parent outer contours.
/// Returns Vec of (outer_index, hole_indices) pairs.
pub fn associate_holes(contours: &[Polygon]) -> Vec<(usize, Vec<usize>)> {
    let (outers, holes) = classify_contours(contours);

    let mut associations: Vec<(usize, Vec<usize>)> = Vec::new();

    // Find original indices
    let outer_indices: Vec<usize> = contours
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_ccw())
        .map(|(i, _)| i)
        .collect();

    let hole_indices: Vec<usize> = contours
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_ccw())
        .map(|(i, _)| i)
        .collect();

    for (outer_idx, outer) in outers.iter().enumerate() {
        let mut contained_holes = Vec::new();

        for (hole_idx, hole) in holes.iter().enumerate() {
            // Check if hole's first point is inside this outer
            if let Some(pt) = hole.points.first() {
                if point_in_polygon(pt, outer) {
                    contained_holes.push(hole_indices[hole_idx]);
                }
            }
        }

        associations.push((outer_indices[outer_idx], contained_holes));
    }

    associations
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Point2;

    #[test]
    fn test_point_in_polygon() {
        let square = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ]);

        assert!(point_in_polygon(&Point2::new(5.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2::new(15.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2::new(-1.0, 5.0), &square));
    }

    #[test]
    fn test_classify_contours() {
        let outer = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ]);

        // CW hole
        let hole = Polygon::new(vec![
            Point2::new(2.0, 2.0),
            Point2::new(2.0, 8.0),
            Point2::new(8.0, 8.0),
            Point2::new(8.0, 2.0),
        ]);

        let contours = vec![outer, hole];
        let (outers, holes) = classify_contours(&contours);

        assert_eq!(outers.len(), 1);
        assert_eq!(holes.len(), 1);
    }
}
