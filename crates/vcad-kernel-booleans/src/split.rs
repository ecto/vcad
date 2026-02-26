//! Face splitting along intersection curves.
//!
//! Given a face and intersection curves that cross it, split the face
//! into sub-faces. Each sub-face inherits the original face's surface
//! but has a new trim loop.
//!
//! For Phase 2, we focus on planar face splitting by lines/segments.
//! Curved face splitting extends naturally once the planar case works.

use vcad_kernel_math::{Point2, Point3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation};

use crate::ssi::IntersectionCurve;

/// Result of splitting a face.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The face IDs of the newly created sub-faces.
    /// If no splitting occurred, contains just the original face ID.
    pub sub_faces: Vec<FaceId>,
}

/// Split a face along an intersection curve.
///
/// The curve must already be trimmed to the face's domain. This function:
/// 1. Projects the curve into UV space
/// 2. Finds where it enters/exits the face boundary
/// 3. Splits the boundary loop at entry/exit points
/// 4. Creates two new face loops
///
/// For the initial implementation, this handles the common case of a
/// planar face split by a line segment. The line must cross the face
/// boundary at exactly 2 points.
pub fn split_face_by_curve(
    brep: &mut BRepSolid,
    face_id: FaceId,
    _curve: &IntersectionCurve,
    entry_point: &Point3,
    exit_point: &Point3,
) -> SplitResult {
    // Get face info
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;
    let _surface = &brep.geometry.surfaces[surface_index];

    // Get outer loop vertices in order
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    let n = loop_verts.len();
    if n < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Find the two edges where the curve enters and exits the face
    let (entry_edge, entry_dist) = find_closest_edge_with_dist(&loop_verts, entry_point);
    let (exit_edge, exit_dist) = find_closest_edge_with_dist(&loop_verts, exit_point);

    // If entry or exit point is too far from any edge, the split line doesn't cross this face
    let max_dist_tolerance = 1.0; // Allow some tolerance for numerical precision
    if entry_dist > max_dist_tolerance || exit_dist > max_dist_tolerance {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    if entry_edge == exit_edge {
        // Curve enters and exits on the same edge — can't split simply
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Insert new vertices at entry and exit points
    let _v_entry = brep.topology.add_vertex(*entry_point);
    let _v_exit = brep.topology.add_vertex(*exit_point);

    // Build two new vertex loops by walking the original loop
    // Loop 1: entry_point → (edges from entry to exit) → exit_point → (cut back)
    // Loop 2: exit_point → (edges from exit to entry) → entry_point → (cut back)

    let mut loop1_points: Vec<Point3> = Vec::new();
    let mut loop2_points: Vec<Point3> = Vec::new();

    // Walk from entry_edge to exit_edge (one direction)
    loop1_points.push(*entry_point);
    let mut idx = (entry_edge + 1) % n;
    while idx != (exit_edge + 1) % n {
        loop1_points.push(loop_verts[idx]);
        idx = (idx + 1) % n;
    }
    loop1_points.push(*exit_point);

    // Walk from exit_edge to entry_edge (other direction)
    loop2_points.push(*exit_point);
    idx = (exit_edge + 1) % n;
    while idx != (entry_edge + 1) % n {
        loop2_points.push(loop_verts[idx]);
        idx = (idx + 1) % n;
    }
    loop2_points.push(*entry_point);

    // Remove consecutive duplicate vertices (can happen when split points
    // coincide with existing vertices)
    let loop1_points = remove_consecutive_duplicates(&loop1_points, 1e-6);
    let loop2_points = remove_consecutive_duplicates(&loop2_points, 1e-6);

    // Need at least 3 vertices for a valid face
    if loop1_points.len() < 3 || loop2_points.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Check for degenerate faces (zero or near-zero area)
    // This can happen when the split line lies along an existing edge
    fn polygon_area_3d(points: &[Point3]) -> f64 {
        if points.len() < 3 {
            return 0.0;
        }
        // Sum cross products of triangles from first vertex
        let mut total = vcad_kernel_math::Vec3::zeros();
        let p0 = points[0];
        for i in 1..points.len() - 1 {
            let e1 = points[i] - p0;
            let e2 = points[i + 1] - p0;
            total += e1.cross(&e2);
        }
        0.5 * total.norm()
    }

    let area1 = polygon_area_3d(&loop1_points);
    let area2 = polygon_area_3d(&loop2_points);
    // Minimum area threshold - faces smaller than this are considered degenerate
    // The value 0.001 catches thin strips while allowing legitimate small faces
    let min_area = 0.001;

    if area1 < min_area || area2 < min_area {
        // One of the faces is degenerate (near-zero area)
        // This happens when the split line lies along an existing edge
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create topology for the two new faces
    let face1 = create_face_from_points(brep, &loop1_points, surface_index, orientation);
    let face2 = create_face_from_points(brep, &loop2_points, surface_index, orientation);

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        // Set shell on new faces
        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face from topology (it's been replaced by sub-faces)
    brep.topology.faces.remove(face_id);

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Find which edge of a polygon a point lies closest to.
/// Returns the index of the starting vertex of that edge.
#[cfg(test)]
fn find_closest_edge(polygon: &[Point3], point: &Point3) -> usize {
    find_closest_edge_with_dist(polygon, point).0
}

/// Find which edge of a polygon a point lies closest to.
/// Returns (edge_index, distance) where edge_index is the starting vertex of that edge.
fn find_closest_edge_with_dist(polygon: &[Point3], point: &Point3) -> (usize, f64) {
    let n = polygon.len();
    let mut best = 0;
    let mut best_dist = f64::INFINITY;

    for i in 0..n {
        let j = (i + 1) % n;
        let dist = point_to_segment_dist(point, &polygon[i], &polygon[j]);
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }

    (best, best_dist)
}

/// Snap a coordinate to 0 if it's very close to 0.
/// This prevents floating point errors like -1e-15 from causing "ear" artifacts.
/// Using 1e-6 as threshold to catch numerical errors from trig operations.
fn snap_coord(v: f64) -> f64 {
    if v.abs() < 1e-6 {
        0.0
    } else {
        v
    }
}

/// Snap a point's coordinates to 0 if they're very close to 0.
fn snap_point(p: Point3) -> Point3 {
    Point3::new(snap_coord(p.x), snap_coord(p.y), snap_coord(p.z))
}

/// Find an existing vertex at the given point, or create a new one.
fn find_or_create_vertex(
    brep: &mut BRepSolid,
    point: &Point3,
    tolerance: f64,
) -> vcad_kernel_topo::VertexId {
    // Snap small values to exactly 0 to avoid floating point artifacts
    let snapped = snap_point(*point);

    // Search for existing vertex within tolerance
    for (vid, vertex) in &brep.topology.vertices {
        let dist = (vertex.point - snapped).norm();
        if dist < tolerance {
            return vid;
        }
    }
    // No existing vertex found, create new one
    brep.topology.add_vertex(snapped)
}

/// Remove consecutive duplicate points from a loop.
/// Also removes duplicates between the last and first point (to handle closed loops).
fn remove_consecutive_duplicates(points: &[Point3], tolerance: f64) -> Vec<Point3> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(points.len());
    result.push(points[0]);

    for p in points.iter().skip(1) {
        let last = result.last().unwrap();
        if (*p - *last).norm() > tolerance {
            result.push(*p);
        }
    }

    // Check if last point duplicates the first (closed loop)
    if result.len() > 1 {
        let first = result[0];
        let last = *result.last().unwrap();
        if (last - first).norm() <= tolerance {
            result.pop();
        }
    }

    result
}

/// Distance from a point to a line segment.
fn point_to_segment_dist(p: &Point3, a: &Point3, b: &Point3) -> f64 {
    let ab = b - a;
    let ap = p - a;
    let len2 = ab.norm_squared();
    if len2 < 1e-20 {
        return ap.norm();
    }
    let t = ap.dot(&ab) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj = a + t * ab;
    (p - proj).norm()
}

/// Find where an infinite line crosses the edges of a 3D polygon.
///
/// The polygon vertices must be coplanar. Returns the crossing points
/// in order along the line direction.
///
/// Uses exact orient2d predicates for robust line-segment intersection detection.
fn find_line_polygon_crossings(
    polygon: &[Point3],
    line: &vcad_kernel_geom::Line3d,
) -> Vec<Point3> {
    use vcad_kernel_math::predicates::{orient2d, Sign};

    let n = polygon.len();
    if n < 3 {
        return Vec::new();
    }

    // Compute the polygon's plane normal from the first 3 vertices
    let e1 = polygon[1] - polygon[0];
    let e2 = polygon[2] - polygon[0];
    let plane_normal = e1.cross(&e2);
    let plane_normal_len = plane_normal.norm();
    if plane_normal_len < 1e-12 {
        return Vec::new(); // Degenerate polygon
    }
    let plane_normal = plane_normal / plane_normal_len;

    // Build a 2D coordinate system on the plane
    let x_axis = e1.normalize();
    let y_axis = plane_normal.cross(&x_axis);

    // Project polygon vertices and line to 2D
    let project_to_2d = |p: &Point3| -> Point2 {
        let d = *p - polygon[0];
        Point2::new(d.dot(&x_axis), d.dot(&y_axis))
    };

    let poly_2d: Vec<Point2> = polygon.iter().map(&project_to_2d).collect();

    // Project line origin and direction
    let line_origin_2d = project_to_2d(&line.origin);
    let dx = line.direction.dot(&x_axis);
    let dy = line.direction.dot(&y_axis);
    let dir_2d_len = (dx * dx + dy * dy).sqrt();

    if dir_2d_len < 1e-12 {
        // Line is perpendicular to the polygon plane - no crossing
        return Vec::new();
    }

    // Create two points on the line for orient2d tests
    let line_pt1 = line_origin_2d;
    let line_pt2 = Point2::new(line_origin_2d.x + dx, line_origin_2d.y + dy);

    let mut crossings = Vec::new();

    for i in 0..n {
        let j = (i + 1) % n;
        let a = &poly_2d[i];
        let b = &poly_2d[j];

        // Skip degenerate segments
        let seg_len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if seg_len < 1e-12 {
            continue;
        }

        // Use orient2d to determine if segment endpoints are on opposite sides of the line
        let sign_a = orient2d(&line_pt1, &line_pt2, a);
        let sign_b = orient2d(&line_pt1, &line_pt2, b);

        // If both endpoints are on the same side (and neither is on the line), no crossing
        if sign_a == sign_b && sign_a != Sign::Zero {
            continue;
        }

        // Handle special cases
        if sign_a == Sign::Zero && sign_b == Sign::Zero {
            // Segment lies on the line - no single crossing point
            continue;
        }

        // Compute intersection parameter using Cramer's rule
        // This is only for finding the intersection point location, not for detection
        let sx = b.x - a.x;
        let sy = b.y - a.y;
        let det = sx * dy - dx * sy;

        if det.abs() < 1e-15 {
            // Lines are parallel (orient2d should have caught this, but be safe)
            continue;
        }

        let rhs_x = a.x - line_origin_2d.x;
        let rhs_y = a.y - line_origin_2d.y;

        let t = (sx * rhs_y - sy * rhs_x) / det;
        let s = (dx * rhs_y - dy * rhs_x) / det;

        // s is the parameter along the segment [0, 1]
        // Use a small tolerance for numerical stability
        if !(-1e-9..=1.0 + 1e-9).contains(&s) {
            continue;
        }

        // Compute the 3D intersection point and snap to clean values
        let intersection = snap_point(line.origin + t * line.direction);

        // Avoid duplicate crossings at vertices
        let is_duplicate = crossings.iter().any(|c: &Point3| (*c - intersection).norm() < 0.01);
        if !is_duplicate {
            crossings.push(intersection);
        }
    }

    // Sort crossings by their parameter along the line
    let line_dir = line.direction.normalize();
    crossings.sort_by(|a, b| {
        let ta = (*a - line.origin).dot(&line_dir);
        let tb = (*b - line.origin).dot(&line_dir);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });

    crossings
}

/// Create a new face in the BRep from a set of 3D points.
///
/// Reuses existing vertices within tolerance, creating new ones only when needed.
fn create_face_from_points(
    brep: &mut BRepSolid,
    points: &[Point3],
    surface_index: usize,
    orientation: Orientation,
) -> FaceId {
    // Create or reuse vertices - reuse existing vertices within tolerance
    let tolerance = 1e-6;
    let verts: Vec<_> = points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Create half-edges
    let hes: Vec<_> = verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    // Create loop
    let loop_id = brep.topology.add_loop(&hes);

    // Create face
    brep.topology.add_face(loop_id, surface_index, orientation)
}

/// Split all intersected faces of a solid.
///
/// For each face that has intersection curves crossing it,
/// split the face into sub-faces.
///
/// Returns a mapping from original face IDs to their split results.
pub fn split_intersected_faces(
    brep: &mut BRepSolid,
    face_intersections: &[(FaceId, IntersectionCurve, Point3, Point3)],
) -> Vec<SplitResult> {
    let mut results = Vec::new();

    for (face_id, curve, entry, exit) in face_intersections {
        let result = split_face_by_curve(brep, *face_id, curve, entry, exit);
        results.push(result);
    }

    results
}

// =============================================================================
// Planar Face Splitting by Circle
// =============================================================================

/// Check if a face's underlying surface is a plane.
pub fn is_planar_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Plane
}

/// Split a planar face along a circle intersection curve.
///
/// When a cylinder axis is perpendicular to a plane and they intersect,
/// the result is a circle on the plane. This function splits the planar face into:
/// - An inner face (the disk bounded by the circle)
/// - An outer face (the original polygon with a circular hole)
///
/// The outer face has two loops:
/// - Outer loop: the original polygon boundary
/// - Inner loop: the circle (oriented opposite to outer loop)
///
/// For tessellation, the inner circle is approximated with `segments` vertices.
pub fn split_planar_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;

    // Get outer loop vertices to check if circle is inside the face
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    if loop_verts.len() < 3 {
        // Degenerate outer loop (1-2 vertices) — circular cap face (e.g. cylinder cap).
        // Check analytically if the intersection circle is inside this cap's boundary.
        // If so, create an inner disk face and add the circle as a hole on the cap.
        // Keep the degenerate outer loop as-is (tessellation handles it).
        if let Some(plane) = brep.geometry.surfaces[surface_index]
            .as_any()
            .downcast_ref::<vcad_kernel_geom::Plane>()
        {
            if let Some(&first_pt) = loop_verts.first() {
                let center = plane.origin;
                let to_v = first_pt - center;
                let normal = plane.normal_dir.into_inner();
                let on_plane = to_v - to_v.dot(&normal) * normal;
                let cap_radius = on_plane.norm();

                // Check: is the intersection circle coplanar and fully inside?
                let circle_to_plane = (circle.center - center).dot(&normal).abs();
                let circle_center_offset = {
                    let d = circle.center - center;
                    (d - d.dot(&normal) * normal).norm()
                };
                let circle_inside = circle_to_plane < 1e-6
                    && circle_center_offset + circle.radius + 1e-6 < cap_radius;

                if circle_inside && cap_radius > 1e-12 {
                    let tolerance = 1e-6;

                    // Generate circle vertices for the inner disk face
                    let circle_verts: Vec<Point3> = (0..segments)
                        .map(|i| {
                            let theta = 2.0 * std::f64::consts::PI * (i as f64)
                                / (segments as f64);
                            let (sin_t, cos_t) = theta.sin_cos();
                            circle.center
                                + circle.radius
                                    * (cos_t * circle.x_dir.into_inner()
                                        + sin_t * circle.y_dir.into_inner())
                        })
                        .collect();

                    // Create inner disk face (will be classified as "inside" and removed)
                    let inner_verts: Vec<_> = circle_verts
                        .iter()
                        .map(|p| find_or_create_vertex(brep, p, tolerance))
                        .collect();
                    let inner_hes: Vec<_> = inner_verts
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let inner_loop = brep.topology.add_loop(&inner_hes);
                    let inner_face =
                        brep.topology
                            .add_face(inner_loop, surface_index, orientation);

                    // Add the circle as an inner loop (hole) on the original cap face.
                    // Determine correct winding: inner loop should be opposite to outer.
                    // For a degenerate outer loop we use the plane normal to decide:
                    // outer is CCW from above → inner should be CW from above.
                    let cap_x_dir = on_plane.normalize();
                    let cap_y_dir = normal.cross(&cap_x_dir);
                    let circle_signed_area = {
                        let project = |p: &Point3| -> (f64, f64) {
                            let d = *p - center;
                            (d.dot(&cap_x_dir), d.dot(&cap_y_dir))
                        };
                        let pts_2d: Vec<_> = circle_verts.iter().map(project).collect();
                        let mut a = 0.0;
                        for i in 0..pts_2d.len() {
                            let j = (i + 1) % pts_2d.len();
                            a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
                        }
                        a / 2.0
                    };

                    // Inner loop winding should be CW (negative area in face-normal space)
                    let hole_verts: Vec<Point3> = if circle_signed_area > 0.0 {
                        circle_verts.iter().rev().cloned().collect()
                    } else {
                        circle_verts.clone()
                    };

                    let hole_vert_ids: Vec<_> = hole_verts
                        .iter()
                        .map(|p| find_or_create_vertex(brep, p, tolerance))
                        .collect();
                    let hole_hes: Vec<_> = hole_vert_ids
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let hole_loop = brep.topology.add_loop(&hole_hes);
                    brep.topology.faces[face_id].inner_loops.push(hole_loop);

                    // Add twin edges between inner disk and hole
                    for i in 0..segments as usize {
                        let inner_he = inner_hes[i];
                        let outer_he =
                            hole_hes[(segments as usize - 1 - i) % segments as usize];
                        brep.topology.add_edge(inner_he, outer_he);
                    }

                    // Add faces to shell
                    if let Some(shell_id) = brep.topology.faces[face_id].shell {
                        brep.topology.shells[shell_id].faces.push(inner_face);
                        brep.topology.faces[inner_face].shell = Some(shell_id);
                    }

                    brep.geometry.add_curve_3d(Box::new(circle.clone()));

                    return SplitResult {
                        sub_faces: vec![inner_face, face_id],
                    };
                }
            }
        }

        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Check if the FULL circle is inside the polygon
    // We need to verify not just that the circle overlaps, but that it's fully contained.
    // If only partially inside, use arc-based splitting instead.
    if !circle_fully_inside_polygon(&loop_verts, circle) {
        // Check if circle partially intersects (crosses exactly 2 edges)
        if circle_partially_inside_polygon(&loop_verts, circle) {
            return split_planar_face_by_arc(brep, face_id, circle, segments);
        }
        // Circle doesn't intersect at all
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Generate circle vertices (CCW when viewed from face normal direction)
    // The circle's normal should align with the plane normal
    let circle_verts: Vec<Point3> = (0..segments)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (sin_t, cos_t) = theta.sin_cos();
            circle.center
                + circle.radius
                    * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
        })
        .collect();

    // Create inner face (disk) - uses circle vertices as its outer loop
    // The inner face's loop should be oriented the same as the parent face
    let tolerance = 1e-6;
    let inner_verts: Vec<_> = circle_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    let inner_hes: Vec<_> = inner_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let inner_loop = brep.topology.add_loop(&inner_hes);
    let inner_face = brep
        .topology
        .add_face(inner_loop, surface_index, orientation);

    // Create outer face (polygon with hole)
    // The outer loop stays the same; we add the circle as an inner loop
    // The inner loop must have OPPOSITE winding to the outer loop in the face's 2D projection

    // Compute the face's 2D coordinate system using the plane surface normal
    // instead of deriving from vertices, which can produce inconsistent normals
    // for rotated/non-axis-aligned faces depending on vertex ordering.
    let face_normal = if let Some(plane) = brep.geometry.surfaces[surface_index]
        .as_any()
        .downcast_ref::<vcad_kernel_geom::Plane>()
    {
        let n = plane.normal_dir.into_inner();
        // Account for face orientation: reversed faces flip the normal
        if orientation == Orientation::Reversed {
            -n
        } else {
            n
        }
    } else {
        // Fallback: derive from vertices (shouldn't happen for planar faces)
        let e1 = loop_verts[1] - loop_verts[0];
        let e2 = loop_verts[2] - loop_verts[0];
        e1.cross(&e2)
    };
    let e1 = loop_verts[1] - loop_verts[0];
    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(&e1).normalize();
    let origin = loop_verts[0];

    // Project to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    // Compute signed area to determine winding direction
    // Positive = CCW, Negative = CW in our 2D projection
    let signed_area = |pts: &[Point3]| -> f64 {
        let pts_2d: Vec<_> = pts.iter().map(&project).collect();
        let mut area = 0.0;
        for i in 0..pts_2d.len() {
            let j = (i + 1) % pts_2d.len();
            area += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
        }
        area / 2.0
    };

    let outer_area = signed_area(&loop_verts);
    let circle_area = signed_area(&circle_verts);

    // Inner loop should have opposite sign to outer loop
    // If they have the same sign, we need to reverse the circle vertices
    let need_reverse = (outer_area > 0.0) == (circle_area > 0.0);

    let inner_loop_verts: Vec<Point3> = if need_reverse {
        circle_verts.iter().rev().cloned().collect()
    } else {
        circle_verts.clone()
    };

    let outer_inner_verts: Vec<_> = inner_loop_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Create new outer loop (copy of original)
    let outer_verts: Vec<_> = loop_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    let outer_hes: Vec<_> = outer_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let new_outer_loop = brep.topology.add_loop(&outer_hes);
    let outer_face = brep
        .topology
        .add_face(new_outer_loop, surface_index, orientation);

    // Add the inner loop (hole) to the outer face
    let hole_hes: Vec<_> = outer_inner_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let hole_loop = brep.topology.add_loop(&hole_hes);
    brep.topology.faces[outer_face].inner_loops.push(hole_loop);

    // Copy existing inner loops from the original face, routing each to the correct
    // sub-face: if a loop is inside the new circle it belongs to the inner face (disk),
    // otherwise it belongs to the outer face (polygon with hole).
    let existing_inner_loops = brep.topology.faces[face_id].inner_loops.clone();
    for existing_loop in existing_inner_loops {
        // Re-create the inner loop with new half-edges for the target face
        let loop_verts_existing: Vec<Point3> = brep
            .topology
            .loop_half_edges(existing_loop)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();

        // Determine which face this inner loop belongs to by checking if its
        // representative vertex is inside the new circle
        let target_face = if !loop_verts_existing.is_empty() {
            // Use the centroid of the inner loop vertices (or just the first vertex
            // for degenerate single-vertex loops) as the test point
            let test_pt = if loop_verts_existing.len() == 1 {
                loop_verts_existing[0]
            } else {
                let n = loop_verts_existing.len() as f64;
                let cx = loop_verts_existing.iter().map(|v| v.x).sum::<f64>() / n;
                let cy = loop_verts_existing.iter().map(|v| v.y).sum::<f64>() / n;
                let cz = loop_verts_existing.iter().map(|v| v.z).sum::<f64>() / n;
                Point3::new(cx, cy, cz)
            };
            let d = test_pt - circle.center;
            let dist = d.norm();
            if dist < circle.radius - tolerance {
                inner_face // Loop is inside the circle → belongs to inner (disk) face
            } else {
                outer_face // Loop is outside the circle → belongs to outer face
            }
        } else {
            outer_face // Empty loop, default to outer
        };

        let new_verts: Vec<_> = loop_verts_existing
            .iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect();

        let new_hes: Vec<_> = new_verts
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();

        let new_loop = brep.topology.add_loop(&new_hes);
        brep.topology.faces[target_face].inner_loops.push(new_loop);
    }

    // Add twin edges between inner face circle and outer face hole
    // (they share the same physical edges but with opposite orientation)
    for i in 0..segments as usize {
        let inner_he = inner_hes[i];
        // The outer hole is reversed, so we need to match edges correctly
        // inner_hes[i] corresponds to outer_inner_hes[segments - 1 - i]
        let outer_he = hole_hes[(segments as usize - 1 - i) % segments as usize];
        brep.topology.add_edge(inner_he, outer_he);
    }

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(inner_face);
        brep.topology.shells[shell_id].faces.push(outer_face);

        brep.topology.faces[inner_face].shell = Some(shell_id);
        brep.topology.faces[outer_face].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add the 3D circle curve to geometry
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![inner_face, outer_face],
    }
}

/// Check if a circle is FULLY inside a polygon (in 3D, assumes coplanar).
///
/// Returns true only if the entire circle is contained within the polygon.
/// Used by split_planar_face_by_circle to decide whether to create a full disk.
fn circle_fully_inside_polygon(polygon: &[Point3], circle: &vcad_kernel_geom::Circle3d) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for the projection plane instead of
    // deriving from the first 3 polygon vertices, which can produce an
    // inconsistent normal direction for rotated/non-axis-aligned faces.
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    // Build a 2D coordinate system from the circle's normal
    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(&u_axis);
    let v_axis_len = v_axis.norm();
    if v_axis_len < 1e-12 {
        return false;
    }
    let v_axis = v_axis / v_axis_len;

    let project = |p: &Point3| -> (f64, f64) {
        let d = p - v0;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    // Project circle center
    let (cx, cy) = project(&circle.center);

    // Project polygon vertices
    let poly_2d: Vec<(f64, f64)> = polygon.iter().map(project).collect();

    // Check if circle center is inside the polygon
    if !point_in_polygon_2d(cx, cy, &poly_2d) {
        return false;
    }

    // Check that the circle doesn't cross any polygon edge
    // i.e., distance from center to each edge must be > radius
    let n = poly_2d.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = poly_2d[i];
        let (x2, y2) = poly_2d[j];

        let dist = point_to_segment_dist_2d(cx, cy, x1, y1, x2, y2);
        if dist < circle.radius - 1e-6 {
            // Circle crosses this edge - not fully inside
            return false;
        }
    }

    true
}

