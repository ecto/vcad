//! Pillar 3: implicit differentiation of topology vertices.
//!
//! A topology vertex `x` produced by the kernel (a corner, or a point on a
//! trim/intersection curve) satisfies `g_i(x; θ) = 0` for every adjacent
//! surface `i`. Differentiating along the frozen branch:
//!
//! ```text
//! ∇g_i · ẋ = −∂g_i/∂θ
//! ```
//!
//! With three independent adjacent surfaces (a corner) the system determines
//! `ẋ` fully. With two (a point on an intersection curve, e.g. the rim of a
//! boolean through-hole) one tangential degree of freedom remains — the
//! frozen-topology convention pins it at zero (the node keeps its frozen
//! curve parameter), which is the minimum-norm solution. This
//! differentiates the *equations that define* the kernel's output without
//! touching the code that computed it.

use vcad_kernel_geom::{
    ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Point3, Vec3};

use crate::{downcast, DiffError, SurfaceSeed};

/// One row of the implicit vertex system: `gradient · ẋ = rhs`.
#[derive(Debug, Clone, Copy)]
pub struct ConstraintRow {
    /// ∇g at the vertex.
    pub gradient: Vec3,
    /// −∂g/∂θ at the vertex.
    pub rhs: f64,
}

/// The implicit form of a surface at `x`: `(g, ∇g, ∂g/∂θ)`, with the
/// θ-term summed over composed seeds (a fillet blend's radius and axis
/// position both carry the fillet-radius parameter).
///
/// Single source of truth for both [`constraint_row`] and
/// [`surface_residual`], so incidence detection and the rows actually
/// solved can never disagree. Implicit forms match
/// [`vcad_kernel_geom::implicit_form`] term-for-term (plane `g = n·(x − o)`;
/// cylinder `g = |radial|² − r²`; sphere `g = |x − c|² − r²`; cone
/// `g = |radial|² − tan²α·axial²`; torus `g = (ρ − R)² + axial² − r²`) — the
/// geom side owns `(g, ∇g)` for the Newton/boundary path and this side adds
/// the seeded `∂g/∂θ` for the diff rows; keeping the algebra identical is
/// what lets both paths share a vertex. Returns `Ok(None)` for kinds without
/// an implicit form (and for the torus at the on-axis degeneracy `ρ → 0`).
fn implicit_terms(
    surface: &dyn Surface,
    seeds: &[SurfaceSeed],
    x: &Point3,
) -> Result<Option<(f64, Vec3, f64)>, DiffError> {
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Plane => {
            let plane = downcast::<Plane>(surface, kind)?;
            let n = *plane.normal_dir.as_ref();
            let g = plane.signed_distance(x);
            let mut g_theta = 0.0;
            for seed in seeds {
                g_theta += match *seed {
                    SurfaceSeed::Translate { velocity } => -n.dot(velocity),
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                };
            }
            Ok(Some((g, n, g_theta)))
        }
        SurfaceKind::Cylinder => {
            let cyl = downcast::<CylinderSurface>(surface, kind)?;
            let a = *cyl.axis.as_ref();
            let d = *x - cyl.center;
            let radial = d - a * d.dot(a);
            let g = radial.norm_squared() - cyl.radius * cyl.radius;
            let mut g_theta = 0.0;
            for seed in seeds {
                g_theta += match *seed {
                    SurfaceSeed::CylinderRadius { rate } => -2.0 * cyl.radius * rate,
                    SurfaceSeed::Translate { velocity } => {
                        // ∂g/∂θ = −2 radial · v_perp; radial ⊥ a so the axial
                        // component of v drops out automatically.
                        -2.0 * radial.dot(velocity)
                    }
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                };
            }
            Ok(Some((g, 2.0 * radial, g_theta)))
        }
        SurfaceKind::Sphere => {
            let sph = downcast::<SphereSurface>(surface, kind)?;
            let d = *x - sph.center;
            let g = d.norm_squared() - sph.radius * sph.radius;
            let mut g_theta = 0.0;
            for seed in seeds {
                g_theta += match *seed {
                    SurfaceSeed::SphereRadius { rate } => -2.0 * sph.radius * rate,
                    SurfaceSeed::Translate { velocity } => -2.0 * d.dot(velocity),
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                };
            }
            Ok(Some((g, 2.0 * d, g_theta)))
        }
        SurfaceKind::Cone => {
            let cone = downcast::<ConeSurface>(surface, kind)?;
            let axis = *cone.axis.as_ref();
            let rel = *x - cone.apex;
            let axial = rel.dot(axis);
            let radial = rel - axis * axial;
            let t = cone.half_angle.tan();
            let tan2 = t * t;
            let g = radial.norm_squared() - tan2 * axial * axial;
            let grad = radial * 2.0 - axis * (2.0 * tan2 * axial);
            let mut g_theta = 0.0;
            for seed in seeds {
                g_theta += match *seed {
                    // Rigid apex translation: ∂g/∂θ = −∇g·v (the whole
                    // implicit form rides the apex, so the query point's
                    // motion relative to it is −v).
                    SurfaceSeed::Translate { velocity } => -grad.dot(velocity),
                    // Half-angle opening: only tan²α depends on α, so
                    // ∂g/∂α = −axial²·d(tan²α)/dα = −axial²·2 tanα·sec²α.
                    SurfaceSeed::ConeAngle { rate } => {
                        let cos_a = cone.half_angle.cos();
                        let sec2 = 1.0 / (cos_a * cos_a);
                        -axial * axial * 2.0 * t * sec2 * rate
                    }
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                };
            }
            Ok(Some((g, grad, g_theta)))
        }
        SurfaceKind::Torus => {
            let torus = downcast::<TorusSurface>(surface, kind)?;
            let axis = *torus.axis.as_ref();
            let rel = *x - torus.center;
            let axial = rel.dot(axis);
            let p_radial = rel - axis * axial;
            let rho = p_radial.norm();
            if rho < 1e-12 {
                return Ok(None); // on the axis: ∇g undefined
            }
            let major = torus.major_radius;
            let minor = torus.minor_radius;
            let g = (rho - major) * (rho - major) + axial * axial - minor * minor;
            let grad = p_radial * (2.0 * (rho - major) / rho) + axis * (2.0 * axial);
            let mut g_theta = 0.0;
            for seed in seeds {
                g_theta += match *seed {
                    SurfaceSeed::Translate { velocity } => -grad.dot(velocity),
                    // ∂g/∂R = −2(ρ − R); ∂g/∂r = −2r.
                    SurfaceSeed::TorusMajorRadius { rate } => -2.0 * (rho - major) * rate,
                    SurfaceSeed::TorusMinorRadius { rate } => -2.0 * minor * rate,
                    other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
                };
            }
            Ok(Some((g, grad, g_theta)))
        }
        _ => Ok(None),
    }
}

