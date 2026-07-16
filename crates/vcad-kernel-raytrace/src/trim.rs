//! Trim testing for determining if a UV point lies within a face boundary.
//!
//! BRep faces are bounded by trim loops that define valid regions of the
//! underlying surface. This module tests whether intersection points are
//! within these boundaries.

use vcad_kernel_geom::Surface;
use vcad_kernel_math::Point2;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation};

/// Test if a UV point is inside a face's trim boundaries.
///
/// Returns `true` if the point is inside the outer loop and outside all inner loops (holes).
pub fn point_in_face(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> bool {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    // Get UV coordinates of the outer loop vertices
    let outer_uvs = loop_uv_coords(brep, face.outer_loop, surface.as_ref());

    // A closed surface covering the whole primitive (a full sphere or
    // torus) is bounded only by its seam: the outer loop projects to a
    // zero-area polygon in UV, which would reject every hit. Treat a
    // degenerate outer loop as "untrimmed" — the face spans the entire
    // surface — and still honour inner loops (holes) below.
    let untrimmed = polygon_area(&outer_uvs).abs() < 1e-9;

    // Check if point is inside outer loop
    if !untrimmed && !point_in_polygon(&uv, &outer_uvs) {
        return false;
    }

    // Check if point is inside any hole (should be outside all holes)
    for &inner_loop in &face.inner_loops {
        let inner_uvs = loop_uv_coords(brep, inner_loop, surface.as_ref());
        if point_in_polygon(&uv, &inner_uvs) {
            return false; // Inside a hole
        }
    }

    true
}

/// Signed area of a UV polygon (shoelace). Near-zero means the loop is
/// degenerate in parameter space — e.g. a closed surface's seam-only loop.
fn polygon_area(polygon: &[Point2]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..polygon.len() {
        let a = &polygon[i];
        let b = &polygon[(i + 1) % polygon.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area / 2.0
}

/// Get the UV coordinates of vertices in a loop by projecting 3D positions onto the surface.
fn loop_uv_coords(
    brep: &BRepSolid,
    loop_id: vcad_kernel_topo::LoopId,
    surface: &dyn Surface,
) -> Vec<Point2> {
    let topo = &brep.topology;

    topo.loop_half_edges(loop_id)
        .map(|he_id| {
            let v_id = topo.half_edges[he_id].origin;
            let point = topo.vertices[v_id].point;
            project_to_surface_uv(surface, &point)
        })
        .collect()
}

/// Project a 3D point onto a surface's UV parameter space.
///
/// This is an inverse evaluation that finds (u, v) such that surface.evaluate(u, v) ≈ point.
fn project_to_surface_uv(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    use vcad_kernel_geom::SurfaceKind;

    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                return plane.project(point);
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                return project_to_cylinder(cyl, point);
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                return project_to_sphere(sph, point);
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                return project_to_cone(cone, point);
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                return project_to_torus(torus, point);
            }
        }
        _ => {}
    }

    // Fallback: Newton iteration for general surfaces
    project_newton(surface, point)
}

fn project_to_cylinder(
    cyl: &vcad_kernel_geom::CylinderSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cyl.axis.as_ref();
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - cyl.center;
    let v = to_point.dot(axis);

    let proj = to_point - v * axis;
    let x = proj.dot(ref_dir);
    let y = proj.dot(y_dir);

    let u = y.atan2(x);
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    Point2::new(u, v)
}

fn project_to_sphere(
    sph: &vcad_kernel_geom::SphereSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = sph.axis.as_ref();
    let ref_dir = sph.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = (point - sph.center) / sph.radius;
    let z = to_point.dot(axis).clamp(-1.0, 1.0);
    let v = z.asin();

    let proj = to_point - z * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_cone(
    cone: &vcad_kernel_geom::ConeSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cone.axis.as_ref();
    let ref_dir = cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);
    let cos_a = cone.half_angle.cos();

    let to_point = point - cone.apex;
    let height = to_point.dot(axis);
    let v = height / cos_a;

    let proj = to_point - height * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_torus(
    torus: &vcad_kernel_geom::TorusSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = torus.axis.as_ref();
    let ref_dir = torus.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - torus.center;
    let h = to_point.dot(axis);

    let proj = to_point - h * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir);
        let y = proj.dot(y_dir);
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    let tube_center_dist = proj_len - torus.major_radius;
    let v = h.atan2(tube_center_dist);
    let v = if v < 0.0 { v + 2.0 * PI } else { v };

    Point2::new(u, v)
}