/// Check if a circle overlaps with a polygon (in 3D, assumes coplanar).
///
/// Returns true if any part of the circle is inside the polygon.
/// This handles edge cases like:
/// - Circle center on polygon boundary or corner
/// - Circle partially overlapping the polygon
#[allow(dead_code)]
fn circle_inside_polygon(polygon: &[Point3], circle: &vcad_kernel_geom::Circle3d) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for consistent projection
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(&u_axis);
    let v_axis_len = v_axis.norm();
    if v_axis_len < 1e-12 {
        return false;
    }
    let v_axis = v_axis / v_axis_len;

    let project = |p: &Point3| -> (f64, f64) {
        let d = p - v0;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    // Project circle center
    let (cx, cy) = project(&circle.center);

    // Project polygon vertices
    let poly_2d: Vec<(f64, f64)> = polygon.iter().map(project).collect();

    // Check if circle center is inside the polygon
    let center_inside = point_in_polygon_2d(cx, cy, &poly_2d);

    // If center is inside, circle definitely overlaps
    if center_inside {
        return true;
    }

    // Check if center is very close to polygon boundary (tolerance for numerical precision)
    let tol = 1e-6;
    let n = poly_2d.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (xi, yi) = poly_2d[i];
        let (xj, yj) = poly_2d[j];

        // Distance from center to this edge
        let dist = point_to_segment_dist_2d(cx, cy, xi, yi, xj, yj);
        if dist < tol {
            // Center is on the boundary - check if any part of the circle is inside
            // Sample points on the circle and check if any are inside
            for k in 0..8 {
                let theta = std::f64::consts::PI * 2.0 * (k as f64) / 8.0;
                let px = cx + circle.radius * theta.cos();
                let py = cy + circle.radius * theta.sin();
                if point_in_polygon_2d(px, py, &poly_2d) {
                    return true;
                }
            }
            // Also check if circle intersects any polygon edge
            return circle_intersects_polygon_edges(cx, cy, circle.radius, &poly_2d);
        }
    }

    // Check if circle intersects any polygon edge
    circle_intersects_polygon_edges(cx, cy, circle.radius, &poly_2d)
}