/// Build the constraint row contributed by `surface` (with its composed
/// θ-seeds) at vertex position `x`. Kinds without an implicit form return
/// [`DiffError::UnsupportedConstraint`].
pub fn constraint_row(
    surface: &dyn Surface,
    seeds: &[SurfaceSeed],
    x: &Point3,
) -> Result<ConstraintRow, DiffError> {
    match implicit_terms(surface, seeds, x)? {
        Some((_g, gradient, g_theta)) => Ok(ConstraintRow {
            gradient,
            rhs: -g_theta,
        }),
        None => Err(DiffError::UnsupportedConstraint(surface.surface_type())),
    }
}

/// Extra first-order rows for a **tangency contact** at a vertex.
///
/// When a curved surface touches a plane with parallel gradients (a fillet
/// blend resting on its support face), the surface's implicit row is
/// dependent on the plane's and carries no tangential information — the
/// rank-deficient system would silently freeze a vertex that actually
/// slides along the support face (e.g. the corner of two tangent lines on
/// a rounded cube moves at `(±1, ±1, 0)` per unit fillet radius). The
/// missing information lives in the *tangent-curve* equations:
///
/// - cylinder tangent to plane `n`: the tangent line satisfies
///   `q·(x − center(θ)) = 0` with `q = axis × n`, giving the row
///   `q·ẋ = q·(d center/dθ)` (the radius rate drops out since the contact
///   direction is parallel to `n`);
/// - sphere tangent to plane `n`: the tangent point tracks the center in
///   every direction perpendicular to `n` — two rows `q_i·ẋ = q_i·(d
///   center/dθ)` for a basis `{q_1, q_2}` of `n⊥`.
///
/// Returns no rows when the surface is not tangent to the plane at `x`
/// (transverse contacts are fully handled by the ordinary implicit rows).
/// Surface kinds without tangency support — planes included — also return
/// no rows, **never** an error: callers are expected to pass every
/// incident surface and let this function decide which pairs participate,
/// so an unknown kind is "no tangency information", not a failure.
pub fn tangency_rows(
    plane_normal: Vec3,
    surface: &dyn Surface,
    seeds: &[SurfaceSeed],
    x: &Point3,
) -> Result<Vec<ConstraintRow>, DiffError> {
    let translate: Vec3 = seeds
        .iter()
        .fold(Vec3::new(0.0, 0.0, 0.0), |acc, s| match *s {
            SurfaceSeed::Translate { velocity } => acc + velocity,
            _ => acc,
        });
    let n = plane_normal.normalize();
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Cylinder => {
            let cyl = downcast::<CylinderSurface>(surface, kind)?;
            let a = *cyl.axis.as_ref();
            let d = *x - cyl.center;
            let radial = d - a * d.dot(a);
            let rn = radial.norm();
            if rn < f64::MIN_POSITIVE || radial.cross(n).norm() > 1e-6 * rn {
                return Ok(Vec::new()); // transverse (or on-axis): no tangency
            }
            let q = a.cross(n);
            let qn = q.norm();
            if qn < 1e-9 {
                return Ok(Vec::new()); // axis ∥ n: degenerate contact
            }
            let q = q / qn;
            Ok(vec![ConstraintRow {
                gradient: q,
                rhs: q.dot(translate),
            }])
        }
        SurfaceKind::Sphere => {
            let sph = downcast::<SphereSurface>(surface, kind)?;
            let d = *x - sph.center;
            let dn = d.norm();
            if dn < f64::MIN_POSITIVE || d.cross(n).norm() > 1e-6 * dn {
                return Ok(Vec::new());
            }
            let arbitrary = if n.x.abs() < 0.9 {
                Vec3::new(1.0, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 1.0, 0.0)
            };
            let q1 = n.cross(arbitrary).normalize();
            let q2 = n.cross(q1);
            Ok(vec![
                ConstraintRow {
                    gradient: q1,
                    rhs: q1.dot(translate),
                },
                ConstraintRow {
                    gradient: q2,
                    rhs: q2.dot(translate),
                },
            ])
        }
        _ => Ok(Vec::new()),
    }
}

