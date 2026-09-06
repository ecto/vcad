//! Ray-cylinder intersection (quadratic equation).

use super::{SurfaceHit, SurfaceHits};
use crate::Ray;
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_math::Point2;

/// Intersect a ray with an infinite cylindrical surface.
///
/// Returns up to 2 intersections (entry and exit points), sorted by t.
/// Only intersections with t >= 0 are returned.
pub fn intersect_cylinder(ray: &Ray, cylinder: &CylinderSurface) -> SurfaceHits {
    let axis = cylinder.axis.as_ref();
    let d = ray.direction.as_ref();
    let oc = ray.origin - cylinder.center;

    // Project ray direction and origin-center onto the plane perpendicular to axis
    let d_perp = d - d.dot(axis) * axis;
    let oc_perp = oc - oc.dot(axis) * axis;

    // Quadratic coefficients: |P_perp(t)|^2 = r^2
    // where P(t) = origin + t*direction
    // |oc_perp + t*d_perp|^2 = r^2
    let a = d_perp.dot(d_perp);
    let b = 2.0 * oc_perp.dot(d_perp);
    let c = oc_perp.dot(oc_perp) - cylinder.radius * cylinder.radius;

    // Ray is parallel to axis
    if a.abs() < 1e-12 {
        return SurfaceHits::new();
    }

    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return SurfaceHits::new();
    }

    let sqrt_disc = discriminant.sqrt();
    let t1 = (-b - sqrt_disc) / (2.0 * a);
    let t2 = (-b + sqrt_disc) / (2.0 * a);

    let mut hits = SurfaceHits::new();

    for t in [t1, t2] {
        if t < 0.0 {
            continue;
        }

        let point = ray.at(t);
        let uv = compute_cylinder_uv(cylinder, &point);
        hits.push(SurfaceHit { t, uv });
    }

    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Compute the (u, v) surface parameters for a point on a cylinder.
fn compute_cylinder_uv(cylinder: &CylinderSurface, point: &vcad_kernel_math::Point3) -> Point2 {
    let axis = cylinder.axis.as_ref();
    let ref_dir = cylinder.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - cylinder.center;

    // v = height along axis
    let v = to_point.dot(axis);

    // Project onto plane perpendicular to axis to get angle
    let proj = to_point - v * axis;
    let x = proj.dot(ref_dir);
    let y = proj.dot(y_dir);

    // u = angle from ref_dir
    let u = y.atan2(x);
    // Normalize to [0, 2π)
    let u = if u < 0.0 {
        u + 2.0 * std::f64::consts::PI
    } else {
        u
    };

    Point2::new(u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_cylinder_perpendicular() {
        let cyl = CylinderSurface::new(5.0);
        // Ray from (-10, 0, 0) pointing +x, hitting cylinder at x = ±5
        let ray = Ray::new(Point3::new(-10.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_cylinder(&ray, &cyl);
        assert_eq!(hits.len(), 2);

        // First hit at x = -5 (t = 5)
        assert!((hits[0].t - 5.0).abs() < 1e-10);
        // Second hit at x = +5 (t = 15)
        assert!((hits[1].t - 15.0).abs() < 1e-10);
    }

    #[test]
    fn test_ray_cylinder_tangent() {
        let cyl = CylinderSurface::new(5.0);
        // Ray tangent to cylinder at (5, 0, 0), going in +y direction
        let ray = Ray::new(Point3::new(5.0, -10.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let hits = intersect_cylinder(&ray, &cyl);
        // Tangent ray should give exactly 1 intersection (or 2 very close ones due to floating point)
        assert!(hits.len() <= 2);
        if hits.len() == 2 {
            assert!((hits[0].t - hits[1].t).abs() < 1e-6);
        }
    }

    #[test]
    fn test_ray_cylinder_miss() {
        let cyl = CylinderSurface::new(5.0);
        // Ray missing the cylinder
        let ray = Ray::new(Point3::new(-10.0, 10.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_cylinder(&ray, &cyl);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_ray_cylinder_parallel_axis() {
        let cyl = CylinderSurface::new(5.0);
        // Ray parallel to axis, inside cylinder
        let ray = Ray::new(Point3::new(2.0, 0.0, -10.0), Vec3::new(0.0, 0.0, 1.0));
        let hits = intersect_cylinder(&ray, &cyl);
        // Infinite cylinder has no hits for parallel rays
        assert!(hits.is_empty());
    }

    #[test]
    fn test_cylinder_uv() {
        let cyl = CylinderSurface::new(5.0);

        // Point at (5, 0, 3) should have u=0, v=3
        let uv1 = compute_cylinder_uv(&cyl, &Point3::new(5.0, 0.0, 3.0));
        assert!(uv1.x.abs() < 1e-10); // u = 0
        assert!((uv1.y - 3.0).abs() < 1e-10); // v = 3

        // Point at (0, 5, 7) should have u=π/2, v=7
        let uv2 = compute_cylinder_uv(&cyl, &Point3::new(0.0, 5.0, 7.0));
        assert!((uv2.x - PI / 2.0).abs() < 1e-10);
        assert!((uv2.y - 7.0).abs() < 1e-10);

        // Point at (-5, 0, -2) should have u=π, v=-2
        let uv3 = compute_cylinder_uv(&cyl, &Point3::new(-5.0, 0.0, -2.0));
        assert!((uv3.x - PI).abs() < 1e-10);
        assert!((uv3.y - (-2.0)).abs() < 1e-10);
    }
}