/// Point-in-polygon test using ray casting (2D version).
fn point_in_polygon_2d(px: f64, py: f64, polygon: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = polygon.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];

        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from point to line segment (2D).
pub(crate) fn point_to_segment_dist_2d(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-15 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = ((px - x1) * dx + (py - y1) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
}

/// Check if a circle intersects any edge of a polygon.
fn circle_intersects_polygon_edges(cx: f64, cy: f64, radius: f64, polygon: &[(f64, f64)]) -> bool {
    let n = polygon.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = polygon[i];
        let (x2, y2) = polygon[j];

        let dist = point_to_segment_dist_2d(cx, cy, x1, y1, x2, y2);
        if dist <= radius {
            return true;
        }
    }
    false
}

// =============================================================================
// Arc-Based Planar Face Splitting
// =============================================================================

/// A circle-polygon intersection point with metadata.
#[derive(Debug, Clone)]
struct CirclePolygonIntersection {
    /// The 3D intersection point.
    point: Point3,
    /// The 2D intersection point (projected onto the polygon plane).
    point_2d: (f64, f64),
    /// The edge index (starting vertex) where the intersection occurs.
    edge_index: usize,
    /// The parameter t along the edge (0 = start vertex, 1 = end vertex).
    #[allow(dead_code)]
    t_along_edge: f64,
    /// The angle on the circle (0 to 2π).
    angle: f64,
}

