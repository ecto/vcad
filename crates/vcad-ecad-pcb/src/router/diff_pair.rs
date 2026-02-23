//! Differential pair router.
//!
//! Routes two traces in parallel with a controlled gap, maintaining
//! impedance matching for high-speed differential signals (USB, HDMI,
//! Ethernet, etc.).
//!
//! The algorithm:
//! 1. Compute a center path from midpoint(start_p, start_n) to midpoint(end_p, end_n).
//! 2. Offset the center polyline by +/- half_sep to produce P and N paths.
//! 3. Handle corners with mitered bends (proper offset polyline geometry).
//! 4. Check phase length matching between P and N.
//! 5. Insert meanders on the shorter trace if the mismatch exceeds tolerance.

use vcad_ir::Vec2;

use super::length_tune::{self, LengthTuneParams, MeanderStyle};
use super::RouteResult;

/// Differential pair router.
///
/// Routes P/N trace pairs with controlled spacing for impedance matching.
pub struct DiffPairRouter {
    /// Width of each trace in the pair (mm).
    trace_width: f64,
    /// Gap between the two traces (mm).
    gap: f64,
    /// Maximum allowed phase length mismatch (mm) before inserting meanders.
    pub phase_tolerance: f64,
}

impl DiffPairRouter {
    /// Create a new differential pair router.
    ///
    /// # Arguments
    ///
    /// * `trace_width` -- Width of each trace in the pair (mm).
    /// * `gap` -- Gap between the two traces (mm).
    pub fn new(trace_width: f64, gap: f64) -> Self {
        Self {
            trace_width,
            gap,
            phase_tolerance: 0.01,
        }
    }

    /// Half the center-to-center separation between P and N traces.
    fn half_sep(&self) -> f64 {
        (self.trace_width + self.gap) / 2.0
    }

    /// Route a differential pair from start pads to end pads.
    ///
    /// Computes a center path, offsets it to produce P and N traces, handles
    /// corner mitering, checks phase matching, and inserts meanders if needed.
    ///
    /// # Arguments
    ///
    /// * `net_p` -- Net name for the positive trace.
    /// * `net_n` -- Net name for the negative trace.
    /// * `start_p` -- Start position of the P trace (mm).
    /// * `start_n` -- Start position of the N trace (mm).
    /// * `end_p` -- End position of the P trace (mm).
    /// * `end_n` -- End position of the N trace (mm).
    pub fn route_pair(
        &mut self,
        net_p: &str,
        net_n: &str,
        start_p: Vec2,
        start_n: Vec2,
        end_p: Vec2,
        end_n: Vec2,
    ) -> (RouteResult, RouteResult) {
        // Step 1: Build center path from midpoint(start) to midpoint(end).
        let center_start = midpoint(start_p, start_n);
        let center_end = midpoint(end_p, end_n);
        let center_path = vec![center_start, center_end];

        // Step 2 & 3: Offset center path to get P and N polylines.
        let half = self.half_sep();
        let mut path_p = offset_polyline(&center_path, half);
        let mut path_n = offset_polyline(&center_path, -half);

        // Step 4: Check phase length matching.
        let len_p = length_tune::path_length(&path_p);
        let len_n = length_tune::path_length(&path_n);
        let mismatch = (len_p - len_n).abs();

        // Step 5: Insert meanders on the shorter trace if mismatch exceeds tolerance.
        if mismatch > self.phase_tolerance {
            let meander_params = |target: f64| LengthTuneParams {
                target_length: target,
                max_amplitude: self.gap * 2.0,
                spacing: self.trace_width * 4.0,
                style: MeanderStyle::Trombone,
            };

            if len_p < len_n {
                let params = meander_params(len_n);
                if let Some(meanders) = length_tune::generate_meanders(&path_p, &params) {
                    path_p = apply_meanders(&path_p, &meanders);
                }
            } else {
                let params = meander_params(len_p);
                if let Some(meanders) = length_tune::generate_meanders(&path_n, &params) {
                    path_n = apply_meanders(&path_n, &meanders);
                }
            }
        }

        let segments_p = polyline_to_segments(&path_p);
        let segments_n = polyline_to_segments(&path_n);

        (
            RouteResult {
                net: net_p.to_string(),
                segments: segments_p,
                vias: vec![],
                success: true,
            },
            RouteResult {
                net: net_n.to_string(),
                segments: segments_n,
                vias: vec![],
                success: true,
            },
        )
    }
}

/// Midpoint of two points.
fn midpoint(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// Offset a polyline by `distance` along the left-hand normal.
///
/// Positive distance offsets to the left of the travel direction (which is
/// the `perp()` side), negative to the right. Corners are handled with
/// mitered joins -- the intersection of adjacent offset edges.
fn offset_polyline(points: &[Vec2], distance: f64) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }

    // For a two-point polyline, just offset both endpoints.
    if points.len() == 2 {
        let dir = (points[1] - points[0]).normalize();
        let normal = dir.perp();
        let offset = normal.scale(distance);
        return vec![points[0] + offset, points[1] + offset];
    }

    // General case: offset each segment, then miter at corners.
    let n = points.len();
    let mut result = Vec::with_capacity(n);

    // Compute per-segment normals.
    let mut normals = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let dir = (points[i + 1] - points[i]).normalize();
        normals.push(dir.perp());
    }

    // First point: offset along the first segment's normal.
    result.push(points[0] + normals[0].scale(distance));

    // Interior points: miter join.
    for i in 1..n - 1 {
        let n0 = normals[i - 1];
        let n1 = normals[i];
        // Average normal, then scale to preserve offset distance.
        let avg = n0 + n1;
        let avg_len = avg.length();
        if avg_len < 1e-12 {
            // Segments are (anti)parallel -- just offset from first normal.
            result.push(points[i] + n0.scale(distance));
        } else {
            let bisector = avg.scale(1.0 / avg_len);
            // The miter scale factor: distance / cos(half_angle).
            let cos_half = bisector.dot(n0);
            if cos_half.abs() < 1e-12 {
                result.push(points[i] + n0.scale(distance));
            } else {
                result.push(points[i] + bisector.scale(distance / cos_half));
            }
        }
    }

    // Last point: offset along the last segment's normal.
    result.push(points[n - 1] + normals[n - 2].scale(distance));

    result
}