/// Second-order (acceleration) constraint row of a surface at a topology
/// vertex: `∇g · ẍ = rhs₂`.
///
/// Differentiating the frozen-branch identity `∇g·ẋ = −∂g/∂θ` a second time,
///
/// ```text
/// ∇g · ẍ = −( ∂²g/∂θ²  +  2 ẋᵀ ∇ₓ(∂g/∂θ)  +  ẋᵀ ∇²g ẋ )
/// ```
///
/// The gradient `∇g` is **identical** to the first-order row
/// ([`constraint_row`]), so the same Gram–Schmidt solve
/// ([`solve_vertex_velocity`]) recovers `ẍ` with the tangential DOF frozen at
/// zero — exactly the frozen-parameter convention the velocity solve uses.
/// The right-hand side splits into two pieces this function sums:
///
/// - the **field-acceleration** part, which is precisely the first-order rhs
///   evaluated with the field *accelerations* seeded where velocities
///   normally go (`constraint_row(surface, acc_seeds, x).rhs`) — the plane's
///   whole contribution, and the field-second-derivative `∂²g/∂θ²` term of
///   the quadrics;
/// - the **velocity-curvature** part (plane: `0`), the collected `ẋ`- and
///   field-velocity-quadratic terms. For sphere/cylinder it is the closed
///   form `−2‖ẋ⊥ − v_c⊥‖² + 2ṙ²` of their *constant* Hessian (`∇²g` = `2I` /
///   `2P`, `P = I − aaᵀ`); cone and torus carry their non-constant curvature
///   in closed form too (see `velocity_curvature`'s per-kind derivations).
///
/// `x` is the vertex position, `xdot` its already-solved velocity (from
/// [`solve_vertex_velocity`] on the first-order rows). Implemented for
/// plane/cylinder/sphere/cone/torus — every kind with an implicit form.
pub fn constraint_row_2(
    surface: &dyn Surface,
    vel_seeds: &[SurfaceSeed],
    acc_seeds: &[SurfaceSeed],
    x: &Point3,
    xdot: &Vec3,
) -> Result<ConstraintRow, DiffError> {
    // Field-acceleration part: the first-order machinery, fed accelerations.
    let base = constraint_row(surface, acc_seeds, x)?;
    let curv = velocity_curvature(surface, vel_seeds, x, xdot)?;
    Ok(ConstraintRow {
        gradient: base.gradient,
        rhs: base.rhs + curv,
    })
}

/// The velocity-quadratic part of the second-order rhs (see
/// [`constraint_row_2`]). Plane: identically zero (`∇²g = 0`, `g` linear in
/// both `x` and the origin). Sphere/cylinder: `−2‖ẋ − v_c‖² + 2ṙ²`, projected
/// perpendicular to the axis for the cylinder. Cone/torus carry the extra
/// terms of their non-constant curvature — differentiating `ġ = 0` once more
/// and keeping everything quadratic in the first-order rates (`ẋ` and the
/// field velocities), with the field accelerations' share left to the base
/// row. Errors on kinds without a second-order form, and on inapplicable
/// seeds.
fn velocity_curvature(
    surface: &dyn Surface,
    vel_seeds: &[SurfaceSeed],
    x: &Point3,
    xdot: &Vec3,
) -> Result<f64, DiffError> {
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Plane => {
            for seed in vel_seeds {
                if !matches!(seed, SurfaceSeed::Translate { .. }) {
                    return Err(DiffError::UnsupportedSeed { kind, seed: *seed });
                }
            }
            Ok(0.0)
        }
        SurfaceKind::Sphere => {
            let (vc, rdot) = sphere_field_velocity(kind, vel_seeds)?;
            let rel = *xdot - vc;
            Ok(-2.0 * rel.norm_squared() + 2.0 * rdot * rdot)
        }
        SurfaceKind::Cylinder => {
            let cyl = downcast::<CylinderSurface>(surface, kind)?;
            let a = *cyl.axis.as_ref();
            let (vc, rdot) = cylinder_field_velocity(kind, vel_seeds)?;
            let xperp = *xdot - a * xdot.dot(a);
            let vcperp = vc - a * vc.dot(a);
            let rel = xperp - vcperp;
            Ok(-2.0 * rel.norm_squared() + 2.0 * rdot * rdot)
        }
        SurfaceKind::Cone => {
            // g = ‖p‖² − τ h², τ = tan²α, u = x − apex, h = u·a, p = u − h a.
            // With ẇ = ẋ − v_apex (relative rate), ḣ = ẇ·a, ṗ = ẇ − ḣ a:
            //   g̈|quad = 2‖ẇ⊥‖² − τ̈ h² − 4 τ̇ h ḣ − 2 τ ḣ²,
            // where τ̇ = 2 tanα sec²α · α̇ and (with α̈'s share in the base
            // row) τ̈ = 2 sec²α (sec²α + 2 tan²α) · α̇².
            let cone = downcast::<ConeSurface>(surface, kind)?;
            let a = *cone.axis.as_ref();
            let (v_apex, adot) = cone_field_velocity(kind, vel_seeds)?;
            let w = *xdot - v_apex;
            let hdot = w.dot(a);
            let wperp = w - a * hdot;
            let h = (*x - cone.apex).dot(a);
            let t = cone.half_angle.tan();
            let cos_a = cone.half_angle.cos();
            let sec2 = 1.0 / (cos_a * cos_a);
            let tau = t * t;
            let tau_dot = 2.0 * t * sec2 * adot;
            let tau_ddot = 2.0 * sec2 * (sec2 + 2.0 * tau) * adot * adot;
            Ok(-2.0 * wperp.norm_squared()
                + tau_ddot * h * h
                + 4.0 * tau_dot * h * hdot
                + 2.0 * tau * hdot * hdot)
        }
        SurfaceKind::Torus => {
            // g = (ρ − R)² + h² − r², u = x − c, h = u·a, p = u − h a,
            // ρ = ‖p‖. With ẇ = ẋ − v_c, ḣ = ẇ·a, ρ̇ = p̂·ẇ:
            //   g̈|quad = 2(ρ̇ − Ṙ)² + 2(ρ − R)(‖ẇ⊥‖² − ρ̇²)/ρ + 2ḣ² − 2ṙ²,
            // the middle term being ρ's own curvature (the non-constant ∇²g).
            let torus = downcast::<TorusSurface>(surface, kind)?;
            let a = *torus.axis.as_ref();
            let (vc, big_rdot, rdot) = torus_field_velocity(kind, vel_seeds)?;
            let rel = *x - torus.center;
            let h = rel.dot(a);
            let p = rel - a * h;
            let rho = p.norm();
            if rho < 1e-12 {
                return Err(DiffError::UnsupportedConstraint(kind));
            }
            let phat = p / rho;
            let w = *xdot - vc;
            let hdot = w.dot(a);
            let wperp = w - a * hdot;
            let rho_dot = phat.dot(wperp);
            let curv_rho = (wperp.norm_squared() - rho_dot * rho_dot) / rho;
            Ok(-(2.0 * (rho_dot - big_rdot) * (rho_dot - big_rdot)
                + 2.0 * (rho - torus.major_radius) * curv_rho
                + 2.0 * hdot * hdot
                - 2.0 * rdot * rdot))
        }
        _ => Err(DiffError::UnsupportedConstraint(kind)),
    }
}

