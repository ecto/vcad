//! Trim intersection curves to face domains.
//!
//! After SSI produces an intersection curve between two surfaces,
//! we need to clip it to the actual bounded region of each face.
//! This module tests whether points lie inside a face's trim loops
//! and finds the parameter ranges where the curve enters/exits the face.
//!
//! # Important Implementation Note: Line Trimming Range
//!
//! When trimming a line to a face, we must compute the parameter range (t_min, t_max)
//! based on where the line actually intersects the face's AABB, NOT based on the
//! face's size divided by direction length.
//!
//! ## The Bug That Was Fixed
//!
//! SSI (Surface-Surface Intersection) computes a line between two planes. The line's
//! origin is set to the intersection of the planes' origins, which can be FAR from
//! the actual faces being intersected.
//!
//! Example: Plate at Y=6, Hole face at Z=36
//! - SSI line origin: (0, 6, 36) - at X=0
//! - Hole face X range: [34, 46]
//! - Direction: (-1, 0, 0)
//!
//! Old (buggy) approach:
//! - Face extent ≈ 17.7, direction length = 1.0
//! - t_range = 17.7 * 2 ≈ 35.5
//! - Sampled t from -35.5 to +35.5
//! - Points sampled: X from 35.5 to -35.5
//! - MISSED the face entirely (X=[34,46] requires t≈-34 to -46)
//!
//! Fixed approach:
//! - Use ray-AABB intersection to find t values where line enters/exits face bounds
//! - For X: t = (34 - 0) / (-1) = -34, t = (46 - 0) / (-1) = -46
//! - Sample t from -46 to -34 (with padding)
//! - Correctly covers the face
//!
//! This bug caused hole wall faces (Z=24, Z=36) to not be split at Y=0 and Y=6,
//! resulting in the entire Z face being classified as Outside instead of having
//! a middle strip (Y=0 to Y=6) classified as Inside.

