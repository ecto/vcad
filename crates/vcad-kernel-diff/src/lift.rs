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
                    SurfaceSeed::ConeAngle { rate } => s.half_angle.dual += rate,
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
                    SurfaceSeed::TorusMajorRadius { rate } => s.major_radius.dual += rate,
                    SurfaceSeed::TorusMinorRadius { rate } => s.minor_radius.dual += rate,
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

/// Nested dual scalar: value, first tangent (`ẋ`), and second tangent (`ẍ`)
/// in one pass, seeded as `((x, ẋ), (ẋ, ẍ))`.
type Dd = Dual<Dual<f64>>;

/// A concrete surface lifted to `Dual<Dual<f64>>` with first- **and**
/// second-order θ-seeds applied — the exact second-order counterpart of
/// [`DualSurface`]. Unlike the first-order lift there is no linearity
/// assumption anywhere: a cone's `tan(half_angle)` or any other nonlinear
/// field dependence differentiates through the nested duals exactly, so
/// every surface kind is supported.
#[derive(Debug, Clone)]
pub enum DualSurface2 {
    /// Lifted plane.
    Plane(Box<Plane<Dd>>),
    /// Lifted cylinder.
    Cylinder(Box<CylinderSurface<Dd>>),
    /// Lifted cone.
    Cone(Box<ConeSurface<Dd>>),
    /// Lifted sphere.
    Sphere(Box<SphereSurface<Dd>>),
    /// Lifted torus.
    Torus(Box<TorusSurface<Dd>>),
    /// Lifted bilinear patch.
    Bilinear(Box<BilinearSurface<Dd>>),
    /// Lifted B-spline surface.
    BSpline(Box<BSplineSurface<Dd>>),
}

/// Add a field's first derivative `v` and second derivative `a` into a
/// nested-dual scalar seeded `((f, ḟ), (ḟ, f̈))`.
fn seed_scalar2(f: &mut Dd, v: f64, a: f64) {
    f.real.dual += v;
    f.dual.real += v;
    f.dual.dual += a;
}

fn seed_point2(p: &mut tang::Point3<Dd>, v: Vec3, a: Vec3) {
    seed_scalar2(&mut p.x, v.x, a.x);
    seed_scalar2(&mut p.y, v.y, a.y);
    seed_scalar2(&mut p.z, v.z, a.z);
}

/// Sum the translation velocities of a seed slice (zero if none), erroring
/// on any seed not in `allowed`.
fn collect2(
    kind: SurfaceKind,
    seeds: &[SurfaceSeed],
    mut on_seed: impl FnMut(&SurfaceSeed) -> bool,
) -> Result<(), DiffError> {
    for seed in seeds {
        if !on_seed(seed) {
            return Err(DiffError::UnsupportedSeed { kind, seed: *seed });
        }
    }
    Ok(())
}