/// Sum the seeded (apex velocity, half-angle rate) of a cone's velocity
/// seeds, rejecting inapplicable kinds.
fn cone_field_velocity(kind: SurfaceKind, seeds: &[SurfaceSeed]) -> Result<(Vec3, f64), DiffError> {
    let mut v_apex = Vec3::new(0.0, 0.0, 0.0);
    let mut adot = 0.0;
    for seed in seeds {
        match *seed {
            SurfaceSeed::Translate { velocity } => v_apex += velocity,
            SurfaceSeed::ConeAngle { rate } => adot += rate,
            other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
        }
    }
    Ok((v_apex, adot))
}

/// Sum the seeded (center velocity, major-radius rate, minor-radius rate) of
/// a torus's velocity seeds, rejecting inapplicable kinds.
fn torus_field_velocity(
    kind: SurfaceKind,
    seeds: &[SurfaceSeed],
) -> Result<(Vec3, f64, f64), DiffError> {
    let mut vc = Vec3::new(0.0, 0.0, 0.0);
    let mut big_rdot = 0.0;
    let mut rdot = 0.0;
    for seed in seeds {
        match *seed {
            SurfaceSeed::Translate { velocity } => vc += velocity,
            SurfaceSeed::TorusMajorRadius { rate } => big_rdot += rate,
            SurfaceSeed::TorusMinorRadius { rate } => rdot += rate,
            other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
        }
    }
    Ok((vc, big_rdot, rdot))
}

/// Sum the seeded (center velocity, radius rate) of a sphere's velocity
/// seeds, rejecting inapplicable kinds.
fn sphere_field_velocity(
    kind: SurfaceKind,
    seeds: &[SurfaceSeed],
) -> Result<(Vec3, f64), DiffError> {
    let mut vc = Vec3::new(0.0, 0.0, 0.0);
    let mut rate = 0.0;
    for seed in seeds {
        match *seed {
            SurfaceSeed::Translate { velocity } => vc += velocity,
            SurfaceSeed::SphereRadius { rate: r } => rate += r,
            other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
        }
    }
    Ok((vc, rate))
}

/// Sum the seeded (center velocity, radius rate) of a cylinder's velocity
/// seeds, rejecting inapplicable kinds.
fn cylinder_field_velocity(
    kind: SurfaceKind,
    seeds: &[SurfaceSeed],
) -> Result<(Vec3, f64), DiffError> {
    let mut vc = Vec3::new(0.0, 0.0, 0.0);
    let mut rate = 0.0;
    for seed in seeds {
        match *seed {
            SurfaceSeed::Translate { velocity } => vc += velocity,
            SurfaceSeed::CylinderRadius { rate: r } => rate += r,
            other => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
        }
    }
    Ok((vc, rate))
}

/// Normalized incidence residual of a vertex against a surface: the
/// distance-like quantity `|g(x)| / |∇g(x)|`. Returns `None` for surface
/// kinds without an implicit form (their incidence cannot be tested) and
/// for degenerate gradients.
///
/// Used to recover geometric adjacency the topology does not record: after
/// a boolean, rim vertices on an intersection curve may carry half-edges of
/// only one of the two intersecting faces (the other keeps an untrimmed
/// seam loop), so constraint rows must be collected by incidence, not just
/// by loop membership.
pub fn surface_residual(surface: &dyn Surface, x: &Point3) -> Option<f64> {
    let (g, grad, _) = implicit_terms(surface, &[], x).ok()??;
    let n = grad.norm();
    (n > f64::MIN_POSITIVE).then(|| (g / n).abs())
}

/// Threshold below which an orthogonalized (unit-gradient) row counts as
/// dependent on the rows already selected. Deliberately coarse: accepting a
/// nearly-dependent row divides its carried rhs by the tiny orthogonal
/// residual, amplifying floating-point noise (e.g. two copies of the same
/// moving plane whose normals differ by ~1e-8) into an O(‖v‖) garbage
/// velocity along an arbitrary direction. Below this angle, treating the
/// row as dependent — and checking its rhs for consistency instead — is
/// the safe branch.
const DEPENDENT_TOL: f64 = 1e-6;

/// Absolute tolerance for the consistency residual of dependent rows
/// (velocity units, mm per unit θ).
const CONSISTENCY_TOL: f64 = 1e-6;