use vcad_kernel_geom::Surface;
use vcad_kernel_math::{Point2, Point3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::FaceId;

use crate::bbox;
use crate::ssi::IntersectionCurve;

/// A trimmed segment of an intersection curve, expressed as a parameter range.
#[derive(Debug, Clone)]
pub struct TrimmedSegment {
    /// Start parameter on the intersection curve.
    pub t_start: f64,
    /// End parameter on the intersection curve.
    pub t_end: f64,
}

/// Test if a 2D point is inside a closed polygon using exact predicates.
///
/// Uses Shewchuk's exact orient2d predicate for robust crossing detection.
/// `polygon` is a sequence of vertices forming a closed loop (last connects to first).
/// Returns true if the point is inside or on the boundary of the polygon.
pub fn point_in_polygon(point: &Point2, polygon: &[Point2]) -> bool {
    use vcad_kernel_math::predicates::{orient2d, point_on_segment_2d, Sign};

    let n = polygon.len();
    if n < 3 {
        return false;
    }

    let mut crossings = 0i32;

    for i in 0..n {
        let j = (i + 1) % n;
        let a = &polygon[i];
        let b = &polygon[j];

        // Check if point is exactly on this edge (using exact predicate)
        if point_on_segment_2d(point, a, b) {
            return true; // On boundary = inside
        }

        // Crossing number algorithm using orient2d:
        // Cast a ray from point in +X direction and count crossings
        //
        // Edge crosses ray if:
        // 1. One endpoint is strictly above the ray (y > point.y) and
        //    one is at or below (y <= point.y)
        // 2. The edge crosses to the right of the point (determined by orient2d)

        let y_a = a.y;
        let y_b = b.y;

        // Check if the edge straddles the horizontal line y = point.y
        let a_below = y_a <= point.y;
        let b_below = y_b <= point.y;

        if a_below != b_below {
            // Edge straddles the ray - check if crossing is to the right
            // orient2d(a, b, point):
            //   Positive: point is to the left of line a->b
            //   Negative: point is to the right of line a->b
            //   Zero: point is on the line (handled above)
            let orientation = orient2d(a, b, point);

            // If a is below and b is above (upward crossing):
            //   count if point is to the left of edge (orientation > 0)
            // If a is above and b is below (downward crossing):
            //   count if point is to the right of edge (orientation < 0)
            if a_below {
                // Upward crossing
                if orientation == Sign::Positive {
                    crossings += 1;
                }
            } else {
                // Downward crossing
                if orientation == Sign::Negative {
                    crossings -= 1;
                }
            }
        }
    }

    // Inside if net crossings is non-zero (winding number != 0)
    crossings != 0
}

/// Test if a 3D point on a surface lies inside a face's boundary.
///
/// Projects the point into the face's (u,v) parameter space and tests
/// against the face's trim loops.
pub fn point_in_face(brep: &BRepSolid, face_id: FaceId, point_3d: &Point3) -> bool {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    // Get the outer loop vertices in 3D
    let outer_verts_3d: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
        .collect();

    // Special case: circular face (e.g., cylinder cap with single circular edge)
    // The loop has only 1 vertex (the seam point), so check using 3D circle geometry
    if outer_verts_3d.len() == 1 {
        // This is likely a circular cap face
        // Get the face's 3D curve (should be a circle)
        let half_edges: Vec<_> = topo.loop_half_edges(face.outer_loop).collect();
        if !half_edges.is_empty() {
            // Try to find the associated 3D curve
            // For now, use a simpler approach: assume it's a disk centered at the surface origin
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                // Project point onto the plane
                let d = point_3d - plane.origin;
                let dist_along_normal = d.dot(plane.normal_dir.as_ref());

                // If point is not on the plane (within tolerance), it's outside
                if dist_along_normal.abs() > 1e-4 {
                    return false;
                }

                // Get the circle radius from the vertex position
                let seam_vert = outer_verts_3d[0];
                let center_to_seam = seam_vert - plane.origin;
                let radius = center_to_seam.norm();

                // Check if point is within the outer circle
                let center_to_point = point_3d - plane.origin;
                let dist_from_center = (center_to_point - dist_along_normal * plane.normal_dir.into_inner()).norm();

                if dist_from_center > radius + 1e-6 {
                    return false;
                }

                // Also check inner loops — point must not be inside any hole.
                // Each inner loop is also a circle (from boolean operations).
                for &inner_loop in &face.inner_loops {
                    let inner_verts: Vec<Point3> = topo
                        .loop_half_edges(inner_loop)
                        .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                        .collect();
                    if inner_verts.len() == 1 {
                        // Degenerate circular inner loop
                        let iv = inner_verts[0];
                        let inner_r = (iv - plane.origin).norm();
                        if dist_from_center < inner_r - 1e-6 {
                            return false; // strictly inside a hole
                        }
                    } else if inner_verts.len() >= 3 {
                        // Polygonal inner loop — project to 2D and check containment
                        let normal = plane.normal_dir.into_inner();
                        let x_axis = if radius > 1e-12 {
                            center_to_seam.normalize()
                        } else {
                            *plane.x_dir.as_ref()
                        };
                        let y_axis = normal.cross(&x_axis);
                        let pt_2d = Point2::new(
                            center_to_point.dot(&x_axis),
                            center_to_point.dot(&y_axis),
                        );
                        let poly_2d: Vec<Point2> = inner_verts
                            .iter()
                            .map(|v| {
                                let d = v - plane.origin;
                                Point2::new(d.dot(&x_axis), d.dot(&y_axis))
                            })
                            .collect();
                        // Ray-casting point-in-polygon
                        if point_in_polygon(&pt_2d, &poly_2d) {
                            return false; // inside a hole
                        }
                    }
                }

                return true;
            }
        }
        // If we can't determine the circle, conservatively return true
        return true;
    }

    // Special case: sphere faces with degenerate pole loops (2 vertices: north + south poles).
    // The outer loop has only 2 unique positions (poles), which don't form a polygon.
    // Check if the point is on the sphere surface using 3D distance to center.
    if surface.surface_type() == vcad_kernel_geom::SurfaceKind::Sphere && outer_verts_3d.len() <= 2
    {
        if let Some(sph) = surface
            .as_any()
            .downcast_ref::<vcad_kernel_geom::SphereSurface>()
        {
            let dist = (*point_3d - sph.center).norm();
            // Point must be on the sphere surface (within tolerance)
            if (dist - sph.radius).abs() > 1e-4 {
                return false;
            }

            // Check inner loops — point must not be inside any hole.
            // Inner loops on sphere faces are circles from boolean intersections.
            for &inner_loop in &face.inner_loops {
                let inner_verts: Vec<Point3> = topo
                    .loop_half_edges(inner_loop)
                    .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                    .collect();
                if inner_verts.len() >= 3 {
                    // Polygonal inner loop (circle approximated with segments).
                    // Compute the hole's centroid and max radius as exclusion zone.
                    let n = inner_verts.len() as f64;
                    let hole_center = Point3::new(
                        inner_verts.iter().map(|v| v.x).sum::<f64>() / n,
                        inner_verts.iter().map(|v| v.y).sum::<f64>() / n,
                        inner_verts.iter().map(|v| v.z).sum::<f64>() / n,
                    );
                    let hole_radius = inner_verts
                        .iter()
                        .map(|v| (*v - hole_center).norm())
                        .fold(0.0f64, f64::max);
                    // Use angular distance on the sphere for containment:
                    // compute the angle subtended from center between point and hole center
                    let to_point = (*point_3d - sph.center).normalize();
                    let to_hole = (hole_center - sph.center).normalize();
                    let angle = to_point.dot(&to_hole).clamp(-1.0, 1.0).acos();
                    let hole_angle = (hole_radius / sph.radius).clamp(0.0, 1.0).asin();
                    if angle < hole_angle - 1e-6 {
                        return false; // inside a hole
                    }
                }
            }

            return true;
        }
    }

    // Project the test point to UV
    let test_uv = project_point_to_uv(surface.as_ref(), point_3d);

    // Special handling for cylindrical surfaces
    // A standard cylinder lateral face has only 2 unique vertices (seam top/bottom)
    // which don't form a proper polygon. Instead, check V range and U wrap.
    if surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder {
        // Get unique vertices by position
        let mut unique_verts: Vec<Point3> = Vec::new();
        for v in &outer_verts_3d {
            let is_dup = unique_verts.iter().any(|u| (*u - *v).norm() < 1e-6);
            if !is_dup {
                unique_verts.push(*v);
            }
        }

        // If we have only 2 unique vertices (standard full cylinder lateral face)
        // then the face spans the full U range and we only need to check V
        if unique_verts.len() == 2 {
            // Get V range from the two seam vertices
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                let v0 = (unique_verts[0] - cyl.center).dot(cyl.axis.as_ref());
                let v1 = (unique_verts[1] - cyl.center).dot(cyl.axis.as_ref());
                let v_min = v0.min(v1);
                let v_max = v0.max(v1);
                let test_v = test_uv.y; // V coordinate

                // For a full cylinder, just check V is in range
                let in_v_range = test_v >= v_min - 1e-6 && test_v <= v_max + 1e-6;

                // Check inner loops (holes) on cylindrical faces
                if in_v_range && !face.inner_loops.is_empty() {
                    let ref_dir = cyl.ref_dir.as_ref();
                    let y_dir = cyl.axis.as_ref().cross(ref_dir);
                    for &inner_loop_id in &face.inner_loops {
                        let inner_verts: Vec<Point3> = topo
                            .loop_half_edges(inner_loop_id)
                            .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                            .collect();
                        if inner_verts.is_empty() {
                            continue;
                        }
                        // Project inner loop vertices to UV
                        let inner_uv: Vec<Point2> = inner_verts
                            .iter()
                            .map(|v| {
                                let d = v - cyl.center;
                                let u = d.dot(&y_dir).atan2(d.dot(ref_dir));
                                let u = if u < 0.0 {
                                    u + 2.0 * std::f64::consts::PI
                                } else {
                                    u
                                };
                                let v_coord = d.dot(cyl.axis.as_ref());
                                Point2::new(u, v_coord)
                            })
                            .collect();
                        if inner_uv.len() >= 3 {
                            // Unwrap U coordinates to avoid seam issues
                            let (inner_unwrapped, seam_cut) =
                                unwrap_cylindrical_loop(&inner_uv);
                            let test_unwrapped =
                                unwrap_cylindrical_uv(&test_uv, seam_cut);
                            if point_in_polygon(&test_unwrapped, &inner_unwrapped) {
                                return false; // inside a hole
                            }
                        }
                    }
                }

                return in_v_range;
            }
        }

        // Fall through to general polygon test for partial cylinder faces
        let outer_uv = project_points_to_uv(surface.as_ref(), &outer_verts_3d);
        let (outer_uv, seam_cut) = unwrap_cylindrical_loop(&outer_uv);
        let inner_uv: Vec<Vec<Point2>> = face
            .inner_loops
            .iter()
            .map(|&inner_loop_id| {
                let inner_verts: Vec<Point3> = topo
                    .loop_half_edges(inner_loop_id)
                    .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                    .collect();
                let inner_uv = project_points_to_uv(surface.as_ref(), &inner_verts);
                unwrap_cylindrical_loop_with_cut(&inner_uv, seam_cut)
            })
            .collect();
        let test_uv = unwrap_cylindrical_uv(&test_uv, seam_cut);

        if !point_in_polygon(&test_uv, &outer_uv) {
            return false;
        }
        for inner_uv in &inner_uv {
            if point_in_polygon(&test_uv, inner_uv) {
                return false;
            }
        }
        return true;
    }

    // Special handling for conical surfaces (similar to cylinder but with v-dependent radius)
    if surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cone {
        let mut unique_verts: Vec<Point3> = Vec::new();
        for v in &outer_verts_3d {
            let is_dup = unique_verts.iter().any(|u| (*u - *v).norm() < 1e-6);
            if !is_dup {
                unique_verts.push(*v);
            }
        }

        // Full cone lateral face with ≤2 unique vertices: check V range only
        if unique_verts.len() <= 2 {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                let ca = cone.half_angle.cos();
                let v_values: Vec<f64> = unique_verts
                    .iter()
                    .map(|p| {
                        let d = *p - cone.apex;
                        d.dot(cone.axis.as_ref()) / ca
                    })
                    .collect();
                let v_min = v_values.iter().cloned().fold(f64::INFINITY, f64::min);
                let v_max = v_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let test_v = test_uv.y;
                return test_v >= v_min - 1e-6 && test_v <= v_max + 1e-6;
            }
        }

        // Fall through to UV polygon test for partial cone faces
        let outer_uv = project_points_to_uv(surface.as_ref(), &outer_verts_3d);
        let (outer_uv, seam_cut) = unwrap_cylindrical_loop(&outer_uv);
        let inner_uv: Vec<Vec<Point2>> = face
            .inner_loops
            .iter()
            .map(|&inner_loop_id| {
                let inner_verts: Vec<Point3> = topo
                    .loop_half_edges(inner_loop_id)
                    .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                    .collect();
                let inner_uv = project_points_to_uv(surface.as_ref(), &inner_verts);
                unwrap_cylindrical_loop_with_cut(&inner_uv, seam_cut)
            })
            .collect();
        let test_uv = unwrap_cylindrical_uv(&test_uv, seam_cut);

        if !point_in_polygon(&test_uv, &outer_uv) {
            return false;
        }
        for inner_uv in &inner_uv {
            if point_in_polygon(&test_uv, inner_uv) {
                return false;
            }
        }
        return true;
    }

    // Special handling for toroidal surfaces (both u and v wrap at 2pi)
    if surface.surface_type() == vcad_kernel_geom::SurfaceKind::Torus {
        let outer_uv = project_points_to_uv(surface.as_ref(), &outer_verts_3d);
        // Unwrap U dimension (same as cylinder)
        let (outer_uv, u_seam_cut) = unwrap_cylindrical_loop(&outer_uv);

        // Unwrap V dimension: find best seam cut for V coordinates
        let v_values: Vec<f64> = outer_uv.iter().map(|p| p.y).collect();
        let v_seam_cut = find_seam_cut_for_values(&v_values);
        let outer_uv: Vec<Point2> = outer_uv
            .iter()
            .map(|p| {
                let mut v = p.y;
                if v < v_seam_cut {
                    v += 2.0 * std::f64::consts::PI;
                }
                Point2::new(p.x, v)
            })
            .collect();

        let inner_uv: Vec<Vec<Point2>> = face
            .inner_loops
            .iter()
            .map(|&inner_loop_id| {
                let inner_verts: Vec<Point3> = topo
                    .loop_half_edges(inner_loop_id)
                    .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                    .collect();
                let inner_uv = project_points_to_uv(surface.as_ref(), &inner_verts);
                let inner_uv = unwrap_cylindrical_loop_with_cut(&inner_uv, u_seam_cut);
                inner_uv
                    .iter()
                    .map(|p| {
                        let mut v = p.y;
                        if v < v_seam_cut {
                            v += 2.0 * std::f64::consts::PI;
                        }
                        Point2::new(p.x, v)
                    })
                    .collect()
            })
            .collect();

        // Unwrap test point with same cuts
        let mut test_uv = unwrap_cylindrical_uv(&test_uv, u_seam_cut);
        if test_uv.y < v_seam_cut {
            test_uv = Point2::new(test_uv.x, test_uv.y + 2.0 * std::f64::consts::PI);
        }

        if !point_in_polygon(&test_uv, &outer_uv) {
            return false;
        }
        for inner in &inner_uv {
            if point_in_polygon(&test_uv, inner) {
                return false;
            }
        }
        return true;
    }

    // Non-cylindrical, non-toroidal surfaces: standard UV polygon test
    let outer_uv = project_points_to_uv(surface.as_ref(), &outer_verts_3d);
    let inner_uv: Vec<Vec<Point2>> = face
        .inner_loops
        .iter()
        .map(|&inner_loop_id| {
            let inner_verts: Vec<Point3> = topo
                .loop_half_edges(inner_loop_id)
                .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                .collect();
            project_points_to_uv(surface.as_ref(), &inner_verts)
        })
        .collect();

    // Test if inside outer loop
    if !point_in_polygon(&test_uv, &outer_uv) {
        return false;
    }

    // Test if outside all inner loops (holes)
    for inner_uv in &inner_uv {
        if point_in_polygon(&test_uv, inner_uv) {
            return false; // inside a hole
        }
    }

    true
}

