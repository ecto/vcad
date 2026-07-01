//! The lift-bridge: forward-mode differentiation of stored surfaces.
//!
//! This is the first executable piece of the *differentiable seam* — the
//! mechanism that turns a concrete, `f64`, `dyn Surface` (as held in a
//! [`GeometryStore`](crate::GeometryStore)) into a dual-number evaluation that
//! carries both the surface position **and** its sensitivity `dx/dθ` to a CAD
//! parameter `θ`.
//!
//! # How it works
//!
//! The kernel talks to geometry through the object-safe, `f64`-only
//! [`Surface`](crate::Surface) trait. But every concrete surface struct
//! (`Plane`, `CylinderSurface`, …) is *scalar-generic* and exposes a
//! `lift::<T>()` that reinterprets it at a new scalar type, plus a generic
//! `evaluate`. This module **bypasses the trait** by downcasting the
//! `dyn Surface` back to its concrete struct (via [`Any`](std::any::Any)),
//! calling `lift::<Dual<f64>>()`, seeding the θ-dependent field(s) with the
//! dual (derivative) part, and evaluating at the **frozen** `(u, v)` sample.
//!
//! Because the `(u, v)` sample is held constant (a [`Dual::constant`]), the
//! only derivative that flows is `dx/dθ` — exactly the interior-sample
//! sensitivity (Pillar 2) needed to differentiate through tessellation without
//! touching the boolean/trim combinatorics.

use crate::{CylinderSurface, Plane, Surface};
use tang::Dual;
use vcad_kernel_math::{Point2, Vec3};

/// How a scalar parameter `θ` perturbs the field(s) of one stored surface.
///
/// A seed is the explicit, testable θ→field map named in the design: it says
/// *which* concrete field carries the derivative and at what rate. Keep it
/// honest — a surface a given `θ` does not touch must be [`SurfaceSeed::Frozen`]
/// so its `dx/dθ` is exactly zero.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceSeed {
    /// `θ` does not move this surface: `dx/dθ = 0`. Valid for any surface kind.
    Frozen,
    /// A [`Plane`] whose `origin` translates by `rate` per unit `θ` (its frame
    /// directions are held fixed). Then `dx/dθ = rate`. For a face that slides
    /// along its own normal at unit rate, `rate = *normal_dir`.
    PlaneTranslate {
        /// `d(origin)/dθ`, the translation velocity of the plane origin.
        rate: Vec3,
    },
    /// A [`CylinderSurface`] whose `radius` *is* the parameter (`dr/dθ = 1`).
    /// Then `dx/dθ` is the outward radial direction at the sample.
    CylinderRadius,
}

/// Error raised when a [`SurfaceSeed`] is applied to the wrong surface kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedMismatch {
    /// The seed variant name that was requested.
    pub seed: &'static str,
    /// The surface kind actually stored.
    pub found: crate::SurfaceKind,
}

impl std::fmt::Display for SeedMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "surface seed {:?} cannot be applied to a {:?} surface",
            self.seed, self.found
        )
    }
}

impl std::error::Error for SeedMismatch {}

/// Evaluate a stored surface at a **frozen** `(u, v)` sample and return the
/// position as a dual number carrying `dx/dθ` in its dual part.
///
/// The real part of the returned point is the ordinary `f64` position; the
/// dual part of each coordinate is `∂x_i/∂θ` under the given [`SurfaceSeed`].
///
/// This is the lift-bridge. It differentiates *per-face, on demand*, only for
/// faces whose surface actually depends on `θ`.
pub fn eval_surface_dual(
    surface: &dyn Surface,
    seed: &SurfaceSeed,
    u: f64,
    v: f64,
) -> Result<tang::Point3<Dual<f64>>, SeedMismatch> {
    match seed {
        // Any surface kind; the derivative is identically zero. We take the
        // concrete f64 position through the object-safe trait and lift it to a
        // constant dual — no downcast required, so every kind is supported.
        SurfaceSeed::Frozen => {
            let p = surface.evaluate(Point2::new(u, v));
            Ok(tang::Point3::new(
                Dual::constant(p.x),
                Dual::constant(p.y),
                Dual::constant(p.z),
            ))
        }

        SurfaceSeed::PlaneTranslate { rate } => {
            let plane = surface
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or(SeedMismatch {
                    seed: "PlaneTranslate",
                    found: surface.surface_type(),
                })?;
            // Lift to Dual (all fields constant), then seed the θ-dependent
            // field: the origin translates at `rate`, so origin.dual = rate.
            let mut lifted: Plane<Dual<f64>> = plane.lift();
            lifted.origin = tang::Point3::new(
                Dual::new(plane.origin.x, rate.x),
                Dual::new(plane.origin.y, rate.y),
                Dual::new(plane.origin.z, rate.z),
            );
            Ok(lifted.evaluate(frozen_uv(u, v)))
        }

        SurfaceSeed::CylinderRadius => {
            let cyl = surface
                .as_any()
                .downcast_ref::<CylinderSurface>()
                .ok_or(SeedMismatch {
                    seed: "CylinderRadius",
                    found: surface.surface_type(),
                })?;
            // Seed the radius field with unit derivative: dr/dθ = 1.
            let mut lifted: CylinderSurface<Dual<f64>> = cyl.lift();
            lifted.radius = Dual::new(cyl.radius, 1.0);
            Ok(lifted.evaluate(frozen_uv(u, v)))
        }
    }
}