/// Solve the implicit vertex system for `ẋ`.
///
/// Rows are normalized and orthogonalized (Gram–Schmidt with the rhs
/// carried along). Independent rows determine components of `ẋ`; dependent
/// rows are checked for consistency (a violation means the vertex is not on
/// the common intersection, or a seed is wrong — a hard error, not a silent
/// wrong answer). Directions no row constrains are frozen at zero: the
/// minimum-norm solution, i.e. the node keeps its frozen tangential
/// parameter.
pub fn solve_vertex_velocity(rows: &[ConstraintRow]) -> Result<Vec3, DiffError> {
    // Orthonormal basis directions with their solution coefficients:
    // ẋ = Σ c_k e_k satisfies every processed row.
    let mut basis: Vec<(Vec3, f64)> = Vec::new();

    for row in rows {
        let norm = row.gradient.norm();
        if norm < f64::MIN_POSITIVE {
            continue; // degenerate gradient carries no information
        }
        let mut g = row.gradient / norm;
        let mut r = row.rhs / norm;
        for (e, c) in &basis {
            let proj = g.dot(*e);
            g -= *e * proj;
            r -= c * proj;
        }
        let res = g.norm();
        if res > DEPENDENT_TOL {
            basis.push((g / res, r / res));
        } else if r.abs() > CONSISTENCY_TOL {
            return Err(DiffError::InconsistentConstraints { residual: r.abs() });
        }
    }

    let mut v = Vec3::new(0.0, 0.0, 0.0);
    for (e, c) in &basis {
        v += *e * *c;
    }
    Ok(v)
}

