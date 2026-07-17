//! Gradients of the input impedance with respect to geometry — the
//! adjoint route, priced by the symmetry the Galerkin fill guarantees.
//!
//! # The identity
//!
//! With a delta-gap drive, `Z I = V = V₀ e_f` and `Z_in = V₀ / I_f`. For
//! any parameter `p` moving the geometry,
//!
//! ```text
//! dZ_in/dp = −(V₀/I_f²) dI_f/dp = (V₀/I_f²) e_fᵀ Z⁻¹ (∂Z/∂p) I
//! ```
//!
//! The adjoint system is `Zᵀ λ = e_f`; the fill keeps `Z` **exactly
//! symmetric** (that was the point), so `λ = Z⁻¹ e_f = I / V₀` — the
//! adjoint currents *are* the forward currents, by reciprocity. The
//! gradient collapses to the classic variational form
//!
//! ```text
//! dZ_in/dp = Iᵀ (∂Z/∂p) I / I_f²
//! ```
//!
//! — one solve total, any number of parameters, each costing two matrix
//! fills for `∂Z/∂p`. Note the **unconjugated** bilinear product: this is
//! reciprocity's transpose, not an inner product.
//!
//! # ∂Z/∂p, and what "frozen" means
//!
//! `∂Z/∂p` is evaluated by central differences **on the matrix fill
//! only** — never through the LU solve, whose conditioning near
//! (anti-)resonance is exactly where gradients are wanted. The perturbed
//! meshes share the unperturbed topology and basis structure *by
//! construction* ([`perturbed_mesh`] moves node coordinates and recomputes
//! segment frames; it never re-segments). This is the hidden-parameter
//! lesson from the particle crate's M2: differencing across probes is
//! only meaningful when the discretization does not move underneath —
//! here the freeze is structural, not procedural. The step is scaled to
//! the mesh (`1e−6` of the mean segment length per unit velocity), far
//! below any physics scale and far above f64 noise for these smooth
//! integrals.
//!
//! An analytic/AD fill derivative can replace the FD fill later without
//! touching callers — the differentiable-seam route — but the adjoint
//! structure above is already exact.

use crate::complex::Complex;
use crate::error::AntennaError;
use crate::geometry::{Mesh, Segment};
use crate::linalg::CMatrix;
use crate::mom::{fill_impedance_matrix, solve_driven, DrivenSolution, SolveOptions};

/// A geometry parameter, given as the velocity of every mesh node per
/// unit parameter (meters per unit `p`). Radii are held fixed.
#[derive(Debug, Clone)]
pub struct ParamVelocity {
    /// Per-node velocity, m per unit parameter.
    pub node_velocity_m: Vec<[f64; 3]>,
}

impl ParamVelocity {
    /// Build from a function of node position (meters in, m/unit-p out).
    pub fn from_fn(mesh: &Mesh, f: impl Fn([f64; 3]) -> [f64; 3]) -> Self {
        ParamVelocity {
            node_velocity_m: mesh.nodes.iter().map(|&n| f(n)).collect(),
        }
    }

    /// Rigid translation along `dir` (unit velocity for every node).
    pub fn translation(mesh: &Mesh, dir: [f64; 3]) -> Self {
        Self::from_fn(mesh, |_| dir)
    }

    fn max_speed(&self) -> f64 {
        self.node_velocity_m
            .iter()
            .map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
            .fold(0.0, f64::max)
    }
}

/// The mesh with nodes displaced by `t · velocity`, topology and basis
/// structure untouched (segment frames recomputed from the moved nodes).
pub fn perturbed_mesh(mesh: &Mesh, vel: &ParamVelocity, t: f64) -> Mesh {
    let mut out = mesh.clone();
    for (n, v) in out.nodes.iter_mut().zip(&vel.node_velocity_m) {
        n[0] += t * v[0];
        n[1] += t * v[1];
        n[2] += t * v[2];
    }
    for s in out.segments.iter_mut() {
        let p0 = out.nodes[s.n0];
        let p1 = out.nodes[s.n1];
        let d = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        *s = Segment {
            p0,
            tangent: [d[0] / len, d[1] / len, d[2] / len],
            len,
            ..*s
        };
    }
    out
}