/// Project a 3D point onto a surface's UV parameter space.
///
/// Uses a simple closest-point heuristic for analytic surfaces.
pub fn project_point_to_uv(surface: &dyn Surface, point: &Point3) -> Point2 {
    use vcad_kernel_geom::SurfaceKind;

    match surface.surface_type() {
        SurfaceKind::Plane => {
            // For planes, we can get the exact UV from the Plane struct
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                plane.project(point)
            } else {
                approx_project_to_uv(surface, point)
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                // Project: u = atan2(dot(p-center, y_dir), dot(p-center, ref_dir))
                //          v = dot(p-center, axis)
                let d = point - cyl.center;
                let ref_dir = cyl.ref_dir.as_ref();
                let y_dir = cyl.axis.as_ref().cross(ref_dir);
                let u = d.dot(&y_dir).atan2(d.dot(ref_dir));
                let u = if u < 0.0 {
                    u + 2.0 * std::f64::consts::PI
                } else {
                    u
                };
                let v = d.dot(cyl.axis.as_ref());
                Point2::new(u, v)
            } else {
                approx_project_to_uv(surface, point)
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                let d = (point - sph.center).normalize();
                let ref_dir = sph.ref_dir.as_ref();
                let y_dir = sph.axis.as_ref().cross(ref_dir);
                let v = d.dot(sph.axis.as_ref()).asin(); // latitude
                let cos_v = v.cos();
                let u = if cos_v.abs() < 1e-12 {
                    0.0 // at pole
                } else {
                    let dx = d.dot(ref_dir) / cos_v;
                    let dy = d.dot(&y_dir) / cos_v;
                    let u = dy.atan2(dx);
                    if u < 0.0 {
                        u + 2.0 * std::f64::consts::PI
                    } else {
                        u
                    }
                };
                Point2::new(u, v)
            } else {
                approx_project_to_uv(surface, point)
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                let d = point - cone.apex;
                let ref_dir = cone.ref_dir.as_ref();
                let y_dir = cone.y_dir();
                let ca = cone.half_angle.cos();

                let d_axis = d.dot(cone.axis.as_ref());
                let d_perp = d - d_axis * cone.axis.into_inner();
                let d_perp_len = d_perp.norm();

                let u = if d_perp_len < 1e-12 {
                    0.0
                } else {
                    let d_perp_norm = d_perp / d_perp_len;
                    let u = d_perp_norm.dot(&y_dir).atan2(d_perp_norm.dot(ref_dir));
                    if u < 0.0 {
                        u + 2.0 * std::f64::consts::PI
                    } else {
                        u
                    }
                };

                let v = d_axis / ca;
                Point2::new(u, v)
            } else {
                approx_project_to_uv(surface, point)
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                // Torus parameterization:
                // P(u,v) = center + (R + r·cos(v))·(cos(u)·ref_dir + sin(u)·y_dir) + r·sin(v)·axis
                //
                // To find u: project point onto the center plane and compute angle
                // To find v: find angle around the tube cross-section

                let d = point - torus.center;
                let ref_dir = torus.ref_dir.as_ref();
                let y_dir = torus.axis.as_ref().cross(ref_dir);

                // Project d onto the center plane (remove axis component)
                let d_axis = d.dot(torus.axis.as_ref());
                let d_plane = d - d_axis * torus.axis.into_inner();
                let d_plane_len = d_plane.norm();

                // u = angle in the center plane
                let u = if d_plane_len < 1e-12 {
                    0.0 // degenerate case: point on axis
                } else {
                    let d_plane_norm = d_plane / d_plane_len;
                    let u = d_plane_norm.dot(&y_dir).atan2(d_plane_norm.dot(ref_dir));
                    if u < 0.0 {
                        u + 2.0 * std::f64::consts::PI
                    } else {
                        u
                    }
                };

                // v = angle around the tube cross-section
                // The tube center at this u is at distance R from center
                let tube_center_dist = torus.major_radius;
                let radial_dist = d_plane_len - tube_center_dist; // distance from tube center in radial direction
                let axial_dist = d_axis; // distance from tube center in axis direction

                let v = axial_dist.atan2(radial_dist);
                // Wrap v to [0, 2pi) to match torus domain convention
                let v = if v < 0.0 {
                    v + 2.0 * std::f64::consts::PI
                } else {
                    v
                };

                Point2::new(u, v)
            } else {
                approx_project_to_uv(surface, point)
            }
        }
        _ => approx_project_to_uv(surface, point),
    }
}