/// Find where a circle intersects a polygon's edges.
///
/// Returns intersection points sorted by angle on the circle.
/// Each intersection includes the edge index, parameter along edge, and angle on circle.
fn find_circle_polygon_intersections(
    _polygon_3d: &[Point3],
    polygon_2d: &[(f64, f64)],
    circle_center_2d: (f64, f64),
    radius: f64,
    origin_3d: Point3,
    u_axis: vcad_kernel_math::Vec3,
    v_axis: vcad_kernel_math::Vec3,
) -> Vec<CirclePolygonIntersection> {
    let n = polygon_2d.len();
    let mut intersections = Vec::new();
    let (cx, cy) = circle_center_2d;
    let tol = 1e-9;

    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = polygon_2d[i];
        let (x2, y2) = polygon_2d[j];

        // Solve for line-circle intersection in 2D.
        // Line: P(t) = (x1, y1) + t * (x2 - x1, y2 - y1)
        // Circle: (x - cx)² + (y - cy)² = r²
        //
        // Substituting:
        // (x1 + t*dx - cx)² + (y1 + t*dy - cy)² = r²
        // Let ax = x1 - cx, ay = y1 - cy, dx = x2 - x1, dy = y2 - y1
        // (ax + t*dx)² + (ay + t*dy)² = r²
        // ax² + 2*ax*t*dx + t²*dx² + ay² + 2*ay*t*dy + t²*dy² = r²
        // t²*(dx² + dy²) + 2*t*(ax*dx + ay*dy) + (ax² + ay² - r²) = 0

        let dx = x2 - x1;
        let dy = y2 - y1;
        let ax = x1 - cx;
        let ay = y1 - cy;

        let a = dx * dx + dy * dy;
        let b = 2.0 * (ax * dx + ay * dy);
        let c = ax * ax + ay * ay - radius * radius;

        if a.abs() < tol {
            // Degenerate edge (zero length)
            continue;
        }

        let discriminant = b * b - 4.0 * a * c;
        if discriminant < -tol {
            // No intersection
            continue;
        }

        let discriminant = discriminant.max(0.0).sqrt();

        for sign in [-1.0, 1.0] {
            let t = (-b + sign * discriminant) / (2.0 * a);

            // Check if intersection is within the segment [0, 1]
            if t < -tol || t > 1.0 + tol {
                continue;
            }

            // Clamp t to [0, 1] for robustness
            let t = t.clamp(0.0, 1.0);

            // Compute 2D intersection point
            let px = x1 + t * dx;
            let py = y1 + t * dy;

            // Compute angle on circle
            let angle = (py - cy).atan2(px - cx);
            let angle = if angle < 0.0 {
                angle + 2.0 * std::f64::consts::PI
            } else {
                angle
            };

            // Compute 3D point
            let point_3d = origin_3d + px * u_axis + py * v_axis;

            // Avoid duplicate intersections (at corners)
            let is_duplicate = intersections.iter().any(|other: &CirclePolygonIntersection| {
                let dist_2d =
                    ((px - other.point_2d.0).powi(2) + (py - other.point_2d.1).powi(2)).sqrt();
                dist_2d < 0.01
            });

            if !is_duplicate {
                intersections.push(CirclePolygonIntersection {
                    point: point_3d,
                    point_2d: (px, py),
                    edge_index: i,
                    t_along_edge: t,
                    angle,
                });
            }
        }
    }

    // Sort by angle for consistent ordering
    intersections.sort_by(|a, b| {
        a.angle
            .partial_cmp(&b.angle)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    intersections
}

/// Split a planar face along an arc where a circle partially intersects it.
///
/// When a circle only partially overlaps a polygon face, this function:
/// 1. Finds where the circle crosses polygon edges (2 intersection points)
/// 2. Determines which arc is inside the polygon
/// 3. Creates two faces:
///    - Face with arc boundary (inside the circle)
///    - Face with chord boundary (outside the circle)
///
/// Returns the original face unchanged if:
/// - The circle doesn't intersect the polygon at exactly 2 points
/// - The intersections are too close together
pub fn split_planar_face_by_arc(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;

    // Get outer loop vertices
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    if loop_verts.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Build 2D coordinate system from polygon
    let v0 = loop_verts[0];
    let v1 = loop_verts[1];
    let v2 = loop_verts[2];

    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let normal = e1.cross(&e2);
    let normal_len = normal.norm();
    if normal_len < 1e-12 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let u_axis = e1.normalize();
    let v_axis = normal.cross(&e1).normalize();
    let origin = v0;

    // Project polygon vertices to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    let poly_2d: Vec<(f64, f64)> = loop_verts.iter().map(&project).collect();

    // Project circle center to 2D
    let center_2d = project(&circle.center);

    // Find circle-polygon intersections
    let intersections = find_circle_polygon_intersections(
        &loop_verts,
        &poly_2d,
        center_2d,
        circle.radius,
        origin,
        u_axis,
        v_axis,
    );

    // Need exactly 2 intersections for a simple split
    if intersections.len() != 2 {
        // Complex case: more than 2 intersections, or circle doesn't cross edges
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let int1 = &intersections[0];
    let int2 = &intersections[1];

    // Check if intersections are too close together (would create degenerate faces)
    let dist = ((int1.point_2d.0 - int2.point_2d.0).powi(2)
        + (int1.point_2d.1 - int2.point_2d.1).powi(2))
    .sqrt();
    if dist < 0.01 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Determine which arc (from int1 to int2) is inside the polygon.
    // The arc from angle1 to angle2 (CCW) might be inside or outside.
    // Check by sampling the arc midpoint.

    let angle1 = int1.angle;
    let angle2 = int2.angle;

    // Arc 1: from angle1 to angle2 (CCW, shorter if angle2 > angle1)
    // Arc 2: from angle2 to angle1 (CCW, wrapping around)
    let arc1_mid_angle = if angle2 >= angle1 {
        (angle1 + angle2) / 2.0
    } else {
        // Wraps around
        let mid = (angle1 + angle2 + 2.0 * std::f64::consts::PI) / 2.0;
        if mid >= 2.0 * std::f64::consts::PI {
            mid - 2.0 * std::f64::consts::PI
        } else {
            mid
        }
    };

    let arc1_mid_x = center_2d.0 + circle.radius * arc1_mid_angle.cos();
    let arc1_mid_y = center_2d.1 + circle.radius * arc1_mid_angle.sin();
    let arc1_inside = point_in_polygon_2d(arc1_mid_x, arc1_mid_y, &poly_2d);

    // Determine which arc is inside and which edge indices to walk
    let (inside_start, inside_end, inside_start_angle, inside_end_angle) = if arc1_inside {
        (int1, int2, angle1, angle2)
    } else {
        (int2, int1, angle2, angle1)
    };

    // Compute arc span (always positive, CCW direction)
    let arc_span = if inside_end_angle >= inside_start_angle {
        inside_end_angle - inside_start_angle
    } else {
        2.0 * std::f64::consts::PI - inside_start_angle + inside_end_angle
    };

    // Number of segments for the arc (proportional to arc length)
    let n_arc = ((segments as f64) * arc_span / (2.0 * std::f64::consts::PI))
        .max(2.0)
        .ceil() as u32;

    // Generate arc vertices (from inside_start to inside_end, CCW)
    let (cx, cy) = center_2d;
    let mut arc_points_2d: Vec<(f64, f64)> = Vec::with_capacity((n_arc + 1) as usize);
    arc_points_2d.push(inside_start.point_2d);
    for i in 1..n_arc {
        let t = i as f64 / n_arc as f64;
        let angle = inside_start_angle + t * arc_span;
        let px = cx + circle.radius * angle.cos();
        let py = cy + circle.radius * angle.sin();
        arc_points_2d.push((px, py));
    }
    arc_points_2d.push(inside_end.point_2d);

    // Convert arc 2D points to 3D
    let arc_points_3d: Vec<Point3> = arc_points_2d
        .iter()
        .map(|&(x, y)| origin + x * u_axis + y * v_axis)
        .collect();

    // Build Face 1: the inside-circle portion
    // Walk polygon from inside_end edge to inside_start edge, then add arc back
    let n = loop_verts.len();
    let mut face1_points: Vec<Point3> = Vec::new();

    // Start at inside_end intersection
    face1_points.push(inside_end.point);

    // Walk polygon from inside_end edge to inside_start edge
    let mut idx = (inside_end.edge_index + 1) % n;
    while idx != (inside_start.edge_index + 1) % n {
        face1_points.push(loop_verts[idx]);
        idx = (idx + 1) % n;
    }

    // Add inside_start intersection
    face1_points.push(inside_start.point);

    // Add arc points (from inside_start to inside_end, forward direction)
    // arc_points_3d goes from inside_start to inside_end, so iterate forward
    // This completes the loop: ... → inside_start → arc → (closes to inside_end)
    for pt in arc_points_3d.iter().skip(1).take(arc_points_3d.len().saturating_sub(2)) {
        face1_points.push(*pt);
    }

    // Build Face 2: the outside-circle portion (polygon outside the arc)
    // Walk polygon from inside_start edge to inside_end edge, then add arc back
    let mut face2_points: Vec<Point3> = Vec::new();

    // Start at inside_start intersection
    face2_points.push(inside_start.point);

    // Walk polygon from inside_start edge to inside_end edge
    idx = (inside_start.edge_index + 1) % n;
    while idx != (inside_end.edge_index + 1) % n {
        face2_points.push(loop_verts[idx]);
        idx = (idx + 1) % n;
    }

    // Add inside_end intersection
    face2_points.push(inside_end.point);

    // Add arc points from inside_end back to inside_start (forward arc direction)
    // arc_points_3d goes from inside_start → inside_end, so we take interior and reverse
    // to go from inside_end → inside_start
    for pt in arc_points_3d.iter().skip(1).rev().skip(1) {
        face2_points.push(*pt);
    }

    // Validate faces have at least 3 vertices
    if face1_points.len() < 3 || face2_points.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create the two new faces
    let tolerance = 1e-6;

    // Face 1 (arc-bounded, inside circle)
    let face1_verts: Vec<_> = face1_points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let face1_hes: Vec<_> = face1_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let face1_loop = brep.topology.add_loop(&face1_hes);
    let face1 = brep
        .topology
        .add_face(face1_loop, surface_index, orientation);

    // Face 2 (chord-bounded, outside circle)
    let face2_verts: Vec<_> = face2_points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let face2_hes: Vec<_> = face2_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let face2_loop = brep.topology.add_loop(&face2_hes);
    let face2 = brep
        .topology
        .add_face(face2_loop, surface_index, orientation);

    // Add twin edges for the chord (shared edge between face1 and face2)
    // In face1, the chord goes from inside_end to inside_start (first edge after arc)
    // In face2, the chord goes from inside_start to inside_end (last edge)
    // These need to be matched correctly based on which edges share the intersection vertices
    let chord_he1 = face1_hes[0]; // First edge of face1 starts at inside_end
    let chord_he2 = face2_hes[face2_hes.len() - 1]; // Last edge of face2 ends at inside_end
    brep.topology.add_edge(chord_he1, chord_he2);

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the arc
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Check if a circle partially intersects a polygon (crosses exactly 2 edges).
///
/// Returns true if the circle crosses the polygon boundary at exactly 2 points,
/// meaning it's only partially inside and needs arc-based splitting.
fn circle_partially_inside_polygon(polygon: &[Point3], circle: &vcad_kernel_geom::Circle3d) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for consistent projection
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(&e1).normalize();

    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - v0;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    let poly_2d: Vec<(f64, f64)> = polygon.iter().map(&project).collect();
    let center_2d = project(&circle.center);

    let intersections = find_circle_polygon_intersections(
        polygon,
        &poly_2d,
        center_2d,
        circle.radius,
        v0,
        u_axis,
        v_axis,
    );

    // Partial intersection means exactly 2 crossing points
    intersections.len() == 2
}

/// Split a planar face along an intersection curve.
///
/// This dispatches to the appropriate split method based on the curve type:
/// - Circle: creates inner disk + outer face with hole
/// - Line: entry/exit split (existing implementation)
pub fn split_planar_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    entry: &Point3,
    exit: &Point3,
    segments: u32,
) -> SplitResult {
    match curve {
        IntersectionCurve::Circle(circle) => {
            split_planar_face_by_circle(brep, face_id, circle, segments)
        }
        IntersectionCurve::Line(line) => {
            // Get face boundary vertices
            let face = &brep.topology.faces[face_id];
            let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
            let loop_verts: Vec<Point3> = loop_hes
                .iter()
                .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();

            // Find where the line intersects the polygon edges
            let crossings = find_line_polygon_crossings(&loop_verts, line);

            if crossings.len() >= 2 {
                // Use the first two crossings as entry/exit
                let actual_entry = crossings[0];
                let actual_exit = crossings[1];
                split_face_by_curve(brep, face_id, curve, &actual_entry, &actual_exit)
            } else {
                // Line doesn't cross the polygon boundary at two points
                SplitResult {
                    sub_faces: vec![face_id],
                }
            }
        }
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            split_planar_face(
                brep,
                face_id,
                &IntersectionCurve::Line(line1.clone()),
                entry,
                exit,
                segments,
            )
        }
        _ => {
            // Use existing line-based split
            split_face_by_curve(brep, face_id, curve, entry, exit)
        }
    }
}

