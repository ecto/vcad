//! Section view generation: plane-mesh intersection, polyline chaining, and hatching.
//!
//! This module provides functionality for generating 2D section views from 3D meshes:
//! - Plane-triangle intersection to find cut lines
//! - Segment chaining to form continuous polylines
//! - 2D projection onto the section plane
//! - Cross-hatch generation for solid regions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_tessellate::TriangleMesh;

use crate::dimension::{ArrowType, DimensionStyle};
use crate::dimension::{RenderedArrow, RenderedDimension, RenderedText, TextAlignment};
use crate::types::{
    BoundingBox2D, HatchPattern, OffsetSectionPlane, Point2D, SectionCurve, SectionPlane,
    SectionView,
};

/// Default tolerance for geometric comparisons (in mm).
const DEFAULT_TOLERANCE: f64 = 1e-6;

// ============================================================================
// Plane-Triangle Intersection
// ============================================================================

/// Intersect a single triangle with a plane.
///
/// Returns 0, 1, or 2 intersection points. When 2 points are returned,
/// they form a line segment where the plane cuts through the triangle.
fn intersect_triangle_with_plane(
    v0: Point3,
    v1: Point3,
    v2: Point3,
    plane_origin: Point3,
    plane_normal: &Vec3,
) -> Vec<Point3> {
    // Compute signed distances from each vertex to the plane
    let d0 = plane_normal.dot(v0 - plane_origin);
    let d1 = plane_normal.dot(v1 - plane_origin);
    let d2 = plane_normal.dot(v2 - plane_origin);

    let tol = DEFAULT_TOLERANCE;

    // Classify vertices
    let on0 = d0.abs() < tol;
    let on1 = d1.abs() < tol;
    let on2 = d2.abs() < tol;

    let pos0 = d0 > tol;
    let pos1 = d1 > tol;
    let pos2 = d2 > tol;

    let neg0 = d0 < -tol;
    let neg1 = d1 < -tol;
    let neg2 = d2 < -tol;

    let mut points = Vec::new();

    // Helper to compute edge-plane intersection
    let intersect_edge = |p0: Point3, p1: Point3, d0: f64, d1: f64| -> Point3 {
        let t = d0 / (d0 - d1);
        Point3::new(
            p0.x + t * (p1.x - p0.x),
            p0.y + t * (p1.y - p0.y),
            p0.z + t * (p1.z - p0.z),
        )
    };

    // Check each edge for intersection
    // Edge v0-v1
    if on0 {
        points.push(v0);
    }
    if on1 && !points.iter().any(|p| (*p - v1).norm() < tol) {
        points.push(v1);
    }
    if on2 && !points.iter().any(|p| (*p - v2).norm() < tol) {
        points.push(v2);
    }

    // Edge v0-v1 (if vertices are on opposite sides)
    if (pos0 && neg1) || (neg0 && pos1) {
        let p = intersect_edge(v0, v1, d0, d1);
        if !points.iter().any(|q| (*q - p).norm() < tol) {
            points.push(p);
        }
    }

    // Edge v1-v2
    if (pos1 && neg2) || (neg1 && pos2) {
        let p = intersect_edge(v1, v2, d1, d2);
        if !points.iter().any(|q| (*q - p).norm() < tol) {
            points.push(p);
        }
    }

    // Edge v2-v0
    if (pos2 && neg0) || (neg2 && pos0) {
        let p = intersect_edge(v2, v0, d2, d0);
        if !points.iter().any(|q| (*q - p).norm() < tol) {
            points.push(p);
        }
    }

    // Return at most 2 points (forming a segment)
    points.truncate(2);
    points
}

/// Intersect a mesh with a plane, returning 3D line segments.
///
/// Each segment is a pair of 3D points where the plane cuts through the mesh.
pub fn intersect_mesh_with_plane(
    mesh: &TriangleMesh,
    plane_origin: Point3,
    plane_normal: Vec3,
) -> Vec<(Point3, Point3)> {
    let normal = plane_normal.normalize();
    let mut segments = Vec::new();

    let num_tris = mesh.indices.len() / 3;
    for i in 0..num_tris {
        let i0 = mesh.indices[i * 3] as usize;
        let i1 = mesh.indices[i * 3 + 1] as usize;
        let i2 = mesh.indices[i * 3 + 2] as usize;

        let v0 = Point3::new(
            mesh.vertices[i0 * 3] as f64,
            mesh.vertices[i0 * 3 + 1] as f64,
            mesh.vertices[i0 * 3 + 2] as f64,
        );
        let v1 = Point3::new(
            mesh.vertices[i1 * 3] as f64,
            mesh.vertices[i1 * 3 + 1] as f64,
            mesh.vertices[i1 * 3 + 2] as f64,
        );
        let v2 = Point3::new(
            mesh.vertices[i2 * 3] as f64,
            mesh.vertices[i2 * 3 + 1] as f64,
            mesh.vertices[i2 * 3 + 2] as f64,
        );

        let pts = intersect_triangle_with_plane(v0, v1, v2, plane_origin, &normal);
        if pts.len() == 2 {
            segments.push((pts[0], pts[1]));
        }
    }

    segments
}