/// Lift a stored surface to `Dual<Dual<f64>>`, seeding each θ-touched field
/// with its first derivative (from `vel_seeds`) and second derivative (from
/// `acc_seeds`). The two slices use the same [`SurfaceSeed`] vocabulary —
/// a rate/velocity in `acc_seeds` is read as the field's `d²/dθ²` — so a
/// first-order seeding lifts to second order by adding accelerations and
/// nothing else (empty `acc_seeds` = fields linear in θ).
pub fn lift_surface_second(
    surface: &dyn Surface,
    vel_seeds: &[SurfaceSeed],
    acc_seeds: &[SurfaceSeed],
) -> Result<DualSurface2, DiffError> {
    let kind = surface.surface_type();
    // Each arm folds both seed slices into (velocity, acceleration) per
    // field, then writes them into the lifted struct via seed_scalar2 /
    // seed_point2.
    match kind {
        SurfaceKind::Plane => {
            let mut s = downcast::<Plane>(surface, kind)?.lift::<Dd>();
            let mut v = Vec3::new(0.0, 0.0, 0.0);
            let mut a = Vec3::new(0.0, 0.0, 0.0);
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                _ => false,
            })?;
            seed_point2(&mut s.origin, v, a);
            Ok(DualSurface2::Plane(Box::new(s)))
        }
        SurfaceKind::Cylinder => {
            let mut s = downcast::<CylinderSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            let (mut rv, mut ra) = (0.0, 0.0);
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                SurfaceSeed::CylinderRadius { rate } => {
                    rv += rate;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                SurfaceSeed::CylinderRadius { rate } => {
                    ra += rate;
                    true
                }
                _ => false,
            })?;
            seed_point2(&mut s.center, v, a);
            seed_scalar2(&mut s.radius, rv, ra);
            Ok(DualSurface2::Cylinder(Box::new(s)))
        }
        SurfaceKind::Cone => {
            let mut s = downcast::<ConeSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            let (mut av, mut aa) = (0.0, 0.0);
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                SurfaceSeed::ConeAngle { rate } => {
                    av += rate;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                SurfaceSeed::ConeAngle { rate } => {
                    aa += rate;
                    true
                }
                _ => false,
            })?;
            seed_point2(&mut s.apex, v, a);
            seed_scalar2(&mut s.half_angle, av, aa);
            Ok(DualSurface2::Cone(Box::new(s)))
        }
        SurfaceKind::Sphere => {
            let mut s = downcast::<SphereSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            let (mut rv, mut ra) = (0.0, 0.0);
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                SurfaceSeed::SphereRadius { rate } => {
                    rv += rate;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                SurfaceSeed::SphereRadius { rate } => {
                    ra += rate;
                    true
                }
                _ => false,
            })?;
            seed_point2(&mut s.center, v, a);
            seed_scalar2(&mut s.radius, rv, ra);
            Ok(DualSurface2::Sphere(Box::new(s)))
        }
        SurfaceKind::Torus => {
            let mut s = downcast::<TorusSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            let (mut mv, mut ma) = (0.0, 0.0);
            let (mut nv, mut na) = (0.0, 0.0);
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                SurfaceSeed::TorusMajorRadius { rate } => {
                    mv += rate;
                    true
                }
                SurfaceSeed::TorusMinorRadius { rate } => {
                    nv += rate;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                SurfaceSeed::TorusMajorRadius { rate } => {
                    ma += rate;
                    true
                }
                SurfaceSeed::TorusMinorRadius { rate } => {
                    na += rate;
                    true
                }
                _ => false,
            })?;
            seed_point2(&mut s.center, v, a);
            seed_scalar2(&mut s.major_radius, mv, ma);
            seed_scalar2(&mut s.minor_radius, nv, na);
            Ok(DualSurface2::Torus(Box::new(s)))
        }
        SurfaceKind::Bilinear => {
            let mut s = downcast::<BilinearSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                _ => false,
            })?;
            for cp in [&mut s.p00, &mut s.p10, &mut s.p01, &mut s.p11] {
                seed_point2(cp, v, a);
            }
            Ok(DualSurface2::Bilinear(Box::new(s)))
        }
        SurfaceKind::BSpline => {
            let mut s = downcast::<BSplineSurface>(surface, kind)?.lift::<Dd>();
            let (mut v, mut a) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
            collect2(kind, vel_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    v += velocity;
                    true
                }
                _ => false,
            })?;
            collect2(kind, acc_seeds, |seed| match *seed {
                SurfaceSeed::Translate { velocity } => {
                    a += velocity;
                    true
                }
                _ => false,
            })?;
            for cp in &mut s.control_points {
                seed_point2(cp, v, a);
            }
            Ok(DualSurface2::BSpline(Box::new(s)))
        }
    }
}

