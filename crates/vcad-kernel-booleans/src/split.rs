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
use vcad_kernel_topo::{FaceId, HalfEdgeId, Orientation};

use crate::ssi::IntersectionCurve;

/// Result of splitting a face.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The face IDs of the newly created sub-faces.
    /// If no splitting occurred, contains just the original face ID.
    pub sub_faces: Vec<FaceId>,
}

/// For a sampled intersection curve, collect the polyline points strictly
/// between the entry and exit points (ordered entry → exit), so the cut
/// edge can follow the true curve instead of a single chord. A chord
/// disagrees with the curved-side split boundary by the arc's sagitta,
/// which is enough to flip classification probes near the cut (the torr
/// B1 family). Returns an empty list for non-sampled curves.
fn cut_polyline_between(
    curve: &IntersectionCurve,
    entry_point: &Point3,
    exit_point: &Point3,
) -> Vec<Point3> {
    let IntersectionCurve::Sampled(points) = curve else {
        return Vec::new();
    };
    if points.len() < 3 {
        return Vec::new();
    }
    // Project a point onto the polyline: (segment index + fraction) as a
    // scalar parameter in [0, N−1].
    let project = |p: &Point3| -> f64 {
        let mut best = 0.0f64;
        let mut best_d = f64::INFINITY;
        for i in 0..points.len() - 1 {
            let a = points[i];
            let b = points[i + 1];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            let q = a + t * ab;
            let d = (*p - q).norm_squared();
            if d < best_d {
                best_d = d;
                best = i as f64 + t;
            }
        }
        best
    };
    let s_entry = project(entry_point);
    let s_exit = project(exit_point);
    let (lo, hi, rev) = if s_entry <= s_exit {
        (s_entry, s_exit, false)
    } else {
        (s_exit, s_entry, true)
    };
    let mut via: Vec<Point3> = points
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i as f64) > lo + 1e-9 && (*i as f64) < hi - 1e-9)
        .map(|(_, p)| *p)
        .collect();
    if rev {
        via.reverse();
    }
    // On a CLOSED polyline the direct parameter walk lo→hi may be the long
    // way around (the cut interval straddles the parameter seam): walking
    // it drags far-side samples — points nowhere near this face — into the
    // cut edge and mints phantom geometry. Take the wrap-around complement
    // when it is shorter.
    let closed = points.len() > 3 && (points[0] - points[points.len() - 1]).norm() < 1e-9;
    if closed {
        let path_len = |pts: &[Point3], a: &Point3, b: &Point3| -> f64 {
            let mut total = 0.0;
            let mut prev = *a;
            for p in pts {
                total += (*p - prev).norm();
                prev = *p;
            }
            total + (*b - prev).norm()
        };
        let direct = path_len(&via, entry_point, exit_point);
        let mut wrap: Vec<Point3> = points
            .iter()
            .enumerate()
            .take(points.len() - 1) // skip the duplicated closing point
            .filter(|(i, _)| (*i as f64) < lo - 1e-9 || (*i as f64) > hi + 1e-9)
            .map(|(_, p)| *p)
            .collect();
        // Complement travels hi → end → start → lo; order it from the exit
        // side: rotate so it starts just after `hi`.
        let pivot = wrap
            .iter()
            .position(|p| project(p) > hi)
            .unwrap_or(0);
        wrap.rotate_left(pivot);
        if rev {
            wrap.reverse();
        }
        let wrapped = path_len(&wrap, entry_point, exit_point);
        if wrapped + 1e-9 < direct {
            return wrap;
        }
    }
    via
}


macro_rules! split_dbg {
    ($($arg:tt)*) => {
        if std::env::var("VCAD_SPLIT_DEBUG").is_ok() {
            eprintln!($($arg)*);
        }
    };
}

