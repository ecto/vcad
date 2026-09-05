//! Ray-BSpline surface intersection (Newton iteration with subdivision fallback).

use super::SurfaceHit;
use crate::Ray;
use vcad_kernel_geom::Surface;
use vcad_kernel_math::Point2;

/// Maximum Newton iterations.
const MAX_ITERATIONS: usize = 30;
/// Convergence tolerance.
const TOLERANCE: f64 = 1e-10;
/// Maximum subdivision depth.
const MAX_SUBDIVISION: usize = 4;

/// Intersect a ray with a B-spline or NURBS surface.
///
/// Uses Newton iteration with subdivision fallback for robustness.
/// This is a general method that works with any surface type via the Surface trait.
pub fn intersect_bspline(ray: &Ray, surface: &dyn Surface) -> Vec<SurfaceHit> {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();

    // Use subdivision to find initial guesses, then refine with Newton
    let mut hits = Vec::new();
    subdivide_and_intersect(ray, surface, u_min, u_max, v_min, v_max, 0, &mut hits);

    // Remove duplicates
    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits.dedup_by(|a, b| (a.t - b.t).abs() < 1e-8);

    hits
}

/// Recursively subdivide the parameter domain and find intersections.
#[allow(clippy::too_many_arguments)]
fn subdivide_and_intersect(
    ray: &Ray,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    depth: usize,
    hits: &mut Vec<SurfaceHit>,
) {
    // Check if the ray might intersect this patch by testing corner bounding box
    let corners = [
        surface.evaluate(Point2::new(u_min, v_min)),
        surface.evaluate(Point2::new(u_max, v_min)),
        surface.evaluate(Point2::new(u_min, v_max)),
        surface.evaluate(Point2::new(u_max, v_max)),
    ];

    // Also sample the center and edge midpoints for better bounding
    let midpoints = [
        surface.evaluate(Point2::new((u_min + u_max) / 2.0, v_min)),
        surface.evaluate(Point2::new((u_min + u_max) / 2.0, v_max)),
        surface.evaluate(Point2::new(u_min, (v_min + v_max) / 2.0)),
        surface.evaluate(Point2::new(u_max, (v_min + v_max) / 2.0)),
        surface.evaluate(Point2::new((u_min + u_max) / 2.0, (v_min + v_max) / 2.0)),
    ];

    // Build conservative AABB
    let mut min_pt = corners[0];
    let mut max_pt = corners[0];

    for p in corners.iter().chain(midpoints.iter()) {
        min_pt.x = min_pt.x.min(p.x);
        min_pt.y = min_pt.y.min(p.y);
        min_pt.z = min_pt.z.min(p.z);
        max_pt.x = max_pt.x.max(p.x);
        max_pt.y = max_pt.y.max(p.y);
        max_pt.z = max_pt.z.max(p.z);
    }

    // Expand AABB slightly to account for surface curvature
    let extent = max_pt - min_pt;
    let expand = 0.1 * extent.norm().max(0.01);
    min_pt.x -= expand;
    min_pt.y -= expand;
    min_pt.z -= expand;
    max_pt.x += expand;
    max_pt.y += expand;
    max_pt.z += expand;

    let aabb = kosm_render::Aabb::new(min_pt, max_pt);
    if ray.intersect_aabb(&aabb).is_none() {
        return; // Ray misses this patch
    }

    // Try Newton iteration from the center of this patch
    let u_mid = (u_min + u_max) / 2.0;
    let v_mid = (v_min + v_max) / 2.0;

    if let Some(hit) = newton_iteration_generic(ray, surface, Point2::new(u_mid, v_mid)) {
        // Verify the hit is within this patch
        if hit.uv.x >= u_min - TOLERANCE
            && hit.uv.x <= u_max + TOLERANCE
            && hit.uv.y >= v_min - TOLERANCE
            && hit.uv.y <= v_max + TOLERANCE
        {
            // Check if this hit is already found
            let is_duplicate = hits.iter().any(|h| (h.t - hit.t).abs() < 1e-8);
            if !is_duplicate {
                hits.push(hit);
            }
            return; // Found intersection in this patch
        }
    }

    // Subdivide if not at max depth
    if depth < MAX_SUBDIVISION {
        let u_half = (u_min + u_max) / 2.0;
        let v_half = (v_min + v_max) / 2.0;

        // Recurse into four subpatches
        subdivide_and_intersect(ray, surface, u_min, u_half, v_min, v_half, depth + 1, hits);
        subdivide_and_intersect(ray, surface, u_half, u_max, v_min, v_half, depth + 1, hits);
        subdivide_and_intersect(ray, surface, u_min, u_half, v_half, v_max, depth + 1, hits);
        subdivide_and_intersect(ray, surface, u_half, u_max, v_half, v_max, depth + 1, hits);
    }
}