// ============================================================================
// Segment Chaining
// ============================================================================

/// Key for endpoint lookup with tolerance-based hashing.
fn point_key(p: &Point3, tolerance: f64) -> (i64, i64, i64) {
    let scale = 1.0 / tolerance;
    (
        (p.x * scale).round() as i64,
        (p.y * scale).round() as i64,
        (p.z * scale).round() as i64,
    )
}

/// Chain individual segments into continuous polylines.
///
/// Uses tolerance-based endpoint matching to connect segments that share endpoints.
/// Returns a list of polylines, each marked as closed or open.
pub fn chain_segments(segments: Vec<(Point3, Point3)>, tolerance: f64) -> Vec<(Vec<Point3>, bool)> {
    if segments.is_empty() {
        return Vec::new();
    }

    // Build adjacency map: point_key -> list of (segment_index, is_end_point)
    let mut adjacency: HashMap<(i64, i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, (p0, p1)) in segments.iter().enumerate() {
        let k0 = point_key(p0, tolerance);
        let k1 = point_key(p1, tolerance);
        adjacency.entry(k0).or_default().push((i, false)); // false = start point
        adjacency.entry(k1).or_default().push((i, true)); // true = end point
    }

    let mut used = vec![false; segments.len()];
    let mut polylines = Vec::new();

    for start_idx in 0..segments.len() {
        if used[start_idx] {
            continue;
        }

        // Start a new chain
        let mut chain = Vec::new();
        let (p0, p1) = segments[start_idx];
        chain.push(p0);
        chain.push(p1);
        used[start_idx] = true;

        // Extend forward from p1
        let mut current = p1;
        loop {
            let key = point_key(&current, tolerance);
            let mut found = false;

            if let Some(neighbors) = adjacency.get(&key) {
                for &(seg_idx, is_end) in neighbors {
                    if used[seg_idx] {
                        continue;
                    }

                    let (s0, s1) = segments[seg_idx];
                    let (next_pt, _match_pt) = if is_end {
                        (s0, s1) // matched at end, so next is start
                    } else {
                        (s1, s0) // matched at start, so next is end
                    };

                    chain.push(next_pt);
                    current = next_pt;
                    used[seg_idx] = true;
                    found = true;
                    break;
                }
            }

            if !found {
                break;
            }
        }

        // Extend backward from p0
        let mut current = p0;
        loop {
            let key = point_key(&current, tolerance);
            let mut found = false;

            if let Some(neighbors) = adjacency.get(&key) {
                for &(seg_idx, is_end) in neighbors {
                    if used[seg_idx] {
                        continue;
                    }

                    let (s0, s1) = segments[seg_idx];
                    let (next_pt, _match_pt) = if is_end { (s0, s1) } else { (s1, s0) };

                    chain.insert(0, next_pt);
                    current = next_pt;
                    used[seg_idx] = true;
                    found = true;
                    break;
                }
            }

            if !found {
                break;
            }
        }

        // Check if closed (first point equals last point within tolerance)
        let is_closed = chain.len() >= 3 && (chain[0] - *chain.last().unwrap()).norm() < tolerance;

        // Remove duplicate endpoint if closed
        if is_closed && chain.len() > 1 {
            chain.pop();
        }

        polylines.push((chain, is_closed));
    }

    polylines
}

// ============================================================================
// 2D Projection
// ============================================================================

/// Project 3D polylines onto the cutting plane, returning 2D section curves.
///
/// Builds an orthonormal frame on the plane using the normal and up vectors.
pub fn project_to_section_plane(
    polylines: &[(Vec<Point3>, bool)],
    plane: &SectionPlane,
) -> Vec<SectionCurve> {
    // Build orthonormal frame on the plane
    let normal = plane.normal_vec().normalize();
    let up = plane.up_vec().normalize();
    let origin = plane.origin_point();

    // right = up × normal (to get a right-handed coordinate system)
    let right = up.cross(normal).normalize();
    // Recompute actual_up to ensure orthogonality
    let actual_up = normal.cross(right);

    let project = |p: &Point3| -> Point2D {
        let d = *p - origin;
        Point2D::new(d.dot(right), d.dot(actual_up))
    };

    polylines
        .iter()
        .map(|(pts, is_closed)| {
            let points_2d: Vec<Point2D> = pts.iter().map(&project).collect();
            SectionCurve::new(points_2d, *is_closed)
        })
        .collect()
}

// ============================================================================
// Hatch Generation
// ============================================================================