impl DualSurface2 {
    /// Position `x(θ)`, velocity `dx/dθ`, and acceleration `d²x/dθ²` at a
    /// frozen `(u, v)` — read off the value / first-tangent / second-tangent
    /// slots of the nested-dual evaluation.
    pub fn evaluate_with_acceleration(&self, uv: Point2) -> (Point3, Vec3, Vec3) {
        let c = |x: f64| -> Dd { Dual::constant(Dual::constant(x)) };
        let uv_d = tang::Point2::new(c(uv.x), c(uv.y));
        let p = match self {
            DualSurface2::Plane(s) => s.evaluate(uv_d),
            DualSurface2::Cylinder(s) => s.evaluate(uv_d),
            DualSurface2::Cone(s) => s.evaluate(uv_d),
            DualSurface2::Sphere(s) => s.evaluate(uv_d),
            DualSurface2::Torus(s) => s.evaluate(uv_d),
            DualSurface2::Bilinear(s) => s.evaluate(uv_d),
            DualSurface2::BSpline(s) => s.eval(uv.x, uv.y),
        };
        (
            Point3::new(p.x.real.real, p.y.real.real, p.z.real.real),
            Vec3::new(p.x.real.dual, p.y.real.dual, p.z.real.dual),
            Vec3::new(p.x.dual.dual, p.y.dual.dual, p.z.dual.dual),
        )
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
    fn cone_angle_lift_matches_central_difference() {
        use vcad_kernel_math::Dir3;
        let cone = ConeSurface {
            apex: Point3::new(0.0, 0.0, 20.0),
            axis: Dir3::new_normalize(-Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle: 0.25_f64.atan(),
        };
        let lifted = lift_surface(&cone, &[SurfaceSeed::ConeAngle { rate: 1.0 }]).unwrap();
        let h = 1e-7;
        for &(u, v) in &[(0.0, 5.0), (1.3, 12.0), (4.7, 18.0)] {
            let uv = Point2::new(u, v);
            let (_, vel) = lifted.evaluate_with_velocity(uv);
            let bump = |da: f64| {
                ConeSurface {
                    half_angle: cone.half_angle + da,
                    ..cone.clone()
                }
                .evaluate(uv)
            };
            let fd = (bump(h) - bump(-h)) / (2.0 * h);
            assert!(
                (vel - fd).norm() < 1e-5,
                "cone ({u},{v}): {vel:?} vs {fd:?}"
            );
        }
    }

    #[test]
    fn torus_radii_lifts_match_central_difference() {
        let torus = TorusSurface::new(7.0, 2.0);
        let h = 1e-7;
        let samples = [(0.0, 0.0), (1.1, 2.2), (3.4, 5.1)];

        // Major radius: dP/dR = tube_center_dir (the u-circle direction).
        let major = lift_surface(&torus, &[SurfaceSeed::TorusMajorRadius { rate: 1.0 }]).unwrap();
        for &(u, v) in &samples {
            let uv = Point2::new(u, v);
            let (_, vel) = major.evaluate_with_velocity(uv);
            let bump = |d: f64| {
                TorusSurface {
                    major_radius: torus.major_radius + d,
                    ..torus.clone()
                }
                .evaluate(uv)
            };
            let fd = (bump(h) - bump(-h)) / (2.0 * h);
            assert!(
                (vel - fd).norm() < 1e-5,
                "major ({u},{v}): {vel:?} vs {fd:?}"
            );
        }

        // Minor radius: dP/dr = surface normal (outward from the tube).
        let minor = lift_surface(&torus, &[SurfaceSeed::TorusMinorRadius { rate: 1.0 }]).unwrap();
        for &(u, v) in &samples {
            let uv = Point2::new(u, v);
            let (_, vel) = minor.evaluate_with_velocity(uv);
            let bump = |d: f64| {
                TorusSurface {
                    minor_radius: torus.minor_radius + d,
                    ..torus.clone()
                }
                .evaluate(uv)
            };
            let fd = (bump(h) - bump(-h)) / (2.0 * h);
            assert!(
                (vel - fd).norm() < 1e-5,
                "minor ({u},{v}): {vel:?} vs {fd:?}"
            );
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

    #[test]
    fn cone_angle_second_order_lift_matches_fd_of_velocity() {
        // ẍ of a cone surface point under a constant half-angle rate: the
        // point is nonlinear in α (tan α), so ẍ ≠ 0 even with α̈ = 0. Oracle:
        // central difference of the *first-order* lift's velocity at α ± h.
        use vcad_kernel_math::Dir3;
        let cone = ConeSurface {
            apex: Point3::new(0.0, 0.0, 20.0),
            axis: Dir3::new_normalize(-Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle: 0.25_f64.atan(),
        };
        let seed = [SurfaceSeed::ConeAngle { rate: 1.0 }];
        let lifted2 = lift_surface_second(&cone, &seed, &[]).unwrap();
        let h = 1e-6;
        for &(u, v) in &[(0.0, 5.0), (1.3, 12.0), (4.7, 18.0)] {
            let uv = Point2::new(u, v);
            let (_, vel, acc) = lifted2.evaluate_with_acceleration(uv);
            // First-order consistency.
            let (_, vel1) = lift_surface(&cone, &seed)
                .unwrap()
                .evaluate_with_velocity(uv);
            assert!((vel - vel1).norm() < 1e-13, "vel mismatch at ({u},{v})");
            // FD of the velocity across α.
            let vel_at = |da: f64| {
                let bumped = ConeSurface {
                    half_angle: cone.half_angle + da,
                    ..cone.clone()
                };
                lift_surface(&bumped, &seed)
                    .unwrap()
                    .evaluate_with_velocity(uv)
                    .1
            };
            let fd = (vel_at(h) - vel_at(-h)) / (2.0 * h);
            assert!(
                (acc - fd).norm() < 1e-4 * fd.norm().max(1.0),
                "cone ẍ ({u},{v}): {acc:?} vs FD {fd:?}"
            );
            assert!(acc.norm() > 1e-3, "cone ẍ should be nonzero (tan α)");
        }
    }

    #[test]
    fn torus_nonlinear_minor_radius_second_order_lift() {
        // r(θ) = θ² at θ0: velocity seeds carry ṙ = 2θ0, acceleration seeds
        // r̈ = 2. Torus points are linear in r at frozen (u, v), so
        // ẍ = r̈·∂x/∂r = 2·(cos v·û + sin v·ẑ), a unit direction scaled by 2.
        let torus = TorusSurface::new(7.0, 2.25); // r = θ0², θ0 = 1.5
        let theta0 = 1.5;
        let vel = [SurfaceSeed::TorusMinorRadius { rate: 2.0 * theta0 }];
        let acc = [SurfaceSeed::TorusMinorRadius { rate: 2.0 }];
        let lifted2 = lift_surface_second(&torus, &vel, &acc).unwrap();
        for &(u, v) in &[(0.0, 0.0), (1.1, 2.2), (3.4, 5.1)] {
            let (_, _, a) = lifted2.evaluate_with_acceleration(Point2::new(u, v));
            assert!(
                (a.norm() - 2.0).abs() < 1e-12,
                "torus ẍ norm {} at ({u},{v}), expected 2 (= r̈)",
                a.norm()
            );
        }
    }

    #[test]
    fn linear_seeding_second_order_lift_has_zero_acceleration() {
        // Plane/cylinder/sphere points are linear in their seeded fields, so
        // empty acceleration seeds ⇒ ẍ = 0 exactly — the invariant the old
        // two-lift shortcut relied on, now a special case of the nested lift.
        let cyl = CylinderSurface::new(5.0);
        let lifted2 =
            lift_surface_second(&cyl, &[SurfaceSeed::CylinderRadius { rate: 1.0 }], &[]).unwrap();
        let (_, vel, acc) = lifted2.evaluate_with_acceleration(Point2::new(1.0, 3.0));
        assert!((vel - Vec3::new(1.0_f64.cos(), 1.0_f64.sin(), 0.0)).norm() < 1e-15);
        assert!(acc.norm() < 1e-15, "linear field ⇒ zero ẍ, got {acc:?}");
    }
}
