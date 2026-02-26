//! Ray-plane intersection (closed-form).

use super::SurfaceHit;
use crate::Ray;
use vcad_kernel_geom::Plane;

/// Intersect a ray with a plane.
///
/// Returns `Some(hit)` if the ray intersects the plane at a positive t,
/// or `None` if the ray is parallel to the plane or intersects behind the origin.
pub fn intersect_plane(ray: &Ray, plane: &Plane) -> Option<SurfaceHit> {
    let normal = plane.normal_dir.as_ref();
    let denom = ray.direction.as_ref().dot(normal);

    // Ray is parallel to plane
    if denom.abs() < 1e-12 {
        return None;
    }

    let t = (plane.origin - ray.origin).dot(normal) / denom;

    // Intersection is behind ray origin
    if t < 0.0 {
        return None;
    }

    // Compute UV coordinates by projecting the intersection point onto the plane
    let point = ray.at(t);
    let uv = plane.project(&point);

    Some(SurfaceHit { t, uv })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_plane_perpendicular() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = intersect_plane(&ray, &plane);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert!((hit.t - 5.0).abs() < 1e-10);
        assert!(hit.uv.x.abs() < 1e-10);
        assert!(hit.uv.y.abs() < 1e-10);
    }

    #[test]
    fn test_ray_plane_offset() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(3.0, 4.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = intersect_plane(&ray, &plane);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert!((hit.t - 10.0).abs() < 1e-10);
        assert!((hit.uv.x - 3.0).abs() < 1e-10);
        assert!((hit.uv.y - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_ray_plane_parallel() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), Vec3::new(1.0, 0.0, 0.0));
        let hit = intersect_plane(&ray, &plane);
        assert!(hit.is_none());
    }

    #[test]
    fn test_ray_plane_behind() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = intersect_plane(&ray, &plane);
        assert!(hit.is_none());
    }

    #[test]
    fn test_ray_plane_angled() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(0.0, 0.0, 10.0), Vec3::new(1.0, 0.0, -1.0));
        let hit = intersect_plane(&ray, &plane);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        // Ray travels diagonally: for each unit in x, drops 1 unit in z
        // At z=0, we've traveled 10 units in z direction
        let expected_t = 10.0 * 2.0_f64.sqrt(); // sqrt(1^2 + 1^2) * 10
        assert!((hit.t - expected_t).abs() < 1e-10);
    }
}