/// Generate hatch lines for a region with optional holes.
///
/// Creates parallel lines at the specified angle and spacing, clipped to the
/// boundary polygon and excluding any holes.
pub fn generate_hatch_lines(
    boundary: &[Point2D],
    holes: &[Vec<Point2D>],
    pattern: &HatchPattern,
) -> Vec<(Point2D, Point2D)> {
    if boundary.len() < 3 || pattern.spacing <= 0.0 {
        return Vec::new();
    }

    // Compute bounding box of the boundary
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for p in boundary {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }

    // Expand bounds slightly for safety
    let margin = pattern.spacing * 2.0;
    min_x -= margin;
    max_x += margin;
    min_y -= margin;
    max_y += margin;

    // Hatch direction
    let cos_a = pattern.angle.cos();
    let sin_a = pattern.angle.sin();

    // Direction vector along hatch lines
    let dir = Point2D::new(cos_a, sin_a);
    // Perpendicular direction (for stepping between lines)
    let perp = Point2D::new(-sin_a, cos_a);

    // Find the range of perpendicular offsets that cover the bounding box
    let corners = [
        Point2D::new(min_x, min_y),
        Point2D::new(max_x, min_y),
        Point2D::new(max_x, max_y),
        Point2D::new(min_x, max_y),
    ];

    let mut min_offset = f64::INFINITY;
    let mut max_offset = f64::NEG_INFINITY;
    for c in &corners {
        let offset = c.x * perp.x + c.y * perp.y;
        min_offset = min_offset.min(offset);
        max_offset = max_offset.max(offset);
    }

    let mut hatch_lines = Vec::new();

    // Generate hatch lines at regular intervals
    let mut offset = min_offset;
    while offset <= max_offset {
        // Line: all points P where P·perp = offset
        // Parametric: P = origin + t * dir, where origin·perp = offset

        // Find a point on this line
        let origin = Point2D::new(perp.x * offset, perp.y * offset);

        // Find intersection with bounding box to get line extent
        let t_min = -1000.0; // Large enough to cover any reasonable model
        let t_max = 1000.0;

        let line_start = Point2D::new(origin.x + t_min * dir.x, origin.y + t_min * dir.y);
        let line_end = Point2D::new(origin.x + t_max * dir.x, origin.y + t_max * dir.y);

        // Clip line to boundary polygon
        let clipped = clip_line_to_polygon(&line_start, &line_end, boundary);

        // For each clipped segment, subtract holes
        for (seg_start, seg_end) in clipped {
            let final_segments = subtract_holes_from_segment(&seg_start, &seg_end, holes, &dir);
            hatch_lines.extend(final_segments);
        }

        offset += pattern.spacing;
    }

    hatch_lines
}

/// Clip a line segment to a polygon using scanline intersection.
///
/// Returns segments that are inside the polygon.
fn clip_line_to_polygon(
    line_start: &Point2D,
    line_end: &Point2D,
    polygon: &[Point2D],
) -> Vec<(Point2D, Point2D)> {
    if polygon.len() < 3 {
        return Vec::new();
    }

    // Direction and length of line
    let dx = line_end.x - line_start.x;
    let dy = line_end.y - line_start.y;
    let line_len = (dx * dx + dy * dy).sqrt();

    if line_len < DEFAULT_TOLERANCE {
        return Vec::new();
    }

    // Find all intersections with polygon edges
    let mut intersections: Vec<f64> = Vec::new(); // t parameters along the line

    let n = polygon.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let e0 = &polygon[i];
        let e1 = &polygon[j];

        if let Some(t) = line_segment_intersection(line_start, line_end, e0, e1) {
            intersections.push(t);
        }
    }

    // Sort intersections
    intersections.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Remove duplicates
    intersections.dedup_by(|a, b| (*a - *b).abs() < DEFAULT_TOLERANCE);

    // Build segments: every other pair of intersections is inside
    let mut segments = Vec::new();

    if intersections.len() < 2 {
        return segments;
    }

    for i in (0..intersections.len() - 1).step_by(2) {
        let t0 = intersections[i];
        let t1 = intersections[i + 1];

        // Check if midpoint is inside polygon
        let mid_t = (t0 + t1) / 2.0;
        let mid = Point2D::new(line_start.x + mid_t * dx, line_start.y + mid_t * dy);

        if point_in_polygon(&mid, polygon) {
            let p0 = Point2D::new(line_start.x + t0 * dx, line_start.y + t0 * dy);
            let p1 = Point2D::new(line_start.x + t1 * dx, line_start.y + t1 * dy);
            segments.push((p0, p1));
        }
    }

    // Also check pairs starting from index 1
    for i in (1..intersections.len() - 1).step_by(2) {
        let t0 = intersections[i];
        let t1 = intersections[i + 1];

        let mid_t = (t0 + t1) / 2.0;
        let mid = Point2D::new(line_start.x + mid_t * dx, line_start.y + mid_t * dy);

        if point_in_polygon(&mid, polygon) {
            let p0 = Point2D::new(line_start.x + t0 * dx, line_start.y + t0 * dy);
            let p1 = Point2D::new(line_start.x + t1 * dx, line_start.y + t1 * dy);

            // Check for duplicates
            let is_dup = segments.iter().any(|(a, b)| {
                (a.x - p0.x).abs() < DEFAULT_TOLERANCE
                    && (a.y - p0.y).abs() < DEFAULT_TOLERANCE
                    && (b.x - p1.x).abs() < DEFAULT_TOLERANCE
                    && (b.y - p1.y).abs() < DEFAULT_TOLERANCE
            });

            if !is_dup {
                segments.push((p0, p1));
            }
        }
    }

    segments
}