/// Newton iteration for generic surfaces.
///
/// Solves: `surface.evaluate(u, v) = ray.origin + t * ray.direction`
fn newton_iteration_generic(ray: &Ray, surface: &dyn Surface, start: Point2) -> Option<SurfaceHit> {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let mut uv = start;

    // Initialize t with a rough estimate
    let p0 = surface.evaluate(uv);
    let d = ray.direction.as_ref();
    let mut t = (p0 - ray.origin).dot(d);

    for _ in 0..MAX_ITERATIONS {
        let p = surface.evaluate(uv);
        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        // Residual: F = P(u, v) - origin - t * direction
        let ray_point = ray.at(t);
        let f = p - ray_point;

        // Check convergence on residual
        if f.norm() < TOLERANCE {
            if t >= 0.0
                && uv.x >= u_min - TOLERANCE
                && uv.x <= u_max + TOLERANCE
                && uv.y >= v_min - TOLERANCE
                && uv.y <= v_max + TOLERANCE
            {
                let final_uv = Point2::new(uv.x.clamp(u_min, u_max), uv.y.clamp(v_min, v_max));
                return Some(SurfaceHit { t, uv: final_uv });
            }
            return None;
        }

        // Jacobian: [du, dv, -d]
        // Solve: J * [delta_u, delta_v, delta_t]^T = -F

        let det = du.x * (dv.y * (-d.z) - dv.z * (-d.y)) - dv.x * (du.y * (-d.z) - du.z * (-d.y))
            + (-d.x) * (du.y * dv.z - du.z * dv.y);

        if det.abs() < 1e-14 {
            return None;
        }

        let rhs = -f;

        let det_u = rhs.x * (dv.y * (-d.z) - dv.z * (-d.y))
            - dv.x * (rhs.y * (-d.z) - rhs.z * (-d.y))
            + (-d.x) * (rhs.y * dv.z - rhs.z * dv.y);

        let det_v = du.x * (rhs.y * (-d.z) - rhs.z * (-d.y))
            - rhs.x * (du.y * (-d.z) - du.z * (-d.y))
            + (-d.x) * (du.y * rhs.z - du.z * rhs.y);

        let det_t = du.x * (dv.y * rhs.z - dv.z * rhs.y) - dv.x * (du.y * rhs.z - du.z * rhs.y)
            + rhs.x * (du.y * dv.z - du.z * dv.y);

        let delta_u = det_u / det;
        let delta_v = det_v / det;
        let delta_t = det_t / det;

        // Update
        uv.x += delta_u;
        uv.y += delta_v;
        t += delta_t;

        // Early termination if way outside domain
        let u_range = (u_max - u_min).max(1.0);
        let v_range = (v_max - v_min).max(1.0);
        if uv.x < u_min - u_range
            || uv.x > u_max + u_range
            || uv.y < v_min - v_range
            || uv.y > v_max + v_range
        {
            return None;
        }
    }

    // Check final solution
    let p = surface.evaluate(uv);
    let ray_point = ray.at(t);
    let f = p - ray_point;

    if f.norm() < TOLERANCE * 10.0
        && t >= 0.0
        && uv.x >= u_min - TOLERANCE
        && uv.x <= u_max + TOLERANCE
        && uv.y >= v_min - TOLERANCE
        && uv.y <= v_max + TOLERANCE
    {
        let final_uv = Point2::new(uv.x.clamp(u_min, u_max), uv.y.clamp(v_min, v_max));
        return Some(SurfaceHit { t, uv: final_uv });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_geom::Plane;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_bspline_on_plane() {
        // Test generic intersector on a plane (which also implements Surface)
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(0.5, 0.5, 5.0), Vec3::new(0.0, 0.0, -1.0));

        let hits = intersect_bspline(&ray, &plane);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].t - 5.0).abs() < 1e-8);
    }

    #[test]
    fn test_newton_convergence() {
        let plane = Plane::xy();
        let ray = Ray::new(Point3::new(3.0, 4.0, 10.0), Vec3::new(0.0, 0.0, -1.0));

        let hit = newton_iteration_generic(&ray, &plane, Point2::new(0.0, 0.0));
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert!((hit.t - 10.0).abs() < 1e-8);
        assert!((hit.uv.x - 3.0).abs() < 1e-8);
        assert!((hit.uv.y - 4.0).abs() < 1e-8);
    }
}
