//! Ray-torus intersection (quartic equation).
//!
//! Uses Ferrari's method to solve the quartic polynomial analytically.

use super::SurfaceHit;
use crate::Ray;
use std::f64::consts::PI;
use vcad_kernel_geom::TorusSurface;
use vcad_kernel_math::Point2;

/// Intersect a ray with a toroidal surface.
///
/// Returns up to 4 intersections, sorted by t.
/// Only intersections with t >= 0 are returned.
pub fn intersect_torus(ray: &Ray, torus: &TorusSurface) -> Vec<SurfaceHit> {
    let r = torus.major_radius;
    let r2 = r * r;
    let a = torus.minor_radius;
    let a2 = a * a;

    let axis = torus.axis.as_ref();
    let d = ray.direction.as_ref();
    let o = ray.origin - torus.center;

    // Project ray origin and direction into torus coordinate system
    let od = o.dot(d);
    let oo = o.dot(o);
    let dd = d.dot(d); // Should be 1 for unit direction

    // Height along axis
    let oa = o.dot(axis);
    let da = d.dot(axis);

    // Coefficients for the quartic equation
    // The torus is defined by: (sqrt(x^2 + y^2) - R)^2 + z^2 = r^2
    // After substituting the ray equation and expanding, we get a quartic in t.

    let sum_r2_a2 = r2 + a2;
    let k = oo - sum_r2_a2;

    // Quartic: c4*t^4 + c3*t^3 + c2*t^2 + c1*t + c0 = 0
    let c4 = dd * dd;
    let c3 = 4.0 * dd * od;
    let c2 = 2.0 * dd * k + 4.0 * od * od + 4.0 * r2 * da * da;
    let c1 = 4.0 * k * od + 8.0 * r2 * oa * da;
    let c0 = k * k - 4.0 * r2 * (a2 - oa * oa);

    // Solve the quartic
    let roots = solve_quartic(c4, c3, c2, c1, c0);

    let mut hits: Vec<SurfaceHit> = roots
        .into_iter()
        .filter(|&t| t >= 0.0)
        .map(|t| {
            let point = ray.at(t);
            let uv = compute_torus_uv(torus, &point);
            SurfaceHit { t, uv }
        })
        .collect();

    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Compute the (u, v) surface parameters for a point on a torus.
fn compute_torus_uv(torus: &TorusSurface, point: &vcad_kernel_math::Point3) -> Point2 {
    let axis = torus.axis.as_ref();
    let ref_dir = torus.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - torus.center;

    // Height above the equatorial plane
    let h = to_point.dot(axis);

    // Project onto equatorial plane
    let proj = to_point - h * axis;
    let proj_len = proj.norm();

    // u = toroidal angle (around the main axis)
    let x = proj.dot(ref_dir);
    let y = proj.dot(y_dir);
    let u = if proj_len > 1e-12 { y.atan2(x) } else { 0.0 };
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    // v = poloidal angle (around the tube)
    // Distance from tube center
    let tube_center_dist = proj_len - torus.major_radius;
    let v = h.atan2(tube_center_dist);
    let v = if v < 0.0 { v + 2.0 * PI } else { v };

    Point2::new(u, v)
}

/// Solve a quartic equation: a*x^4 + b*x^3 + c*x^2 + d*x + e = 0
///
/// Uses Ferrari's method via a resolvent cubic.
fn solve_quartic(a: f64, b: f64, c: f64, d: f64, e: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        // Degenerate to cubic
        return solve_cubic(b, c, d, e);
    }

    // Normalize: x^4 + px^3 + qx^2 + rx + s = 0
    let p = b / a;
    let q = c / a;
    let r = d / a;
    let s = e / a;

    // Depressed quartic via substitution x = y - p/4
    // y^4 + py^2 + qy + r = 0 (new coefficients)
    let p2 = p * p;
    let p3 = p2 * p;
    let p4 = p2 * p2;

    let a2 = q - 3.0 * p2 / 8.0;
    let a1 = r - p * q / 2.0 + p3 / 8.0;
    let a0 = s - p * r / 4.0 + p2 * q / 16.0 - 3.0 * p4 / 256.0;

    // Resolvent cubic: u^3 + (a2/2)*u^2 + ((a2^2 - 4*a0)/16)*u - a1^2/64 = 0
    // Or use the standard form: 8u^3 + 8*a2*u^2 + (2*a2^2 - 8*a0)*u - a1^2 = 0
    let cubic_roots = solve_cubic(8.0, 8.0 * a2, 2.0 * a2 * a2 - 8.0 * a0, -a1 * a1);

    // Find a positive root of the resolvent cubic
    let u = cubic_roots.into_iter().find(|&u| u > 1e-12).unwrap_or(0.0);

    let sqrt_2u = (2.0 * u).max(0.0).sqrt();

    let mut roots = Vec::new();

    if sqrt_2u.abs() > 1e-12 {
        // Two quadratics
        let alpha = a2 + 2.0 * u;
        let beta = a1 / sqrt_2u;

        // y^2 + sqrt(2u)*y + (alpha + beta)/2 = 0
        let disc1 = sqrt_2u * sqrt_2u - 2.0 * (alpha + beta);
        if disc1 >= 0.0 {
            let sqrt_disc1 = disc1.sqrt();
            roots.push((-sqrt_2u + sqrt_disc1) / 2.0 - p / 4.0);
            roots.push((-sqrt_2u - sqrt_disc1) / 2.0 - p / 4.0);
        }

        // y^2 - sqrt(2u)*y + (alpha - beta)/2 = 0
        let disc2 = sqrt_2u * sqrt_2u - 2.0 * (alpha - beta);
        if disc2 >= 0.0 {
            let sqrt_disc2 = disc2.sqrt();
            roots.push((sqrt_2u + sqrt_disc2) / 2.0 - p / 4.0);
            roots.push((sqrt_2u - sqrt_disc2) / 2.0 - p / 4.0);
        }
    } else {
        // u ≈ 0, special case: y^4 + a2*y^2 + a0 = 0 (biquadratic)
        let disc = a2 * a2 - 4.0 * a0;
        if disc >= 0.0 {
            let sqrt_disc = disc.sqrt();
            let y2_1 = (-a2 + sqrt_disc) / 2.0;
            let y2_2 = (-a2 - sqrt_disc) / 2.0;

            if y2_1 >= 0.0 {
                let y = y2_1.sqrt();
                roots.push(y - p / 4.0);
                roots.push(-y - p / 4.0);
            }
            if y2_2 >= 0.0 {
                let y = y2_2.sqrt();
                roots.push(y - p / 4.0);
                roots.push(-y - p / 4.0);
            }
        }
    }

    // Filter out duplicates and invalid roots
    roots.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    roots.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
    roots
}