/// Subtract holes from a line segment.
fn subtract_holes_from_segment(
    seg_start: &Point2D,
    seg_end: &Point2D,
    holes: &[Vec<Point2D>],
    _dir: &Point2D,
) -> Vec<(Point2D, Point2D)> {
    let mut current_segments = vec![(*seg_start, *seg_end)];

    for hole in holes {
        if hole.len() < 3 {
            continue;
        }

        let mut new_segments = Vec::new();

        for (s0, s1) in current_segments {
            // Split this segment by the hole
            let split = split_segment_by_polygon(&s0, &s1, hole);
            new_segments.extend(split);
        }

        current_segments = new_segments;
    }

    current_segments
}

/// Split a segment by removing the portion inside a polygon.
fn split_segment_by_polygon(
    seg_start: &Point2D,
    seg_end: &Point2D,
    polygon: &[Point2D],
) -> Vec<(Point2D, Point2D)> {
    let dx = seg_end.x - seg_start.x;
    let dy = seg_end.y - seg_start.y;
    let seg_len = (dx * dx + dy * dy).sqrt();

    if seg_len < DEFAULT_TOLERANCE {
        return Vec::new();
    }

    // Find intersections with polygon
    let mut intersections: Vec<f64> = Vec::new();

    let n = polygon.len();
    for i in 0..n {
        let j = (i + 1) % n;
        if let Some(t) = line_segment_intersection(seg_start, seg_end, &polygon[i], &polygon[j]) {
            if (0.0..=1.0).contains(&t) {
                intersections.push(t);
            }
        }
    }

    // Add start and end
    intersections.push(0.0);
    intersections.push(1.0);

    // Sort and deduplicate
    intersections.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    intersections.dedup_by(|a, b| (*a - *b).abs() < DEFAULT_TOLERANCE);

    // Keep segments whose midpoint is outside the hole
    let mut result = Vec::new();

    for i in 0..intersections.len() - 1 {
        let t0 = intersections[i];
        let t1 = intersections[i + 1];

        if (t1 - t0).abs() < DEFAULT_TOLERANCE {
            continue;
        }

        let mid_t = (t0 + t1) / 2.0;
        let mid = Point2D::new(seg_start.x + mid_t * dx, seg_start.y + mid_t * dy);

        // Keep if outside the hole
        if !point_in_polygon(&mid, polygon) {
            let p0 = Point2D::new(seg_start.x + t0 * dx, seg_start.y + t0 * dy);
            let p1 = Point2D::new(seg_start.x + t1 * dx, seg_start.y + t1 * dy);
            result.push((p0, p1));
        }
    }

    result
}

/// Line-line intersection, returning t parameter for first line if intersecting.
fn line_segment_intersection(
    p0: &Point2D,
    p1: &Point2D,
    e0: &Point2D,
    e1: &Point2D,
) -> Option<f64> {
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let ex = e1.x - e0.x;
    let ey = e1.y - e0.y;

    let denom = dx * ey - dy * ex;

    if denom.abs() < DEFAULT_TOLERANCE {
        return None; // Parallel
    }

    let t = ((e0.x - p0.x) * ey - (e0.y - p0.y) * ex) / denom;
    let s = ((e0.x - p0.x) * dy - (e0.y - p0.y) * dx) / denom;

    // Check if intersection is on the edge segment
    if (0.0..=1.0).contains(&s) {
        Some(t)
    } else {
        None
    }
}

/// Point-in-polygon test using ray casting.
fn point_in_polygon(p: &Point2D, polygon: &[Point2D]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }

    let mut inside = false;

    let mut j = n - 1;
    for i in 0..n {
        let vi = &polygon[i];
        let vj = &polygon[j];

        if ((vi.y > p.y) != (vj.y > p.y))
            && (p.x < (vj.x - vi.x) * (p.y - vi.y) / (vj.y - vi.y) + vi.x)
        {
            inside = !inside;
        }

        j = i;
    }

    inside
}

// ============================================================================
// Main Entry Point
// ============================================================================

/// Generate a section view by cutting a mesh with a plane.
///
/// # Arguments
/// * `mesh` - Triangle mesh to section
/// * `plane` - Section plane definition
/// * `hatch_pattern` - Optional hatch pattern for cross-hatching
///
/// # Returns
/// A `SectionView` containing the intersection curves and optional hatch lines.
pub fn section_mesh(
    mesh: &TriangleMesh,
    plane: &SectionPlane,
    hatch_pattern: Option<&HatchPattern>,
) -> SectionView {
    // Step 1: Intersect mesh with plane
    let segments = intersect_mesh_with_plane(mesh, plane.origin_point(), plane.normal_vec());

    if segments.is_empty() {
        return SectionView::new();
    }

    // Step 2: Chain segments into polylines
    let tolerance = DEFAULT_TOLERANCE * 100.0; // Use slightly larger tolerance for chaining
    let polylines = chain_segments(segments, tolerance);

    // Step 3: Project to 2D
    let curves = project_to_section_plane(&polylines, plane);

    // Step 4: Compute bounds
    let mut bounds = BoundingBox2D::empty();
    for curve in &curves {
        for p in &curve.points {
            bounds.include_point(*p);
        }
    }

    // Step 5: Generate hatch lines if pattern provided
    let hatch_lines = if let Some(pattern) = hatch_pattern {
        hatch_closed_curves(&curves, pattern)
    } else {
        Vec::new()
    };

    // Update bounds to include hatch lines
    let mut final_bounds = bounds;
    for (p0, p1) in &hatch_lines {
        final_bounds.include_point(*p0);
        final_bounds.include_point(*p1);
    }

    SectionView {
        curves,
        hatch_lines,
        bounds: final_bounds,
    }
}