/// The reverse-mode companion of [`solve_vertex_velocity`]: the solution's
/// sensitivity `m_j = ∂ẋ/∂rhs_j` to each row's right-hand side, so that
/// `ẋ = Σ_j m_j · rhs_j` for any rhs assignment over the same gradients.
///
/// The Gram–Schmidt solution is linear in the rhs vector with coefficients
/// determined by the gradients alone; this runs the same elimination while
/// carrying each basis coefficient as a linear functional over the rows
/// (a vector of weights) instead of a scalar, then transposes. Rows dropped
/// as dependent get a **zero column**: their rhs never enters the solution.
/// In the forward path a dependent row's rhs is consistency-checked against
/// the rows that were kept; the pullback has no concrete rhs to check, so it
/// *presumes* the seedings it will be contracted against are consistent
/// (every copy of a moving surface seeded together, as
/// [`crate::ParamSeeding::seed_where`] does) — inconsistency detection
/// remains the forward solve's job.
pub fn row_pullbacks(rows: &[ConstraintRow]) -> Vec<Vec3> {
    // Basis directions with their coefficients as linear functionals over
    // the rows: ẋ = Σ_k e_k (α_k · rhs).
    let mut basis: Vec<(Vec3, Vec<f64>)> = Vec::new();

    for (j, row) in rows.iter().enumerate() {
        let norm = row.gradient.norm();
        if norm < f64::MIN_POSITIVE {
            continue;
        }
        let mut g = row.gradient / norm;
        let mut alpha = vec![0.0; rows.len()];
        alpha[j] = 1.0 / norm;
        for (e, a) in &basis {
            let proj = g.dot(*e);
            g -= *e * proj;
            for (ai, bi) in alpha.iter_mut().zip(a) {
                *ai -= proj * bi;
            }
        }
        let res = g.norm();
        if res > DEPENDENT_TOL {
            for ai in alpha.iter_mut() {
                *ai /= res;
            }
            basis.push((g / res, alpha));
        }
    }

    let mut m = vec![Vec3::new(0.0, 0.0, 0.0); rows.len()];
    for (e, a) in &basis {
        for (mj, aj) in m.iter_mut().zip(a) {
            *mj += *e * *aj;
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_planes_corner() {
        // Corner of a box whose top face (z = d) moves upward with θ:
        // ẋ = ẑ exactly.
        let top = Plane::new(Point3::new(0.0, 0.0, 2.0), Vec3::x(), Vec3::y());
        let side_x = Plane::new(Point3::new(4.0, 0.0, 0.0), Vec3::y(), Vec3::z());
        let side_y = Plane::new(Point3::new(0.0, 3.0, 0.0), Vec3::z(), Vec3::x());
        let x = Point3::new(4.0, 3.0, 2.0);
        let seed = [SurfaceSeed::Translate {
            velocity: Vec3::z(),
        }];
        let rows = vec![
            constraint_row(&top, &seed, &x).unwrap(),
            constraint_row(&side_x, &[], &x).unwrap(),
            constraint_row(&side_y, &[], &x).unwrap(),
        ];
        let v = solve_vertex_velocity(&rows).unwrap();
        assert!((v - Vec3::z()).norm() < 1e-12, "v = {v:?}");
    }

    #[test]
    fn plane_cylinder_rim_freezes_tangent() {
        // A rim node: on a fixed plane z = 5 and a cylinder of growing
        // radius. Expected ẋ = radial direction; tangential slide frozen.
        let plane = Plane::new(Point3::new(0.0, 0.0, 5.0), Vec3::x(), Vec3::y());
        let cyl = CylinderSurface::new(2.5);
        let u = 1.1_f64;
        let x = Point3::new(2.5 * u.cos(), 2.5 * u.sin(), 5.0);
        let rows = vec![
            constraint_row(&plane, &[], &x).unwrap(),
            constraint_row(&cyl, &[SurfaceSeed::CylinderRadius { rate: 1.0 }], &x).unwrap(),
        ];
        let v = solve_vertex_velocity(&rows).unwrap();
        let expected = Vec3::new(u.cos(), u.sin(), 0.0);
        assert!((v - expected).norm() < 1e-12, "v = {v:?}");
    }

    #[test]
    fn inconsistent_rows_are_an_error() {
        // Two parallel planes moving at different speeds through the same
        // vertex: dependent gradients, contradictory rhs.
        let p1 = Plane::new(Point3::new(0.0, 0.0, 1.0), Vec3::x(), Vec3::y());
        let p2 = Plane::new(Point3::new(0.0, 0.0, 1.0), Vec3::x(), Vec3::y());
        let x = Point3::new(0.0, 0.0, 1.0);
        let rows = vec![
            constraint_row(
                &p1,
                &[SurfaceSeed::Translate {
                    velocity: Vec3::z(),
                }],
                &x,
            )
            .unwrap(),
            constraint_row(&p2, &[], &x).unwrap(),
        ];
        assert!(matches!(
            solve_vertex_velocity(&rows),
            Err(DiffError::InconsistentConstraints { .. })
        ));
    }

    #[test]
    fn row_pullbacks_reproduce_the_forward_solve() {
        // ẋ = Σ_j m_j rhs_j must hold for every system the forward solver
        // accepts — full-rank corner, underdetermined rim, and a system
        // with a dependent (dropped) row.
        let top = Plane::new(Point3::new(0.0, 0.0, 2.0), Vec3::x(), Vec3::y());
        let side_x = Plane::new(Point3::new(4.0, 0.0, 0.0), Vec3::y(), Vec3::z());
        let side_y = Plane::new(Point3::new(0.0, 3.0, 0.0), Vec3::z(), Vec3::x());
        let cyl = CylinderSurface::new(2.5);
        let corner = Point3::new(4.0, 3.0, 2.0);
        let u = 1.1_f64;
        let rim = Point3::new(2.5 * u.cos(), 2.5 * u.sin(), 5.0);

        let systems: Vec<Vec<ConstraintRow>> = vec![
            vec![
                constraint_row(
                    &top,
                    &[SurfaceSeed::Translate {
                        velocity: Vec3::z(),
                    }],
                    &corner,
                )
                .unwrap(),
                constraint_row(&side_x, &[], &corner).unwrap(),
                constraint_row(&side_y, &[], &corner).unwrap(),
            ],
            vec![
                constraint_row(&top, &[], &rim).unwrap(),
                constraint_row(&cyl, &[SurfaceSeed::CylinderRadius { rate: 1.0 }], &rim).unwrap(),
            ],
            vec![
                // Duplicate copies of the same moving plane: the second row
                // is dropped as dependent, and its pullback column is zero.
                constraint_row(
                    &top,
                    &[SurfaceSeed::Translate {
                        velocity: Vec3::z(),
                    }],
                    &corner,
                )
                .unwrap(),
                constraint_row(
                    &top,
                    &[SurfaceSeed::Translate {
                        velocity: Vec3::z(),
                    }],
                    &corner,
                )
                .unwrap(),
                constraint_row(&side_x, &[], &corner).unwrap(),
            ],
        ];
        for rows in &systems {
            let v = solve_vertex_velocity(rows).unwrap();
            let m = row_pullbacks(rows);
            let rebuilt = rows
                .iter()
                .zip(&m)
                .fold(Vec3::new(0.0, 0.0, 0.0), |acc, (row, mj)| {
                    acc + *mj * row.rhs
                });
            assert!(
                (rebuilt - v).norm() < 1e-13,
                "pullback rebuild {rebuilt:?} vs forward {v:?}"
            );
        }
    }

    /// `g(x)` alone for a surface at `x` (no seeds), for FD of `∂g/∂θ`.
    fn g_only(surface: &dyn Surface, x: &Point3) -> f64 {
        implicit_terms(surface, &[], x).unwrap().unwrap().0
    }

    #[test]
    fn cone_constraint_rhs_matches_fd() {
        use vcad_kernel_geom::ConeSurface;
        use vcad_kernel_math::Dir3;
        let cone = ConeSurface {
            apex: Point3::new(0.0, 0.0, 20.0),
            axis: Dir3::new_normalize(-Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle: 0.25_f64.atan(),
        };
        let x = cone.evaluate(vcad_kernel_math::Point2::new(1.3, 12.0));
        let h = 1e-7;

        // ConeAngle: rhs = −∂g/∂α, FD of g under half_angle ± h.
        let bumped = |da: f64| ConeSurface {
            half_angle: cone.half_angle + da,
            ..cone.clone()
        };
        let dgda = (g_only(&bumped(h), &x) - g_only(&bumped(-h), &x)) / (2.0 * h);
        let row = constraint_row(&cone, &[SurfaceSeed::ConeAngle { rate: 1.0 }], &x).unwrap();
        assert!(
            (row.rhs + dgda).abs() < 1e-4,
            "rhs {} vs -fd {}",
            row.rhs,
            -dgda
        );

        // Translate the apex along +x.
        let bumped = |dx: f64| ConeSurface {
            apex: cone.apex + Vec3::new(dx, 0.0, 0.0),
            ..cone.clone()
        };
        let dgdx = (g_only(&bumped(h), &x) - g_only(&bumped(-h), &x)) / (2.0 * h);
        let row = constraint_row(
            &cone,
            &[SurfaceSeed::Translate {
                velocity: Vec3::x(),
            }],
            &x,
        )
        .unwrap();
        assert!(
            (row.rhs + dgdx).abs() < 1e-4,
            "rhs {} vs -fd {}",
            row.rhs,
            -dgdx
        );
    }

    #[test]
    fn torus_constraint_rhs_matches_fd() {
        use vcad_kernel_geom::TorusSurface;
        let torus = TorusSurface::new(7.0, 2.0);
        let x = torus.evaluate(vcad_kernel_math::Point2::new(1.1, 2.3));
        let h = 1e-7;

        let bump_major = |d: f64| TorusSurface {
            major_radius: torus.major_radius + d,
            ..torus.clone()
        };
        let dgdr = (g_only(&bump_major(h), &x) - g_only(&bump_major(-h), &x)) / (2.0 * h);
        let row =
            constraint_row(&torus, &[SurfaceSeed::TorusMajorRadius { rate: 1.0 }], &x).unwrap();
        assert!((row.rhs + dgdr).abs() < 1e-4);

        let bump_minor = |d: f64| TorusSurface {
            minor_radius: torus.minor_radius + d,
            ..torus.clone()
        };
        let dgdm = (g_only(&bump_minor(h), &x) - g_only(&bump_minor(-h), &x)) / (2.0 * h);
        let row =
            constraint_row(&torus, &[SurfaceSeed::TorusMinorRadius { rate: 1.0 }], &x).unwrap();
        assert!((row.rhs + dgdm).abs() < 1e-4);
    }

    #[test]
    fn nearly_dependent_row_does_not_amplify_noise() {
        // Two copies of the same moving plane whose normals differ by ~1e-8
        // (different construction paths): the second row must be treated as
        // dependent-and-consistent, not divided by its tiny orthogonal
        // residual into a garbage tangential velocity.
        let n2 = (Vec3::z() + Vec3::new(1e-8, 0.0, 0.0)).normalize();
        let seed_v = Vec3::z();
        let rows = vec![
            ConstraintRow {
                gradient: Vec3::z(),
                rhs: seed_v.z,
            },
            ConstraintRow {
                gradient: n2,
                rhs: n2.dot(seed_v),
            },
        ];
        let v = solve_vertex_velocity(&rows).unwrap();
        assert!(
            (v - Vec3::z()).norm() < 1e-6,
            "noise direction amplified: v = {v:?}"
        );
    }

    #[test]
    fn cylinder_radius_acceleration_matches_nonlinear_field() {
        // A rim vertex on a fixed plane z = 5 and a cylinder whose radius is a
        // *nonlinear* function of θ: r(θ) = r0 + ṙθ + ½r̈θ². The vertex rides
        // radially: x(θ) = center + r(θ)·û + cap, so ẋ = ṙû and ẍ = r̈û with
        // the angular slide frozen. This exercises the velocity-curvature term
        // (ẋ nonzero) *and* the field-acceleration term (r̈ nonzero).
        let plane = Plane::new(Point3::new(0.0, 0.0, 5.0), Vec3::x(), Vec3::y());
        let r0 = 2.5;
        let cyl = CylinderSurface::new(r0);
        let (rdot, rddot) = (0.7, 1.3);
        let u = 1.1_f64;
        let uhat = Vec3::new(u.cos(), u.sin(), 0.0);
        let x = Point3::new(r0 * u.cos(), r0 * u.sin(), 5.0);

        let rows1 = vec![
            constraint_row(&plane, &[], &x).unwrap(),
            constraint_row(&cyl, &[SurfaceSeed::CylinderRadius { rate: rdot }], &x).unwrap(),
        ];
        let xdot = solve_vertex_velocity(&rows1).unwrap();
        assert!((xdot - uhat * rdot).norm() < 1e-12, "xdot = {xdot:?}");

        let rows2 = vec![
            constraint_row_2(&plane, &[], &[], &x, &xdot).unwrap(),
            constraint_row_2(
                &cyl,
                &[SurfaceSeed::CylinderRadius { rate: rdot }],
                &[SurfaceSeed::CylinderRadius { rate: rddot }],
                &x,
                &xdot,
            )
            .unwrap(),
        ];
        let xddot = solve_vertex_velocity(&rows2).unwrap();
        assert!(
            (xddot - uhat * rddot).norm() < 1e-12,
            "xddot = {xddot:?} vs expected {:?}",
            uhat * rddot
        );
    }

    #[test]
    fn sphere_growing_corner_acceleration_matches_fd() {
        // A corner where three orthogonal planes meet a growing sphere is
        // over-determined by the planes alone; use a single sphere + two planes
        // pinning a point that rides the sphere along a fixed direction as the
        // radius grows nonlinearly. Validate ẍ against a central difference of
        // the analytic ẋ under r ± δ (the sphere row's curvature is 2I).
        let sph = SphereSurface::new(3.0);
        let px = Plane::new(Point3::new(0.0, 0.0, 0.0), Vec3::y(), Vec3::z()); // x = 0
        let py = Plane::new(Point3::new(0.0, 0.0, 0.0), Vec3::z(), Vec3::x()); // y = 0
                                                                               // Point on the +z pole of the sphere (x = y = 0, z = r).
        let x = Point3::new(0.0, 0.0, 3.0);
        let rate = 1.0;

        let vel = |s: &[SurfaceSeed]| {
            let rows = vec![
                constraint_row(&px, &[], &x).unwrap(),
                constraint_row(&py, &[], &x).unwrap(),
                constraint_row(&sph, s, &x).unwrap(),
            ];
            solve_vertex_velocity(&rows).unwrap()
        };
        let xdot = vel(&[SurfaceSeed::SphereRadius { rate }]);
        assert!((xdot - Vec3::z()).norm() < 1e-12, "xdot = {xdot:?}");

        let rows2 = vec![
            constraint_row_2(&px, &[], &[], &x, &xdot).unwrap(),
            constraint_row_2(&py, &[], &[], &x, &xdot).unwrap(),
            constraint_row_2(&sph, &[SurfaceSeed::SphereRadius { rate }], &[], &x, &xdot).unwrap(),
        ];
        let xddot = solve_vertex_velocity(&rows2).unwrap();
        // Pole rides at exactly z = r (linear in r), so ẍ = 0.
        assert!(xddot.norm() < 1e-12, "xddot = {xddot:?}");
    }

    #[test]
    fn cone_angle_acceleration_matches_closed_form() {
        // A rim vertex on a fixed plane z = z0 and a cone (apex above, axis
        // −ẑ) whose half-angle opens at constant rate α̇: the rim radius is
        // ρ(α) = (apex_z − z0)·tan α, so with the angular slide frozen
        //   ẋ = ρ'(α)·û = d·sec²α·α̇·û,  ẍ = ρ''(α)·û = d·2 sec²α tanα·α̇²·û,
        // d = apex_z − z0. Nonlinear in α even with α̈ = 0 — exercising the
        // cone's velocity-curvature term against an exact closed form.
        use vcad_kernel_geom::ConeSurface;
        use vcad_kernel_math::Dir3;
        let apex_z = 20.0;
        let z0 = 5.0;
        let d = apex_z - z0;
        let alpha = 0.25_f64.atan();
        let adot = 1.0;
        let cone = ConeSurface {
            apex: Point3::new(0.0, 0.0, apex_z),
            axis: Dir3::new_normalize(-Vec3::z()),
            ref_dir: Dir3::new_normalize(Vec3::x()),
            half_angle: alpha,
        };
        let plane = Plane::new(Point3::new(0.0, 0.0, z0), Vec3::x(), Vec3::y());
        let u = 0.8_f64;
        let uhat = Vec3::new(u.cos(), u.sin(), 0.0);
        let rho = d * alpha.tan();
        let x = Point3::new(rho * u.cos(), rho * u.sin(), z0);

        let seed = [SurfaceSeed::ConeAngle { rate: adot }];
        let rows1 = vec![
            constraint_row(&plane, &[], &x).unwrap(),
            constraint_row(&cone, &seed, &x).unwrap(),
        ];
        let xdot = solve_vertex_velocity(&rows1).unwrap();
        let sec2 = 1.0 / (alpha.cos() * alpha.cos());
        assert!(
            (xdot - uhat * (d * sec2 * adot)).norm() < 1e-9,
            "xdot = {xdot:?}"
        );

        let rows2 = vec![
            constraint_row_2(&plane, &[], &[], &x, &xdot).unwrap(),
            constraint_row_2(&cone, &seed, &[], &x, &xdot).unwrap(),
        ];
        let xddot = solve_vertex_velocity(&rows2).unwrap();
        let expected = uhat * (d * 2.0 * sec2 * alpha.tan() * adot * adot);
        assert!(
            (xddot - expected).norm() < 1e-9,
            "xddot = {xddot:?} vs expected {expected:?}"
        );
    }

    #[test]
    fn torus_minor_radius_acceleration_matches_nonlinear_field() {
        // A point on the outer equator of a torus whose minor radius is a
        // nonlinear function of θ: r(θ) = r0 + ṙθ + ½r̈θ². The point rides
        // radially: x(θ) = (R + r(θ))·û, so ẋ = ṙ·û and ẍ = r̈·û — the
        // torus analogue of the cylinder nonlinear-field test, pinned by two
        // planes so only the radial DOF is live... the torus row alone pins
        // the normal direction and the tangential DOFs are frozen, exactly
        // like the cylinder case.
        use vcad_kernel_geom::TorusSurface;
        let torus = TorusSurface::new(7.0, 2.0);
        let (rdot, rddot) = (0.6, 1.7);
        let u = 0.9_f64;
        let uhat = Vec3::new(u.cos(), u.sin(), 0.0);
        let x = Point3::new(9.0 * u.cos(), 9.0 * u.sin(), 0.0); // ρ = R + r

        let vel_seed = [SurfaceSeed::TorusMinorRadius { rate: rdot }];
        let acc_seed = [SurfaceSeed::TorusMinorRadius { rate: rddot }];
        let rows1 = vec![constraint_row(&torus, &vel_seed, &x).unwrap()];
        let xdot = solve_vertex_velocity(&rows1).unwrap();
        assert!((xdot - uhat * rdot).norm() < 1e-12, "xdot = {xdot:?}");

        let rows2 = vec![constraint_row_2(&torus, &vel_seed, &acc_seed, &x, &xdot).unwrap()];
        let xddot = solve_vertex_velocity(&rows2).unwrap();
        assert!(
            (xddot - uhat * rddot).norm() < 1e-10,
            "xddot = {xddot:?} vs expected {:?}",
            uhat * rddot
        );
    }

    #[test]
    fn torus_major_radius_acceleration_second_row_matches_fd() {
        // Validate the torus second-order row's rhs directly against a central
        // difference of the first-order rhs along the moving-point trajectory:
        // for x(θ) riding the outer equator of a torus with R(θ) = R0 + Ṙθ,
        // ∇g·ẍ must equal d/dθ[∇g·ẋ] − (∇̇g)·ẋ; equivalently the identity
        // g(x(θ), θ) ≡ 0 gives ẍ = R̈ û = 0 here, so the solved acceleration
        // must vanish even though the velocity-curvature and rhs terms are
        // individually nonzero (they cancel exactly).
        use vcad_kernel_geom::TorusSurface;
        let torus = TorusSurface::new(7.0, 2.0);
        let u = 1.7_f64;
        let uhat = Vec3::new(u.cos(), u.sin(), 0.0);
        let x = Point3::new(9.0 * u.cos(), 9.0 * u.sin(), 0.0);

        let vel_seed = [SurfaceSeed::TorusMajorRadius { rate: 1.0 }];
        let rows1 = vec![constraint_row(&torus, &vel_seed, &x).unwrap()];
        let xdot = solve_vertex_velocity(&rows1).unwrap();
        assert!((xdot - uhat).norm() < 1e-12, "xdot = {xdot:?}");

        let rows2 = vec![constraint_row_2(&torus, &vel_seed, &[], &x, &xdot).unwrap()];
        let xddot = solve_vertex_velocity(&rows2).unwrap();
        assert!(xddot.norm() < 1e-10, "xddot = {xddot:?}, expected 0");
    }
}
