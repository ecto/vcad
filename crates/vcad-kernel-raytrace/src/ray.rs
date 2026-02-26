//! Ray representation and basic ray-geometry tests.

use vcad_kernel_booleans::bbox::Aabb3;
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};
use vcad_kernel_topo::FaceId;

/// A ray in 3D space defined by origin and direction.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    /// Origin point of the ray.
    pub origin: Point3,
    /// Unit direction of the ray.
    pub direction: Dir3,
    /// Precomputed reciprocal of direction components for fast AABB tests.
    inv_direction: Vec3,
    /// Sign of direction components (0 if positive, 1 if negative).
    sign: [usize; 3],
}

impl Ray {
    /// Create a new ray from origin and direction.
    ///
    /// The direction will be normalized.
    pub fn new(origin: Point3, direction: Vec3) -> Self {
        let dir = Dir3::new_normalize(direction);
        let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let sign = [
            if inv.x < 0.0 { 1 } else { 0 },
            if inv.y < 0.0 { 1 } else { 0 },
            if inv.z < 0.0 { 1 } else { 0 },
        ];
        Self {
            origin,
            direction: dir,
            inv_direction: inv,
            sign,
        }
    }

    /// Evaluate the ray at parameter `t`: `origin + t * direction`.
    #[inline]
    pub fn at(&self, t: f64) -> Point3 {
        self.origin + t * self.direction.as_ref()
    }

    /// Test ray-AABB intersection using the slab method.
    ///
    /// Returns `Some((t_min, t_max))` if the ray intersects the box,
    /// where `t_min` and `t_max` are the entry and exit parameters.
    /// Returns `None` if no intersection.
    ///
    /// Handles infinite values correctly for axis-aligned rays.
    #[inline]
    pub fn intersect_aabb(&self, aabb: &Aabb3) -> Option<(f64, f64)> {
        let bounds = [aabb.min, aabb.max];

        let tx1 = (bounds[self.sign[0]].x - self.origin.x) * self.inv_direction.x;
        let tx2 = (bounds[1 - self.sign[0]].x - self.origin.x) * self.inv_direction.x;

        let mut t_min = tx1;
        let mut t_max = tx2;

        let ty1 = (bounds[self.sign[1]].y - self.origin.y) * self.inv_direction.y;
        let ty2 = (bounds[1 - self.sign[1]].y - self.origin.y) * self.inv_direction.y;

        t_min = t_min.max(ty1);
        t_max = t_max.min(ty2);

        let tz1 = (bounds[self.sign[2]].z - self.origin.z) * self.inv_direction.z;
        let tz2 = (bounds[1 - self.sign[2]].z - self.origin.z) * self.inv_direction.z;

        t_min = t_min.max(tz1);
        t_max = t_max.min(tz2);

        if t_max >= t_min && t_max >= 0.0 {
            Some((t_min.max(0.0), t_max))
        } else {
            None
        }
    }
}

/// Result of a ray-surface intersection.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    /// Parameter along the ray where intersection occurs.
    pub t: f64,
    /// 3D intersection point.
    pub point: Point3,
    /// Surface normal at the intersection (pointing outward).
    pub normal: Dir3,
    /// Surface parameter coordinates (u, v) at intersection.
    pub uv: Point2,
    /// Face ID that was hit.
    pub face_id: FaceId,
}

impl RayHit {
    /// Create a new ray hit.
    pub fn new(t: f64, point: Point3, normal: Dir3, uv: Point2, face_id: FaceId) -> Self {
        Self {
            t,
            point,
            normal,
            uv,
            face_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ray_at() {
        let ray = Ray::new(Point3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let p = ray.at(5.0);
        assert!((p.x - 5.0).abs() < 1e-12);
        assert!(p.y.abs() < 1e-12);
        assert!(p.z.abs() < 1e-12);
    }

    #[test]
    fn test_ray_aabb_hit() {
        let ray = Ray::new(Point3::new(-5.0, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0));
        let aabb = Aabb3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        let result = ray.intersect_aabb(&aabb);
        assert!(result.is_some());
        let (t_min, t_max) = result.unwrap();
        assert!((t_min - 5.0).abs() < 1e-10);
        assert!((t_max - 6.0).abs() < 1e-10);
    }

    #[test]
    fn test_ray_aabb_miss() {
        let ray = Ray::new(Point3::new(-5.0, 5.0, 5.0), Vec3::new(1.0, 0.0, 0.0));
        let aabb = Aabb3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        let result = ray.intersect_aabb(&aabb);
        assert!(result.is_none());
    }

    #[test]
    fn test_ray_inside_aabb() {
        // Ray origin inside the box
        let ray = Ray::new(Point3::new(0.5, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0));
        let aabb = Aabb3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        let result = ray.intersect_aabb(&aabb);
        assert!(result.is_some());
        let (t_min, t_max) = result.unwrap();
        assert!(t_min >= 0.0);
        assert!((t_max - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_ray_aabb_diagonal() {
        let ray = Ray::new(Point3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        let aabb = Aabb3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        let result = ray.intersect_aabb(&aabb);
        assert!(result.is_some());
    }

    #[test]
    fn test_ray_aabb_behind() {
        // Ray pointing away from box
        let ray = Ray::new(Point3::new(-5.0, 0.5, 0.5), Vec3::new(-1.0, 0.0, 0.0));
        let aabb = Aabb3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        let result = ray.intersect_aabb(&aabb);
        assert!(result.is_none());
    }
}