// =============================================================================
// Cylindrical Face Splitting
// =============================================================================

/// Split a spherical face by a circle intersection curve.
///
/// When a plane or another sphere intersects a sphere, the result is a circle
/// on the sphere's surface. This function:
/// 1. Adds the circle as an inner loop (hole) on the sphere face
/// 2. Creates a planar disk face bounded by the circle (for classification)
///
/// The sphere face retains its degenerate pole loop as the outer boundary.
/// The circle becomes an inner loop, similar to how cylinder cap splits work.
pub fn split_spherical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify this is a sphere surface
    let sph = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::SphereSurface>()
    {
        Some(s) => s.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Verify the circle lies on the sphere: distance from center to circle center
    // plus Pythagorean check should satisfy r_circle^2 + d^2 ≈ R^2
    let d = (circle.center - sph.center).norm();
    let expected_r = (sph.radius * sph.radius - d * d).max(0.0).sqrt();
    if (expected_r - circle.radius).abs() > 1e-4 {
        // Circle doesn't lie on this sphere — skip
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let tolerance = 1e-6;

    // Generate circle vertices
    let circle_verts: Vec<Point3> = (0..segments)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (sin_t, cos_t) = theta.sin_cos();
            circle.center
                + circle.radius
                    * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
        })
        .collect();

    // Create inner disk face on the plane of the circle.
    // This face will be classified as inside/outside the other solid.
    // First, find or create the planar surface for the disk.
    let circle_normal = circle.x_dir.into_inner().cross(&circle.y_dir.into_inner());
    let disk_plane = vcad_kernel_geom::Plane::new(
        circle.center,
        circle.x_dir.into_inner(),
        circle.y_dir.into_inner(),
    );
    let disk_surf_idx = brep.geometry.add_surface(Box::new(disk_plane));

    let inner_verts: Vec<_> = circle_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let inner_hes: Vec<_> = inner_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let inner_loop = brep.topology.add_loop(&inner_hes);
    let inner_face = brep
        .topology
        .add_face(inner_loop, disk_surf_idx, orientation);

    // Add the circle as an inner loop (hole) on the sphere face.
    // The hole winding must be opposite to the inner disk face's winding
    // when viewed from outside the sphere.
    // Determine winding: compute signed area of circle_verts projected along circle normal
    let circle_signed_area = {
        // Use the circle's own coordinate system
        let pts_2d: Vec<(f64, f64)> = circle_verts
            .iter()
            .map(|p| {
                let d = *p - circle.center;
                (
                    d.dot(&circle.x_dir.into_inner()),
                    d.dot(&circle.y_dir.into_inner()),
                )
            })
            .collect();
        let mut a = 0.0;
        for i in 0..pts_2d.len() {
            let j = (i + 1) % pts_2d.len();
            a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
        }
        a / 2.0
    };

    // The sphere's outward direction at the circle center
    let sphere_outward = (circle.center - sph.center).normalize();
    // If circle_normal aligns with sphere_outward, the CCW vertices (as seen from outside)
    // should be reversed for the hole
    let normals_aligned = circle_normal.dot(&sphere_outward) > 0.0;

    // Inner loop on sphere should be CW when viewed from outside the sphere
    // If circle area is positive and normals are aligned, the vertices are CCW from outside → reverse
    let need_reverse = (circle_signed_area > 0.0) == normals_aligned;

    let hole_verts: Vec<Point3> = if need_reverse {
        circle_verts.iter().rev().cloned().collect()
    } else {
        circle_verts.clone()
    };

    let hole_vert_ids: Vec<_> = hole_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let hole_hes: Vec<_> = hole_vert_ids
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let hole_loop = brep.topology.add_loop(&hole_hes);
    brep.topology.faces[face_id].inner_loops.push(hole_loop);

    // Add twin edges between inner disk and hole on sphere
    for i in 0..segments as usize {
        let inner_he = inner_hes[i];
        let outer_he = hole_hes[(segments as usize - 1 - i) % segments as usize];
        brep.topology.add_edge(inner_he, outer_he);
    }

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(inner_face);
        brep.topology.faces[inner_face].shell = Some(shell_id);
    }

    // Add 3D curve
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![inner_face, face_id],
    }
}

/// Check if a face's underlying surface is a cylinder.
pub fn is_cylindrical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder
}

/// Check if a face's underlying surface is a sphere.
pub fn is_spherical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Sphere
}

/// Check if a face's underlying surface is a torus.
pub fn is_toroidal_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Torus
}

/// Check if a face's underlying surface is a cone.
pub fn is_conical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cone
}

/// Split a conical face along an intersection curve.
pub fn split_conical_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
) -> SplitResult {
    match curve {
        IntersectionCurve::Circle(circle) => split_conical_face_by_circle(brep, face_id, circle),
        IntersectionCurve::Sampled(_) | IntersectionCurve::Line(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::Empty | IntersectionCurve::Point(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::TwoLines(line1, _line2) => {
            split_conical_face(brep, face_id, &IntersectionCurve::Line(line1.clone()))
        }
    }
}

/// Split a conical face along a circle intersection curve.
fn split_conical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    let cone = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::ConeSurface>()
    {
        Some(c) => c.clone(),
        None => return SplitResult { sub_faces: vec![face_id] },
    };

    let ca = cone.half_angle.cos();
    let apex_to_circle = (circle.center - cone.apex).dot(cone.axis.as_ref());

    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult { sub_faces: vec![face_id] };
    }

    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    for &he_id in &loop_hes {
        let v_id = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[v_id].point;
        let d = point - cone.apex;
        let v = d.dot(cone.axis.as_ref()) / ca;
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    let v_split = apex_to_circle / ca;
    if v_split <= v_min + 1e-9 || v_split >= v_max - 1e-9 {
        return SplitResult { sub_faces: vec![face_id] };
    }

    let sa = cone.half_angle.sin();
    let seam_point = cone.apex
        + v_split * (ca * cone.axis.into_inner() + sa * cone.ref_dir.into_inner());
    let v_split_seam = brep.topology.add_vertex(seam_point);

    let mut v_bottom = None;
    let mut v_top = None;
    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let d = point - cone.apex;
        let v = d.dot(cone.axis.as_ref()) / ca;
        if (v - v_min).abs() < 1e-6 { v_bottom = Some(vid); }
        if (v - v_max).abs() < 1e-6 { v_top = Some(vid); }
    }

    let (v_bottom, v_top) = match (v_bottom, v_top) {
        (Some(b), Some(t)) => (b, t),
        _ => return SplitResult { sub_faces: vec![face_id] },
    };

    // Lower face: v_min to v_split
    let he_lower_bot = brep.topology.add_half_edge(v_bottom);
    let he_lower_seam_up = brep.topology.add_half_edge(v_bottom);
    let he_lower_split = brep.topology.add_half_edge(v_split_seam);
    let he_lower_seam_down = brep.topology.add_half_edge(v_split_seam);
    let lower_loop = brep.topology.add_loop(&[
        he_lower_bot, he_lower_seam_up, he_lower_split, he_lower_seam_down,
    ]);
    let lower_face = brep.topology.add_face(lower_loop, surface_index, orientation);

    // Upper face: v_split to v_max
    let he_upper_split = brep.topology.add_half_edge(v_split_seam);
    let he_upper_seam_up = brep.topology.add_half_edge(v_split_seam);
    let he_upper_top = brep.topology.add_half_edge(v_top);
    let he_upper_seam_down = brep.topology.add_half_edge(v_top);
    let upper_loop = brep.topology.add_loop(&[
        he_upper_split, he_upper_seam_up, he_upper_top, he_upper_seam_down,
    ]);
    let upper_face = brep.topology.add_face(upper_loop, surface_index, orientation);

    brep.topology.add_edge(he_lower_seam_up, he_lower_seam_down);
    brep.topology.add_edge(he_upper_seam_up, he_upper_seam_down);
    brep.topology.add_edge(he_lower_split, he_upper_split);

    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);
        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);
        brep.topology.shells[shell_id].faces.retain(|&f| f != face_id);
    }

    brep.topology.faces.remove(face_id);
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![lower_face, upper_face],
    }
}