/// Hatch every closed curve in a set, treating closed curves contained in
/// another closed curve as holes of that boundary.
pub fn hatch_closed_curves(
    curves: &[SectionCurve],
    pattern: &HatchPattern,
) -> Vec<(Point2D, Point2D)> {
    let mut all_hatch_lines = Vec::new();

    for curve in curves {
        if curve.is_closed && curve.points.len() >= 3 {
            // Use this closed curve as a hatch boundary; treat other closed
            // curves whose centroid lies inside it as holes.
            let holes: Vec<Vec<Point2D>> = curves
                .iter()
                .filter(|c| c.is_closed && c.points.len() >= 3)
                .filter(|c| {
                    if c.points.is_empty() || std::ptr::eq(*c, curve) {
                        return false;
                    }
                    let center = c.points.iter().fold(Point2D::new(0.0, 0.0), |acc, p| {
                        Point2D::new(acc.x + p.x, acc.y + p.y)
                    });
                    let n = c.points.len() as f64;
                    let center = Point2D::new(center.x / n, center.y / n);
                    point_in_polygon(&center, &curve.points)
                })
                .map(|c| c.points.clone())
                .collect();

            let lines = generate_hatch_lines(&curve.points, &holes, pattern);
            all_hatch_lines.extend(lines);
        }
    }

    all_hatch_lines
}

// ============================================================================
// Offset (Stepped) Sections
// ============================================================================

/// Clip a closed polygon to the vertical strip `u_start <= x <= u_end`
/// using Sutherland–Hodgman against the two half-planes.
fn clip_polygon_to_strip(points: &[Point2D], u_start: f64, u_end: f64) -> Vec<Point2D> {
    let clip_half = |poly: &[Point2D],
                     inside: &dyn Fn(&Point2D) -> bool,
                     intersect: &dyn Fn(&Point2D, &Point2D) -> Point2D|
     -> Vec<Point2D> {
        let mut out = Vec::new();
        let n = poly.len();
        for i in 0..n {
            let cur = poly[i];
            let prev = poly[(i + n - 1) % n];
            let cur_in = inside(&cur);
            let prev_in = inside(&prev);
            if cur_in {
                if !prev_in {
                    out.push(intersect(&prev, &cur));
                }
                out.push(cur);
            } else if prev_in {
                out.push(intersect(&prev, &cur));
            }
        }
        out
    };

    let at_x = |a: &Point2D, b: &Point2D, x: f64| -> Point2D {
        let t = (x - a.x) / (b.x - a.x);
        Point2D::new(x, a.y + t * (b.y - a.y))
    };

    let step1 = clip_half(points, &|p| p.x >= u_start, &|a, b| at_x(a, b, u_start));
    if step1.is_empty() {
        return step1;
    }
    clip_half(&step1, &|p| p.x <= u_end, &|a, b| at_x(a, b, u_end))
}

