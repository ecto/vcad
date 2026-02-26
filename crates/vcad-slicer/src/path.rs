//! Toolpath types and optimization.

use vcad_kernel_math::Point2;

/// A 2D polygon (closed path).
#[derive(Debug, Clone)]
pub struct Polygon {
    /// Vertices of the polygon in order.
    pub points: Vec<Point2>,
}

impl Polygon {
    /// Create a new polygon from points.
    pub fn new(points: Vec<Point2>) -> Self {
        Self { points }
    }

    /// Check if the polygon is empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Number of vertices.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Signed area of the polygon.
    /// Positive for counter-clockwise, negative for clockwise.
    pub fn signed_area(&self) -> f64 {
        let n = self.points.len();
        if n < 3 {
            return 0.0;
        }
        let mut area = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            area += self.points[i].x * self.points[j].y;
            area -= self.points[j].x * self.points[i].y;
        }
        area / 2.0
    }

    /// Is the polygon counter-clockwise?
    pub fn is_ccw(&self) -> bool {
        self.signed_area() > 0.0
    }

    /// Reverse the winding order.
    pub fn reverse(&mut self) {
        self.points.reverse();
    }

    /// Ensure counter-clockwise winding.
    pub fn ensure_ccw(&mut self) {
        if !self.is_ccw() {
            self.reverse();
        }
    }

    /// Ensure clockwise winding.
    pub fn ensure_cw(&mut self) {
        if self.is_ccw() {
            self.reverse();
        }
    }

    /// Perimeter length.
    pub fn perimeter(&self) -> f64 {
        let n = self.points.len();
        if n < 2 {
            return 0.0;
        }
        let mut length = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            length += (self.points[j] - self.points[i]).norm();
        }
        length
    }

    /// Compute centroid of the polygon.
    pub fn centroid(&self) -> Point2 {
        if self.points.is_empty() {
            return Point2::origin();
        }
        let sum: Point2 = self.points.iter().fold(Point2::origin(), |acc, p| {
            Point2::new(acc.x + p.x, acc.y + p.y)
        });
        Point2::new(
            sum.x / self.points.len() as f64,
            sum.y / self.points.len() as f64,
        )
    }

    /// Offset the polygon inward (shrink) or outward (expand) by distance.
    /// Positive distance = inward (for outer contours).
    pub fn offset(&self, distance: f64) -> Option<Self> {
        if self.points.len() < 3 {
            return None;
        }

        let n = self.points.len();
        let mut offset_points = Vec::with_capacity(n);

        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = (i + 1) % n;

            let p0 = self.points[prev];
            let p1 = self.points[i];
            let p2 = self.points[next];

            // Edge vectors
            let e1 = (p1 - p0).normalize();
            let e2 = (p2 - p1).normalize();

            // Inward normals (rotate 90° CCW for CCW polygon, CW for CW polygon)
            let sign = if self.is_ccw() { 1.0 } else { -1.0 };
            let n1 = Point2::new(-e1.y * sign, e1.x * sign);
            let n2 = Point2::new(-e2.y * sign, e2.x * sign);

            // Bisector direction (average of normals)
            let bisector = (n1.to_vec() + n2.to_vec()).normalize();

            // Offset distance along bisector (adjusted for corner angle)
            let dot = n1.to_vec().dot(bisector);
            let offset_dist = if dot.abs() > 0.001 {
                distance / dot
            } else {
                distance
            };

            // Limit offset to avoid self-intersection at sharp corners
            let max_offset = distance * 2.0;
            let clamped_offset = offset_dist.clamp(-max_offset, max_offset);

            let offset_pt = Point2::new(
                p1.x + bisector.x * clamped_offset,
                p1.y + bisector.y * clamped_offset,
            );
            offset_points.push(offset_pt);
        }

        // Check if offset polygon collapsed
        let result = Polygon::new(offset_points);
        if result.signed_area().abs() < 1e-10 {
            return None;
        }

        Some(result)
    }
}

/// An open polyline (non-closed path).
#[derive(Debug, Clone)]
pub struct Polyline {
    /// Points along the path.
    pub points: Vec<Point2>,
}

impl Polyline {
    /// Create a new polyline.
    pub fn new(points: Vec<Point2>) -> Self {
        Self { points }
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Number of points.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Total length of the polyline.
    pub fn length(&self) -> f64 {
        if self.points.len() < 2 {
            return 0.0;
        }
        self.points.windows(2).map(|w| (w[1] - w[0]).norm()).sum()
    }

    /// Starting point.
    pub fn start(&self) -> Option<&Point2> {
        self.points.first()
    }

    /// Ending point.
    pub fn end(&self) -> Option<&Point2> {
        self.points.last()
    }
}

/// Optimize ordering of polygons to minimize travel moves.
/// Uses nearest-neighbor heuristic.
pub fn optimize_polygon_order(polygons: &mut [Polygon]) {
    if polygons.len() < 2 {
        return;
    }

    let mut current_pos = Point2::origin();
    let mut remaining: Vec<usize> = (0..polygons.len()).collect();
    let mut order: Vec<usize> = Vec::with_capacity(polygons.len());

    while !remaining.is_empty() {
        // Find nearest polygon start to current position
        let (best_idx, _) = remaining
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| {
                let dist_a = if let Some(p) = polygons[a].points.first() {
                    (current_pos - *p).norm()
                } else {
                    f64::MAX
                };
                let dist_b = if let Some(p) = polygons[b].points.first() {
                    (current_pos - *p).norm()
                } else {
                    f64::MAX
                };
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let poly_idx = remaining.remove(best_idx);
        if let Some(last) = polygons[poly_idx].points.last() {
            current_pos = *last;
        }
        order.push(poly_idx);
    }

    // Reorder in place using order indices
    let mut sorted: Vec<Polygon> = order.into_iter().map(|i| polygons[i].clone()).collect();
    polygons.swap_with_slice(&mut sorted);
}

/// Optimize ordering of polylines to minimize travel moves.
pub fn optimize_polyline_order(polylines: &mut [Polyline]) {
    if polylines.len() < 2 {
        return;
    }

    let mut current_pos = Point2::origin();
    let mut remaining: Vec<usize> = (0..polylines.len()).collect();
    let mut order: Vec<usize> = Vec::with_capacity(polylines.len());

    while !remaining.is_empty() {
        let (best_idx, _) = remaining
            .iter()
            .enumerate()
            .min_by(|(_, &a), (_, &b)| {
                let dist_a = if let Some(p) = polylines[a].start() {
                    (current_pos - *p).norm()
                } else {
                    f64::MAX
                };
                let dist_b = if let Some(p) = polylines[b].start() {
                    (current_pos - *p).norm()
                } else {
                    f64::MAX
                };
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let line_idx = remaining.remove(best_idx);
        if let Some(last) = polylines[line_idx].end() {
            current_pos = *last;
        }
        order.push(line_idx);
    }

    let mut sorted: Vec<Polyline> = order.into_iter().map(|i| polylines[i].clone()).collect();
    polylines.swap_with_slice(&mut sorted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polygon_area() {
        // Unit square CCW
        let square = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ]);
        assert!((square.signed_area() - 1.0).abs() < 1e-10);
        assert!(square.is_ccw());
    }

    #[test]
    fn test_polygon_offset() {
        let square = Polygon::new(vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ]);
        let offset = square.offset(1.0).unwrap();
        // Should be 8x8 after 1mm inward offset
        let area = offset.signed_area().abs();
        assert!((area - 64.0).abs() < 1.0);
    }
}