/// Newton iteration to find UV coordinates for a point on a surface.
fn project_newton(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let mut uv = Point2::new((u_min + u_max) / 2.0, (v_min + v_max) / 2.0);

    for _ in 0..20 {
        let p = surface.evaluate(uv);
        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        let residual = p - point;

        // Solve 2x2 system: [du, dv]^T * [delta_u, delta_v]^T = residual
        // Using least squares: (J^T J) * delta = J^T * residual
        let a11 = du.dot(du);
        let a12 = du.dot(dv);
        let a22 = dv.dot(dv);
        let b1 = du.dot(residual);
        let b2 = dv.dot(residual);

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-14 {
            break;
        }

        let delta_u = (a22 * b1 - a12 * b2) / det;
        let delta_v = (a11 * b2 - a12 * b1) / det;

        uv.x -= delta_u;
        uv.y -= delta_v;

        // Clamp to domain
        uv.x = uv.x.clamp(u_min, u_max);
        uv.y = uv.y.clamp(v_min, v_max);

        if delta_u.abs() < 1e-10 && delta_v.abs() < 1e-10 {
            break;
        }
    }

    uv
}

/// Point-in-polygon test using the winding number algorithm.
///
/// Works correctly for both convex and concave polygons.
pub fn point_in_polygon(point: &Point2, polygon: &[Point2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    let mut winding = 0i32;
    let n = polygon.len();

    for i in 0..n {
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];

        if p1.y <= point.y {
            if p2.y > point.y {
                // Upward crossing
                if is_left(&p1, &p2, point) > 0.0 {
                    winding += 1;
                }
            }
        } else if p2.y <= point.y {
            // Downward crossing
            if is_left(&p1, &p2, point) < 0.0 {
                winding -= 1;
            }
        }
    }

    winding != 0
}

/// Compute the signed area of the triangle (p0, p1, p2).
/// Positive if p2 is to the left of the line p0->p1.
#[inline]
fn is_left(p0: &Point2, p1: &Point2, p2: &Point2) -> f64 {
    (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y)
}

/// Extract the UV coordinates of a face's outer loop.
///
/// This returns the UV coordinates of the outer boundary loop vertices,
/// suitable for point-in-polygon testing.
pub fn extract_face_uv_loop(brep: &BRepSolid, face_id: FaceId) -> Vec<Point2> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    loop_uv_coords(brep, face.outer_loop, surface.as_ref())
}

/// Extract UV coordinates for all inner loops (holes) of a face.
///
/// Returns a vector of inner loops, where each inner loop is a vector of UV points.
pub fn extract_face_inner_loops(brep: &BRepSolid, face_id: FaceId) -> Vec<Vec<Point2>> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    face.inner_loops
        .iter()
        .map(|&loop_id| loop_uv_coords(brep, loop_id, surface.as_ref()))
        .collect()
}

/// Get the face normal considering orientation.
pub fn face_normal(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> vcad_kernel_math::Dir3 {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let n = surface.normal(uv);

    match face.orientation {
        Orientation::Forward => n,
        Orientation::Reversed => vcad_kernel_math::Dir3::new_normalize(-n.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_point_in_polygon_square() {
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &square));
        assert!(point_in_polygon(&Point2::new(0.1, 0.1), &square));
        assert!(!point_in_polygon(&Point2::new(1.5, 0.5), &square));
        assert!(!point_in_polygon(&Point2::new(-0.1, 0.5), &square));
    }

    #[test]
    fn test_point_in_polygon_concave() {
        // L-shaped polygon
        let l_shape = vec![
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(2.0, 1.0),
            Point2::new(1.0, 1.0),
            Point2::new(1.0, 2.0),
            Point2::new(0.0, 2.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &l_shape));
        assert!(point_in_polygon(&Point2::new(0.5, 1.5), &l_shape));
        assert!(!point_in_polygon(&Point2::new(1.5, 1.5), &l_shape)); // In the concave notch
    }

    #[test]
    fn test_point_in_cube_face() {
        let cube = make_cube(10.0, 10.0, 10.0);

        // Get the first face and test a point in the middle
        let face_id = cube.topology.faces.iter().next().unwrap().0;

        // The face should be a 10x10 square in some plane
        // UV coordinates depend on the face, but center should be valid
        let center_uv = Point2::new(5.0, 5.0);
        assert!(point_in_face(&cube, face_id, center_uv));
    }

    #[test]
    fn test_project_to_cylinder() {
        use std::f64::consts::PI;
        let cyl = vcad_kernel_geom::CylinderSurface::new(5.0);

        // Point at (5, 0, 3) should project to u=0, v=3
        let uv = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(5.0, 0.0, 3.0));
        assert!(uv.x.abs() < 1e-10);
        assert!((uv.y - 3.0).abs() < 1e-10);

        // Point at (0, 5, 7) should project to u=π/2, v=7
        let uv2 = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(0.0, 5.0, 7.0));
        assert!((uv2.x - PI / 2.0).abs() < 1e-10);
        assert!((uv2.y - 7.0).abs() < 1e-10);
    }
}