/// Clip an open polyline to the strip, splitting into sub-polylines.
fn clip_polyline_to_strip(points: &[Point2D], u_start: f64, u_end: f64) -> Vec<Vec<Point2D>> {
    let mut result = Vec::new();
    let mut current: Vec<Point2D> = Vec::new();

    let inside =
        |p: &Point2D| p.x >= u_start - DEFAULT_TOLERANCE && p.x <= u_end + DEFAULT_TOLERANCE;

    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Clip segment [a, b] to the strip parametrically.
        let mut t0: f64 = 0.0;
        let mut t1: f64 = 1.0;
        let dx = b.x - a.x;
        if dx.abs() < DEFAULT_TOLERANCE {
            if !inside(&a) {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                }
                continue;
            }
        } else {
            let ta = (u_start - a.x) / dx;
            let tb = (u_end - a.x) / dx;
            let (lo, hi) = if ta < tb { (ta, tb) } else { (tb, ta) };
            t0 = t0.max(lo);
            t1 = t1.min(hi);
            if t0 > t1 {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                }
                continue;
            }
        }
        let pa = Point2D::new(a.x + t0 * dx, a.y + t0 * (b.y - a.y));
        let pb = Point2D::new(a.x + t1 * dx, a.y + t1 * (b.y - a.y));
        if current.is_empty() {
            current.push(pa);
        } else if current.last().unwrap().distance(&pa) > DEFAULT_TOLERANCE * 100.0 {
            result.push(std::mem::take(&mut current));
            current.push(pa);
        }
        current.push(pb);
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Generate an offset (stepped) section view.
///
/// Each step's parallel plane is intersected with the mesh, the resulting
/// curves are projected into the **base** plane's frame (flattening the jogs,
/// per drafting convention), and clipped to the step's U span. Closed regions
/// are hatched with `hatch_pattern` if provided.
///
/// If `plane.steps` is empty this is equivalent to [`section_mesh`] on the
/// base plane.
pub fn offset_section_mesh(
    mesh: &TriangleMesh,
    plane: &OffsetSectionPlane,
    hatch_pattern: Option<&HatchPattern>,
) -> SectionView {
    if plane.steps.is_empty() {
        return section_mesh(mesh, &plane.base, hatch_pattern);
    }

    let normal = plane.base.normal_vec().normalize();
    let base_origin = plane.base.origin_point();
    let tolerance = DEFAULT_TOLERANCE * 100.0;

    let mut curves: Vec<SectionCurve> = Vec::new();

    for step in &plane.steps {
        let origin = base_origin + normal * step.offset;
        let segments = intersect_mesh_with_plane(mesh, origin, normal);
        if segments.is_empty() {
            continue;
        }
        let polylines = chain_segments(segments, tolerance);
        // Project into the *base* frame: the offset is purely along the
        // normal, so in-plane (U, V) coordinates are unaffected.
        let projected = project_to_section_plane(&polylines, &plane.base);

        let (lo, hi) = if step.u_start <= step.u_end {
            (step.u_start, step.u_end)
        } else {
            (step.u_end, step.u_start)
        };

        for curve in projected {
            if curve.is_closed {
                let clipped = clip_polygon_to_strip(&curve.points, lo, hi);
                if clipped.len() >= 3 {
                    curves.push(SectionCurve::new(clipped, true));
                }
            } else {
                for piece in clip_polyline_to_strip(&curve.points, lo, hi) {
                    if piece.len() >= 2 {
                        curves.push(SectionCurve::new(piece, false));
                    }
                }
            }
        }
    }

    let mut bounds = BoundingBox2D::empty();
    for curve in &curves {
        for p in &curve.points {
            bounds.include_point(*p);
        }
    }

    let hatch_lines = if let Some(pattern) = hatch_pattern {
        hatch_closed_curves(&curves, pattern)
    } else {
        Vec::new()
    };

    for (p0, p1) in &hatch_lines {
        bounds.include_point(*p0);
        bounds.include_point(*p1);
    }

    SectionView {
        curves,
        hatch_lines,
        bounds,
    }
}

// ============================================================================
// Cutting-Plane Callout (parent-view annotation)
// ============================================================================

/// A cutting-plane line drawn on the parent view: the section trace with
/// viewing-direction arrows and letter labels at both ends
/// (e.g. `A —·—·—·— A` for "SECTION A-A").
///
/// The polyline is expressed in the parent view's 2D coordinates. For an
/// offset section, include the jog corners as intermediate points. Renders
/// down to [`RenderedDimension`] primitives, so every downstream consumer
/// (SVG, DXF, PDF) works unchanged; consumers should draw the polyline with
/// the phantom / dash-dot line class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionCutLine {
    /// Polyline of the cutting-plane trace in parent-view coordinates.
    pub points: Vec<Point2D>,
    /// Letter label placed at both ends (e.g. "A").
    pub label: String,
    /// Viewing direction of the section arrows, radians (0 = +X).
    pub arrow_direction: f64,
    /// Dimension style used for arrow size and text height.
    pub style: DimensionStyle,
}

impl SectionCutLine {
    /// Create a straight cutting-plane line between two points.
    pub fn straight(
        p0: Point2D,
        p1: Point2D,
        label: impl Into<String>,
        arrow_direction: f64,
    ) -> Self {
        Self {
            points: vec![p0, p1],
            label: label.into(),
            arrow_direction,
            style: DimensionStyle::default(),
        }
    }

    /// Create a cutting-plane polyline (offset section trace with jogs).
    pub fn polyline(points: Vec<Point2D>, label: impl Into<String>, arrow_direction: f64) -> Self {
        Self {
            points,
            label: label.into(),
            arrow_direction,
            style: DimensionStyle::default(),
        }
    }

    /// Set the dimension style.
    pub fn with_style(mut self, style: DimensionStyle) -> Self {
        self.style = style;
        self
    }

    /// The name of the resulting section view, e.g. "SECTION A-A".
    pub fn section_name(&self) -> String {
        format!("SECTION {label}-{label}", label = self.label)
    }

