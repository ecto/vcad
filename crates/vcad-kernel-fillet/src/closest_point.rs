//! Surface point projection — closest point on a surface to a 3D point.

use std::f64::consts::PI;
use vcad_kernel_geom::{ConeSurface, CylinderSurface, Plane, Surface, SurfaceKind};
use vcad_kernel_math::{Point2, Point3, Vec3};

/// Compute the closest point on a surface to a given 3D point, returning UV parameters.
///
/// For analytic surfaces (Plane, Cylinder, Sphere, Cone), uses geometric solutions.
/// For general surfaces, uses multi-start Newton iteration.
pub fn closest_point_uv(surface: &dyn Surface, point: &Point3, tolerance: f64) -> Option<Point2> {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            let plane = surface.as_any().downcast_ref::<Plane>()?;
            Some(plane.project(point))
        }
        SurfaceKind::Cylinder => {
            let cyl = surface.as_any().downcast_ref::<CylinderSurface>()?;
            let d = *point - cyl.center;
            let v = d.dot(cyl.axis.as_ref());
            let radial = d - v * cyl.axis.as_ref();
            let u = radial.dot(&cyl.y_dir()).atan2(radial.dot(cyl.ref_dir.as_ref()));
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            Some(Point2::new(u, v))
        }
        SurfaceKind::Sphere => {
            let sphere = surface.as_any().downcast_ref::<vcad_kernel_geom::SphereSurface>()?;
            let d = *point - sphere.center;
            let len = d.norm();
            if len < 1e-15 {
                return None;
            }
            let d_norm = d / len;
            let v = d_norm.dot(sphere.axis.as_ref()).asin();
            let cos_v = v.cos();
            if cos_v.abs() < 1e-15 {
                return Some(Point2::new(0.0, v));
            }
            let x_comp = d_norm.dot(sphere.ref_dir.as_ref()) / cos_v;
            let y_comp = d_norm.dot(&sphere.y_dir()) / cos_v;
            let u = y_comp.atan2(x_comp);
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            Some(Point2::new(u, v))
        }
        SurfaceKind::Cone => {
            let cone = surface.as_any().downcast_ref::<ConeSurface>()?;
            let d = *point - cone.apex;
            // v parameter: distance along the cone axis direction
            let v = d.dot(cone.axis.as_ref());
            if v < 1e-15 {
                // At the apex — u is undefined, return 0
                return Some(Point2::new(0.0, 0.0));
            }
            // Radial component perpendicular to axis
            let radial = d - v * cone.axis.as_ref();
            let y_dir: Vec3 = cone.axis.as_ref().cross(cone.ref_dir.as_ref());
            let u = radial.dot(&y_dir).atan2(radial.dot(cone.ref_dir.as_ref()));
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            Some(Point2::new(u, v))
        }
        _ => {
            // Multi-start Newton iteration for general surfaces
            closest_point_uv_multistart(surface, point, tolerance)
        }
    }
}

/// Multi-start Newton iteration to find closest point on a general surface.
///
/// Scans a 4x4 grid over the UV domain, picks the 3 best starting points
/// by 3D distance, and runs Newton from each. Returns the result with
/// smallest final distance.
fn closest_point_uv_multistart(surface: &dyn Surface, point: &Point3, tolerance: f64) -> Option<Point2> {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();

    // Clamp infinite domains to a reasonable range for grid search
    let u_lo = u_min.max(-100.0);
    let u_hi = u_max.min(100.0);
    let v_lo = v_min.max(-100.0);
    let v_hi = v_max.min(100.0);

    let grid_n = 5; // 5x5 = 25 sample points
    let mut candidates: Vec<(f64, f64, f64)> = Vec::with_capacity(grid_n * grid_n);

    for iu in 0..grid_n {
        let u = u_lo + (u_hi - u_lo) * iu as f64 / (grid_n - 1) as f64;
        for iv in 0..grid_n {
            let v = v_lo + (v_hi - v_lo) * iv as f64 / (grid_n - 1) as f64;
            let p = surface.evaluate(Point2::new(u, v));
            let dist = (p - *point).norm();
            candidates.push((dist, u, v));
        }
    }

    // Sort by distance, take best 3
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let num_starts = candidates.len().min(3);

    let mut best_uv: Option<Point2> = None;
    let mut best_dist = f64::INFINITY;

    for &(_, u0, v0) in candidates.iter().take(num_starts) {
        if let Some((uv, dist)) = newton_from(surface, point, u0, v0, tolerance) {
            if dist < best_dist {
                best_dist = dist;
                best_uv = Some(uv);
            }
        }
    }

    best_uv
}

/// Run Newton iteration from a single starting point.
/// Returns the converged UV and the final 3D distance.
fn newton_from(
    surface: &dyn Surface,
    point: &Point3,
    u0: f64,
    v0: f64,
    tolerance: f64,
) -> Option<(Point2, f64)> {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let mut u = u0;
    let mut v = v0;

    for _ in 0..50 {
        let uv = Point2::new(u, v);
        let p = surface.evaluate(uv);
        let diff = p - *point;
        let dist = diff.norm();
        if dist < tolerance {
            return Some((uv, dist));
        }

        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        let a11 = du.dot(&du);
        let a12 = du.dot(&dv);
        let a22 = dv.dot(&dv);
        let b1 = -diff.dot(&du);
        let b2 = -diff.dot(&dv);

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-30 {
            break;
        }

        let delta_u = (a22 * b1 - a12 * b2) / det;
        let delta_v = (a11 * b2 - a12 * b1) / det;

        u = (u + delta_u).clamp(u_min, u_max);
        v = (v + delta_v).clamp(v_min, v_max);
    }

    let uv = Point2::new(u, v);
    let p = surface.evaluate(uv);
    let dist = (p - *point).norm();
    Some((uv, dist))
}
