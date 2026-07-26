//! Ray-triangle intersection (Möller–Trumbore).
//!
//! Triangles are the one primitive in this crate that is *not* an analytic
//! surface: they back mesh-only solids (frozen topology-optimization
//! results, imported STL/GLB parts) which carry no BRep to trace. They
//! therefore bypass [`intersect_surface`](super::intersect_surface) — there
//! is no `SurfaceKind` for them — and are dispatched directly from the BVH's
//! mesh leaves.

use crate::Ray;
use vcad_kernel_math::Point3;

/// Result of a ray-triangle intersection.
#[derive(Debug, Clone, Copy)]
pub struct TriangleHit {
    /// Parameter along the ray.
    pub t: f64,
    /// Barycentric weight of the second vertex (`v1`).
    pub u: f64,
    /// Barycentric weight of the third vertex (`v2`).
    pub v: f64,
}

impl TriangleHit {
    /// Barycentric weight of the first vertex (`v0`): `1 - u - v`.
    #[inline]
    pub fn w(&self) -> f64 {
        1.0 - self.u - self.v
    }
}

/// Relative epsilon for the determinant test. Scaled by the triangle's edge
/// magnitudes so that the test is size-independent: an absolute epsilon
/// rejects legitimate hits on millimetre-scale triangles and accepts
/// degenerate ones on metre-scale parts.
const DET_EPS: f64 = 1e-12;

/// Intersect a ray with a triangle using the Möller–Trumbore algorithm.
///
/// Double-sided: back-facing triangles hit as well as front-facing ones.
/// Mesh solids have no reliable winding guarantee (a decimated or imported
/// mesh may be inconsistently wound), and the renderer face-forwards the
/// shading normal anyway, so culling here would only punch holes in parts.
///
/// Returns `None` when the ray misses, is parallel to the triangle plane,
/// the triangle is degenerate, or the hit lies behind the ray origin.
pub fn intersect_triangle(ray: &Ray, v0: Point3, v1: Point3, v2: Point3) -> Option<TriangleHit> {
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let d = ray.direction.as_ref();

    let pvec = d.cross(e2);
    let det = e1.dot(pvec);

    // Size-relative degeneracy/parallel guard.
    let scale = e1.norm() * e2.norm();
    if det.abs() <= DET_EPS * scale.max(1.0) {
        return None;
    }

    let inv_det = 1.0 / det;
    let tvec = ray.origin - v0;
    let u = tvec.dot(pvec) * inv_det;
    if !(-1e-12..=1.0 + 1e-12).contains(&u) {
        return None;
    }

    let qvec = tvec.cross(e1);
    let v = d.dot(qvec) * inv_det;
    if v < -1e-12 || u + v > 1.0 + 1e-12 {
        return None;
    }

    let t = e2.dot(qvec) * inv_det;
    if t <= 0.0 {
        return None;
    }

    Some(TriangleHit { t, u, v })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Vec3;

    fn tri() -> (Point3, Point3, Point3) {
        (
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        )
    }

    #[test]
    fn hits_interior_point() {
        let (v0, v1, v2) = tri();
        let ray = Ray::new(Point3::new(0.25, 0.25, -3.0), Vec3::new(0.0, 0.0, 1.0));
        let hit = intersect_triangle(&ray, v0, v1, v2).expect("should hit");
        assert!((hit.t - 3.0).abs() < 1e-12, "t = {}", hit.t);
        assert!((hit.u - 0.25).abs() < 1e-12);
        assert!((hit.v - 0.25).abs() < 1e-12);
        assert!((hit.w() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn misses_outside_the_triangle() {
        let (v0, v1, v2) = tri();
        let ray = Ray::new(Point3::new(0.9, 0.9, -3.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(intersect_triangle(&ray, v0, v1, v2).is_none());
    }

    #[test]
    fn hits_from_the_back_side_too() {
        let (v0, v1, v2) = tri();
        let ray = Ray::new(Point3::new(0.25, 0.25, 3.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = intersect_triangle(&ray, v0, v1, v2).expect("double-sided");
        assert!((hit.t - 3.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_hits_behind_the_origin() {
        let (v0, v1, v2) = tri();
        let ray = Ray::new(Point3::new(0.25, 0.25, 3.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(intersect_triangle(&ray, v0, v1, v2).is_none());
    }

    #[test]
    fn parallel_ray_misses() {
        let (v0, v1, v2) = tri();
        let ray = Ray::new(Point3::new(0.25, 0.25, 1.0), Vec3::new(1.0, 0.0, 0.0));
        assert!(intersect_triangle(&ray, v0, v1, v2).is_none());
    }

    #[test]
    fn degenerate_triangle_misses() {
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let ray = Ray::new(Point3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(intersect_triangle(&ray, v0, v0, v0).is_none());
    }

    #[test]
    fn small_triangles_still_hit() {
        // Size-relative determinant guard: a 1 µm triangle must not be
        // rejected as degenerate.
        let s = 1e-3;
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let v1 = Point3::new(s, 0.0, 0.0);
        let v2 = Point3::new(0.0, s, 0.0);
        let ray = Ray::new(
            Point3::new(s / 4.0, s / 4.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
        );
        assert!(intersect_triangle(&ray, v0, v1, v2).is_some());
    }
}