/// Split a cylindrical face along a circle intersection curve.
///
/// When a plane perpendicular to the cylinder axis intersects the cylinder,
/// the result is a circle at constant height. In the cylinder's UV space
/// `[0, 2π] × [v_min, v_max]`, this circle becomes a horizontal line at `v = h`.
///
/// This function splits the cylindrical face into two strips:
/// - Lower strip: `[0, 2π] × [v_min, h]`
/// - Upper strip: `[0, 2π] × [h, v_max]`
///
/// The split is performed by:
/// 1. Computing the intersection height `v_split` from the circle center
/// 2. Creating new 3D vertices at the intersection points (on the seam)
/// 3. Creating two new face loops that share the intersection edge
/// 4. Removing the original face and adding the two new sub-faces
pub fn split_cylindrical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let _outer_loop = face.outer_loop;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify surface is a cylinder
    let cyl = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        Some(c) => c.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Compute the split height: v_split = projection of circle center onto cylinder axis
    // v = (circle.center - cyl.center) · axis
    let v_split = (circle.center - cyl.center).dot(cyl.axis.as_ref());

    // Get the current face's v bounds from its boundary vertices
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // For a cylinder lateral face, we typically have:
    // - 2 vertices (top and bottom seam points)
    // - 4 half-edges: bottom circle, seam up, top circle, seam down
    // The v coordinates of these vertices give us v_min and v_max
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;

    for &he_id in &loop_hes {
        let v_id = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[v_id].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    // Check if split height is within the face's v range
    if v_split <= v_min + 1e-9 || v_split >= v_max - 1e-9 {
        // Split line doesn't cross the face interior
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create new vertex at the seam point at height v_split
    // At u=0, the point is: center + radius * ref_dir + v_split * axis
    let seam_point_at_split =
        cyl.center + cyl.radius * cyl.ref_dir.as_ref() + v_split * cyl.axis.as_ref();
    let v_split_seam = brep.topology.add_vertex(seam_point_at_split);

    // Get the existing top and bottom seam vertices
    // For a standard cylinder lateral face:
    // - loop_hes[0] origin = bottom seam point (at v_min)
    // - loop_hes[2] origin = top seam point (at v_max)
    // But this depends on how the cylinder was constructed.
    // Let's identify vertices by their v coordinate.
    let mut v_bottom = None;
    let mut v_top = None;

    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        if (v - v_min).abs() < 1e-9 {
            v_bottom = Some(vid);
        }
        if (v - v_max).abs() < 1e-9 {
            v_top = Some(vid);
        }
    }

    let (v_bottom, v_top) = match (v_bottom, v_top) {
        (Some(b), Some(t)) => (b, t),
        _ => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Now create the two new sub-faces.
    // Each face has a similar structure to the original:
    // - A circular edge at one end
    // - A seam edge connecting to the split
    // - The split circle edge
    // - A seam edge back

    // For simplicity, we'll create new faces with the same topology structure
    // but different boundary vertices.

    // Lower face: v_min to v_split
    // Boundary: bottom_circle (v_bottom → v_bottom) → seam_up (v_bottom → v_split_seam)
    //        → split_circle (v_split_seam → v_split_seam) → seam_down (v_split_seam → v_bottom)
    let he_lower_bot = brep.topology.add_half_edge(v_bottom);
    let he_lower_seam_up = brep.topology.add_half_edge(v_bottom);
    let he_lower_split = brep.topology.add_half_edge(v_split_seam);
    let he_lower_seam_down = brep.topology.add_half_edge(v_split_seam);

    let lower_loop = brep.topology.add_loop(&[
        he_lower_bot,
        he_lower_seam_up,
        he_lower_split,
        he_lower_seam_down,
    ]);
    let lower_face = brep
        .topology
        .add_face(lower_loop, surface_index, orientation);

    // Upper face: v_split to v_max
    // Boundary: split_circle (v_split_seam → v_split_seam) → seam_up (v_split_seam → v_top)
    //        → top_circle (v_top → v_top) → seam_down (v_top → v_split_seam)
    let he_upper_split = brep.topology.add_half_edge(v_split_seam);
    let he_upper_seam_up = brep.topology.add_half_edge(v_split_seam);
    let he_upper_top = brep.topology.add_half_edge(v_top);
    let he_upper_seam_down = brep.topology.add_half_edge(v_top);

    let upper_loop = brep.topology.add_loop(&[
        he_upper_split,
        he_upper_seam_up,
        he_upper_top,
        he_upper_seam_down,
    ]);
    let upper_face = brep
        .topology
        .add_face(upper_loop, surface_index, orientation);

    // Add twin edges
    // Lower seam edges
    brep.topology.add_edge(he_lower_seam_up, he_lower_seam_down);
    // Upper seam edges
    brep.topology.add_edge(he_upper_seam_up, he_upper_seam_down);
    // The split circle edges from upper and lower faces are twins
    brep.topology.add_edge(he_lower_split, he_upper_split);

    // Link bottom circle: lower face shares with bottom cap
    // Link top circle: upper face shares with top cap
    // These would need to be re-linked if we had access to the original edges
    // For now, we'll skip re-linking circular edges as they're handled elsewhere

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);

        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curves for the split circle
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![lower_face, upper_face],
    }
}

/// Compute the U parameter for a point on a cylinder surface.
fn compute_cylinder_u(point: &Point3, cyl: &vcad_kernel_geom::CylinderSurface) -> f64 {
    let d = *point - cyl.center;
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);
    let u = d.dot(&y_dir).atan2(d.dot(ref_dir));
    if u < 0.0 { u + 2.0 * std::f64::consts::PI } else { u }
}

/// Check if angle `u` is within the range from `u_start` to `u_end` (CCW direction).
/// Handles wrap-around at 2π. For wrap-around cases, u_end may be > 2π.
fn angle_in_range(u: f64, u_start: f64, u_end: f64) -> bool {
    let tol = 0.01;
    let two_pi = 2.0 * std::f64::consts::PI;

    // If u_end > 2π, the face wraps around. Check if u is in [u_start, 2π) or [0, u_end - 2π)
    if u_end > two_pi {
        let end_wrapped = u_end - two_pi;
        // u is in range if it's in [u_start, 2π) or [0, end_wrapped)
        (u > u_start + tol && u < two_pi - tol) || (u > tol && u < end_wrapped - tol)
    } else if u_end >= u_start {
        // Simple case: range doesn't wrap around
        u > u_start + tol && u < u_end - tol
    } else {
        // Range wraps around 2π (e.g., from 5.5 to 0.5)
        u > u_start + tol || u < u_end - tol
    }
}