    /// Render the cutting-plane line into drawing primitives.
    ///
    /// Returns `None` if the polyline has fewer than two points.
    pub fn render(&self) -> Option<RenderedDimension> {
        if self.points.len() < 2 {
            return None;
        }
        let mut rd = RenderedDimension::new();

        // The trace polyline itself.
        for w in self.points.windows(2) {
            rd.add_line(w[0], w[1]);
        }

        let arrow_size = self.style.arrow_size * 1.5;
        let text_height = self.style.text_height * 1.4;
        let (dir_cos, dir_sin) = (self.arrow_direction.cos(), self.arrow_direction.sin());

        for (end, neighbor) in [
            (self.points[0], self.points[1]),
            (
                *self.points.last().unwrap(),
                self.points[self.points.len() - 2],
            ),
        ] {
            // Extend the trace slightly beyond the geometry.
            let out_len = (end.x - neighbor.x).hypot(end.y - neighbor.y);
            if out_len < 1e-12 {
                continue;
            }
            let ux = (end.x - neighbor.x) / out_len;
            let uy = (end.y - neighbor.y) / out_len;
            let ext = Point2D::new(end.x + ux * arrow_size * 1.5, end.y + uy * arrow_size * 1.5);
            rd.add_line(end, ext);

            // Viewing-direction arrow at the extended end.
            let tip = Point2D::new(
                ext.x + dir_cos * arrow_size * 2.0,
                ext.y + dir_sin * arrow_size * 2.0,
            );
            rd.add_line(ext, tip);
            rd.add_arrow(RenderedArrow::new(
                tip,
                self.arrow_direction,
                ArrowType::ClosedFilled,
                arrow_size,
            ));

            // Letter label just beyond the arrow, offset away from the trace.
            let label_pos = Point2D::new(
                ext.x + ux * text_height * 1.5,
                ext.y + uy * text_height * 1.5,
            );
            rd.add_text(
                RenderedText::new(label_pos, self.label.clone(), text_height)
                    .with_alignment(TextAlignment::MiddleCenter),
            );
        }

        Some(rd)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a simple cube mesh for testing.
    fn make_cube(size: f64) -> TriangleMesh {
        #[rustfmt::skip]
        let vertices: Vec<f32> = vec![
            0.0, 0.0, 0.0,           // 0
            size as f32, 0.0, 0.0,   // 1
            size as f32, size as f32, 0.0,  // 2
            0.0, size as f32, 0.0,   // 3
            0.0, 0.0, size as f32,   // 4
            size as f32, 0.0, size as f32,  // 5
            size as f32, size as f32, size as f32, // 6
            0.0, size as f32, size as f32, // 7
        ];

        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            // Bottom (-Z)
            0, 2, 1, 0, 3, 2,
            // Top (+Z)
            4, 5, 6, 4, 6, 7,
            // Front (-Y)
            0, 1, 5, 0, 5, 4,
            // Back (+Y)
            2, 3, 7, 2, 7, 6,
            // Left (-X)
            0, 4, 7, 0, 7, 3,
            // Right (+X)
            1, 2, 6, 1, 6, 5,
        ];

        TriangleMesh {
            vertices,
            indices,
            normals: Vec::new(),
            face_kinds: Vec::new(),
            face_ids: Vec::new(),
        }
    }

    #[test]
    fn test_triangle_no_intersection() {
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let v1 = Point3::new(1.0, 0.0, 0.0);
        let v2 = Point3::new(0.0, 1.0, 0.0);

        // Plane above the triangle
        let origin = Point3::new(0.0, 0.0, 10.0);
        let normal = Vec3::new(0.0, 0.0, 1.0);

        let pts = intersect_triangle_with_plane(v0, v1, v2, origin, &normal);
        assert!(pts.is_empty() || pts.len() < 2);
    }