/// Split a face along an intersection curve.
///
/// The curve must already be trimmed to the face's domain. This function:
/// 1. Projects the curve into UV space
/// 2. Finds where it enters/exits the face boundary
/// 3. Splits the boundary loop at entry/exit points
/// 4. Creates two new face loops, joined along the curve's polyline (or a
///    straight chord for analytic curves)
pub fn split_face_by_curve(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
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

    // Find the two edges where the curve enters and exits the face. An
    // endpoint that lands on (or near) a CORNER is within tolerance of two
    // edges; picking the nearest edge independently for entry and exit can
    // then assign both to the same edge and reject a perfectly valid cut.
    // Instead collect every edge within slack of each endpoint and pick a
    // DISTINCT pair with minimal combined distance when one exists.
    let edge_candidates = |p: &Point3| -> Vec<(usize, f64)> {
        let (best_e, best_d) = find_closest_edge_with_dist(&loop_verts, p);
        let slack = (best_d * 1.5).max(1e-6);
        let mut out = Vec::new();
        for i in 0..loop_verts.len() {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % loop_verts.len()];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            let d = (*p - (a + t * ab)).norm();
            if d <= slack + 1e-9 {
                out.push((i, d));
            }
        }
        if out.is_empty() {
            out.push((best_e, best_d));
        }
        out
    };
    let entry_cands = edge_candidates(entry_point);
    let exit_cands = edge_candidates(exit_point);
    let mut entry_edge = entry_cands[0].0;
    let mut exit_edge = exit_cands[0].0;
    let mut entry_dist = entry_cands[0].1;
    let mut exit_dist = exit_cands[0].1;
    let mut best_sum = f64::INFINITY;
    let mut found_distinct = false;
    for &(e1, d1) in &entry_cands {
        for &(e2, d2) in &exit_cands {
            if e1 == e2 {
                continue;
            }
            if d1 + d2 < best_sum {
                best_sum = d1 + d2;
                entry_edge = e1;
                exit_edge = e2;
                entry_dist = d1;
                exit_dist = d2;
                found_distinct = true;
            }
        }
    }
    if !found_distinct {
        entry_edge = entry_cands[0].0;
        exit_edge = exit_cands[0].0;
        entry_dist = entry_cands[0].1;
        exit_dist = exit_cands[0].1;
    }

    // If entry or exit point is too far from any edge, the split line doesn't cross this face
    let max_dist_tolerance = 1.0; // Allow some tolerance for numerical precision
    if entry_dist > max_dist_tolerance || exit_dist > max_dist_tolerance {
        split_dbg!("sfbc: entry/exit off-boundary d=({entry_dist:.4},{exit_dist:.4})");
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    if entry_edge == exit_edge || (*exit_point - *entry_point).norm() < 1e-6 {
        // Curve enters and exits on the same edge, or grazes a single
        // vertex — no real cut.
        split_dbg!(
            "sfbc: same-edge or zero cut (edges {entry_edge},{exit_edge}) entry={:?} exit={:?} loop={:?}",
            entry_point,
            exit_point,
            loop_verts
                .iter()
                .map(|p| ((p.x * 100.0).round() / 100.0, (p.y * 100.0).round() / 100.0, (p.z * 100.0).round() / 100.0))
                .collect::<Vec<_>>()
        );
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // A cut whose midpoint lies ON the face boundary runs along an existing
    // edge (two operand planes can share their intersection line with this
    // face — e.g. a blade's tangent bottom plane and its side plane both
    // cross an end cap along the same chord). Splitting again along that
    // chord would emit a duplicate of the face plus a zero-area sliver.
    {
        let mid = Point3::new(
            0.5 * (entry_point.x + exit_point.x),
            0.5 * (entry_point.y + exit_point.y),
            0.5 * (entry_point.z + exit_point.z),
        );
        let (_, mid_dist) = find_closest_edge_with_dist(&loop_verts, &mid);
        if mid_dist < 1e-7 {
            split_dbg!("sfbc: cut runs along boundary");
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    }

    // Insert new vertices at entry and exit points
    let _v_entry = brep.topology.add_vertex(*entry_point);
    let _v_exit = brep.topology.add_vertex(*exit_point);

    // Build two new vertex loops by walking the original loop
    // Loop 1: entry_point → (edges from entry to exit) → exit_point → (cut back)
    // Loop 2: exit_point → (edges from exit to entry) → entry_point → (cut back)

    // The cut edge between exit and entry follows the true intersection
    // curve when it is sampled (via points), not a single chord.
    let via = cut_polyline_between(curve, entry_point, exit_point);

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
    // Close along the cut curve: exit → entry (via reversed).
    loop1_points.extend(via.iter().rev());

    // Walk from exit_edge to entry_edge (other direction)
    loop2_points.push(*exit_point);
    idx = (exit_edge + 1) % n;
    while idx != (entry_edge + 1) % n {
        loop2_points.push(loop_verts[idx]);
        idx = (idx + 1) % n;
    }
    loop2_points.push(*entry_point);
    // Close along the cut curve: entry → exit (via forward).
    loop2_points.extend(via.iter());

    // Remove consecutive duplicate vertices (can happen when split points
    // coincide with existing vertices)
    let loop1_points = remove_consecutive_duplicates(&loop1_points, 1e-6);
    let loop2_points = remove_consecutive_duplicates(&loop2_points, 1e-6);
    {
        let zr = |pts: &[Point3]| {
            (
                pts.iter().map(|p| p.z).fold(f64::MAX, f64::min),
                pts.iter().map(|p| p.z).fold(f64::MIN, f64::max),
            )
        };
        let (fz0, fz1) = zr(&loop_verts);
        let (a0, a1) = zr(&loop1_points);
        let (b0, b1) = zr(&loop2_points);
        if a0 < fz0 - 1.0 || a1 > fz1 + 1.0 || b0 < fz0 - 1.0 || b1 > fz1 + 1.0 {
            split_dbg!(
                "sfbc RANGE: face z[{fz0:.2},{fz1:.2}] loops z[{a0:.2},{a1:.2}]/[{b0:.2},{b1:.2}] entry {entry_point:?} exit {exit_point:?} via {} pts",
                via.len()
            );
        }
    }

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
            total += e1.cross(e2);
        }
        0.5 * total.norm()
    }

    let area1 = polygon_area_3d(&loop1_points);
    let area2 = polygon_area_3d(&loop2_points);
    // Minimum area threshold - faces smaller than this are considered degenerate
    // The value 0.001 catches thin strips while allowing legitimate small faces
    let min_area = 0.001;

    if area1 < min_area || area2 < min_area {
        split_dbg!("sfbc: degenerate areas {area1:.5} {area2:.5}");
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
pub(crate) fn find_or_create_vertex(
    brep: &mut BRepSolid,
    point: &Point3,
    tolerance: f64,
) -> vcad_kernel_topo::VertexId {
    if std::env::var("VCAD_VERTEX_TRAP").is_ok() && (point.z < -1.0 || point.z > 14.0) {
        panic!("vertex trap: {point:?}");
    }
    if std::env::var("VCAD_RING_TRAP").is_ok()
        && (point.z - 8.8967).abs() < 1e-3
        && point.x < -15.0
    {
        panic!("ring trap: {point:?}");
    }
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
    let t = ap.dot(ab) / len2;
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
fn find_line_polygon_crossings(polygon: &[Point3], line: &vcad_kernel_geom::Line3d) -> Vec<Point3> {
    use vcad_kernel_math::predicates::{orient2d, Sign};

    let n = polygon.len();
    if n < 3 {
        return Vec::new();
    }

    // Compute the polygon's plane normal from the first 3 vertices
    let e1 = polygon[1] - polygon[0];
    let e2 = polygon[2] - polygon[0];
    let plane_normal = e1.cross(e2);
    let plane_normal_len = plane_normal.norm();
    if plane_normal_len < 1e-12 {
        return Vec::new(); // Degenerate polygon
    }
    let plane_normal = plane_normal / plane_normal_len;

    // Build a 2D coordinate system on the plane
    let x_axis = e1.normalize();
    let y_axis = plane_normal.cross(x_axis);

    // Project polygon vertices and line to 2D
    let project_to_2d = |p: &Point3| -> Point2 {
        let d = *p - polygon[0];
        Point2::new(d.dot(x_axis), d.dot(y_axis))
    };

    let poly_2d: Vec<Point2> = polygon.iter().map(&project_to_2d).collect();

    // Project line origin and direction
    let line_origin_2d = project_to_2d(&line.origin);
    let dx = line.direction.dot(x_axis);
    let dy = line.direction.dot(y_axis);
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
        let is_duplicate = crossings
            .iter()
            .any(|c: &Point3| (*c - intersection).norm() < 0.01);
        if !is_duplicate {
            crossings.push(intersection);
        }
    }

    // Sort crossings by their parameter along the line
    let line_dir = line.direction.normalize();
    crossings.sort_by(|a, b| {
        let ta = (*a - line.origin).dot(line_dir);
        let tb = (*b - line.origin).dot(line_dir);
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
    // A circle approximated by fewer than 3 vertices can't form a valid face
    // loop; constructing one would feed an empty slice to `add_loop` and panic.
    // The kernel resolves the `0 = auto` sentinel upstream, so this only trips
    // for callers that drive the booleans crate directly with a bad count —
    // leave the face unsplit rather than crash the whole evaluation.
    if segments < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

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
                let on_plane = to_v - to_v.dot(normal) * normal;
                let cap_radius = on_plane.norm();

                // Check: is the intersection circle coplanar and fully inside?
                // Tangency counts as inside — a boss ring exactly inscribed to
                // the cap rim (offset + r == cap_radius) must still punch its
                // hole through the cap; leaving the membrane whole corrupts
                // the volume integral by the full contact-disk flux.
                let circle_to_plane = (circle.center - center).dot(normal).abs();
                let circle_center_offset = {
                    let d = circle.center - center;
                    (d - d.dot(normal) * normal).norm()
                };
                // A circle that IS the cap's own rim (concentric, same
                // radius — e.g. a press-fit ring whose wall lies exactly on
                // the wall that bounds this cap) has nothing to split:
                // admitting it would punch a hole covering the entire cap
                // and leave a duplicate full-size disk. This is the
                // degenerate-loop analog of `circle_is_own_boundary`.
                if circle_to_plane < 1e-6
                    && circle_center_offset < 1e-6
                    && (circle.radius - cap_radius).abs() < 1e-6
                {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
                let circle_inside = circle_to_plane < 1e-6
                    && circle_center_offset + circle.radius <= cap_radius + 1e-6;
                // Partial overlap: part of the circle is within the rim, part
                // sticks out (a boss overhanging the edge of a disc).
                let circle_overlaps = circle_to_plane < 1e-6
                    && !circle_inside
                    && circle_center_offset - circle.radius < cap_radius - 1e-6
                    && circle_center_offset < cap_radius + circle.radius - 1e-6;

                if circle_overlaps && cap_radius > 1e-12 {
                    // The degenerate single-vertex outer loop can't express a
                    // partial split. Sample the rim into an explicit polygon
                    // outer loop and re-enter this function: the polygonal
                    // path detects the partial overlap and routes to the arc
                    // splitter. Sample densely — the polygon becomes the
                    // cap's real boundary from here on, and a coarse rim
                    // under-integrates the cap area.
                    let n_rim = segments.max(128) as usize;
                    let cap_x = on_plane.normalize();
                    let cap_y = normal.cross(cap_x);
                    let rim_ids: Vec<_> = (0..n_rim)
                        .map(|i| {
                            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n_rim as f64);
                            let p =
                                center + cap_radius * (theta.cos() * cap_x + theta.sin() * cap_y);
                            find_or_create_vertex(brep, &p, 1e-6)
                        })
                        .collect();
                    let rim_hes: Vec<_> = rim_ids
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let new_outer = brep.topology.add_loop(&rim_hes);
                    brep.topology.faces[face_id].outer_loop = new_outer;
                    return split_planar_face_by_circle(brep, face_id, circle, segments);
                }

                if circle_inside && cap_radius > 1e-12 {
                    let tolerance = 1e-6;

                    // Generate circle vertices for the inner disk face
                    let raw_circle_verts: Vec<Point3> = (0..segments)
                        .map(|i| {
                            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                            let (sin_t, cos_t) = theta.sin_cos();
                            circle.center
                                + circle.radius
                                    * (cos_t * circle.x_dir.into_inner()
                                        + sin_t * circle.y_dir.into_inner())
                        })
                        .collect();

                    // The SSI circle's (x_dir, y_dir) frame is arbitrary, so
                    // the generated ring can wind either way. The tessellator
                    // expects loops CCW around the STORED surface normal
                    // (`Orientation::Reversed` flips triangles afterwards) —
                    // normalize the disk loop to that convention. The disk is
                    // usually classified Inside and dropped, but when it
                    // survives (a bore mouth kept as the visible floor under
                    // a press-fit ring) a backwards loop tessellates facing
                    // into the solid and the volume integral goes negative.
                    let cap_x_dir = on_plane.normalize();
                    let cap_y_dir = normal.cross(cap_x_dir);
                    let circle_signed_area = {
                        let project = |p: &Point3| -> (f64, f64) {
                            let d = *p - center;
                            (d.dot(cap_x_dir), d.dot(cap_y_dir))
                        };
                        let pts_2d: Vec<_> = raw_circle_verts.iter().map(project).collect();
                        let mut a = 0.0;
                        for i in 0..pts_2d.len() {
                            let j = (i + 1) % pts_2d.len();
                            a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
                        }
                        a / 2.0
                    };
                    let circle_verts: Vec<Point3> = if circle_signed_area < 0.0 {
                        raw_circle_verts.iter().rev().cloned().collect()
                    } else {
                        raw_circle_verts
                    };

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
                    let inner_face = brep
                        .topology
                        .add_face(inner_loop, surface_index, orientation);

                    // Route the cap's pre-existing inner loops (holes from prior
                    // booleans) to the sub-face that now contains them, mirroring
                    // the polygonal-outer-loop path below. A hole inside the
                    // splitting circle belongs to the new disk face; leaving it on
                    // the cap both strands a nested hole on the ring and lets the
                    // disk seal the hole with a phantom membrane (e.g. a bearing
                    // bore closed at its mouth), which corrupts point-in-solid
                    // parity for every later boolean against this solid.
                    let existing_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
                    for lp in existing_inner {
                        let lp_verts: Vec<Point3> = brep
                            .topology
                            .loop_half_edges(lp)
                            .map(|he| {
                                brep.topology.vertices[brep.topology.half_edges[he].origin].point
                            })
                            .collect();
                        if lp_verts.is_empty() {
                            continue;
                        }
                        let test_pt = if lp_verts.len() == 1 {
                            lp_verts[0]
                        } else {
                            let n = lp_verts.len() as f64;
                            Point3::new(
                                lp_verts.iter().map(|v| v.x).sum::<f64>() / n,
                                lp_verts.iter().map(|v| v.y).sum::<f64>() / n,
                                lp_verts.iter().map(|v| v.z).sum::<f64>() / n,
                            )
                        };
                        if (test_pt - circle.center).norm() < circle.radius - tolerance {
                            brep.topology.faces[face_id]
                                .inner_loops
                                .retain(|&l| l != lp);
                            brep.topology.faces[inner_face].inner_loops.push(lp);
                        }
                    }

                    // Add the circle as an inner loop (hole) on the original cap face.
                    // Inner loops wind opposite to the outer convention: the
                    // disk loop is CCW around the surface normal, so the hole
                    // is its reverse (CW).
                    let hole_verts: Vec<Point3> = circle_verts.iter().rev().cloned().collect();

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
                        let outer_he = hole_hes[(segments as usize - 1 - i) % segments as usize];
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

    // Generate circle vertices in the SSI circle's own (x_dir, y_dir) frame;
    // the winding relative to the face is normalized below.
    let raw_circle_verts: Vec<Point3> = (0..segments)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (sin_t, cos_t) = theta.sin_cos();
            circle.center
                + circle.radius
                    * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
        })
        .collect();

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
        e1.cross(e2)
    };
    let e1 = loop_verts[1] - loop_verts[0];
    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(e1).normalize();
    let origin = loop_verts[0];

    // Project to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u_axis), d.dot(v_axis))
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
    let circle_area = signed_area(&raw_circle_verts);

    // The disk sub-face's outer loop must wind the SAME way as the parent's
    // outer loop (both are outer loops of faces with identical surface and
    // orientation — the tessellator triangulates by loop order and flips for
    // `Orientation::Reversed`, so a backwards disk loop would tessellate
    // facing into the solid and corrupt the volume integral whenever the
    // disk survives classification). The hole loop is its reverse.
    let circle_verts: Vec<Point3> = if (outer_area > 0.0) == (circle_area > 0.0) {
        raw_circle_verts
    } else {
        raw_circle_verts.iter().rev().cloned().collect()
    };

    // Create inner face (disk) - uses circle vertices as its outer loop
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

    // Create outer face (polygon with hole): the outer loop stays the same;
    // the circle joins as an inner loop with opposite winding to the outer.
    let inner_loop_verts: Vec<Point3> = circle_verts.iter().rev().cloned().collect();

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
    let v_axis = normal.cross(u_axis);
    let v_axis_len = v_axis.norm();
    if v_axis_len < 1e-12 {
        return false;
    }
    let v_axis = v_axis / v_axis_len;

    let project = |p: &Point3| -> (f64, f64) {
        let d = p - v0;
        (d.dot(u_axis), d.dot(v_axis))
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
    // i.e., distance from center to each edge must be > radius.
    // A circle tangent to a polygon edge (distance ≈ radius) also disqualifies
    // it from the "strictly inside" path: splitting a face whose inner hole
    // touches its outer boundary produces degenerate tessellation, and the
    // inscribed cylinder case is handled correctly by leaving the face whole
    // and relying on coincidence-based classification.
    let n = poly_2d.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = poly_2d[i];
        let (x2, y2) = poly_2d[j];

        let dist = point_to_segment_dist_2d(cx, cy, x1, y1, x2, y2);
        if dist < circle.radius + 1e-6 {
            return false;
        }
    }

    true
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
pub(crate) fn point_to_segment_dist_2d(
    px: f64,
    py: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> f64 {
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
            let is_duplicate = intersections
                .iter()
                .any(|other: &CirclePolygonIntersection| {
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
    let normal = e1.cross(e2);
    let normal_len = normal.norm();
    if normal_len < 1e-12 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let u_axis = e1.normalize();
    let v_axis = normal.cross(e1).normalize();
    let origin = v0;

    // Project polygon vertices to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u_axis), d.dot(v_axis))
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

    // Tangent-inside case: when the circle is fully inside the polygon but
    // tangent to two polygon edges, both arcs (the two halves of the circle
    // separated by the chord through the tangent points) lie inside the
    // polygon. Picking either arc arbitrarily produces a face whose boundary
    // bulges through the disk in the wrong direction (issue #165). Split the
    // polygon along the chord first, then recurse: each sub-polygon contains
    // exactly one of the two arcs, so the inside/outside check resolves
    // unambiguously.
    let arc2_mid_angle = arc1_mid_angle + std::f64::consts::PI;
    let arc2_mid_x = center_2d.0 + circle.radius * arc2_mid_angle.cos();
    let arc2_mid_y = center_2d.1 + circle.radius * arc2_mid_angle.sin();
    let arc2_inside = point_in_polygon_2d(arc2_mid_x, arc2_mid_y, &poly_2d);
    if arc1_inside && arc2_inside {
        return split_planar_face_tangent_inside(brep, face_id, circle, segments, int1, int2);
    }
    if !arc1_inside && !arc2_inside {
        // Neither arc's midpoint lies inside the polygon: the circle runs
        // along the face boundary (a re-application of a circle this face
        // was already cut by — its arc IS an edge now, and the midpoint
        // probe lands on/outside it). Blindly taking the complementary arc
        // here swept a near-full circle through the face and minted a
        // phantom sub-face covering the far side of the disk.
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

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

    let _ = arc_span;
    // Arc travels CCW in the (u, v) plane frame = CCW about u×v. Interior
    // points MUST come from the canonical absolute grid so the cylindrical
    // wall bordering this same circle emits identical vertices.
    let plane_normal = u_axis.cross(v_axis);
    let arc_points_3d = canonical_arc_points(
        circle.center,
        circle.radius,
        plane_normal,
        inside_start.point,
        inside_end.point,
        segments,
    );

    // A cut whose arc midpoint lies ON the face boundary runs along an
    // existing arc edge: the same circle reaches this face once per wall
    // piece of the other operand, and re-splitting along the previous cut
    // emits a duplicate face plus a phantom sliver (the arc analog of the
    // duplicate-chord guard in split_face_by_curve). The probe must be the
    // TRUE angular midpoint of the arc — short arcs carry no interior
    // polyline vertices, and an endpoint always sits on the boundary.
    {
        let n_hat = plane_normal.normalize();
        let d0 = inside_start.point - circle.center;
        let d1 = inside_end.point - circle.center;
        let ang = {
            let cross = d0.cross(&d1).dot(&n_hat);
            let a = cross.atan2(d0.dot(&d1));
            a.rem_euclid(2.0 * std::f64::consts::PI)
        };
        let half = 0.5 * ang;
        let (sin_h, cos_h) = half.sin_cos();
        // Rodrigues rotation of d0 by `half` about n̂.
        let rot = d0 * cos_h + n_hat.cross(&d0) * sin_h + n_hat * n_hat.dot(&d0) * (1.0 - cos_h);
        let mid = circle.center + rot.normalize() * circle.radius;
        let mut min_d = f64::INFINITY;
        let n = loop_verts.len();
        for i in 0..n {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % n];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((mid - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            min_d = min_d.min((mid - (a + t * ab)).norm());
        }
        split_dbg!("arc guard: mid {mid:?} min_d {min_d:.2e} nv {n}");
        let fz0 = loop_verts.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let fz1 = loop_verts.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        let az0 = arc_points_3d.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let az1 = arc_points_3d.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        if az0 < fz0 - 1.0 || az1 > fz1 + 1.0 {
            split_dbg!(
                "arc RANGE: face z[{fz0:.2},{fz1:.2}] arc z[{az0:.2},{az1:.2}] start {:?} end {:?}",
                inside_start.point,
                inside_end.point
            );
        }
        // Tolerance covers the ≤5 µm chord sag between the true circle and
        // a previously inserted arc polyline; a fresh cut hugging the
        // boundary this closely would only mint a sub-sag sliver anyway.
        if min_d < 6e-3 && std::env::var("VCAD_NO_ARCGUARD").is_err() {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    }

    // Build Face 1: the inside-circle portion
    // Walk polygon from inside_end edge to inside_start edge, then add arc back
    let n = loop_verts.len();
    let same_edge = inside_start.edge_index == inside_end.edge_index;
    let mut face1_points: Vec<Point3> = Vec::new();

    // Start at inside_end intersection
    face1_points.push(inside_end.point);

    // Walk polygon from inside_end edge to inside_start edge. When both
    // intersections lie on the same polygon edge, the inside-circle piece
    // is bounded by only the arc and the chord segment on that edge — no
    // polygon vertices are traversed.
    if !same_edge {
        let mut idx = (inside_end.edge_index + 1) % n;
        while idx != (inside_start.edge_index + 1) % n {
            face1_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    }

    // Add inside_start intersection
    face1_points.push(inside_start.point);

    // Add arc points (from inside_start to inside_end, forward direction)
    // arc_points_3d goes from inside_start to inside_end, so iterate forward
    // This completes the loop: ... → inside_start → arc → (closes to inside_end)
    for pt in arc_points_3d
        .iter()
        .skip(1)
        .take(arc_points_3d.len().saturating_sub(2))
    {
        face1_points.push(*pt);
    }

    // Build Face 2: the outside-circle portion (polygon outside the arc)
    // Walk polygon from inside_start edge to inside_end edge, then add arc back
    let mut face2_points: Vec<Point3> = Vec::new();

    // Start at inside_start intersection
    face2_points.push(inside_start.point);

    // Walk polygon from inside_start edge to inside_end edge. In the same-edge
    // case, the outside-circle piece encloses the *entire* polygon apart from
    // the chord segment, so we walk all n polygon vertices starting after
    // inside_start's edge.
    if same_edge {
        let mut idx = (inside_start.edge_index + 1) % n;
        for _ in 0..n {
            face2_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    } else {
        let mut idx = (inside_start.edge_index + 1) % n;
        while idx != (inside_end.edge_index + 1) % n {
            face2_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    }

    // Add inside_end intersection
    face2_points.push(inside_end.point);

    // Add arc points from inside_end back to inside_start (forward arc direction)
    // arc_points_3d goes from inside_start → inside_end, so we take interior and reverse
    // to go from inside_end → inside_start
    for pt in arc_points_3d.iter().skip(1).rev().skip(1) {
        face2_points.push(*pt);
    }

    // When the intersection circle passes through polygon vertices, the polygon walk
    // includes the vertex AND the function then pushes the intersection point, creating
    // a zero-length duplicate edge. Remove consecutive duplicates (including wrap-around)
    // before validating and building faces.
    let tolerance = 1e-6;
    let face1_points = remove_consecutive_duplicates(&face1_points, tolerance);
    let face2_points = remove_consecutive_duplicates(&face2_points, tolerance);

    // Validate faces have at least 3 vertices
    if face1_points.len() < 3 || face2_points.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create the two new faces

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

    // Re-home the original face's inner loops (holes from prior booleans)
    // onto whichever sub-face contains them — dropping them here would seal
    // each hole with a phantom membrane that corrupts point-in-solid parity
    // and the volume integral. A degenerate single-vertex loop is a full
    // circle; its seam vertex stands in for the whole hole, which is safe
    // because the arc split never runs between a hole and its own boundary.
    let existing_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
    if !existing_inner.is_empty() {
        let face1_2d: Vec<(f64, f64)> = face1_points
            .iter()
            .map(|p| {
                let d = *p - origin;
                (d.dot(u_axis), d.dot(v_axis))
            })
            .collect();
        for lp in existing_inner {
            let lp_verts: Vec<Point3> = brep
                .topology
                .loop_half_edges(lp)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if lp_verts.is_empty() {
                continue;
            }
            let test_pt = if lp_verts.len() == 1 {
                lp_verts[0]
            } else {
                let n = lp_verts.len() as f64;
                Point3::new(
                    lp_verts.iter().map(|v| v.x).sum::<f64>() / n,
                    lp_verts.iter().map(|v| v.y).sum::<f64>() / n,
                    lp_verts.iter().map(|v| v.z).sum::<f64>() / n,
                )
            };
            let d = test_pt - origin;
            let target = if point_in_polygon_2d(d.dot(u_axis), d.dot(v_axis), &face1_2d) {
                face1
            } else {
                face2
            };
            brep.topology.faces[target].inner_loops.push(lp);
        }
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the arc
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Handle the tangent-inside case where a circle lies fully inside a polygon
/// but is tangent to two polygon edges.
///
/// Strategy: split the polygon along the chord between the two tangent points
/// using `split_face_by_curve` (line-based), producing two simple sub-polygons.
/// Each sub-polygon contains exactly one of the two circle arcs, so calling
/// `split_planar_face_by_arc` on each resolves the disk's relevant half-circle
/// unambiguously.
///
/// This is essential when a Difference cutter is itself a Union of primitives
/// whose lateral surfaces share a tangent boundary (e.g. a rect with a tangent
/// cylinder), where the cylinder's full SSI circle on the target's planar face
/// would otherwise straddle two regions of different classification.
fn split_planar_face_tangent_inside(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    int1: &CirclePolygonIntersection,
    int2: &CirclePolygonIntersection,
) -> SplitResult {
    // Build a Line3d through the two tangent points to act as the chord.
    let chord_dir = int2.point - int1.point;
    let chord_len = chord_dir.norm();
    if chord_len < 1e-9 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }
    let chord_line = vcad_kernel_geom::Line3d {
        origin: int1.point,
        direction: chord_dir / chord_len,
    };
    let chord_curve = IntersectionCurve::Line(chord_line);

    // Split the polygon along the chord; sub-faces split on the original face's
    // outer loop at int1 and int2.
    let chord_result = split_face_by_curve(brep, face_id, &chord_curve, &int1.point, &int2.point);
    if chord_result.sub_faces.len() < 2 {
        return chord_result;
    }

    // Now split each sub-face by the arc. Each sub-polygon should contain
    // exactly one of the two arc midpoints, so the inside/outside check
    // resolves correctly.
    let mut all_sub_faces = Vec::new();
    for sub_id in chord_result.sub_faces {
        if !brep.topology.faces.contains_key(sub_id) {
            continue;
        }
        let sub_result = split_planar_face_by_arc(brep, sub_id, circle, segments);
        // If the recursive split made progress (>= 2 sub-faces), use those;
        // otherwise the sub-face was left intact (e.g. its arc midpoint missed
        // the polygon by a tolerance margin) — keep the unsplit sub-face.
        if sub_result.sub_faces.len() >= 2 {
            all_sub_faces.extend(sub_result.sub_faces);
        } else {
            all_sub_faces.push(sub_id);
        }
    }

    SplitResult {
        sub_faces: all_sub_faces,
    }
}

/// Check if a circle partially intersects a polygon (crosses exactly 2 edges).
///
/// Returns true if the circle crosses the polygon boundary at exactly 2 points,
/// meaning it's only partially inside and needs arc-based splitting.
fn circle_partially_inside_polygon(
    polygon: &[Point3],
    circle: &vcad_kernel_geom::Circle3d,
) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for consistent projection
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(e1).normalize();

    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - v0;
        (d.dot(u_axis), d.dot(v_axis))
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
                // A grazing line (both crossings at one vertex) is a
                // zero-length cut: "splitting" would emit a copy of the
                // face plus a degenerate sliver.
                if (actual_exit - actual_entry).norm() < 1e-6 {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
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

    // A face whose loop is a real spherical polygon (a fillet corner
    // patch, or a sub-face left by an earlier circle split) must be
    // CLIPPED by the circle — the whole-sphere path below throws the
    // existing boundary away and hands back two faces bounded by this
    // circle alone. Splitting such a face three times (the three planes
    // of a box cutting a corner blend) then yielded four identical,
    // fully-overlapping patches: volume counted several times over and a
    // shell full of holes. An untrimmed sphere keeps the 4-vertex seam
    // cycle, which is not a boundary, so it still takes the path below.
    if brep
        .topology
        .loop_len(brep.topology.faces[face_id].outer_loop)
        != 4
    {
        if let Some(result) =
            clip_spherical_face_by_circle(brep, face_id, circle, segments, tolerance)
        {
            return result;
        }
        // The clip declined (circle misses the patch, crosses its
        // boundary somewhere other than exactly twice, or can't seat its
        // connector). Fall through to the whole-sphere split — that is
        // what this face got before the clip existed, so declining never
        // costs behavior that used to work.
    }

    // Generate the N shared circle vertices.
    let n = segments as usize;
    let circle_verts: Vec<Point3> = (0..segments)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (sin_t, cos_t) = theta.sin_cos();
            circle.center
                + circle.radius
                    * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
        })
        .collect();
    let circle_vert_ids: Vec<_> = circle_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Cap-A walks the circle in forward order: v[0], v[1], ..., v[N-1].
    // Half-edge i has origin v[i] and points toward v[(i+1) mod N].
    let he_a: Vec<_> = (0..n)
        .map(|i| brep.topology.add_half_edge(circle_vert_ids[i]))
        .collect();
    let loop_a = brep.topology.add_loop(&he_a);

    // Cap-B walks the circle in reverse: v[0], v[N-1], v[N-2], ..., v[1].
    // Half-edge j has origin v[(N - j) mod N] (so j=0 → v[0], j=1 → v[N-1], ...).
    // Cap-B's edge j spans {v[(N-j) mod N], v[(N-1-j) mod N]} — same physical
    // edge as cap-A's edge i = N-1-j, traversed in reverse.
    let he_b: Vec<_> = (0..n)
        .map(|j| brep.topology.add_half_edge(circle_vert_ids[(n - j) % n]))
        .collect();
    let loop_b = brep.topology.add_loop(&he_b);

    // Repurpose the original face as cap-A: clear holes, replace outer loop.
    // Same surface, orientation, shell. The original 4-edge seam loop is
    // now orphaned (no face references it) — benign because all kernel
    // traversal goes from faces.
    {
        let f = &mut brep.topology.faces[face_id];
        f.outer_loop = loop_a;
        f.inner_loops.clear();
    }

    // Cap-B is a new face with the same surface and orientation.
    let face_b = brep.topology.add_face(loop_b, surface_index, orientation);

    // Twin-pair the two caps' half-edges along the shared circle.
    for i in 0..n {
        brep.topology.add_edge(he_a[i], he_b[n - 1 - i]);
    }

    // Add cap-B to the same shell as the original face.
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face_b);
        brep.topology.faces[face_b].shell = Some(shell_id);
    }

    // Add the SSI circle as a 3D curve in the geometry store.
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![face_id, face_b],
    }
}

/// Clip an already-trimmed spherical face by a circle lying on its sphere.
///
/// The face's loop is walked as a spherical polygon; where it crosses the
/// circle's plane the crossing point is solved on the great arc between
/// the straddling vertices, and the two sides are closed off with a
/// shared, identically-sampled arc of the circle so they weld.
///
/// Returns `None` — leaving the caller to fall back to the whole-sphere
/// split — when the circle misses the patch, when it crosses the
/// boundary anywhere other than exactly twice (a multiply-crossing clip
/// needs region tracking this doesn't do), or when the connecting arc
/// can't be seated inside the patch.
fn clip_spherical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    tolerance: f64,
) -> Option<SplitResult> {
    use vcad_kernel_math::Vec3;

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let sph = brep.geometry.surfaces[surface_index]
        .as_any()
        .downcast_ref::<vcad_kernel_geom::SphereSurface>()?
        .clone();
    if !face.inner_loops.is_empty() {
        return None;
    }
    let center = sph.center;
    let radius = sph.radius.abs();
    if radius < 1e-9 {
        return None;
    }

    let x_dir = circle.x_dir.into_inner();
    let y_dir = circle.y_dir.into_inner();
    let normal = x_dir.cross(y_dir).normalize();
    let plane_d = (circle.center - center).dot(normal);
    let side = |p: &Point3| (*p - center).dot(normal) - plane_d;

    let verts: Vec<Point3> = brep
        .topology
        .loop_vertices(brep.topology.faces[face_id].outer_loop)
        .iter()
        .map(|v| brep.topology.vertices[*v].point)
        .collect();
    let m = verts.len();
    if m < 3 {
        return None;
    }
    let eps = (radius * 1e-9).max(1e-9);
    let sides: Vec<f64> = verts.iter().map(side).collect();
    if sides.iter().all(|s| *s > -eps) || sides.iter().all(|s| *s < eps) {
        // The circle doesn't cross this patch's boundary. Either it lies
        // somewhere else on the sphere entirely — in which case there is
        // nothing to split and falling through to the whole-sphere path
        // would REPLACE this patch with two copies bounded by a circle
        // it never touches — or it sits wholly inside the patch, which
        // needs an inner-loop split this routine doesn't build yet.
        // Probe the circle: if no point of it is in the face, report a
        // clean no-op instead of declining.
        let mut any_inside = false;
        for k in 0..16 {
            let ang = 2.0 * std::f64::consts::PI * k as f64 / 16.0;
            let p = circle.center + circle.radius * (ang.cos() * x_dir + ang.sin() * y_dir);
            if crate::trim::point_in_face(brep, face_id, &p) {
                any_inside = true;
                break;
            }
        }
        if !any_inside {
            return Some(SplitResult {
                sub_faces: vec![face_id],
            });
        }
        return None;
    }

    // Crossing point on the great arc between two straddling vertices:
    // bisect the arc on the plane's signed distance.
    let arc_crossing = |a: &Point3, b: &Point3| -> Point3 {
        let (ua, ub) = ((*a - center) / radius, (*b - center) / radius);
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        let point_at = |t: f64| -> Point3 {
            let dir = ua + t * (ub - ua);
            let n = dir.norm();
            if n < 1e-12 {
                center + radius * ua
            } else {
                center + radius * (dir / n)
            }
        };
        let s_lo = side(&point_at(0.0));
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if side(&point_at(mid)) * s_lo > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        point_at(0.5 * (lo + hi))
    };

    let mut crossings: Vec<(usize, Point3)> = Vec::new();
    for i in 0..m {
        let j = (i + 1) % m;
        let (si, sj) = (sides[i], sides[j]);
        if (si > eps && sj < -eps) || (si < -eps && sj > eps) {
            crossings.push((i, arc_crossing(&verts[i], &verts[j])));
        }
    }
    if crossings.len() != 2 {
        return None;
    }
    let (e1, x_a) = crossings[0];
    let (e2, x_b) = crossings[1];

    // The two boundary chains between the crossings.
    let walk = |from: usize, to: usize| -> Vec<Point3> {
        let mut out = Vec::new();
        let mut i = (from + 1) % m;
        loop {
            if i == (to + 1) % m {
                break;
            }
            out.push(verts[i]);
            i = (i + 1) % m;
        }
        out
    };
    let chain1 = walk(e1, e2);
    let chain2 = walk(e2, e1);

    // Connector: the arc of the circle between the two crossings that
    // runs through the patch. Both candidate arcs are tested by their
    // midpoint; the one inside the original face wins.
    let angle_of = |p: &Point3| -> f64 {
        let d = *p - circle.center;
        d.dot(y_dir).atan2(d.dot(x_dir))
    };
    let (a_ang, b_ang) = (angle_of(&x_a), angle_of(&x_b));
    let on_circle = |ang: f64| -> Point3 {
        circle.center + circle.radius * (ang.cos() * x_dir + ang.sin() * y_dir)
    };
    let mut sweep = b_ang - a_ang;
    while sweep <= -std::f64::consts::PI {
        sweep += 2.0 * std::f64::consts::PI;
    }
    while sweep > std::f64::consts::PI {
        sweep -= 2.0 * std::f64::consts::PI;
    }
    let alt = if sweep > 0.0 {
        sweep - 2.0 * std::f64::consts::PI
    } else {
        sweep + 2.0 * std::f64::consts::PI
    };
    let inside = |s: f64| crate::trim::point_in_face(brep, face_id, &on_circle(a_ang + 0.5 * s));
    let sweep = if inside(sweep) {
        sweep
    } else if inside(alt) {
        alt
    } else {
        return None;
    };

    // Sample the connector on the circle's CANONICAL grid — the same
    // θ = 2πk/segments schedule every other splitter uses for this
    // circle — rather than by evenly dividing this particular sweep.
    // Any face bounded by the same circle then lands on identical
    // points instead of a phase-shifted set that zippers open.
    let two_pi = 2.0 * std::f64::consts::PI;
    let step = two_pi / segments.max(3) as f64;
    let ang_eps = step * 1e-6;
    let mut conn_angles: Vec<f64> = Vec::new();
    if sweep > 0.0 {
        let mut k = (a_ang / step).floor() + 1.0;
        while k * step < a_ang + sweep - ang_eps {
            if k * step > a_ang + ang_eps {
                conn_angles.push(k * step);
            }
            k += 1.0;
        }
    } else {
        let mut k = (a_ang / step).ceil() - 1.0;
        while k * step > a_ang + sweep + ang_eps {
            if k * step < a_ang - ang_eps {
                conn_angles.push(k * step);
            }
            k -= 1.0;
        }
    }
    let connector: Vec<Point3> = conn_angles.iter().map(|a| on_circle(*a)).collect();

    // Materialize vertices. The connector is shared by both sub-faces
    // (same ids, opposite traversal) so the seam pairs into edges.
    let va = find_or_create_vertex(brep, &x_a, tolerance);
    let vb = find_or_create_vertex(brep, &x_b, tolerance);
    let conn_ids: Vec<_> = connector
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let chain_ids = |brep: &mut BRepSolid, pts: &[Point3]| -> Vec<vcad_kernel_topo::VertexId> {
        pts.iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect()
    };
    let chain1_ids = chain_ids(brep, &chain1);
    let chain2_ids = chain_ids(brep, &chain2);

    // Loop A: x_a → chain1 → x_b → connector reversed → back to x_a.
    let mut loop_a_verts = vec![va];
    loop_a_verts.extend(chain1_ids);
    loop_a_verts.push(vb);
    loop_a_verts.extend(conn_ids.iter().rev().copied());
    // Loop B: x_b → chain2 → x_a → connector forward → back to x_b.
    let mut loop_b_verts = vec![vb];
    loop_b_verts.extend(chain2_ids);
    loop_b_verts.push(va);
    loop_b_verts.extend(conn_ids.iter().copied());

    loop_a_verts.dedup();
    loop_b_verts.dedup();
    if loop_a_verts.len() < 3 || loop_b_verts.len() < 3 {
        return None;
    }

    let make_loop = |brep: &mut BRepSolid, vs: &[vcad_kernel_topo::VertexId]| {
        let hes: Vec<HalfEdgeId> = vs.iter().map(|v| brep.topology.add_half_edge(*v)).collect();
        let loop_id = brep.topology.add_loop(&hes);
        (loop_id, hes)
    };
    let (loop_a, he_a) = make_loop(brep, &loop_a_verts);
    let (loop_b, he_b) = make_loop(brep, &loop_b_verts);

    {
        let f = &mut brep.topology.faces[face_id];
        f.outer_loop = loop_a;
        f.inner_loops.clear();
    }
    let face_b = brep.topology.add_face(loop_b, surface_index, orientation);
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face_b);
        brep.topology.faces[face_b].shell = Some(shell_id);
    }

    // Pair the shared seam: A walks x_b → connector → x_a, B walks the
    // same vertices in reverse.
    let a_seam_start = loop_a_verts.len() - conn_ids.len() - 1;
    let b_seam_start = loop_b_verts.len() - conn_ids.len() - 1;
    let seam_len = conn_ids.len() + 1;
    for k in 0..seam_len {
        let ha = he_a[(a_seam_start + k) % he_a.len()];
        let hb = he_b[(b_seam_start + seam_len - 1 - k) % he_b.len()];
        brep.topology.add_edge(ha, hb);
    }

    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    let _ = Vec3::zeros();
    Some(SplitResult {
        sub_faces: vec![face_id, face_b],
    })
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
        IntersectionCurve::TwoSampled(_, _) => SplitResult {
            sub_faces: vec![face_id],
        },
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
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    let ca = cone.half_angle.cos();
    let apex_to_circle = (circle.center - cone.apex).dot(cone.axis.as_ref());

    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
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
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let sa = cone.half_angle.sin();
    let seam_point =
        cone.apex + v_split * (ca * cone.axis.into_inner() + sa * cone.ref_dir.into_inner());
    let v_split_seam = brep.topology.add_vertex(seam_point);

    let mut v_bottom = None;
    let mut v_top = None;
    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let d = point - cone.apex;
        let v = d.dot(cone.axis.as_ref()) / ca;
        if (v - v_min).abs() < 1e-6 {
            v_bottom = Some(vid);
        }
        if (v - v_max).abs() < 1e-6 {
            v_top = Some(vid);
        }
    }

    let (v_bottom, v_top) = match (v_bottom, v_top) {
        (Some(b), Some(t)) => (b, t),
        _ => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    // Lower face: v_min to v_split
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

    brep.topology.add_edge(he_lower_seam_up, he_lower_seam_down);
    brep.topology.add_edge(he_upper_seam_up, he_upper_seam_down);
    brep.topology.add_edge(he_lower_split, he_upper_split);

    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);
        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
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
pub fn split_cylindrical_face_by_circle_logged(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) -> SplitResult {
    split_dbg!(
        "legacy circle split on face {face_id:?} nv={} at z {:.4}",
        brep.topology.loop_len(brep.topology.faces[face_id].outer_loop),
        circle.center.z
    );
    split_cylindrical_face_by_circle(brep, face_id, circle)
}

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
/// Segment count for discretizing a circle. Both the planar-cap and
/// cylinder-wall splitters must use this same count so their shared arcs
/// discretize identically. It also has to agree with the display
/// tessellator's re-sampling of ANALYTIC circle boundaries that survive the
/// boolean untouched (`TessellationParams::from_segments(n)` resamples at
/// exactly `n`), so until boolean results freeze every boundary (see the
/// kernel-seam-freeze WIP branch) this must stay at the caller's count —
/// sag-adaptive densification here reopens every analytic/frozen seam.
pub(crate) fn arc_segments(radius: f64, segments: u32) -> u32 {
    const SAG: f64 = 5e-3;
    let n = if radius > SAG {
        let arg = (1.0 - SAG / radius).clamp(-1.0, 1.0);
        (std::f64::consts::PI / arg.acos()).ceil() as u32
    } else {
        3
    };
    n.max(segments).clamp(3, 512)
}

/// Canonical, frame-independent discretization of a circular arc.
///
/// Every face that borders (a sub-arc of) the same 3D circle must emit the
/// same interior points, or the sewn shell can never conform: the planar cap
/// samples in its plane frame, the cylinder wall in its `ref_dir` frame, and
/// relative-fraction sampling gives every arc its own phase. This helper
/// derives a canonical in-plane frame purely from the circle geometry
/// (sign-normalized axis + most-orthogonal global axis) and samples interior
/// points on the absolute angular grid `θ_k = 2πk/n` in that frame, so two
/// faces sharing an arc reproduce bit-identical interior points regardless
/// of their own parameterizations. Returns `[start, interior…, end]`; travel
/// is counterclockwise about `normal` from `start` to `end`.
pub(crate) fn canonical_arc_points(
    center: Point3,
    radius: f64,
    normal: vcad_kernel_math::Vec3,
    start: Point3,
    end: Point3,
    segments: u32,
) -> Vec<Point3> {
    use std::f64::consts::PI;
    let mut n_hat = normal.normalize();
    // Sign-canonicalize the axis so the grid is invariant under normal flip.
    let mut flipped = false;
    for c in [n_hat.x, n_hat.y, n_hat.z] {
        if c.abs() > 1e-9 {
            if c < 0.0 {
                n_hat = -n_hat;
                flipped = true;
            }
            break;
        }
    }
    // Canonical in-plane frame: global axis least parallel to n̂.
    let cand = [
        vcad_kernel_math::Vec3::new(1.0, 0.0, 0.0),
        vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0),
        vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
    ];
    let e = cand
        .into_iter()
        .min_by(|a, b| a.dot(n_hat).abs().partial_cmp(&b.dot(n_hat).abs()).unwrap())
        .unwrap();
    let x_axis = (e - n_hat * e.dot(n_hat)).normalize();
    let y_axis = n_hat.cross(x_axis);

    let angle_of = |p: Point3| -> f64 {
        let d = p - center;
        let a = d.dot(y_axis).atan2(d.dot(x_axis));
        if a < 0.0 {
            a + 2.0 * PI
        } else {
            a
        }
    };
    let a_start = angle_of(start);
    let a_end = angle_of(end);
    // CCW about the ORIGINAL normal; in the canonical frame that is CCW
    // unless the axis was flipped.
    let ccw = !flipped;
    let span = if ccw {
        (a_end - a_start).rem_euclid(2.0 * PI)
    } else {
        (a_start - a_end).rem_euclid(2.0 * PI)
    };
    let span = if span < 1e-12 { 2.0 * PI } else { span };

    let n = arc_segments(radius, segments);
    let step = 2.0 * PI / n as f64;
    let mut pts = vec![start];
    // Interior grid angles strictly inside the traversal (ε away from the
    // endpoints so a grid point coincident with an endpoint isn't doubled).
    let eps = 1e-9;
    // Walk grid indices in traversal order: find the first grid angle
    // strictly after a_start (in travel direction), then step until a_end.
    let first_idx = if ccw {
        (a_start / step).floor() + 1.0
    } else {
        (a_start / step).ceil() - 1.0
    };
    let mut idx = first_idx;
    loop {
        let ang = idx * step;
        let traveled = if ccw {
            (ang - a_start).rem_euclid(2.0 * PI)
        } else {
            (a_start - ang).rem_euclid(2.0 * PI)
        };
        if traveled <= eps || traveled >= span - eps {
            break;
        }
        let a = ang.rem_euclid(2.0 * PI);
        let (sin_a, cos_a) = a.sin_cos();
        pts.push(snap_point(
            center + radius * (cos_a * x_axis + sin_a * y_axis),
        ));
        if ccw {
            idx += 1.0;
        } else {
            idx -= 1.0;
        }
        if pts.len() > n as usize + 2 {
            break; // safety
        }
    }
    pts.push(end);
    pts
}

fn compute_cylinder_u(point: &Point3, cyl: &vcad_kernel_geom::CylinderSurface) -> f64 {
    let d = *point - cyl.center;
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);
    let u = d.dot(y_dir).atan2(d.dot(ref_dir));
    if u < 0.0 {
        u + 2.0 * std::f64::consts::PI
    } else {
        u
    }
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
    segments: u32,
) -> SplitResult {
    split_dbg!(
        "legacy line split on face {face_id:?} nv={}",
        brep.topology.loop_len(brep.topology.faces[face_id].outer_loop)
    );
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
    let u_split = d.dot(y_dir).atan2(d.dot(ref_dir));
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
    let bottom_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_min).abs() < 1e-6)
        .cloned()
        .collect();
    let top_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_max).abs() < 1e-6)
        .cloned()
        .collect();

    // Determine face type and get corner vertices
    let (u_start, u_end, v_start_bot, v_end_bot, v_start_top, v_end_top, is_full_face) =
        if bottom_verts.len() == 1 && top_verts.len() == 1 {
            // Full cylindrical face with single seam vertex at each end
            // U spans from 0 (seam) around to 2π (back to seam)
            let seam_u = bottom_verts[0].2;
            (
                seam_u,
                seam_u + 2.0 * std::f64::consts::PI,
                bottom_verts[0].0,
                bottom_verts[0].0,
                top_verts[0].0,
                top_verts[0].0,
                true,
            )
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

            (
                start_bot.2,
                end_u,
                start_bot.0,
                end_bot.0,
                start_top.0,
                end_top.0,
                false,
            )
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
        (u_split - seam_u).abs() > 0.01
            && (u_split - seam_u - 2.0 * std::f64::consts::PI).abs() > 0.01
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
    //
    // The top/bottom arcs are emitted as DENSE canonical chains (not single
    // chords): the neighboring cap faces discretize the same circles via
    // `canonical_arc_points`, so both sides produce identical vertices and
    // the sewn shell conforms. The resulting two-chain loops are exactly the
    // shape `tessellate_ruled_two_chain` renders verbatim.
    let axis = *cyl.axis.as_ref();
    let bot_center = cyl.center + v_min * axis;
    let top_center = cyl.center + v_max * axis;
    let chain_vids = |brep: &mut BRepSolid, center: Point3, from: Point3, to: Point3| {
        // Increasing u = CCW about the cylinder axis.
        canonical_arc_points(center, cyl.radius, axis, from, to, segments)
            .into_iter()
            .map(|p| find_or_create_vertex(brep, &p, tolerance))
            .collect::<Vec<_>>()
    };
    let build_face = |brep: &mut BRepSolid,
                      v_bot_a: vcad_kernel_topo::VertexId,
                      v_bot_b: vcad_kernel_topo::VertexId,
                      v_top_a: vcad_kernel_topo::VertexId,
                      v_top_b: vcad_kernel_topo::VertexId|
     -> (FaceId, HalfEdgeId, HalfEdgeId) {
        let p_bot_a = brep.topology.vertices[v_bot_a].point;
        let p_bot_b = brep.topology.vertices[v_bot_b].point;
        let p_top_a = brep.topology.vertices[v_top_a].point;
        let p_top_b = brep.topology.vertices[v_top_b].point;
        // Bottom chain ascending u (a→b), then up, then top chain
        // descending u (b→a), then down.
        let mut bot = chain_vids(brep, bot_center, p_bot_a, p_bot_b);
        let mut top = chain_vids(brep, top_center, p_top_a, p_top_b);
        // Reuse the canonical endpoint vertex ids.
        *bot.first_mut().unwrap() = v_bot_a;
        *bot.last_mut().unwrap() = v_bot_b;
        *top.first_mut().unwrap() = v_top_a;
        *top.last_mut().unwrap() = v_top_b;
        top.reverse(); // descending u: b → a

        // Loop origins: bottom chain a..b (b starts the "up" edge), then top
        // chain b..a (a starts the "down" edge closing to bottom a).
        let mut origins: Vec<vcad_kernel_topo::VertexId> = Vec::new();
        origins.extend(&bot[..bot.len() - 1]);
        origins.extend(&top[..top.len() - 1]);
        // `up` half-edge is the one whose origin is bot-end (last of bot
        // slice above is bot[len-2]; the up edge origin is bot[len-1]).
        origins.insert(bot.len() - 1, bot[bot.len() - 1]);
        // and the closing `down` half-edge originates at top's last (== a).
        origins.push(top[top.len() - 1]);

        let hes: Vec<_> = origins
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let lp = brep.topology.add_loop(&hes);
        let face = brep.topology.add_face(lp, surface_index, orientation);
        // Return (face, up_he, down_he) — the vertical edges at u=b and u=a.
        let up_he = hes[bot.len() - 1];
        let down_he = hes[hes.len() - 1];
        (face, up_he, down_he)
    };

    let (face1, he1_up, he1_down) =
        build_face(brep, v_start_bot, v_split_bottom, v_start_top, v_split_top);
    let (face2, he2_up, he2_down) =
        build_face(brep, v_split_bottom, v_end_bot, v_split_top, v_end_top);

    // Twin the shared vertical split line: face1 goes up at u_split, face2
    // comes back down at u_split.
    brep.topology.add_edge(he1_up, he2_down);
    // For a full face the two sub-faces also share the seam line.
    if is_full_face {
        brep.topology.add_edge(he1_down, he2_up);
    }

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
/// - Sampled: general oblique split via the band machinery (`cyl_band`)
///
/// Faces with wavy (non-constant-v) boundaries — produced by earlier
/// oblique splits — must NEVER reach the legacy rectangular splitters,
/// which infer a full-height rectangle from the v-extremes of the loop and
/// would emit overlapping phantom geometry. When the band machinery
/// declines such a face (curve misses it, or lands on its boundary), the
/// face is returned unsplit instead.
pub fn split_cylindrical_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    segments: u32,
) -> SplitResult {
    let wavy = crate::cyl_band::face_is_wavy_band(brep, face_id);
    // A dense (frozen-polyline) loop must not reach the legacy circle
    // splitter: it re-emits analytic seam loops sampled on the raw
    // `segments` grid, silently thawing a frozen boundary back into one
    // that can never conform with its sag-dense neighbors. The band
    // machinery preserves the loop's own columns. (Line splits stay
    // legacy-first: the legacy line splitter emits canonical dense chains
    // itself and handles full-face seam bookkeeping the band path lacks.)
    let dense = brep.topology.loop_len(brep.topology.faces[face_id].outer_loop) > 6;
    match curve {
        IntersectionCurve::Circle(circle) => {
            if wavy || dense {
                // `require_wavy` only when the face really is wavy: a dense
                // frozen band is rectangular (constant-v rings) and must
                // still be split by the band machinery. When the band path
                // declines (e.g. the circle grazes a band edge), fall
                // through to the legacy splitter rather than giving up.
                if let Some(r) =
                    crate::cyl_band::split_wavy_band_by_circle(brep, face_id, circle, wavy, segments)
                {
                    return r;
                }
                if wavy {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            }
            let result = split_cylindrical_face_by_circle_logged(brep, face_id, circle);
            if result.sub_faces.len() >= 2 {
                return result;
            }
            // The legacy splitter only parses degenerate seam loops and
            // 4-corner rectangles; arc-extruded walls carry dense sampled
            // loops it refuses. Retry through the band machinery, which
            // parses any two-chain loop (rectangular included).
            crate::cyl_band::split_wavy_band_by_circle(brep, face_id, circle, false, segments)
                .unwrap_or(result)
        }
        IntersectionCurve::Line(line) => {
            if wavy || dense {
                if let Some(r) =
                    crate::cyl_band::split_wavy_band_by_line(brep, face_id, line, wavy)
                {
                    return r;
                }
                if wavy {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            }
            let result = split_cylindrical_face_by_line(brep, face_id, line, segments);
            if result.sub_faces.len() >= 2 {
                return result;
            }
            crate::cyl_band::split_wavy_band_by_line(brep, face_id, line, false).unwrap_or(result)
        }
        IntersectionCurve::Sampled(points) => {
            // Oblique intersection (e.g. a tilted plane crossing the
            // cylinder in an ellipse): split in (u, v) parameter space via
            // the band machinery.
            match crate::cyl_band::split_cylindrical_face_by_sampled(brep, face_id, points) {
                Some(result) => result,
                None => {
                    // The curve is not a closed single-valued profile (e.g.
                    // a bounded cylinder-cylinder quartic arc) or the face
                    // is outside the band family. Log so the caller sees
                    // the operation failed instead of silently producing a
                    // geometrically wrong result.
                    eprintln!(
                        "[vcad-kernel-booleans] split_cylindrical_face: Sampled \
                         intersection ({} points) is not a closed single-valued \
                         profile on this face; returning face unsplit. Downstream \
                         boolean result may be incorrect for this face.",
                        points.len()
                    );
                    SplitResult {
                        sub_faces: vec![face_id],
                    }
                }
            }
        }
        IntersectionCurve::Empty | IntersectionCurve::Point(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            split_cylindrical_face(
                brep,
                face_id,
                &IntersectionCurve::Line(line1.clone()),
                segments,
            )
        }
        IntersectionCurve::TwoSampled(_, _) => {
            // TwoSampled is the analytic Steinmetz pair from
            // `ssi::cylinder_cylinder`. Cylinder × cylinder unions are
            // routed by `boolean_op` to a specialized mesh emitter
            // (`cylinder_cylinder_mesh_boolean`) that handles the
            // figure-8 boundary directly, so by the time we reach the
            // BRep splitter pipeline this curve type should already be
            // diverted. Defensive no-op for any caller that didn't
            // pre-filter.
            SplitResult {
                sub_faces: vec![face_id],
            }
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

    // Check if it has a single vertex (analytic circular boundary) or a
    // dense frozen ring (canonical polyline from crate::freeze) — every
    // vertex equidistant from the plane origin.
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 && !is_frozen_ring(brep, face_id) {
        return false;
    }

    // No inner loops
    face.inner_loops.is_empty()
}

/// True when the face's outer loop is a dense ring of vertices equidistant
/// from the plane origin — the canonical polyline `crate::freeze` writes in
/// place of an analytic full-circle boundary. 8+ vertices distinguishes a
/// frozen ring from box faces and low-count polygons that happen to be
/// cyclic.
fn is_frozen_ring(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let plane = match surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
        Some(p) => p,
        None => return false,
    };
    let center = plane.origin;
    let normal = *plane.normal_dir.as_ref();
    let mut radius: Option<f64> = None;
    let mut pts: Vec<Point3> = Vec::new();
    for he in brep.topology.loop_half_edges(face.outer_loop) {
        let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
        let r = (p - center).norm();
        match radius {
            None => radius = Some(r),
            Some(r0) => {
                if (r - r0).abs() > 1e-6_f64.max(r0 * 1e-9) {
                    return false;
                }
            }
        }
        pts.push(p);
    }
    if pts.len() < 8 {
        return false;
    }
    // All vertices equidistant is necessary but NOT sufficient: a half-disk
    // produced by a diameter cut has every vertex ON the circle too (the
    // chord endpoints are ring vertices), and treating it as a full disk
    // makes the next same-line split rebuild BOTH halves from the full
    // circle — duplicate faces and phantom slivers. A genuine frozen ring
    // walks the circle in uniform sag-scale steps; any large angular jump
    // between consecutive loop vertices is a chord, not a ring edge.
    let x_axis = {
        let d = pts[0] - center;
        let on = d - d.dot(&normal) * normal;
        if on.norm() < 1e-9 {
            return false;
        }
        on.normalize()
    };
    let y_axis = normal.cross(&x_axis);
    let ang = |p: &Point3| -> f64 {
        let d = *p - center;
        d.dot(&y_axis).atan2(d.dot(&x_axis))
    };
    for w in 0..pts.len() {
        let a = ang(&pts[w]);
        let b = ang(&pts[(w + 1) % pts.len()]);
        let mut d = (b - a).rem_euclid(2.0 * std::f64::consts::PI);
        if d > std::f64::consts::PI {
            d = 2.0 * std::f64::consts::PI - d;
        }
        if d > 0.2 {
            return false;
        }
    }
    true
}

/// Get the circle parameters of a circular disk face.
///
/// Returns (center, radius, normal) if the face is a valid circular disk.
pub fn get_disk_circle_params(
    brep: &BRepSolid,
    face_id: FaceId,
) -> Option<(Point3, f64, vcad_kernel_math::Vec3)> {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;

    // Get the seam vertex - this is on the circle at angle 0. A dense
    // frozen ring qualifies too: all its vertices are equidistant from the
    // plane origin, so the first one fixes the radius.
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 && !is_frozen_ring(brep, face_id) {
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
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
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
    if line_dir.dot(normal).abs() > 0.999 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Project line onto the plane
    // Find the closest point on the line to the circle center
    let to_center = center - line.origin;
    let t_closest = to_center.dot(line_dir);
    let closest_point = line.origin + t_closest * line_dir;

    // Distance from line to center
    let dist_to_center = (closest_point - center).norm();

    // If line doesn't intersect the circle, no split needed
    if dist_to_center > radius - 1e-9 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
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
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    if plane.signed_distance(&p1).abs() > 0.1 || plane.signed_distance(&p2).abs() > 0.1 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Compute angles of intersection points relative to center
    // Use the plane's local coordinate system
    let x_axis = plane.x_dir.normalize();
    let y_axis = plane.y_dir.normalize();

    let to_p1 = p1 - center;
    let to_p2 = p2 - center;

    let angle1 = to_p1.dot(y_axis).atan2(to_p1.dot(x_axis));
    let angle2 = to_p2.dot(y_axis).atan2(to_p2.dot(x_axis));

    // Normalize angles to [0, 2π)
    let angle1 = if angle1 < 0.0 {
        angle1 + 2.0 * std::f64::consts::PI
    } else {
        angle1
    };
    let angle2 = if angle2 < 0.0 {
        angle2 + 2.0 * std::f64::consts::PI
    } else {
        angle2
    };

    // Order angles so we know which arc is which
    let (_start_angle, _end_angle, start_pt, end_pt) = if angle1 < angle2 {
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

    // Both arcs travel counterclockwise about the plane frame's normal.
    // Sampling MUST be the canonical absolute grid — the cylinder-wall
    // splitter borders the same circles and has to emit identical points.
    let circle_normal = x_axis.cross(y_axis);
    let arc1_points = canonical_arc_points(
        center,
        radius,
        circle_normal,
        snap_point(start_pt),
        snap_point(end_pt),
        segments,
    );
    let arc2_points = canonical_arc_points(
        center,
        radius,
        circle_normal,
        snap_point(end_pt),
        snap_point(start_pt),
        segments,
    );

    // Create Face 1: arc from start to end + chord from end to start
    // Loop: start → arc points → end → chord → back to start
    let mut face1_verts: Vec<vcad_kernel_topo::VertexId> = Vec::new();
    face1_verts.push(v_start);
    for pt in arc1_points.iter().skip(1).take(arc1_points.len() - 2) {
        face1_verts.push(find_or_create_vertex(brep, pt, tolerance));
    }
    face1_verts.push(v_end);

    // Create half-edges and loop for face 1
    let hes1: Vec<_> = face1_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
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
    let hes2: Vec<_> = face2_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
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
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the split line (chord)
    brep.geometry
        .add_curve_3d(Box::new(vcad_kernel_geom::Line3d::from_points(
            start_pt, end_pt,
        )));

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
        IntersectionCurve::Line(line) => split_circular_face_by_line(brep, face_id, line, segments),
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
            SplitResult {
                sub_faces: all_faces,
            }
        }
        _ => {
            // No split for other curve types on circular faces
            SplitResult {
                sub_faces: vec![face_id],
            }
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
        println!(
            "Circle: center=({:.1},{:.1},{:.1}), r={:.1}",
            circle.center.x, circle.center.y, circle.center.z, circle.radius
        );

        // Check if circle is partially inside
        let is_partial = circle_partially_inside_polygon(&verts, &circle);
        println!("Circle partially inside polygon: {}", is_partial);

        // Try to split
        let initial_faces = brep.topology.faces.len();
        let result = split_planar_face_by_circle(&mut brep, z0_face_id, &circle, 32);
        println!("Split result: {} sub-faces", result.sub_faces.len());
        println!(
            "Total faces after: {} (was {})",
            brep.topology.faces.len(),
            initial_faces
        );

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

                println!(
                    "  {:?}: {} verts, x=[{:.1},{:.1}], y=[{:.1},{:.1}]",
                    fid,
                    vs.len(),
                    min_x,
                    max_x,
                    min_y,
                    max_y
                );
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

/// Sub-resolution fallback for thin faces: when a sampled cut curve crosses
/// a face narrower than its own sample spacing, the trim finds no interval,
/// but the two exact crossing vertices already sit ON the face's boundary
/// (inserted by the adjacent faces' splits against the same surface pair).
/// Detect exactly two such vertices and split along the chord between them.
pub(crate) fn split_thin_face_at_curve_vertices(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve_pts: &[Point3],
) -> Option<SplitResult> {
    if curve_pts.len() < 4 {
        return None;
    }
    let face = &brep.topology.faces[face_id];
    if !face.inner_loops.is_empty() {
        return None;
    }
    let loop_verts: Vec<Point3> = brep
        .topology
        .loop_half_edges(face.outer_loop)
        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();
    let n = loop_verts.len();
    if n < 4 {
        return None;
    }
    // Distance from a point to the sampled polyline. The crossing vertices
    // lie ON the true curve, which sags up to ~5 µm off the polyline chords.
    let dist_to_curve = |p: &Point3| -> f64 {
        let mut best = f64::INFINITY;
        for w in curve_pts.windows(2) {
            let ab = w[1] - w[0];
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - w[0]).dot(ab) / len2).clamp(0.0, 1.0)
            };
            best = best.min((*p - (w[0] + t * ab)).norm());
        }
        best
    };
    const ON_CURVE_TOL: f64 = 6e-3;
    let hits: Vec<usize> = (0..n)
        .filter(|&i| dist_to_curve(&loop_verts[i]) < ON_CURVE_TOL)
        .collect();
    if hits.len() != 2 {
        return None;
    }
    let (i, j) = (hits[0], hits[1]);
    // The two vertices must not be adjacent along the loop (a boundary edge
    // already runs between adjacent ones) and the chord must be a genuine
    // sub-sample cut.
    if j == i + 1 || (i == 0 && j == n - 1) {
        return None;
    }
    let (a, b) = (loop_verts[i], loop_verts[j]);
    let chord = (b - a).norm();
    let sample_step = (curve_pts[1] - curve_pts[0]).norm().max(1e-9);
    if chord < 1e-6 || chord > 2.0 * sample_step {
        return None;
    }
    // Chord midpoint must be strictly inside the face (not along an edge).
    let mid = Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z));
    let mut min_d = f64::INFINITY;
    for k in 0..n {
        let e0 = loop_verts[k];
        let e1 = loop_verts[(k + 1) % n];
        let ab = e1 - e0;
        let len2 = ab.norm_squared();
        let t = if len2 < 1e-18 {
            0.0
        } else {
            ((mid - e0).dot(ab) / len2).clamp(0.0, 1.0)
        };
        min_d = min_d.min((mid - (e0 + t * ab)).norm());
    }
    if min_d < 1e-7 || !crate::trim::point_in_face(brep, face_id, &mid) {
        return None;
    }
    let result = split_face_by_curve(brep, face_id, &IntersectionCurve::Empty, &a, &b);
    if result.sub_faces.len() >= 2 {
        Some(result)
    } else {
        None
    }
}