/// Split a cylindrical face along a line intersection curve.
///
/// When a plane parallel to the cylinder axis intersects the cylinder,
/// the result is a vertical line on the cylinder surface. In the cylinder's
/// UV space `[0, 2π] × [v_min, v_max]`, this line becomes a vertical line
/// at constant u = u_split.
///
/// This function splits the cylindrical face into two parts:
/// - One part: `[u_min, u_split] × [v_min, v_max]`
/// - Other part: `[u_split, u_max] × [v_min, v_max]`
///
/// Works for both:
/// - Full lateral faces (single seam vertex, u spans 0 to 2π)
/// - Partial lateral faces (4 corner vertices from previous splits)
pub fn split_cylindrical_face_by_line(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify surface is a cylinder
    let cyl = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        Some(c) => c.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);

    // Find the U parameter of the split line
    let d = line.origin - cyl.center;
    let u_split = d.dot(&y_dir).atan2(d.dot(ref_dir));
    let u_split = if u_split < 0.0 {
        u_split + 2.0 * std::f64::consts::PI
    } else {
        u_split
    };

    // Get the current face's vertex bounds
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Collect all unique vertices with their (v, u) coordinates
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    let mut all_verts: Vec<(vcad_kernel_topo::VertexId, f64, f64)> = Vec::new(); // (vid, v, u)

    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        v_min = v_min.min(v);
        v_max = v_max.max(v);

        // Only add if not duplicate
        if !all_verts.iter().any(|(id, _, _)| *id == vid) {
            let u = compute_cylinder_u(&point, &cyl);
            all_verts.push((vid, v, u));
        }
    }

    // Separate into top and bottom vertices
    let bottom_verts: Vec<_> = all_verts.iter()
        .filter(|(_, v, _)| (*v - v_min).abs() < 1e-6)
        .cloned()
        .collect();
    let top_verts: Vec<_> = all_verts.iter()
        .filter(|(_, v, _)| (*v - v_max).abs() < 1e-6)
        .cloned()
        .collect();


    // Determine face type and get corner vertices
    let (u_start, u_end, v_start_bot, v_end_bot, v_start_top, v_end_top, is_full_face) =
        if bottom_verts.len() == 1 && top_verts.len() == 1 {
            // Full cylindrical face with single seam vertex at each end
            // U spans from 0 (seam) around to 2π (back to seam)
            let seam_u = bottom_verts[0].2;
            (seam_u, seam_u + 2.0 * std::f64::consts::PI,
             bottom_verts[0].0, bottom_verts[0].0,
             top_verts[0].0, top_verts[0].0,
             true)
        } else if bottom_verts.len() == 2 && top_verts.len() == 2 {
            // Partial cylindrical face with 4 corner vertices
            // Use the loop order to determine the U direction (CCW in UV space)
            //
            // For a face with loop: u0 -> u1 -> u1 -> u0, going CCW:
            // - If u1 > u0, the face spans [u0, u1]
            // - If u1 < u0, the face spans [u0, 2π] ∪ [0, u1] (wrap-around)

            // Find the first two distinct U values in the loop
            let mut first_u: Option<f64> = None;
            let mut second_u: Option<f64> = None;
            for &he_id in &loop_hes {
                let vid = brep.topology.half_edges[he_id].origin;
                let point = brep.topology.vertices[vid].point;
                let u = compute_cylinder_u(&point, &cyl);

                match first_u {
                    None => first_u = Some(u),
                    Some(fu) if (u - fu).abs() > 0.01 => {
                        second_u = Some(u);
                        break;
                    }
                    _ => {}
                }
            }

            let (u0, u1) = match (first_u, second_u) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            };

            // Determine if the face wraps around based on the direction of travel
            // If we go from u0 to u1 CCW and u1 < u0, we're wrapping around 2π
            let wraps_around = u1 < u0 - 0.01;

            // Find start/end vertices based on the U values
            let (b1, b2) = (bottom_verts[0], bottom_verts[1]);
            let (t1, t2) = (top_verts[0], top_verts[1]);

            let (start_bot, end_bot) = if (b1.2 - u0).abs() < 0.01 {
                (b1, b2)
            } else {
                (b2, b1)
            };

            let (start_top, end_top) = if (t1.2 - u0).abs() < 0.01 {
                (t1, t2)
            } else {
                (t2, t1)
            };

            // For wrap-around faces, adjust end_u to be > 2π for proper range checking
            let end_u = if wraps_around {
                end_bot.2 + 2.0 * std::f64::consts::PI
            } else {
                end_bot.2
            };

            (start_bot.2, end_u,
             start_bot.0, end_bot.0,
             start_top.0, end_top.0,
             false)
        } else {
            // Unexpected face structure
            return SplitResult {
                sub_faces: vec![face_id],
            };
        };

    // Check if split line is within the face's U range
    let in_range = if is_full_face {
        // For full face, any u_split is valid (except exactly at the seam)
        let seam_u = u_start;
        (u_split - seam_u).abs() > 0.01 &&
        (u_split - seam_u - 2.0 * std::f64::consts::PI).abs() > 0.01
    } else {
        angle_in_range(u_split, u_start, u_end)
    };

    if !in_range {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Compute 3D points at the split line's top and bottom
    let sin_u = u_split.sin();
    let cos_u = u_split.cos();
    let radial = cyl.radius * (cos_u * ref_dir + sin_u * y_dir);
    let point_bottom = cyl.center + radial + v_min * cyl.axis.as_ref();
    let point_top = cyl.center + radial + v_max * cyl.axis.as_ref();

    // Create or reuse vertices at the split points
    let tolerance = 1e-6;
    let v_split_bottom = find_or_create_vertex(brep, &point_bottom, tolerance);
    let v_split_top = find_or_create_vertex(brep, &point_top, tolerance);

    // Create two new faces by splitting at the u_split line:
    // Face 1: from start to split (smaller U arc)
    // Face 2: from split to end (larger U arc, or to seam for full face)

    // Face 1: arc from start to split
    let he1_bot = brep.topology.add_half_edge(v_start_bot);
    let he1_left = brep.topology.add_half_edge(v_split_bottom);
    let he1_top = brep.topology.add_half_edge(v_split_top);
    let he1_right = brep.topology.add_half_edge(v_start_top);

    let loop1 = brep
        .topology
        .add_loop(&[he1_bot, he1_left, he1_top, he1_right]);
    let face1 = brep
        .topology
        .add_face(loop1, surface_index, orientation);

    // Face 2: arc from split to end
    let he2_bot = brep.topology.add_half_edge(v_split_bottom);
    let he2_left = brep.topology.add_half_edge(v_end_bot);
    let he2_top = brep.topology.add_half_edge(v_end_top);
    let he2_right = brep.topology.add_half_edge(v_split_top);

    let loop2 = brep
        .topology
        .add_loop(&[he2_bot, he2_left, he2_top, he2_right]);
    let face2 = brep
        .topology
        .add_face(loop2, surface_index, orientation);

    // Add twin edges for the shared split line
    brep.topology.add_edge(he1_left, he2_right);
    brep.topology.add_edge(he1_top, he2_bot);

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the split line
    brep.geometry.add_curve_3d(Box::new(line.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Split a cylindrical face along an intersection curve.
///
/// This dispatches to the appropriate split method based on the curve type:
/// - Circle: horizontal split (perpendicular plane intersection)
/// - Line: vertical split (parallel plane intersection)
/// - Sampled: general oblique split - TODO
pub fn split_cylindrical_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
) -> SplitResult {
    match curve {
        IntersectionCurve::Circle(circle) => {
            split_cylindrical_face_by_circle(brep, face_id, circle)
        }
        IntersectionCurve::Line(line) => {
            split_cylindrical_face_by_line(brep, face_id, line)
        }
        IntersectionCurve::Sampled(_points) => {
            // TODO: Implement general oblique split
            SplitResult {
                sub_faces: vec![face_id],
            }
        }
        IntersectionCurve::Empty | IntersectionCurve::Point(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            split_cylindrical_face(brep, face_id, &IntersectionCurve::Line(line1.clone()))
        }
    }
}

// =============================================================================
// Circular Face (Disk) Splitting by Line
// =============================================================================

/// Check if a face is a circular disk (a planar face bounded by a single circle).
///
/// A circular disk has:
/// - A planar underlying surface
/// - A single vertex in its outer loop (the seam point on the circle)
/// - No inner loops
pub fn is_circular_disk_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    // Must be a plane
    if surface.surface_type() != vcad_kernel_geom::SurfaceKind::Plane {
        return false;
    }

    // Check if it has a single vertex (circular boundary)
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 {
        return false;
    }

    // No inner loops
    face.inner_loops.is_empty()
}

/// Get the circle parameters of a circular disk face.
///
/// Returns (center, radius, normal) if the face is a valid circular disk.
pub fn get_disk_circle_params(brep: &BRepSolid, face_id: FaceId) -> Option<(Point3, f64, vcad_kernel_math::Vec3)> {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;

    // Get the seam vertex - this is on the circle at angle 0
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 {
        return None;
    }

    let seam_vertex_id = brep.topology.half_edges[loop_hes[0]].origin;
    let seam_point = brep.topology.vertices[seam_vertex_id].point;

    // Circle center is the plane origin
    let center = plane.origin;
    let radius = (seam_point - center).norm();
    let normal = *plane.normal_dir.as_ref();

    Some((center, radius, normal))
}

/// Split a circular disk face along a line intersection curve.
///
/// When a plane intersects another plane that contains a circular disk,
/// the result is a line that may cross the disk. This function splits
/// the disk into two parts along the line:
///
/// - If the line passes through the center: two half-disks (semicircles)
/// - If the line is a chord: a smaller chord segment + larger segment
///
/// The line must actually cross the disk boundary at two points for
/// splitting to occur.
///
/// Each resulting face has:
/// - A straight edge along the split line
/// - An arc edge along the original circle
pub fn split_circular_face_by_line(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
    segments: u32,
) -> SplitResult {
    // Get disk parameters
    let (center, radius, normal) = match get_disk_circle_params(brep, face_id) {
        Some(params) => params,
        None => return SplitResult { sub_faces: vec![face_id] },
    };

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;

    // Project the line onto the disk's plane to find intersection points with the circle
    // The line-circle intersection in 2D:
    // Circle: |p - center| = radius
    // Line: p = origin + t * direction

    // Find direction perpendicular to line in the plane
    let line_dir = line.direction.normalize();

    // Check if line is parallel to the plane normal (no intersection)
    if line_dir.dot(&normal).abs() > 0.999 {
        return SplitResult { sub_faces: vec![face_id] };
    }

    // Project line onto the plane
    // Find the closest point on the line to the circle center
    let to_center = center - line.origin;
    let t_closest = to_center.dot(&line_dir);
    let closest_point = line.origin + t_closest * line_dir;

    // Distance from line to center
    let dist_to_center = (closest_point - center).norm();

    // If line doesn't intersect the circle, no split needed
    if dist_to_center > radius - 1e-9 {
        return SplitResult { sub_faces: vec![face_id] };
    }

    // Compute intersection points with circle
    // Half-chord length: sqrt(r² - d²)
    let half_chord = (radius * radius - dist_to_center * dist_to_center).sqrt();

    // Intersection points
    let p1 = closest_point - half_chord * line_dir;
    let p2 = closest_point + half_chord * line_dir;

    // Verify both points are on the plane (within tolerance)
    let surface = &brep.geometry.surfaces[surface_index];
    let plane = match surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
        Some(p) => p,
        None => return SplitResult { sub_faces: vec![face_id] },
    };

    if plane.signed_distance(&p1).abs() > 0.1 || plane.signed_distance(&p2).abs() > 0.1 {
        return SplitResult { sub_faces: vec![face_id] };
    }

    // Compute angles of intersection points relative to center
    // Use the plane's local coordinate system
    let x_axis = plane.x_dir.normalize();
    let y_axis = plane.y_dir.normalize();

    let to_p1 = p1 - center;
    let to_p2 = p2 - center;

    let angle1 = to_p1.dot(&y_axis).atan2(to_p1.dot(&x_axis));
    let angle2 = to_p2.dot(&y_axis).atan2(to_p2.dot(&x_axis));

    // Normalize angles to [0, 2π)
    let angle1 = if angle1 < 0.0 { angle1 + 2.0 * std::f64::consts::PI } else { angle1 };
    let angle2 = if angle2 < 0.0 { angle2 + 2.0 * std::f64::consts::PI } else { angle2 };

    // Order angles so we know which arc is which
    let (start_angle, end_angle, start_pt, end_pt) = if angle1 < angle2 {
        (angle1, angle2, p1, p2)
    } else {
        (angle2, angle1, p2, p1)
    };

    // Create vertices for the intersection points
    let tolerance = 1e-6;
    let v_start = find_or_create_vertex(brep, &start_pt, tolerance);
    let v_end = find_or_create_vertex(brep, &end_pt, tolerance);

    // Generate arc vertices for both faces
    // Face 1: arc from start_angle to end_angle (shorter arc if < π, longer otherwise)
    // Face 2: arc from end_angle to start_angle (wrapping around 2π)

    let arc1_span = end_angle - start_angle;
    let arc2_span = 2.0 * std::f64::consts::PI - arc1_span;

    // Number of segments for each arc (proportional to arc length)
    let n1 = ((segments as f64) * arc1_span / (2.0 * std::f64::consts::PI)).max(2.0) as u32;
    let n2 = ((segments as f64) * arc2_span / (2.0 * std::f64::consts::PI)).max(2.0) as u32;

    // Generate arc 1 vertices (from start to end, counterclockwise)
    let mut arc1_points: Vec<Point3> = Vec::with_capacity((n1 + 1) as usize);
    arc1_points.push(snap_point(start_pt));
    for i in 1..n1 {
        let t = i as f64 / n1 as f64;
        let angle = start_angle + t * arc1_span;
        let (sin_a, cos_a) = angle.sin_cos();
        let pt = center + radius * (cos_a * x_axis + sin_a * y_axis);
        arc1_points.push(snap_point(pt));
    }
    arc1_points.push(snap_point(end_pt));

    // Generate arc 2 vertices (from end to start, counterclockwise, wrapping around)
    let mut arc2_points: Vec<Point3> = Vec::with_capacity((n2 + 1) as usize);
    arc2_points.push(snap_point(end_pt));
    for i in 1..n2 {
        let t = i as f64 / n2 as f64;
        let angle = end_angle + t * arc2_span;
        let (sin_a, cos_a) = angle.sin_cos();
        let pt = center + radius * (cos_a * x_axis + sin_a * y_axis);
        arc2_points.push(snap_point(pt));
    }
    arc2_points.push(snap_point(start_pt));

    // Create Face 1: arc from start to end + chord from end to start
    // Loop: start → arc points → end → chord → back to start
    let mut face1_verts: Vec<vcad_kernel_topo::VertexId> = Vec::new();
    face1_verts.push(v_start);
    for pt in arc1_points.iter().skip(1).take(arc1_points.len() - 2) {
        face1_verts.push(find_or_create_vertex(brep, pt, tolerance));
    }
    face1_verts.push(v_end);

    // Create half-edges and loop for face 1
    let hes1: Vec<_> = face1_verts.iter().map(|&v| brep.topology.add_half_edge(v)).collect();
    let loop1 = brep.topology.add_loop(&hes1);
    let face1 = brep.topology.add_face(loop1, surface_index, orientation);

    // Create Face 2: arc from end to start + chord from start to end
    let mut face2_verts: Vec<vcad_kernel_topo::VertexId> = Vec::new();
    face2_verts.push(v_end);
    for pt in arc2_points.iter().skip(1).take(arc2_points.len() - 2) {
        face2_verts.push(find_or_create_vertex(brep, pt, tolerance));
    }
    face2_verts.push(v_start);

    // Create half-edges and loop for face 2
    let hes2: Vec<_> = face2_verts.iter().map(|&v| brep.topology.add_half_edge(v)).collect();
    let loop2 = brep.topology.add_loop(&hes2);
    let face2 = brep.topology.add_face(loop2, surface_index, orientation);

    // Add twin edges for the chord (shared edge between face1 and face2)
    // In face1, the chord goes from v_end to v_start (last edge)
    // In face2, the chord goes from v_start to v_end (last edge)
    // These are twins
    let chord_he1 = hes1[hes1.len() - 1]; // v_end → v_start in face1
    let chord_he2 = hes2[hes2.len() - 1]; // v_start → v_end in face2
    brep.topology.add_edge(chord_he1, chord_he2);

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id].faces.retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the split line (chord)
    brep.geometry.add_curve_3d(Box::new(vcad_kernel_geom::Line3d::from_points(start_pt, end_pt)));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Split a circular disk face along an intersection curve.