/// Convert a polyline (list of points) to a list of (start, end) segments.
fn polyline_to_segments(points: &[Vec2]) -> Vec<(Vec2, Vec2)> {
    if points.len() < 2 {
        return vec![];
    }
    let mut segments = Vec::with_capacity(points.len() - 1);
    for i in 0..points.len() - 1 {
        segments.push((points[i], points[i + 1]));
    }
    segments
}

/// Apply meander segments to a polyline, producing a new polyline with
/// meanders spliced in.
fn apply_meanders(original: &[Vec2], meanders: &[length_tune::MeanderSegment]) -> Vec<Vec2> {
    if meanders.is_empty() {
        return original.to_vec();
    }

    let mut result = Vec::new();
    let mut seg_idx = 0;

    for i in 0..original.len().saturating_sub(1) {
        if seg_idx < meanders.len() && meanders[seg_idx].segment_index == i {
            // Replace this segment with meander waypoints.
            let meander = &meanders[seg_idx];
            // Skip the first point of the meander if we already have it.
            if result.is_empty() {
                result.extend_from_slice(&meander.points);
            } else {
                result.extend_from_slice(&meander.points[1..]);
            }
            seg_idx += 1;
        } else {
            // Keep original segment.
            if result.is_empty() {
                result.push(original[i]);
            }
            result.push(original[i + 1]);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_pair() {
        // Straight horizontal route: P on top, N on bottom.
        let mut router = DiffPairRouter::new(0.2, 0.15);
        let (p, n) = router.route_pair(
            "USB_D+",
            "USB_D-",
            Vec2::new(0.0, 0.175),
            Vec2::new(0.0, -0.175),
            Vec2::new(10.0, 0.175),
            Vec2::new(10.0, -0.175),
        );
        assert!(p.success);
        assert!(n.success);
        assert_eq!(p.segments.len(), 1);
        assert_eq!(n.segments.len(), 1);

        // The gap between parallel traces should equal trace_width + gap = 0.35mm
        // (center-to-center). The P trace should be above the N trace.
        let p_y = p.segments[0].0.y;
        let n_y = n.segments[0].0.y;
        let separation = (p_y - n_y).abs();
        let expected_sep = 0.2 + 0.15; // trace_width + gap
        assert!(
            (separation - expected_sep).abs() < 1e-10,
            "separation={separation}, expected={expected_sep}"
        );
    }

    #[test]
    fn angled_pair() {
        // 45-degree route.
        let mut router = DiffPairRouter::new(0.2, 0.15);
        let half_sep = router.half_sep();

        // Start pads offset perpendicular to the 45-degree direction.
        let dir = Vec2::new(1.0, 1.0).normalize();
        let normal = dir.perp();

        let start_center = Vec2::new(0.0, 0.0);
        let end_center = Vec2::new(10.0, 10.0);

        let start_p = start_center + normal.scale(half_sep);
        let start_n = start_center - normal.scale(half_sep);
        let end_p = end_center + normal.scale(half_sep);
        let end_n = end_center - normal.scale(half_sep);

        let (p, n) = router.route_pair("D+", "D-", start_p, start_n, end_p, end_n);
        assert!(p.success);
        assert!(n.success);

        // Both should have segments.
        assert!(!p.segments.is_empty());
        assert!(!n.segments.is_empty());

        // The P trace midpoint should be offset from center in the normal direction.
        let p_mid = midpoint(p.segments[0].0, p.segments[0].1);
        let center_mid = midpoint(start_center, end_center);
        let offset_vec = p_mid - center_mid;
        let perp_component = offset_vec.dot(normal);
        assert!(
            perp_component > 0.0,
            "P trace should be on the positive-normal side"
        );
    }

    #[test]
    fn phase_matched() {
        // Straight route: P and N should have identical lengths.
        let mut router = DiffPairRouter::new(0.2, 0.15);
        let (p, n) = router.route_pair(
            "D+",
            "D-",
            Vec2::new(0.0, 0.175),
            Vec2::new(0.0, -0.175),
            Vec2::new(20.0, 0.175),
            Vec2::new(20.0, -0.175),
        );

        let len_p = trace_length(&p.segments);
        let len_n = trace_length(&n.segments);
        let mismatch = (len_p - len_n).abs();
        assert!(
            mismatch <= router.phase_tolerance,
            "mismatch={mismatch}, tolerance={}",
            router.phase_tolerance
        );
    }

    #[test]
    fn route_returns_success() {
        let mut router = DiffPairRouter::new(0.25, 0.2);
        let (p, n) = router.route_pair(
            "LVDS+",
            "LVDS-",
            Vec2::new(1.0, 1.225),
            Vec2::new(1.0, 0.775),
            Vec2::new(50.0, 1.225),
            Vec2::new(50.0, 0.775),
        );
        assert!(p.success, "P route should succeed");
        assert!(n.success, "N route should succeed");
        assert_eq!(p.net, "LVDS+");
        assert_eq!(n.net, "LVDS-");
    }

    /// Helper: total length of a list of segments.
    fn trace_length(segments: &[(Vec2, Vec2)]) -> f64 {
        segments.iter().map(|(a, b)| (*b - *a).length()).sum()
    }
}
