//! Ray-bilinear surface intersection (Newton iteration).

use super::SurfaceHit;
use crate::Ray;
use vcad_kernel_geom::{BilinearSurface, Surface};
use vcad_kernel_math::Point2;

/// Maximum Newton iterations.
const MAX_ITERATIONS: usize = 20;
/// Convergence tolerance.
const TOLERANCE: f64 = 1e-10;

/// Intersect a ray with a bilinear surface patch.
///
/// Uses Newton iteration to find intersections. Returns all valid intersections
/// with t >= 0 and (u, v) within [0, 1].
pub fn intersect_bilinear(ray: &Ray, surface: &BilinearSurface) -> Vec<SurfaceHit> {
    // For planar bilinear patches, use the simpler plane intersection
    if surface.is_planar() {
        return intersect_planar_quad(ray, surface);
    }

    // Try multiple starting points to find all intersections
    let starts = [
        Point2::new(0.25, 0.25),
        Point2::new(0.75, 0.25),
        Point2::new(0.25, 0.75),
        Point2::new(0.75, 0.75),
        Point2::new(0.5, 0.5),
    ];

    let mut hits = Vec::new();

    for start in &starts {
        if let Some(hit) = newton_iteration(ray, surface, *start) {
            // Check if this hit is already found
            let is_duplicate = hits.iter().any(|h: &SurfaceHit| (h.t - hit.t).abs() < 1e-8);
            if !is_duplicate {
                hits.push(hit);
            }
        }
    }

    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Newton iteration to find ray-surface intersection.
///
/// Solves: `surface.evaluate(u, v) = ray.origin + t * ray.direction`
fn newton_iteration(ray: &Ray, surface: &BilinearSurface, start: Point2) -> Option<SurfaceHit> {
    let mut uv = start;

    // Initialize t with a rough estimate by projecting start point onto ray
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
                && uv.x >= -TOLERANCE
                && uv.x <= 1.0 + TOLERANCE
                && uv.y >= -TOLERANCE
                && uv.y <= 1.0 + TOLERANCE
            {
                let final_uv = Point2::new(uv.x.clamp(0.0, 1.0), uv.y.clamp(0.0, 1.0));
                return Some(SurfaceHit { t, uv: final_uv });
            }
            return None;
        }

        // Jacobian: [du, dv, -d]
        // Solve: J * [delta_u, delta_v, delta_t]^T = -F

        // Using Cramer's rule for 3x3 system
        let det = du.x * (dv.y * (-d.z) - dv.z * (-d.y)) - dv.x * (du.y * (-d.z) - du.z * (-d.y))
            + (-d.x) * (du.y * dv.z - du.z * dv.y);

        if det.abs() < 1e-14 {
            return None; // Singular Jacobian
        }

        // Solve for corrections (RHS = -F)
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

        // Early termination if we've gone way outside the domain
        if uv.x < -1.0 || uv.x > 2.0 || uv.y < -1.0 || uv.y > 2.0 {
            return None;
        }
    }

    // Check final solution
    let p = surface.evaluate(uv);
    let ray_point = ray.at(t);
    let f = p - ray_point;

    if f.norm() < TOLERANCE * 10.0
        && t >= 0.0
        && uv.x >= -TOLERANCE
        && uv.x <= 1.0 + TOLERANCE
        && uv.y >= -TOLERANCE
        && uv.y <= 1.0 + TOLERANCE
    {
        let final_uv = Point2::new(uv.x.clamp(0.0, 1.0), uv.y.clamp(0.0, 1.0));
        return Some(SurfaceHit { t, uv: final_uv });
    }

    None // Did not converge
}

/// Intersect ray with a planar quad (degenerate bilinear surface).
fn intersect_planar_quad(ray: &Ray, surface: &BilinearSurface) -> Vec<SurfaceHit> {
    // Compute plane from first three corners
    let e1 = surface.p10 - surface.p00;
    let e2 = surface.p01 - surface.p00;
    let normal = e1.cross(e2);
    let n_len = normal.norm();

    if n_len < 1e-12 {
        return Vec::new(); // Degenerate
    }

    let n = normal / n_len;
    let d = ray.direction.as_ref();
    let denom = d.dot(n);

    if denom.abs() < 1e-12 {
        return Vec::new(); // Parallel to plane
    }

    let t = (surface.p00 - ray.origin).dot(n) / denom;
    if t < 0.0 {
        return Vec::new();
    }

    let point = ray.at(t);

    // Check if point is inside the quad using barycentric-like test
    // Project onto the dominant axis plane for 2D test
    let abs_n = vcad_kernel_math::Vec3::new(n.x.abs(), n.y.abs(), n.z.abs());
    let (i0, i1) = if abs_n.x >= abs_n.y && abs_n.x >= abs_n.z {
        (1, 2) // Project onto YZ
    } else if abs_n.y >= abs_n.z {
        (0, 2) // Project onto XZ
    } else {
        (0, 1) // Project onto XY
    };

    let get = |p: &vcad_kernel_math::Point3| -> (f64, f64) {
        let coords = [p.x, p.y, p.z];
        (coords[i0], coords[i1])
    };

    let (px, py) = get(&point);
    let (p00x, p00y) = get(&surface.p00);
    let (p10x, p10y) = get(&surface.p10);
    let (p11x, p11y) = get(&surface.p11);
    let (p01x, p01y) = get(&surface.p01);

    // Check if point is inside quad using cross product signs
    let quad = [(p00x, p00y), (p10x, p10y), (p11x, p11y), (p01x, p01y)];
    if !point_in_quad_2d(px, py, &quad) {
        return Vec::new();
    }

    // Compute UV by solving the bilinear inverse
    let uv = inverse_bilinear_2d(px, py, &quad);

    vec![SurfaceHit { t, uv }]
}