///
/// Dispatches to the appropriate method based on curve type:
/// - Line: splits disk into two arc-bounded segments
/// - Circle: not applicable (circle on circle is degenerate)
/// - Other: no split
pub fn split_circular_disk_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    segments: u32,
) -> SplitResult {
    match curve {
        IntersectionCurve::Line(line) => {
            split_circular_face_by_line(brep, face_id, line, segments)
        }
        IntersectionCurve::TwoLines(line1, line2) => {
            // Split by the first line, then by the second
            let result1 = split_circular_face_by_line(brep, face_id, line1, segments);
            if result1.sub_faces.len() < 2 {
                return result1;
            }
            // Now split each resulting face by the second line
            let mut all_faces = Vec::new();
            for &fid in &result1.sub_faces {
                // Check if this face is still a circular disk (it won't be after the first split)
                // For non-disk faces after first split, we'd need polygon splitting
                // For now, just add them as-is
                if is_circular_disk_face(brep, fid) {
                    let result2 = split_circular_face_by_line(brep, fid, line2, segments);
                    all_faces.extend(result2.sub_faces);
                } else {
                    // The face is now a chord-segment, not a full disk
                    // Try to split it as a planar face by the line
                    let result2 = split_planar_face(
                        brep,
                        fid,
                        &IntersectionCurve::Line(line2.clone()),
                        &Point3::origin(),
                        &Point3::origin(),
                        segments,
                    );
                    all_faces.extend(result2.sub_faces);
                }
            }
            SplitResult { sub_faces: all_faces }
        }
        _ => {
            // No split for other curve types on circular faces
            SplitResult { sub_faces: vec![face_id] }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_find_closest_edge() {
        let square = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        // Point on the bottom edge
        let edge = find_closest_edge(&square, &Point3::new(5.0, 0.0, 0.0));
        assert_eq!(edge, 0);

        // Point on the right edge
        let edge = find_closest_edge(&square, &Point3::new(10.0, 5.0, 0.0));
        assert_eq!(edge, 1);

        // Point on the top edge
        let edge = find_closest_edge(&square, &Point3::new(5.0, 10.0, 0.0));
        assert_eq!(edge, 2);

        // Point on the left edge
        let edge = find_closest_edge(&square, &Point3::new(0.0, 5.0, 0.0));
        assert_eq!(edge, 3);
    }

    #[test]
    fn test_point_to_segment_dist() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(10.0, 0.0, 0.0);

        // Point on the segment
        assert!(point_to_segment_dist(&Point3::new(5.0, 0.0, 0.0), &a, &b) < 1e-10);

        // Point above the segment midpoint
        let dist = point_to_segment_dist(&Point3::new(5.0, 3.0, 0.0), &a, &b);
        assert!((dist - 3.0).abs() < 1e-10);

        // Point beyond endpoint
        let dist = point_to_segment_dist(&Point3::new(15.0, 0.0, 0.0), &a, &b);
        assert!((dist - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_split_face_cube() {
        let mut brep = make_cube(10.0, 10.0, 10.0);

        // Find the bottom face (z=0)
        let bottom_face = brep
            .topology
            .faces
            .iter()
            .find(|(fid, _)| {
                let verts: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(brep.topology.faces[*fid].outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                verts.iter().all(|v| v.z.abs() < 1e-10)
            })
            .map(|(fid, _)| fid);

        if let Some(face_id) = bottom_face {
            let initial_face_count = brep.topology.faces.len();

            // Split the bottom face with a line from (5,0,0) to (5,10,0)
            let entry = Point3::new(5.0, 0.0, 0.0);
            let exit = Point3::new(5.0, 10.0, 0.0);
            let curve = IntersectionCurve::Line(vcad_kernel_geom::Line3d {
                origin: entry,
                direction: exit - entry,
            });

            let result = split_face_by_curve(&mut brep, face_id, &curve, &entry, &exit);

            // Should produce 2 sub-faces
            assert_eq!(result.sub_faces.len(), 2);

            // Total faces should increase by 1 (original removed, 2 new added: +2 - 1 = +1)
            assert_eq!(brep.topology.faces.len(), initial_face_count + 1);
        }
    }

    /// Test splitting a cube's z=0 face by a circle centered at its corner.
    /// This is the exact scenario for cube-cylinder difference at origin.
    #[test]
    fn test_split_z0_face_by_corner_circle() {
        use vcad_kernel_geom::Circle3d;

        let mut brep = make_cube(20.0, 20.0, 20.0);
        println!("\n=== Test: Split z=0 face by corner circle ===");

        // Find the z=0 face (bottom)
        let z0_face = brep
            .topology
            .faces
            .iter()
            .find(|(fid, _)| {
                let face = &brep.topology.faces[*fid];
                let verts: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                // z=0 face has all vertices with z ≈ 0
                verts.iter().all(|v| v.z.abs() < 0.01)
            })
            .map(|(fid, _)| fid);

        let z0_face_id = z0_face.expect("Should find z=0 face");
        println!("Found z=0 face: {:?}", z0_face_id);

        // Print face vertices
        let face = &brep.topology.faces[z0_face_id];
        let verts: Vec<Point3> = brep
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();
        println!("Face vertices ({}):", verts.len());
        for (i, v) in verts.iter().enumerate() {
            println!("  v{}: ({:.1}, {:.1}, {:.1})", i, v.x, v.y, v.z);
        }

        // Create circle centered at corner (0,0,0) with radius 10
        let circle = Circle3d::with_normal(
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
        );
        println!("Circle: center=({:.1},{:.1},{:.1}), r={:.1}",
            circle.center.x, circle.center.y, circle.center.z, circle.radius);

        // Check if circle is partially inside
        let is_partial = circle_partially_inside_polygon(&verts, &circle);
        println!("Circle partially inside polygon: {}", is_partial);

        // Try to split
        let initial_faces = brep.topology.faces.len();
        let result = split_planar_face_by_circle(&mut brep, z0_face_id, &circle, 32);
        println!("Split result: {} sub-faces", result.sub_faces.len());
        println!("Total faces after: {} (was {})", brep.topology.faces.len(), initial_faces);

        // Print result face info
        for &fid in &result.sub_faces {
            if brep.topology.faces.contains_key(fid) {
                let f = &brep.topology.faces[fid];
                let vs: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(f.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();

                let min_x = vs.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
                let max_x = vs.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
                let min_y = vs.iter().map(|v| v.y).fold(f64::INFINITY, f64::min);
                let max_y = vs.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);

                println!("  {:?}: {} verts, x=[{:.1},{:.1}], y=[{:.1},{:.1}]",
                    fid, vs.len(), min_x, max_x, min_y, max_y);
            }
        }

        // The split should produce 2 faces
        assert!(
            result.sub_faces.len() >= 2,
            "Expected at least 2 sub-faces from arc split, got {}",
            result.sub_faces.len()
        );
    }
}
