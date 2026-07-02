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

use vcad_kernel_geom::{CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind};
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
/// solved can never disagree. Implicit forms: plane `g = n·(x − o)`;
/// cylinder `g = |radial|² − r²`; sphere `g = |x − c|² − r²`. Returns
/// `Ok(None)` for kinds without an implicit form.
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
}
