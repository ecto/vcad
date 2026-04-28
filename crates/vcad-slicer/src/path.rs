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
    ///
    /// Antiparallel consecutive edges (180° spikes) and zero-length edges
    /// would otherwise produce NaN-valued bisectors; both fall back to a
    /// single edge normal so the result stays finite. Hoist `is_ccw()`
    /// outside the per-vertex loop — it's an O(n) scan and was previously
    /// re-evaluated for every vertex.
    pub fn offset(&self, distance: f64) -> Option<Self> {
        if self.points.len() < 3 {
            return None;
        }

        let n = self.points.len();
        let sign = if self.is_ccw() { 1.0 } else { -1.0 };
        let mut offset_points = Vec::with_capacity(n);

        let safe_normalize = |v: vcad_kernel_math::Vec2| -> Option<vcad_kernel_math::Vec2> {
            let len = v.norm();
            if len > 1e-12 {
                Some(v / len)
            } else {
                None
            }
        };

        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = (i + 1) % n;

            let p0 = self.points[prev];
            let p1 = self.points[i];
            let p2 = self.points[next];

            // Edge directions; if either edge is zero-length, fall back to the
            // other one. If both are zero, the vertex is degenerate — keep it
            // in place rather than introducing NaN.
            let e1 = safe_normalize(p1 - p0);
            let e2 = safe_normalize(p2 - p1);

            let (e1, e2) = match (e1, e2) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                (None, None) => {
                    offset_points.push(p1);
                    continue;
                }
            };

            // Inward normals (rotate 90° CCW for CCW polygon, CW for CW polygon).
            let n1 = vcad_kernel_math::Vec2::new(-e1.y * sign, e1.x * sign);
            let n2 = vcad_kernel_math::Vec2::new(-e2.y * sign, e2.x * sign);

            // Bisector — fall back to n1 if the two normals cancel (antiparallel
            // edges, e.g. an offset that folded a short edge back on itself).
            let sum = n1 + n2;
            let bisector = match safe_normalize(sum) {
                Some(b) => b,
                None => n1,
            };

            // Offset distance along bisector (adjusted for corner angle).
            let dot = n1.dot(bisector);
            let offset_dist = if dot.abs() > 0.001 {
                distance / dot
            } else {
                distance
            };

            // Limit offset to avoid self-intersection at sharp corners.
            let max_offset = distance.abs() * 2.0;
            let clamped_offset = offset_dist.clamp(-max_offset, max_offset);

            let offset_pt = Point2::new(
                p1.x + bisector.x * clamped_offset,
                p1.y + bisector.y * clamped_offset,
            );
            offset_points.push(offset_pt);
        }

        // Check if offset polygon collapsed.
        let result = Polygon::new(offset_points);
        let area = result.signed_area();
        if !area.is_finite() || area.abs() < 1e-10 {
            return None;
        }

        Some(result)
    }

    /// Remove vertices that are collinear with their neighbors (or coincident).
    /// Cleans up the spurious "midpoint" vertices that the per-triangle slice
    /// step produces — without this, two-triangle faces leave a vertex midway
    /// along each edge of the contour, and downstream `offset()` overshoots
    /// those short segments and produces non-simple polygons.
    pub fn dedupe_collinear(&mut self, eps: f64) {
        if self.points.len() < 3 {
            return;
        }

        // 1) Drop consecutive duplicate points.
        self.points.dedup_by(|a, b| {
            let dx = a.x - b.x;
            let dy = a.y - b.y;
            (dx * dx + dy * dy).sqrt() < eps
        });
        if let (Some(first), Some(last)) = (self.points.first(), self.points.last()) {
            let dx = first.x - last.x;
            let dy = first.y - last.y;
            if (dx * dx + dy * dy).sqrt() < eps {
                self.points.pop();
            }
        }
        if self.points.len() < 3 {
            return;
        }

        // 2) Drop vertices that lie on the line between their neighbors.
        let mut keep: Vec<bool> = vec![true; self.points.len()];
        let mut changed = true;
        while changed {
            changed = false;
            let n = self.points.len();
            // Recompute "active" set after each pass.
            let active_indices: Vec<usize> =
                (0..n).filter(|&i| keep[i]).collect();
            if active_indices.len() < 3 {
                break;
            }
            for w in 0..active_indices.len() {
                let i = active_indices[w];
                let prev = active_indices[(w + active_indices.len() - 1) % active_indices.len()];
                let next = active_indices[(w + 1) % active_indices.len()];
                let p0 = self.points[prev];
                let p1 = self.points[i];
                let p2 = self.points[next];
                let cross = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
                let base = ((p2 - p0).norm()).max(1.0);
                if cross.abs() / base < eps {
                    keep[i] = false;
                    changed = true;
                    if active_indices.len() - 1 < 3 {
                        break;
                    }
                }
            }
        }

        let mut compacted = Vec::with_capacity(self.points.len());
        for (i, &pt) in self.points.iter().enumerate() {
            if keep[i] {
                compacted.push(pt);
            }
        }
        self.points = compacted;
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
