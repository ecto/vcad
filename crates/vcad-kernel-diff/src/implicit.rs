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

use crate::{DiffError, SurfaceSeed};

/// One row of the implicit vertex system: `gradient · ẋ = rhs`.
#[derive(Debug, Clone, Copy)]
pub struct ConstraintRow {
    /// ∇g at the vertex.
    pub gradient: Vec3,
    /// −∂g/∂θ at the vertex.
    pub rhs: f64,
}

fn downcast<T: 'static>(surface: &dyn Surface, kind: SurfaceKind) -> Result<&T, DiffError> {
    surface
        .as_any()
        .downcast_ref::<T>()
        .ok_or(DiffError::DowncastFailed(kind))
}

/// Build the constraint row contributed by `surface` (with its θ-seed) at
/// vertex position `x`.
///
/// Implicit forms: plane `g = n·(x − o)`; cylinder `g = |radial|² − r²`;
/// sphere `g = |x − c|² − r²`. Other kinds have no implicit form yet and
/// return [`DiffError::UnsupportedConstraint`].
pub fn constraint_row(
    surface: &dyn Surface,
    seed: Option<SurfaceSeed>,
    x: &Point3,
) -> Result<ConstraintRow, DiffError> {
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Plane => {
            let plane = downcast::<Plane>(surface, kind)?;
            let n = *plane.normal_dir.as_ref();
            let g_theta = match seed {
                None => 0.0,
                Some(SurfaceSeed::Translate { velocity }) => -n.dot(velocity),
                Some(other) => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
            };
            Ok(ConstraintRow {
                gradient: n,
                rhs: -g_theta,
            })
        }
        SurfaceKind::Cylinder => {
            let cyl = downcast::<CylinderSurface>(surface, kind)?;
            let a = *cyl.axis.as_ref();
            let d = *x - cyl.center;
            let radial = d - a * d.dot(a);
            let g_theta = match seed {
                None => 0.0,
                Some(SurfaceSeed::CylinderRadius { rate }) => -2.0 * cyl.radius * rate,
                Some(SurfaceSeed::Translate { velocity }) => {
                    // ∂g/∂θ = −2 radial · v_perp; radial ⊥ a so the axial
                    // component of v drops out automatically.
                    -2.0 * radial.dot(velocity)
                }
                Some(other) => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
            };
            Ok(ConstraintRow {
                gradient: 2.0 * radial,
                rhs: -g_theta,
            })
        }
        SurfaceKind::Sphere => {
            let sph = downcast::<SphereSurface>(surface, kind)?;
            let d = *x - sph.center;
            let g_theta = match seed {
                None => 0.0,
                Some(SurfaceSeed::SphereRadius { rate }) => -2.0 * sph.radius * rate,
                Some(SurfaceSeed::Translate { velocity }) => -2.0 * d.dot(velocity),
                Some(other) => return Err(DiffError::UnsupportedSeed { kind, seed: other }),
            };
            Ok(ConstraintRow {
                gradient: 2.0 * d,
                rhs: -g_theta,
            })
        }
        other => Err(DiffError::UnsupportedConstraint(other)),
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
    let kind = surface.surface_type();
    match kind {
        SurfaceKind::Plane => {
            let plane = surface.as_any().downcast_ref::<Plane>()?;
            Some(plane.signed_distance(x).abs())
        }
        SurfaceKind::Cylinder => {
            let cyl = surface.as_any().downcast_ref::<CylinderSurface>()?;
            let a = *cyl.axis.as_ref();
            let d = *x - cyl.center;
            let radial = d - a * d.dot(a);
            let g = radial.norm_squared() - cyl.radius * cyl.radius;
            let grad = 2.0 * radial.norm();
            (grad > f64::MIN_POSITIVE).then(|| (g / grad).abs())
        }
        SurfaceKind::Sphere => {
            let sph = surface.as_any().downcast_ref::<SphereSurface>()?;
            let d = *x - sph.center;
            let g = d.norm_squared() - sph.radius * sph.radius;
            let grad = 2.0 * d.norm();
            (grad > f64::MIN_POSITIVE).then(|| (g / grad).abs())
        }
        _ => None,
    }
}

/// Relative threshold below which an orthogonalized gradient counts as
/// dependent on the rows already selected.
const DEPENDENT_TOL: f64 = 1e-9;

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
        let seed = Some(SurfaceSeed::Translate {
            velocity: Vec3::z(),
        });
        let rows = vec![
            constraint_row(&top, seed, &x).unwrap(),
            constraint_row(&side_x, None, &x).unwrap(),
            constraint_row(&side_y, None, &x).unwrap(),
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
            constraint_row(&plane, None, &x).unwrap(),
            constraint_row(&cyl, Some(SurfaceSeed::CylinderRadius { rate: 1.0 }), &x).unwrap(),
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
                Some(SurfaceSeed::Translate {
                    velocity: Vec3::z(),
                }),
                &x,
            )
            .unwrap(),
            constraint_row(&p2, None, &x).unwrap(),
        ];
        assert!(matches!(
            solve_vertex_velocity(&rows),
            Err(DiffError::InconsistentConstraints { .. })
        ));
    }
}