/// Solve a cubic equation: a*x^3 + b*x^2 + c*x + d = 0
///
/// Uses Cardano's formula.
fn solve_cubic(a: f64, b: f64, c: f64, d: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        // Degenerate to quadratic
        return solve_quadratic(b, c, d);
    }

    // Normalize: x^3 + px^2 + qx + r = 0
    let p = b / a;
    let q = c / a;
    let r = d / a;

    // Depressed cubic via substitution x = t - p/3
    // t^3 + at + b = 0
    let p2 = p * p;
    let aa = q - p2 / 3.0;
    let bb = r - p * q / 3.0 + 2.0 * p2 * p / 27.0;

    let delta = bb * bb / 4.0 + aa * aa * aa / 27.0;

    let mut roots = Vec::new();
    let shift = p / 3.0;

    if delta > 1e-12 {
        // One real root
        let sqrt_delta = delta.sqrt();
        let u = cbrt(-bb / 2.0 + sqrt_delta);
        let v = cbrt(-bb / 2.0 - sqrt_delta);
        roots.push(u + v - shift);
    } else if delta.abs() <= 1e-12 {
        // Multiple roots
        if aa.abs() < 1e-12 && bb.abs() < 1e-12 {
            // Triple root
            roots.push(-shift);
        } else {
            // Double root
            let u = cbrt(-bb / 2.0);
            roots.push(2.0 * u - shift);
            roots.push(-u - shift);
        }
    } else {
        // Three real roots (Vieta's trigonometric solution)
        let m = 2.0 * (-aa / 3.0).sqrt();
        let theta = (3.0 * bb / (aa * m)).clamp(-1.0, 1.0).acos() / 3.0;

        roots.push(m * theta.cos() - shift);
        roots.push(m * (theta - 2.0 * PI / 3.0).cos() - shift);
        roots.push(m * (theta + 2.0 * PI / 3.0).cos() - shift);
    }

    roots
}