/// Check if a 2D point is inside a convex quad.
fn point_in_quad_2d(px: f64, py: f64, quad: &[(f64, f64); 4]) -> bool {
    let mut sign = 0i32;

    for i in 0..4 {
        let (x0, y0) = quad[i];
        let (x1, y1) = quad[(i + 1) % 4];
        let cross = (x1 - x0) * (py - y0) - (y1 - y0) * (px - x0);

        if cross.abs() > 1e-12 {
            let s = if cross > 0.0 { 1 } else { -1 };
            if sign == 0 {
                sign = s;
            } else if sign != s {
                return false;
            }
        }
    }

    true
}

/// Compute UV coordinates for a point inside a bilinear quad (2D projection).
fn inverse_bilinear_2d(px: f64, py: f64, quad: &[(f64, f64); 4]) -> Point2 {
    // Use iterative refinement
    let (p00x, p00y) = quad[0];
    let (p10x, p10y) = quad[1];
    let (p11x, p11y) = quad[2];
    let (p01x, p01y) = quad[3];

    // Start at center
    let mut u = 0.5;
    let mut v = 0.5;

    for _ in 0..10 {
        // Evaluate bilinear at current (u, v)
        let u1 = 1.0 - u;
        let v1 = 1.0 - v;

        let qx = u1 * v1 * p00x + u * v1 * p10x + u * v * p11x + u1 * v * p01x;
        let qy = u1 * v1 * p00y + u * v1 * p10y + u * v * p11y + u1 * v * p01y;

        let residual_x = px - qx;
        let residual_y = py - qy;

        if residual_x.abs() < 1e-10 && residual_y.abs() < 1e-10 {
            break;
        }

        // Partial derivatives
        let du_x = -v1 * p00x + v1 * p10x + v * p11x - v * p01x;
        let du_y = -v1 * p00y + v1 * p10y + v * p11y - v * p01y;
        let dv_x = -u1 * p00x - u * p10x + u * p11x + u1 * p01x;
        let dv_y = -u1 * p00y - u * p10y + u * p11y + u1 * p01y;

        // Solve 2x2 system
        let det = du_x * dv_y - du_y * dv_x;
        if det.abs() < 1e-14 {
            break;
        }

        let delta_u = (residual_x * dv_y - residual_y * dv_x) / det;
        let delta_v = (du_x * residual_y - du_y * residual_x) / det;

        u += delta_u;
        v += delta_v;

        u = u.clamp(0.0, 1.0);
        v = v.clamp(0.0, 1.0);
    }

    Point2::new(u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_planar_quad() {
        // Planar quad in XY plane
        let surface = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        );

        let ray = Ray::new(Point3::new(0.5, 0.5, 5.0), Vec3::new(0.0, 0.0, -1.0));

        let hits = intersect_bilinear(&ray, &surface);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].t - 5.0).abs() < 1e-10);
        assert!((hits[0].uv.x - 0.5).abs() < 1e-10);
        assert!((hits[0].uv.y - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_ray_planar_quad_miss() {
        let surface = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        );

        // Ray misses the quad
        let ray = Ray::new(Point3::new(5.0, 5.0, 5.0), Vec3::new(0.0, 0.0, -1.0));

        let hits = intersect_bilinear(&ray, &surface);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_ray_warped_bilinear() {
        // Non-planar bilinear (saddle shape)
        let surface = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 1.0), // Lifted corner
        );

        let ray = Ray::new(Point3::new(0.5, 0.5, 5.0), Vec3::new(0.0, 0.0, -1.0));

        let hits = intersect_bilinear(&ray, &surface);
        assert_eq!(hits.len(), 1);
        // At u=0.5, v=0.5: z = 0.5 * 0.5 * 1.0 = 0.25
        assert!((ray.at(hits[0].t).z - 0.25).abs() < 1e-8);
    }

    #[test]
    fn test_point_in_quad_2d() {
        let quad = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

        assert!(point_in_quad_2d(0.5, 0.5, &quad));
        assert!(point_in_quad_2d(0.1, 0.1, &quad));
        assert!(!point_in_quad_2d(1.5, 0.5, &quad));
        assert!(!point_in_quad_2d(-0.1, 0.5, &quad));
    }
}