/// Approximate UV projection by searching over the parameter domain.
fn approx_project_to_uv(surface: &dyn Surface, point: &Point3) -> Point2 {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    // Clamp domain for search
    let u_min = u_min.max(-100.0);
    let u_max = u_max.min(100.0);
    let v_min = v_min.max(-100.0);
    let v_max = v_max.min(100.0);

    let n = 20;
    let mut best_uv = Point2::new(0.5 * (u_min + u_max), 0.5 * (v_min + v_max));
    let mut best_dist = f64::INFINITY;

    for i in 0..=n {
        for j in 0..=n {
            let u = u_min + (u_max - u_min) * i as f64 / n as f64;
            let v = v_min + (v_max - v_min) * j as f64 / n as f64;
            let uv = Point2::new(u, v);
            let p = surface.evaluate(uv);
            let dist = (p - point).norm_squared();
            if dist < best_dist {
                best_dist = dist;
                best_uv = uv;
            }
        }
    }

    best_uv
}

/// Project multiple 3D points to UV space on a surface.
fn project_points_to_uv(surface: &dyn Surface, points: &[Point3]) -> Vec<Point2> {
    points
        .iter()
        .map(|p| project_point_to_uv(surface, p))
        .collect()
}

/// Trim an intersection curve to the domain of a face.
///
/// Samples the curve at regular intervals and checks which samples
/// lie inside the face. Returns parameter ranges where the curve
/// is inside the face.
pub fn trim_curve_to_face(
    curve: &IntersectionCurve,
    face_id: FaceId,
    brep: &BRepSolid,
    n_samples: usize,
) -> Vec<TrimmedSegment> {
    let aabb = bbox::face_aabb(brep, face_id);
    let diag = (aabb.max - aabb.min).norm();
    let merge_tol = (diag * 1e-6).max(1e-6);
    match curve {
        IntersectionCurve::Empty => Vec::new(),
        IntersectionCurve::Point(p) => {
            if point_in_face(brep, face_id, p) {
                vec![TrimmedSegment {
                    t_start: 0.0,
                    t_end: 0.0,
                }]
            } else {
                Vec::new()
            }
        }
        IntersectionCurve::Line(line) => {
            // CRITICAL: Use ray-AABB intersection to find the parameter range.
            //
            // The line's origin comes from SSI (Surface-Surface Intersection) and may be
            // FAR from the face. We cannot simply use face_extent/direction_length as the
            // range - that was a bug that caused hole walls to not be split correctly.
            //
            // Example of the bug:
            //   Line origin: (0, 6, 36), direction: (-1, 0, 0)
            //   Face X range: [34, 46]
            //   Old code: t_range = 17.7, sampled t from -17.7 to +17.7
            //   But we need t = -34 to -46 to reach the face!
            //
            // See module-level docs for full explanation.
            let dir_len = line.direction.norm();
            if dir_len < 1e-15 {
                return Vec::new();
            }

            // Ray-slab intersection: for each axis, find t where line enters/exits AABB
            let mut t_min = f64::NEG_INFINITY;
            let mut t_max = f64::INFINITY;

            // X axis
            if line.direction.x.abs() > 1e-15 {
                let t1 = (aabb.min.x - line.origin.x) / line.direction.x;
                let t2 = (aabb.max.x - line.origin.x) / line.direction.x;
                let (t_enter, t_exit) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
                t_min = t_min.max(t_enter);
                t_max = t_max.min(t_exit);
            } else if line.origin.x < aabb.min.x - 1e-9 || line.origin.x > aabb.max.x + 1e-9 {
                return Vec::new(); // Line parallel to X but outside X range
            }

            // Y axis
            if line.direction.y.abs() > 1e-15 {
                let t1 = (aabb.min.y - line.origin.y) / line.direction.y;
                let t2 = (aabb.max.y - line.origin.y) / line.direction.y;
                let (t_enter, t_exit) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
                t_min = t_min.max(t_enter);
                t_max = t_max.min(t_exit);
            } else if line.origin.y < aabb.min.y - 1e-9 || line.origin.y > aabb.max.y + 1e-9 {
                return Vec::new(); // Line parallel to Y but outside Y range
            }

            // Z axis
            if line.direction.z.abs() > 1e-15 {
                let t1 = (aabb.min.z - line.origin.z) / line.direction.z;
                let t2 = (aabb.max.z - line.origin.z) / line.direction.z;
                let (t_enter, t_exit) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
                t_min = t_min.max(t_enter);
                t_max = t_max.min(t_exit);
            } else if line.origin.z < aabb.min.z - 1e-9 || line.origin.z > aabb.max.z + 1e-9 {
                return Vec::new(); // Line parallel to Z but outside Z range
            }

            // If t_min > t_max, line doesn't intersect AABB
            if t_min > t_max {
                return Vec::new();
            }

            // Expand slightly to ensure we catch boundaries
            let padding = (t_max - t_min).max(1.0) * 0.1;
            t_min -= padding;
            t_max += padding;

            let segments = sample_and_trim(
                |t| line.origin + t * line.direction,
                t_min,
                t_max,
                n_samples,
                face_id,
                brep,
            );
            merge_segments(|t| line.origin + t * line.direction, &segments, merge_tol)
        }
        IntersectionCurve::Circle(circle) => {
            use std::f64::consts::PI;
            let segments = sample_and_trim(
                |t| {
                    let (sin_t, cos_t) = t.sin_cos();
                    circle.center
                        + circle.radius
                            * (cos_t * circle.x_dir.into_inner()
                                + sin_t * circle.y_dir.into_inner())
                },
                0.0,
                2.0 * PI,
                n_samples,
                face_id,
                brep,
            );
            merge_segments(
                |t| {
                    let (sin_t, cos_t) = t.sin_cos();
                    circle.center
                        + circle.radius
                            * (cos_t * circle.x_dir.into_inner()
                                + sin_t * circle.y_dir.into_inner())
                },
                &segments,
                merge_tol,
            )
        }
        IntersectionCurve::Sampled(points) => {
            // For sampled curves, test each point directly
            let mut segments = Vec::new();
            let n = points.len();
            if n == 0 {
                return segments;
            }

            let mut in_segment = false;
            let mut seg_start = 0.0;

            for (i, p) in points.iter().enumerate() {
                let t = i as f64 / (n - 1).max(1) as f64;
                let inside = point_in_face(brep, face_id, p);

                if inside && !in_segment {
                    seg_start = t;
                    in_segment = true;
                } else if !inside && in_segment {
                    segments.push(TrimmedSegment {
                        t_start: seg_start,
                        t_end: t,
                    });
                    in_segment = false;
                }
            }

            if in_segment {
                segments.push(TrimmedSegment {
                    t_start: seg_start,
                    t_end: 1.0,
                });
            }

            merge_segments(|t| sample_curve(points, t), &segments, merge_tol)
        }
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            trim_curve_to_face(&IntersectionCurve::Line(line1.clone()), face_id, brep, n_samples)
        }
    }
}

