//! Ray-cone intersection (quadratic equation).

use super::SurfaceHit;
use crate::Ray;
use std::f64::consts::PI;
use vcad_kernel_geom::ConeSurface;
use vcad_kernel_math::Point2;

/// Intersect a ray with a conical surface.
///
/// Returns up to 2 intersections, sorted by t.
/// Only intersections with t >= 0 and v >= 0 (on the cone, not the nappes) are returned.
pub fn intersect_cone(ray: &Ray, cone: &ConeSurface) -> Vec<SurfaceHit> {
    let axis = cone.axis.as_ref();
    let d = ray.direction.as_ref();
    let co = ray.origin - cone.apex;

    let cos_a = cone.half_angle.cos();
    let sin_a = cone.half_angle.sin();
    let cos2 = cos_a * cos_a;
    let _sin2 = sin_a * sin_a;

    // The cone equation: (P - apex) · axis = |P - apex| * cos(half_angle)
    // Squared: ((P - apex) · axis)^2 = |P - apex|^2 * cos^2(half_angle)
    // Which gives: (d·axis)^2 - cos^2 = sin^2 * (d - (d·axis)*axis)^2
    // After substitution P = origin + t*d:

    let d_dot_a = d.dot(axis);
    let co_dot_a = co.dot(axis);

    // Quadratic coefficients
    let a = d_dot_a * d_dot_a - cos2;
    let b = 2.0 * (d_dot_a * co_dot_a - cos2 * d.dot(&co));
    let c = co_dot_a * co_dot_a - cos2 * co.dot(&co);

    let mut hits = Vec::new();

    if a.abs() < 1e-12 {
        // Linear case (ray direction makes exactly the cone half-angle with axis)
        if b.abs() > 1e-12 {
            let t = -c / b;
            if t >= 0.0 {
                let point = ray.at(t);
                // Check if on the correct nappe (v >= 0)
                let v = (point - cone.apex).dot(axis) / cos_a;
                if v >= 0.0 {
                    let uv = compute_cone_uv(cone, &point);
                    hits.push(SurfaceHit { t, uv });
                }
            }
        }
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant >= 0.0 {
            let sqrt_disc = discriminant.sqrt();
            let t1 = (-b - sqrt_disc) / (2.0 * a);
            let t2 = (-b + sqrt_disc) / (2.0 * a);

            for t in [t1, t2] {
                if t < 0.0 {
                    continue;
                }

                let point = ray.at(t);

                // Check which nappe: v >= 0 means the correct nappe (away from apex along axis)
                let to_point = point - cone.apex;
                let height_along_axis = to_point.dot(axis);

                // v parameter: distance from apex along the generator
                // The cone surface is: apex + v * (cos(a)*axis + sin(a)*radial_dir)
                // So v = height_along_axis / cos(a)
                let v = height_along_axis / cos_a;

                if v >= 0.0 {
                    let uv = compute_cone_uv(cone, &point);
                    hits.push(SurfaceHit { t, uv });
                }
            }
        }
    }

    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Compute the (u, v) surface parameters for a point on a cone.
fn compute_cone_uv(cone: &ConeSurface, point: &vcad_kernel_math::Point3) -> Point2 {
    let axis = cone.axis.as_ref();
    let ref_dir = cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - cone.apex;
    let cos_a = cone.half_angle.cos();

    // v = distance from apex along generator
    let height = to_point.dot(axis);
    let v = height / cos_a;

    // Project onto plane perpendicular to axis to get angle
    let proj = to_point - height * axis;
    let proj_len = proj.norm();

    if proj_len < 1e-12 {
        // On the axis (at apex) - angle is undefined
        return Point2::new(0.0, v);
    }

    let x = proj.dot(ref_dir) / proj_len;
    let y = proj.dot(&y_dir) / proj_len;

    let u = y.atan2(x);
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    Point2::new(u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_cone_through_axis() {
        // Cone with 45° half-angle, apex at origin, axis along +Z
        let cone = ConeSurface::new(PI / 4.0);

        // Ray along -X axis at z=5, should hit cone at (±5, 0, 5)
        let ray = Ray::new(Point3::new(-20.0, 0.0, 5.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_cone(&ray, &cone);

        // Should have 2 hits
        assert_eq!(hits.len(), 2);

        // First hit at x = -5 (t = 15)
        assert!((hits[0].t - 15.0).abs() < 1e-10);
        // Second hit at x = +5 (t = 25)
        assert!((hits[1].t - 25.0).abs() < 1e-10);
    }

    #[test]
    fn test_ray_cone_miss() {
        let cone = ConeSurface::new(PI / 6.0); // 30° half-angle

        // Ray pointing away from the cone (in -z direction from positive z)
        let ray = Ray::new(
            Point3::new(0.0, 0.0, 10.0),
            Vec3::new(1.0, 0.0, 0.0), // Perpendicular to axis, not hitting
        );
        let hits = intersect_cone(&ray, &cone);

        // Ray is perpendicular to axis and at z=10, the cone radius is tan(30°)*10 ≈ 5.77
        // Ray at y=0 should hit at x = ±5.77... so this actually hits.
        // Let's use a ray that starts outside the cone's reach
        let ray2 = Ray::new(
            Point3::new(0.0, 20.0, 10.0), // Far from axis
            Vec3::new(1.0, 0.0, 0.0),
        );
        let hits2 = intersect_cone(&ray2, &cone);
        assert!(hits2.is_empty(), "Expected no hits, got {:?}", hits2);
    }

    #[test]
    fn test_ray_cone_wrong_nappe() {
        let cone = ConeSurface::new(PI / 4.0);

        // Ray hitting the opposite nappe (z < 0)
        let ray = Ray::new(Point3::new(-20.0, 0.0, -5.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_cone(&ray, &cone);

        // Opposite nappe should be filtered out
        assert!(hits.is_empty());
    }

    #[test]
    fn test_cone_uv() {
        let cone = ConeSurface::new(PI / 4.0);

        // Point at (5, 0, 5) - u=0, v should be 5*sqrt(2) (distance from apex along generator)
        let uv1 = compute_cone_uv(&cone, &Point3::new(5.0, 0.0, 5.0));
        assert!(uv1.x.abs() < 1e-10);
        let expected_v = 5.0 * 2.0_f64.sqrt();
        assert!((uv1.y - expected_v).abs() < 1e-10);

        // Point at (0, 5, 5) - u=π/2
        let uv2 = compute_cone_uv(&cone, &Point3::new(0.0, 5.0, 5.0));
        assert!((uv2.x - PI / 2.0).abs() < 1e-10);
    }
}