/// Forward solution plus `dZ_in/dp` for each parameter.
#[derive(Debug, Clone)]
pub struct GradientResult {
    /// The forward (and, by reciprocity, adjoint) solution.
    pub solution: DrivenSolution,
    /// `dZ_in/dp` per parameter, Ω per unit parameter.
    pub dz_dp: Vec<Complex>,
}

/// Solve once, then price `dZ_in/dp` for every parameter via the adjoint
/// identity (see module docs). Fails closed if a parameter would move a
/// grounded node off the plane (the image structure would change
/// discontinuously).
pub fn z_in_gradient(
    mesh: &Mesh,
    feed_basis: usize,
    freq_hz: f64,
    params: &[ParamVelocity],
    opts: &SolveOptions,
) -> Result<GradientResult, AntennaError> {
    let solution = solve_driven(mesh, feed_basis, freq_hz, opts)?;
    let i = &solution.currents;
    let i_f = solution.currents[feed_basis];
    let i_f2 = i_f * i_f;

    let l_char = mesh.segments.iter().map(|s| s.len).sum::<f64>() / mesh.segments.len() as f64;

    let mut dz_dp = Vec::with_capacity(params.len());
    for vel in params {
        if vel.node_velocity_m.len() != mesh.nodes.len() {
            return Err(AntennaError::ParamVelocityMismatch {
                nodes: mesh.nodes.len(),
                velocities: vel.node_velocity_m.len(),
            });
        }
        if mesh.ground_plane {
            for (ni, (n, v)) in mesh.nodes.iter().zip(&vel.node_velocity_m).enumerate() {
                if n[2] == 0.0 && v[2] != 0.0 {
                    return Err(AntennaError::GroundedNodeMoved { node: ni });
                }
            }
        }
        let speed = vel.max_speed();
        if speed == 0.0 {
            dz_dp.push(Complex::ZERO);
            continue;
        }
        let h = 1e-6 * l_char / speed;
        let z_plus = fill_impedance_matrix(&perturbed_mesh(mesh, vel, h), solution.k, opts);
        let z_minus = fill_impedance_matrix(&perturbed_mesh(mesh, vel, -h), solution.k, opts);
        let dz = contract(&z_plus, &z_minus, i, 0.5 / h);
        dz_dp.push(dz / i_f2);
    }

    Ok(GradientResult { solution, dz_dp })
}

/// `Iᵀ [(Z₊ − Z₋)·scale] I` — the unconjugated bilinear contraction.
fn contract(z_plus: &CMatrix, z_minus: &CMatrix, i: &[Complex], scale: f64) -> Complex {
    let n = i.len();
    let mut acc = Complex::ZERO;
    for a in 0..n {
        let ia = i[a];
        for (b, &ib) in i.iter().enumerate() {
            let dz = (z_plus.at(a, b) - z_minus.at(a, b)).scale(scale);
            acc += ia * dz * ib;
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::WireGrid;

    #[test]
    fn rigid_translation_has_zero_gradient() {
        // Free space is translation-invariant: moving the whole antenna
        // must not change Z_in. A strong end-to-end check of the
        // contraction machinery (any bookkeeping slip shows up as a
        // spurious gradient).
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 16)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let opts = SolveOptions::default();
        let params = vec![
            ParamVelocity::translation(&mesh, [1.0, 0.0, 0.0]),
            ParamVelocity::translation(&mesh, [0.0, 0.7, 0.7]),
        ];
        let res = z_in_gradient(&mesh, feed, 143.6e6, &params, &opts).unwrap();
        // Scale: a real geometry gradient (arm stretch) is ~1e3 Ω/m here.
        for dz in &res.dz_dp {
            assert!(
                dz.abs() < 1e-3,
                "translation gradient should vanish, got {dz:?} Ω/m"
            );
        }
    }

    #[test]
    fn grounded_node_velocity_fails_closed() {
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 500.0], 1.0, 8)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let bad = ParamVelocity::translation(&mesh, [0.0, 0.0, 1.0]);
        match z_in_gradient(&mesh, feed, 143e6, &[bad], &SolveOptions::default()) {
            Err(AntennaError::GroundedNodeMoved { .. }) => {}
            other => panic!("expected grounded-node error, got {other:?}"),
        }
    }
}