/// Binary search to refine the exact parameter where inside/outside status changes.
fn refine_crossing(
    eval: &impl Fn(f64) -> Point3,
    t_inside: f64,
    t_outside: f64,
    face_id: FaceId,
    brep: &BRepSolid,
    iterations: usize,
) -> f64 {
    let mut t_in = t_inside;
    let mut t_out = t_outside;
    for _ in 0..iterations {
        let t_mid = 0.5 * (t_in + t_out);
        let p = eval(t_mid);
        if point_in_face(brep, face_id, &p) {
            t_in = t_mid;
        } else {
            t_out = t_mid;
        }
    }
    // Return the inside boundary (last point that's inside)
    t_in
}

/// Generic helper: sample a curve, test each sample point against a face,
/// and return parameter ranges where the curve is inside the face.
/// Uses binary search to refine boundary crossings for accuracy.
fn sample_and_trim(
    eval: impl Fn(f64) -> Point3,
    t_min: f64,
    t_max: f64,
    n_samples: usize,
    face_id: FaceId,
    brep: &BRepSolid,
) -> Vec<TrimmedSegment> {
    let mut segments = Vec::new();
    let n = n_samples.max(2);

    // First pass: find sample transitions
    let mut samples: Vec<(f64, bool)> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = t_min + (t_max - t_min) * i as f64 / n as f64;
        let p = eval(t);
        let inside = point_in_face(brep, face_id, &p);
        samples.push((t, inside));
    }

    // Find transitions and refine them
    let mut in_segment = false;
    let mut seg_start = t_min;

    for i in 1..samples.len() {
        let (t_prev, inside_prev) = samples[i - 1];
        let (t_curr, inside_curr) = samples[i];

        if inside_curr && !inside_prev {
            // Transition from outside to inside - refine to find exact entry
            seg_start = refine_crossing(&eval, t_curr, t_prev, face_id, brep, 20);
            in_segment = true;
        } else if !inside_curr && inside_prev {
            // Transition from inside to outside - refine to find exact exit
            let seg_end = refine_crossing(&eval, t_prev, t_curr, face_id, brep, 20);
            if in_segment {
                segments.push(TrimmedSegment {
                    t_start: seg_start,
                    t_end: seg_end,
                });
                in_segment = false;
            }
        }
    }

    // Handle segment that extends to end
    if in_segment {
        // Check if we should refine the end or use t_max
        let (t_last, inside_last) = samples[samples.len() - 1];
        if inside_last {
            segments.push(TrimmedSegment {
                t_start: seg_start,
                t_end: t_last,
            });
        }
    }

    // Handle case where first sample is inside (segment starts from beginning)
    if !segments.is_empty() {
        return segments;
    }

    // If no transitions found but some samples are inside, the entire sampled range might be inside
    if samples.iter().any(|(_, inside)| *inside) && samples.iter().all(|(_, inside)| *inside) {
        return vec![TrimmedSegment {
            t_start: t_min,
            t_end: t_max,
        }];
    }

    segments
}