    #[test]
    fn test_triangle_edge_intersection() {
        let v0 = Point3::new(0.0, 0.0, -1.0);
        let v1 = Point3::new(1.0, 0.0, 1.0);
        let v2 = Point3::new(0.0, 1.0, -1.0);

        // Plane at z=0
        let origin = Point3::new(0.0, 0.0, 0.0);
        let normal = Vec3::new(0.0, 0.0, 1.0);

        let pts = intersect_triangle_with_plane(v0, v1, v2, origin, &normal);
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn test_cube_horizontal_section() {
        let mesh = make_cube(10.0);

        // Section at z=5 (middle of cube)
        let plane = SectionPlane::horizontal(5.0);
        let view = section_mesh(&mesh, &plane, None);

        // Should produce one closed curve (may have more points than 4 due to triangulation)
        assert_eq!(view.curves.len(), 1, "Should have 1 curve");
        assert!(view.curves[0].is_closed, "Curve should be closed");
        // Each face has 2 triangles, plane cuts 4 faces, each triangle cut adds 1 edge point
        // So we expect 8 points (2 per face side)
        assert!(
            view.curves[0].points.len() >= 4,
            "Should have at least 4 vertices, got {}",
            view.curves[0].points.len()
        );

        // Bounds should be 10x10
        let width = view.bounds.width();
        let height = view.bounds.height();
        assert!(
            (width - 10.0).abs() < 0.1,
            "Width should be ~10, got {width}"
        );
        assert!(
            (height - 10.0).abs() < 0.1,
            "Height should be ~10, got {height}"
        );
    }

    #[test]
    fn test_cube_section_with_hatch() {
        let mesh = make_cube(10.0);
        let plane = SectionPlane::horizontal(5.0);
        let pattern = HatchPattern::new(1.0, 0.0); // 1mm horizontal hatching

        let view = section_mesh(&mesh, &plane, Some(&pattern));

        assert!(!view.curves.is_empty(), "Should have curves");
        assert!(!view.hatch_lines.is_empty(), "Should have hatch lines");

        // With 1mm spacing over 10mm, should have ~10 hatch lines
        let num_hatch = view.hatch_lines.len();
        assert!(
            num_hatch >= 5,
            "Should have at least 5 hatch lines, got {num_hatch}"
        );
    }

    #[test]
    fn test_cube_outside_section() {
        let mesh = make_cube(10.0);

        // Section outside the cube (z=20)
        let plane = SectionPlane::horizontal(20.0);
        let view = section_mesh(&mesh, &plane, None);

        assert!(view.curves.is_empty(), "Should have no curves");
    }

    #[test]
    fn test_point_in_polygon() {
        let square = vec![
            Point2D::new(0.0, 0.0),
            Point2D::new(10.0, 0.0),
            Point2D::new(10.0, 10.0),
            Point2D::new(0.0, 10.0),
        ];

        assert!(point_in_polygon(&Point2D::new(5.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2D::new(15.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2D::new(-5.0, 5.0), &square));
    }

    #[test]
    fn test_section_plane_helpers() {
        let horiz = SectionPlane::horizontal(5.0);
        assert!((horiz.origin[2] - 5.0).abs() < 1e-10);
        assert!((horiz.normal[2] - 1.0).abs() < 1e-10);

        let front = SectionPlane::front(3.0);
        assert!((front.origin[1] - 3.0).abs() < 1e-10);

        let right = SectionPlane::right(2.0);
        assert!((right.origin[0] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_offset_section_no_steps_equals_full_section() {
        let mesh = make_cube(10.0);
        let plane = OffsetSectionPlane::new(SectionPlane::horizontal(5.0), Vec::new());
        let view = offset_section_mesh(&mesh, &plane, None);
        assert_eq!(view.curves.len(), 1);
        assert!(view.curves[0].is_closed);
    }

    #[test]
    fn test_offset_section_two_steps_covers_cube() {
        let mesh = make_cube(10.0);
        // Horizontal base at z=3; second half of the U span jogs up to z=7.
        // U axis for horizontal() is up × normal = (0,1,0) × (0,0,1) = +X.
        let base = SectionPlane::horizontal(3.0);
        let plane = OffsetSectionPlane::new(
            base,
            vec![
                crate::types::OffsetSectionStep::new(-1.0, 5.0, 0.0),
                crate::types::OffsetSectionStep::new(5.0, 11.0, 4.0),
            ],
        );
        let view = offset_section_mesh(&mesh, &plane, Some(&HatchPattern::horizontal(1.0)));

        // Both steps cut the cube: two clipped closed regions, flattened
        // into a single 10×10 footprint.
        assert_eq!(view.curves.len(), 2, "one region per step");
        assert!(view.curves.iter().all(|c| c.is_closed));
        assert!((view.bounds.width() - 10.0).abs() < 0.1);
        assert!(!view.hatch_lines.is_empty(), "clipped regions are hatched");

        // Each region spans exactly its strip (5 mm wide).
        for curve in &view.curves {
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &curve.points {
                lo = lo.min(p.x);
                hi = hi.max(p.x);
            }
            assert!((hi - lo - 5.0).abs() < 0.1, "strip width, got {}", hi - lo);
        }
    }

    #[test]
    fn test_offset_section_step_outside_solid_is_empty() {
        let mesh = make_cube(10.0);
        let plane = OffsetSectionPlane::new(
            SectionPlane::horizontal(5.0),
            vec![crate::types::OffsetSectionStep::new(0.0, 10.0, 20.0)],
        );
        let view = offset_section_mesh(&mesh, &plane, None);
        assert!(view.curves.is_empty());
    }

    #[test]
    fn test_section_cut_line_render() {
        let cut = SectionCutLine::straight(
            Point2D::new(0.0, 5.0),
            Point2D::new(10.0, 5.0),
            "A",
            std::f64::consts::FRAC_PI_2,
        );
        assert_eq!(cut.section_name(), "SECTION A-A");
        let rd = cut.render().expect("renders");
        // Trace + 2 extensions + 2 arrow stems = 5 lines, 2 arrows, 2 labels.
        assert_eq!(rd.lines.len(), 5);
        assert_eq!(rd.arrows.len(), 2);
        assert_eq!(rd.texts.len(), 2);
        assert!(rd.texts.iter().all(|t| t.text == "A"));
    }

    #[test]
    fn test_section_cut_line_polyline_jogs() {
        let cut = SectionCutLine::polyline(
            vec![
                Point2D::new(0.0, 5.0),
                Point2D::new(5.0, 5.0),
                Point2D::new(5.0, 8.0),
                Point2D::new(10.0, 8.0),
            ],
            "B",
            std::f64::consts::FRAC_PI_2,
        );
        let rd = cut.render().unwrap();
        // 3 trace segments + 2 extensions + 2 arrow stems.
        assert_eq!(rd.lines.len(), 7);
    }

    #[test]
    fn test_section_cut_line_degenerate() {
        let cut = SectionCutLine::polyline(vec![Point2D::new(0.0, 0.0)], "A", 0.0);
        assert!(cut.render().is_none());
    }

    #[test]
    fn test_hatch_pattern_default() {
        let pattern = HatchPattern::default();
        assert!((pattern.angle - std::f64::consts::FRAC_PI_4).abs() < 1e-10);
        assert!((pattern.spacing - 2.0).abs() < 1e-10);
    }
}
