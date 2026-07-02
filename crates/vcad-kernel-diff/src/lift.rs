//! The lift-bridge: recover the concrete surface behind a `dyn Surface`,
//! lift it to `Dual<f64>`, and seed the θ-dependent fields.
//!
//! This deliberately bypasses the concrete `f64` [`Surface`] trait by
//! dropping to the underlying scalar-generic struct — the trait and the
//! geometry store stay untouched, and differentiation happens per-face, on
//! demand, only for faces whose surface depends on θ.

use tang::Dual;
use vcad_kernel_geom::{
    BilinearSurface, ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind,
    TorusSurface,
};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_nurbs::BSplineSurface;

use crate::{downcast, DiffError, SurfaceSeed};

type D = Dual<f64>;

/// A concrete surface lifted to `Dual<f64>` with a θ-seed applied.
#[derive(Debug, Clone)]
pub enum DualSurface {
    /// Lifted plane.
    Plane(Plane<D>),
    /// Lifted cylinder.
    Cylinder(CylinderSurface<D>),
    /// Lifted cone.
    Cone(ConeSurface<D>),
    /// Lifted sphere.
    Sphere(SphereSurface<D>),
    /// Lifted torus.
    Torus(TorusSurface<D>),
    /// Lifted bilinear patch.
    Bilinear(BilinearSurface<D>),
    /// Lifted B-spline surface.
    BSpline(BSplineSurface<D>),
}

fn seed_point(p: &mut tang::Point3<D>, velocity: Vec3) {
    p.x.dual += velocity.x;
    p.y.dual += velocity.y;
    p.z.dual += velocity.z;
}

/// Lift a stored (concrete, `f64`) surface to `Dual<f64>`, seeding the
/// field(s) touched by θ. An empty seed slice lifts with all dual parts
/// zero (a θ-independent surface); multiple seeds compose additively (a
/// fillet blend's radius and axis position both move with the fillet
/// radius).
///
/// Each arm matches on [`SurfaceKind`], recovers the concrete struct, calls
/// its existing `lift::<Dual<f64>>()`, and writes the seeds into the lifted
/// fields. Inapplicable seeds are a hard error, never silently ignored.
pub fn lift_surface(
    surface: &dyn Surface,
    seeds: &[SurfaceSeed],
) -> Result<DualSurface, DiffError> {
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Plane => {
            let mut s = downcast::<Plane>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => seed_point(&mut s.origin, velocity),
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Plane(s))
        }
        SurfaceKind::Cylinder => {
            let mut s = downcast::<CylinderSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => seed_point(&mut s.center, velocity),
                    SurfaceSeed::CylinderRadius { rate } => s.radius.dual += rate,
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Cylinder(s))
        }
        SurfaceKind::Cone => {
            let mut s = downcast::<ConeSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => seed_point(&mut s.apex, velocity),
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Cone(s))
        }
        SurfaceKind::Sphere => {
            let mut s = downcast::<SphereSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => seed_point(&mut s.center, velocity),
                    SurfaceSeed::SphereRadius { rate } => s.radius.dual += rate,
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Sphere(s))
        }
        SurfaceKind::Torus => {
            let mut s = downcast::<TorusSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => seed_point(&mut s.center, velocity),
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Torus(s))
        }
        SurfaceKind::Bilinear => {
            let mut s = downcast::<BilinearSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => {
                        seed_point(&mut s.p00, velocity);
                        seed_point(&mut s.p10, velocity);
                        seed_point(&mut s.p01, velocity);
                        seed_point(&mut s.p11, velocity);
                    }
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::Bilinear(s))
        }
        SurfaceKind::BSpline => {
            let mut s = downcast::<BSplineSurface>(surface, kind)?.lift::<D>();
            for seed in seeds {
                match *seed {
                    SurfaceSeed::Translate { velocity } => {
                        for cp in &mut s.control_points {
                            seed_point(cp, velocity);
                        }
                    }
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                }
            }
            Ok(DualSurface::BSpline(s))
        }
    }
}

impl DualSurface {
    /// Evaluate at a frozen `(u, v)` (constants — the sample pattern does
    /// not move with θ), returning the dual-valued position.
    pub fn evaluate(&self, uv: Point2) -> tang::Point3<D> {
        let uv_d = tang::Point2::new(D::constant(uv.x), D::constant(uv.y));
        match self {
            DualSurface::Plane(s) => s.evaluate(uv_d),
            DualSurface::Cylinder(s) => s.evaluate(uv_d),
            DualSurface::Cone(s) => s.evaluate(uv_d),
            DualSurface::Sphere(s) => s.evaluate(uv_d),
            DualSurface::Torus(s) => s.evaluate(uv_d),
            DualSurface::Bilinear(s) => s.evaluate(uv_d),
            DualSurface::BSpline(s) => s.eval(uv.x, uv.y),
        }
    }

    /// Position `x(θ)` and velocity `dx/dθ` at a frozen `(u, v)`.
    pub fn evaluate_with_velocity(&self, uv: Point2) -> (Point3, Vec3) {
        let p = self.evaluate(uv);
        (
            Point3::new(p.x.real, p.y.real, p.z.real),
            Vec3::new(p.x.dual, p.y.dual, p.z.dual),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_offset_lift_matches_exact_velocity() {
        // Plane translating along its normal: dx/dθ = ẑ everywhere.
        let plane = Plane::new(Point3::new(0.0, 0.0, 2.0), Vec3::x(), Vec3::y());
        let lifted = lift_surface(
            &plane,
            &[SurfaceSeed::Translate {
                velocity: Vec3::z(),
            }],
        )
        .unwrap();
        for &(u, v) in &[(0.0, 0.0), (1.5, -2.5), (10.0, 3.0)] {
            let (p, vel) = lifted.evaluate_with_velocity(Point2::new(u, v));
            assert!((p.z - 2.0).abs() < 1e-15);
            assert!((vel - Vec3::z()).norm() < 1e-15);
        }
    }

    #[test]
    fn cylinder_radius_lift_matches_exact_velocity() {
        // Radius seed: dx/dr = radial direction (cos u, sin u, 0).
        let cyl = CylinderSurface::new(5.0);
        let lifted = lift_surface(&cyl, &[SurfaceSeed::CylinderRadius { rate: 1.0 }]).unwrap();
        for &(u, v) in &[(0.0, 0.0), (1.0, 3.0), (4.5, -2.0)] {
            let (p, vel) = lifted.evaluate_with_velocity(Point2::new(u, v));
            let radial = Vec3::new(u.cos(), u.sin(), 0.0);
            assert!(
                (vel - radial).norm() < 1e-15,
                "vel {vel:?} vs radial {radial:?}"
            );
            assert!((p.z - v).abs() < 1e-15);
        }
    }

    #[test]
    fn inapplicable_seed_is_an_error() {
        let plane = Plane::xy();
        let err = lift_surface(&plane, &[SurfaceSeed::CylinderRadius { rate: 1.0 }]);
        assert!(matches!(err, Err(DiffError::UnsupportedSeed { .. })));
    }

    #[test]
    fn unseeded_lift_has_zero_velocity() {
        let sph = SphereSurface::new(3.0);
        let lifted = lift_surface(&sph, &[]).unwrap();
        let (_, vel) = lifted.evaluate_with_velocity(Point2::new(0.7, 0.3));
        assert!(vel.norm() < 1e-15);
    }
}