fn merge_segments(
    eval: impl Fn(f64) -> Point3,
    segments: &[TrimmedSegment],
    tolerance: f64,
) -> Vec<TrimmedSegment> {
    if segments.len() < 2 {
        return segments.to_vec();
    }
    let mut sorted = segments.to_vec();
    sorted.sort_by(|a, b| a.t_start.partial_cmp(&b.t_start).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged = Vec::new();
    let mut current = sorted[0].clone();
    for next in sorted.into_iter().skip(1) {
        let end = eval(current.t_end);
        let start = eval(next.t_start);
        if (start - end).norm() <= tolerance {
            current.t_end = next.t_end.max(current.t_end);
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

fn sample_curve(points: &[Point3], t: f64) -> Point3 {
    if points.is_empty() {
        return Point3::origin();
    }
    if points.len() == 1 {
        return points[0];
    }
    let t = t.clamp(0.0, 1.0);
    let idx = (t * (points.len() - 1) as f64).round() as usize;
    points[idx.min(points.len() - 1)]
}

fn unwrap_cylindrical_loop(loop_uv: &[Point2]) -> (Vec<Point2>, f64) {
    let seam_cut = find_seam_cut(loop_uv);
    (
        unwrap_cylindrical_loop_with_cut(loop_uv, seam_cut),
        seam_cut,
    )
}

fn unwrap_cylindrical_loop_with_cut(loop_uv: &[Point2], seam_cut: f64) -> Vec<Point2> {
    loop_uv
        .iter()
        .map(|p| unwrap_cylindrical_uv(p, seam_cut))
        .collect()
}

fn unwrap_cylindrical_uv(uv: &Point2, seam_cut: f64) -> Point2 {
    let mut u = uv.x;
    if u < seam_cut {
        u += 2.0 * std::f64::consts::PI;
    }
    Point2::new(u, uv.y)
}

fn find_seam_cut(loop_uv: &[Point2]) -> f64 {
    if loop_uv.is_empty() {
        return 0.0;
    }
    let u_values: Vec<f64> = loop_uv.iter().map(|p| p.x).collect();
    find_seam_cut_for_values(&u_values)
}

/// Find the optimal seam cut angle for a set of periodic values in [0, 2pi).
///
/// Returns the cut angle such that all values, when unwrapped past the cut,
/// span the smallest angular range. Used for both U and V on periodic surfaces.
fn find_seam_cut_for_values(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut best_gap = -1.0;
    let mut cut = sorted[0];
    for w in sorted.windows(2) {
        let gap = w[1] - w[0];
        if gap > best_gap {
            best_gap = gap;
            cut = w[1];
        }
    }
    let wrap_gap = sorted[0] + 2.0 * std::f64::consts::PI - sorted[sorted.len() - 1];
    if wrap_gap > best_gap {
        cut = sorted[0];
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_in_polygon_square() {
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ];

        assert!(point_in_polygon(&Point2::new(5.0, 5.0), &square));
        assert!(point_in_polygon(&Point2::new(10.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2::new(15.0, 5.0), &square));
        assert!(!point_in_polygon(&Point2::new(-1.0, 5.0), &square));
    }

    #[test]
    fn test_point_in_polygon_triangle() {
        let triangle = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(5.0, 10.0),
        ];

        assert!(point_in_polygon(&Point2::new(5.0, 3.0), &triangle));
        assert!(!point_in_polygon(&Point2::new(0.0, 10.0), &triangle));
    }

    #[test]
    fn test_point_in_face_cube() {
        use vcad_kernel_primitives::make_cube;

        let brep = make_cube(10.0, 10.0, 10.0);
        // Pick the bottom face (z=0 plane)
        // Find a face whose vertices all have z=0
        let bottom_face = brep.topology.faces.iter().find(|(fid, _)| {
            let verts: Vec<Point3> = brep
                .topology
                .loop_half_edges(brep.topology.faces[*fid].outer_loop)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            verts.iter().all(|v| v.z.abs() < 1e-10)
        });

        if let Some((fid, _)) = bottom_face {
            // Point on the face interior
            let inside = point_in_face(&brep, fid, &Point3::new(5.0, 5.0, 0.0));
            assert!(inside);

            // Point outside the face
            let outside = point_in_face(&brep, fid, &Point3::new(15.0, 5.0, 0.0));
            assert!(!outside);
        }
    }

    #[test]
    fn test_unwrap_cylindrical_loop() {
        let loop_uv = vec![
            Point2::new(6.2, 0.0),
            Point2::new(0.1, 0.0),
            Point2::new(0.2, 0.0),
        ];
        let (unwrapped, seam_cut) = unwrap_cylindrical_loop(&loop_uv);
        assert!(unwrapped.iter().all(|p| p.x >= seam_cut));
        let min_u = unwrapped.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_u = unwrapped
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(max_u - min_u < 2.0 * std::f64::consts::PI);
    }

    #[test]
    fn test_trim_empty_curve() {
        use vcad_kernel_primitives::make_cube;
        let brep = make_cube(10.0, 10.0, 10.0);
        let face_id = brep.topology.faces.iter().next().unwrap().0;

        let segments = trim_curve_to_face(&IntersectionCurve::Empty, face_id, &brep, 100);
        assert!(segments.is_empty());
    }
}