/// Solve a quadratic equation: a*x^2 + b*x + c = 0
fn solve_quadratic(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        // Linear
        if b.abs() > 1e-12 {
            return vec![-c / b];
        }
        return Vec::new();
    }

    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Vec::new();
    }

    let sqrt_disc = disc.sqrt();
    vec![(-b - sqrt_disc) / (2.0 * a), (-b + sqrt_disc) / (2.0 * a)]
}

/// Cube root that handles negative numbers.
fn cbrt(x: f64) -> f64 {
    if x >= 0.0 {
        x.powf(1.0 / 3.0)
    } else {
        -(-x).powf(1.0 / 3.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};

    #[test]
    fn test_ray_torus_through_center() {
        let torus = TorusSurface::new(10.0, 3.0); // R=10, r=3

        // Ray from outside, through the center of the torus ring
        let ray = Ray::new(Point3::new(-20.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_torus(&ray, &torus);

        // Should hit 4 times: outer, inner, inner, outer
        assert_eq!(hits.len(), 4);

        // First hit at x = -13 (outer)
        assert!((hits[0].t - 7.0).abs() < 1e-8); // 20 - 13 = 7
                                                 // Second hit at x = -7 (inner)
        assert!((hits[1].t - 13.0).abs() < 1e-8); // 20 - 7 = 13
                                                  // Third hit at x = 7 (inner)
        assert!((hits[2].t - 27.0).abs() < 1e-8); // 20 + 7 = 27
                                                  // Fourth hit at x = 13 (outer)
        assert!((hits[3].t - 33.0).abs() < 1e-8); // 20 + 13 = 33
    }

    #[test]
    fn test_ray_torus_miss() {
        let torus = TorusSurface::new(10.0, 3.0);

        // Ray above the torus
        let ray = Ray::new(Point3::new(-20.0, 0.0, 10.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_torus(&ray, &torus);

        assert!(hits.is_empty());
    }

    #[test]
    fn test_ray_torus_two_hits() {
        let torus = TorusSurface::new(10.0, 3.0);

        // Ray hitting only the outer part (through the hole)
        let ray = Ray::new(Point3::new(-20.0, 0.0, 2.0), Vec3::new(1.0, 0.0, 0.0));
        let hits = intersect_torus(&ray, &torus);

        // Should hit 2 or 4 times depending on whether it clips the inner tube
        assert!(hits.len() == 2 || hits.len() == 4);
    }

    #[test]
    fn test_torus_uv() {
        let torus = TorusSurface::new(10.0, 3.0);

        // Point at (13, 0, 0) - outer equator, u=0, v=0
        let uv1 = compute_torus_uv(&torus, &Point3::new(13.0, 0.0, 0.0));
        assert!(uv1.x.abs() < 1e-10);
        assert!(uv1.y.abs() < 1e-10);

        // Point at (10, 0, 3) - top of tube at u=0, v=π/2
        let uv2 = compute_torus_uv(&torus, &Point3::new(10.0, 0.0, 3.0));
        assert!(uv2.x.abs() < 1e-10);
        assert!((uv2.y - PI / 2.0).abs() < 1e-10);

        // Point at (0, 13, 0) - outer equator, u=π/2, v=0
        let uv3 = compute_torus_uv(&torus, &Point3::new(0.0, 13.0, 0.0));
        assert!((uv3.x - PI / 2.0).abs() < 1e-10);
        assert!(uv3.y.abs() < 1e-10);
    }

    #[test]
    fn test_solve_quadratic() {
        let roots = solve_quadratic(1.0, -3.0, 2.0); // (x-1)(x-2) = 0
        assert_eq!(roots.len(), 2);
        assert!((roots[0] - 1.0).abs() < 1e-10);
        assert!((roots[1] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_solve_cubic() {
        // (x-1)(x-2)(x-3) = x^3 - 6x^2 + 11x - 6
        let roots = solve_cubic(1.0, -6.0, 11.0, -6.0);
        assert_eq!(roots.len(), 3);
        roots.iter().for_each(|r| {
            assert!(
                (*r - 1.0).abs() < 1e-10 || (*r - 2.0).abs() < 1e-10 || (*r - 3.0).abs() < 1e-10
            );
        });
    }
}
