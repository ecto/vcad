//! Ray-sphere intersection (quadratic equation).

use super::SurfaceHit;
use crate::Ray;
use std::f64::consts::PI;
use vcad_kernel_geom::SphereSurface;
use vcad_kernel_math::Point2;

/// Intersect a ray with a spherical surface.
///
/// Returns up to 2 intersections (entry and exit points), sorted by t.
/// Only intersections with t >= 0 are returned.
pub fn intersect_sphere(ray: &Ray, sphere: &SphereSurface) -> Vec<SurfaceHit> {
    let oc = ray.origin - sphere.center;
    let d = ray.direction.as_ref();

    // Quadratic: |oc + t*d|^2 = r^2
    let a = d.dot(d); // Always 1 for unit direction, but explicit for clarity
    let b = 2.0 * oc.dot(d);
    let c = oc.dot(oc) - sphere.radius * sphere.radius;

    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return Vec::new();
    }

    let sqrt_disc = discriminant.sqrt();
    let t1 = (-b - sqrt_disc) / (2.0 * a);
    let t2 = (-b + sqrt_disc) / (2.0 * a);

    let mut hits = Vec::new();

    for t in [t1, t2] {
        if t < 0.0 {
            continue;
        }

        let point = ray.at(t);
        let uv = compute_sphere_uv(sphere, &point);
        hits.push(SurfaceHit { t, uv });
    }

    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Compute the (u, v) surface parameters for a point on a sphere.
///
/// u = longitude [0, 2π), v = latitude [-π/2, π/2]
fn compute_sphere_uv(sphere: &SphereSurface, point: &vcad_kernel_math::Point3) -> Point2 {
    let axis = sphere.axis.as_ref();
    let ref_dir = sphere.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = (point - sphere.center) / sphere.radius;

    // v = latitude (angle from equator)
    let z = to_point.dot(axis);
    let v = z.clamp(-1.0, 1.0).asin();

    // Project onto equatorial plane for longitude
    let proj = to_point - z * axis;
    let proj_len = proj.norm();

    if proj_len < 1e-12 {
        // At a pole - longitude is undefined, use 0
        return Point2::new(0.0, v);
    }

    let x = proj.dot(ref_dir) / proj_len;
    let y = proj.dot(y_dir) / proj_len;

    // u = longitude
    let u = y.atan2(x);
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    Point2::new(u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_sphere_through_center() {
        let sphere = SphereSurface::new(5.0);
        // Ray from (-10, 0, 0) pointing +x, hitting sphere at x = ±5
        let ray = Ray::new(Point3::new(-10.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_sphere(&ray, &sphere);
        assert_eq!(hits.len(), 2);

        // First hit at x = -5 (t = 5)
        assert!((hits[0].t - 5.0).abs() < 1e-10);
        // Second hit at x = +5 (t = 15)
        assert!((hits[1].t - 15.0).abs() < 1e-10);
    }

    #[test]
    fn test_ray_sphere_tangent() {
        let sphere = SphereSurface::new(5.0);
        // Ray tangent to sphere at (5, 0, 0)
        let ray = Ray::new(Point3::new(5.0, -10.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let hits = intersect_sphere(&ray, &sphere);
        // Tangent ray gives 1 or 2 very close intersections
        assert!(hits.len() <= 2);
        if hits.len() == 2 {
            assert!((hits[0].t - hits[1].t).abs() < 1e-6);
        }
    }

    #[test]
    fn test_ray_sphere_miss() {
        let sphere = SphereSurface::new(5.0);
        let ray = Ray::new(Point3::new(-10.0, 10.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_sphere(&ray, &sphere);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_ray_sphere_from_inside() {
        let sphere = SphereSurface::new(5.0);
        // Ray from inside the sphere
        let ray = Ray::new(Point3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_sphere(&ray, &sphere);
        // Only one hit (exit point) since entry is at t < 0
        assert_eq!(hits.len(), 1);
        assert!((hits[0].t - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_sphere_uv() {
        let sphere = SphereSurface::new(10.0);

        // Point at (10, 0, 0) - equator, u=0
        let uv1 = compute_sphere_uv(&sphere, &Point3::new(10.0, 0.0, 0.0));
        assert!(uv1.x.abs() < 1e-10);
        assert!(uv1.y.abs() < 1e-10);

        // North pole (0, 0, 10) - v = π/2
        let uv2 = compute_sphere_uv(&sphere, &Point3::new(0.0, 0.0, 10.0));
        assert!((uv2.y - PI / 2.0).abs() < 1e-10);

        // South pole (0, 0, -10) - v = -π/2
        let uv3 = compute_sphere_uv(&sphere, &Point3::new(0.0, 0.0, -10.0));
        assert!((uv3.y - (-PI / 2.0)).abs() < 1e-10);

        // Point at (0, 10, 0) - equator, u = π/2
        let uv4 = compute_sphere_uv(&sphere, &Point3::new(0.0, 10.0, 0.0));
        assert!((uv4.x - PI / 2.0).abs() < 1e-10);
        assert!(uv4.y.abs() < 1e-10);
    }
}