/// A `(u, v)` sample as a *constant* dual pair — no derivative flows from the
/// parameter domain, only from the seeded surface field.
#[inline]
fn frozen_uv(u: f64, v: f64) -> tang::Point2<Dual<f64>> {
    tang::Point2::new(Dual::constant(u), Dual::constant(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use vcad_kernel_math::Point3;

    const GATE: f64 = 1e-6;

    /// Central-difference of a surface position wrt θ, rebuilding the surface
    /// at θ±h. `build` maps θ to a concrete surface.
    fn fd_dxdt<S: Surface>(build: impl Fn(f64) -> S, theta: f64, u: f64, v: f64, h: f64) -> Vec3 {
        let p = |t: f64| build(t).evaluate(Point2::new(u, v));
        let plus = p(theta + h);
        let minus = p(theta - h);
        (plus - minus) / (2.0 * h)
    }

    fn rel_err(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-12)
    }

    #[test]
    fn cylinder_radius_matches_fd() {
        // θ = radius. dx/dr = outward radial direction.
        let r0 = 7.3;
        let (u, v) = (0.9, 2.1);
        let surf = CylinderSurface::new(r0);
        let dual = eval_surface_dual(&surf, &SurfaceSeed::CylinderRadius, u, v).unwrap();

        let fd = fd_dxdt(CylinderSurface::new, r0, u, v, 1e-6);
        assert!(
            rel_err(dual.x.dual, fd.x) < GATE,
            "x: {} vs {}",
            dual.x.dual,
            fd.x
        );
        assert!(
            rel_err(dual.y.dual, fd.y) < GATE,
            "y: {} vs {}",
            dual.y.dual,
            fd.y
        );
        // z is along the axis; independent of radius → derivative 0.
        assert!(dual.z.dual.abs() < 1e-12);

        // Analytic radial direction: (cos u, sin u, 0) for the canonical cylinder.
        assert!(rel_err(dual.x.dual, u.cos()) < GATE);
        assert!(rel_err(dual.y.dual, u.sin()) < GATE);
        // Real part must equal the primal position.
        assert!((dual.x.real - r0 * u.cos()).abs() < 1e-9);
        assert!((dual.y.real - r0 * u.sin()).abs() < 1e-9);
        assert!((dual.z.real - v).abs() < 1e-9);
    }

    #[test]
    fn plane_translate_matches_fd() {
        // A z=θ plane sliding along +z. dx/dθ = (0,0,1).
        let build = |t: f64| Plane::new(Point3::new(0.0, 0.0, t), Vec3::x(), Vec3::y());
        let t0 = 4.0;
        let (u, v) = (2.5, -1.5);
        let surf = build(t0);
        let dual = eval_surface_dual(
            &surf,
            &SurfaceSeed::PlaneTranslate { rate: Vec3::z() },
            u,
            v,
        )
        .unwrap();
        let fd = fd_dxdt(build, t0, u, v, 1e-6);
        assert!(rel_err(dual.z.dual, fd.z) < GATE);
        assert!((dual.z.dual - 1.0).abs() < GATE);
        assert!(dual.x.dual.abs() < 1e-12 && dual.y.dual.abs() < 1e-12);
        // Primal position: origin + u*x + v*y.
        assert!((dual.x.real - u).abs() < 1e-9);
        assert!((dual.y.real - v).abs() < 1e-9);
        assert!((dual.z.real - t0).abs() < 1e-9);
    }

    #[test]
    fn frozen_seed_is_zero_derivative_any_kind() {
        let surf = CylinderSurface::new(3.0);
        let dual = eval_surface_dual(&surf, &SurfaceSeed::Frozen, PI / 3.0, 1.0).unwrap();
        assert!(dual.x.dual == 0.0 && dual.y.dual == 0.0 && dual.z.dual == 0.0);
        // Real part still tracks position.
        assert!((dual.x.real - 3.0 * (PI / 3.0).cos()).abs() < 1e-12);
    }

    #[test]
    fn seed_kind_mismatch_errors() {
        let surf = CylinderSurface::new(2.0);
        let err = eval_surface_dual(
            &surf,
            &SurfaceSeed::PlaneTranslate { rate: Vec3::z() },
            0.0,
            0.0,
        );
        assert!(err.is_err());
    }
}
