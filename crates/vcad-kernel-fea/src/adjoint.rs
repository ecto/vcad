//! Discrete shape adjoint: `dJ/d(node coordinates)` for the static solve.
//!
//! This is the gradient half of the structural seam. The forward solve
//! answers *will it break*; this answers *which way do I move the
//! geometry to fix it*, at the cost of **one extra linear solve** — the
//! stiffness matrix is symmetric, so the adjoint system reuses the same
//! operator and the same PCG as the forward pass.
//!
//! # What is differentiated, and what is not
//!
//! The parameter enters through **node coordinates of a frozen
//! discretization**. Given `K(x)·u = f`, for an objective `J(u, x)`:
//!
//! ```text
//! K·λ = ∂J/∂u,      dJ/dx = ∂J/∂x|_u − λᵀ·(∂K/∂x)·u
//! ```
//!
//! Everything discrete is held fixed: the tet connectivity, which nodes a
//! load or support region selects, and which elements exist at all. That
//! is a deliberate scope choice, not an oversight —
//!
//! - **The lattice fill is not differentiable.** [`crate::mesh::tet_fill`]
//!   re-voxelizes for each geometry: a parameter sweep pops whole tets in
//!   and out of existence, and `J` is a step function of the parameter at
//!   those events. Re-meshing per optimizer step and finite-differencing
//!   across it measures the staircase, not the physics. A consumer must
//!   therefore mesh **once** and feed node velocities `dx/dθ` for the
//!   parameter of interest — which is exactly what `vcad-kernel-diff`'s
//!   frozen-tessellation seam produces.
//! - **Region membership is discrete.** A parameter that drags a node
//!   across a load or support region boundary changes the model. Pad
//!   regions clear of the nodes they mean to catch (see
//!   [`crate::solve`]'s `assemble_bc`).
//! - **Loads are prescribed nodal forces**, so `∂f/∂x = 0`. A future
//!   pressure or body load would make that term live and it would have to
//!   be added here.
//!
//! # The element shape derivatives
//!
//! For a linear tet with barycentric gradients `g_i` and volume `V`, both
//! derivatives are closed-form and pleasantly symmetric:
//!
//! ```text
//! ∂g_{m,p} / ∂x_{k,j}  =  − g_{k,p} · g_{m,j}
//! ∂V       / ∂x_{k,j}  =    V · g_{k,j}
//! ```
//!
//! (both valid for all four nodes `k`, including node 0, via `Σ_i g_i = 0`).
//! Writing `H_v = Σ_i v_i ⊗ g_i` for the gradient of a nodal field and
//! `σ(·)` for the unit-E isotropic stress operator, the element's
//! contribution to `λᵀK u` is `V · σ(H_u) : H_λ`, whose derivative is
//!
//! ```text
//! ∂(λᵀK_e u)/∂x_{k,j} = V·g_{k,j}·(σ(H_u):H_λ)
//!                       − V·[ H_u[:,j] · (σ(H_λ)·g_k)
//!                           + H_λ[:,j] · (σ(H_u)·g_k) ]
//! ```
//!
//! so the whole per-node gradient costs one pass over the elements — no
//! assembled matrix, no per-parameter re-solve.
//!
//! # Objectives
//!
//! [`Qoi::Compliance`] is self-adjoint (`λ = u/E`) and skips the adjoint
//! solve. [`Qoi::MeanDisplacement`] is a linear functional. Both have
//! `∂J/∂x|_u = 0`.
//!
//! [`Qoi::SmoothMaxVonMises`] needs the treatment
//! `vcad-kernel-thermal` uses for peak temperature: a hard max is not
//! differentiable — at a tie the gradient jumps between elements and an
//! optimizer chasing it chatters — so it is smoothed to a p-norm,
//!
//! ```text
//! J = τ + ( Σ_e (vm_e − τ)₊ᵖ )^(1/p)
//! max ≤ J ≤ τ + (max − τ)·N_active^(1/p)
//! ```
//!
//! computed normalized by the peak excess so no `p` overflows. Both `J`
//! and the hard max are reported so the bracket is checkable. This
//! objective *does* carry an explicit `∂J/∂x|_u` term, because moving
//! nodes changes the strain read from a fixed displacement field.
//!
//! **The threshold τ is what makes this usable as a constraint.** With
//! τ = 0 every stressed element is active, so on a fine mesh the bracket
//! factor is `N^(1/p)` over the *whole* element count — on an 80-cell
//! cantilever, p = 8 reads 150 MPa against a true peak of 72. Setting τ
//! near the stress that actually matters (a fraction of yield) drops
//! `N_active` by orders of magnitude and pulls `J` down onto the peak;
//! the same p = 8 with τ = 55 MPa reads within a few percent. Optimizing
//! against an unthresholded p-norm is not wrong, but it is answering a
//! question about the whole stress field rather than about the peak.
//!
//! The p-norm is unweighted, which is exact for the uniform Kuhn lattice
//! (all tets share one volume) and would need volume weights on a graded
//! mesh — noted for the boundary-conforming upgrade.

use crate::mesh::TetMesh;
use crate::solve::{
    assemble_bc, build_elements, dot, grad_of, lame, norm, pcg, stress_of, summarize, von_mises,
    Elem, Solution, SolveError, SolveOptions,
};
use crate::spec::{FeaSpec, RegionBox};

/// Objective to differentiate.
#[derive(Debug, Clone, PartialEq)]
pub enum Qoi {
    /// Compliance `fᵀu` (N·mm) — global stiffness. Lower is stiffer.
    /// Self-adjoint: no second solve.
    Compliance,
    /// Mean displacement of the nodes in `region`, along `direction`
    /// (mm). `direction` is normalized; the sign is meaningful (a
    /// downward load with a `+z` direction gives a negative value).
    MeanDisplacement {
        /// Nodes to average over. Fail-closed if it selects none.
        region: RegionBox,
        /// Direction to project onto (normalized internally).
        direction: [f64; 3],
    },
    /// Smoothed maximum element von Mises stress (MPa), as a p-norm of
    /// the stress *excess* over a threshold.
    SmoothMaxVonMises {
        /// Exponent `p > 1`. Larger sharpens the bracket around the hard
        /// max and concentrates the gradient on fewer elements.
        p: f64,
        /// Only stress above this (MPa) enters the norm. `None` means
        /// zero — every stressed element counts, which on a fine mesh
        /// makes `n_active` the whole element count and the bracket
        /// correspondingly loose. Set it near the stress you actually
        /// care about (a fraction of yield) to tighten the bracket by
        /// orders of magnitude; see [`ShapeGradient::hard_max_mpa`].
        threshold_mpa: Option<f64>,
    },
}

/// Gradient failures.
#[derive(Debug, Clone, PartialEq)]
pub enum GradError {
    /// The forward or adjoint solve failed.
    Solve(SolveError),
    /// The smoothing exponent was not finite and > 1.
    InvalidSmoothingExponent(f64),
    /// The stress threshold was not finite and non-negative.
    InvalidThreshold(f64),
    /// A QoI region selected no mesh node (fail-closed).
    EmptyQoiRegion,
    /// The QoI direction was zero-length.
    ZeroDirection,
    /// The stress field is identically zero, so the smoothed max has no
    /// defined gradient (fail-closed rather than reporting zeros).
    ZeroStressField,
}

impl std::fmt::Display for GradError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GradError::Solve(e) => write!(f, "{e}"),
            GradError::InvalidSmoothingExponent(p) => {
                write!(f, "smoothing exponent must be finite and > 1, got {p}")
            }
            GradError::InvalidThreshold(t) => {
                write!(f, "stress threshold must be finite and >= 0, got {t}")
            }
            GradError::EmptyQoiRegion => write!(
                f,
                "objective region selected no mesh node — check its coordinates against the \
                 part's bounding box (fail-closed)"
            ),
            GradError::ZeroDirection => write!(f, "objective direction is zero-length"),
            GradError::ZeroStressField => write!(
                f,
                "stress field is identically zero — the smoothed max has no gradient here"
            ),
        }
    }
}

impl std::error::Error for GradError {}

impl From<SolveError> for GradError {
    fn from(e: SolveError) -> Self {
        GradError::Solve(e)
    }
}

/// A solved objective and its gradient with respect to every node
/// coordinate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShapeGradient {
    /// Objective value, in the QoI's own units (N·mm, mm, or MPa).
    pub value: f64,
    /// `dJ/dx` per node, same indexing as [`TetMesh::nodes`]. Units are
    /// the objective's per mm.
    pub d_nodes: Vec<[f64; 3]>,
    /// Hard max element von Mises (MPa) — the lower edge of the p-norm
    /// bracket. Only set for [`Qoi::SmoothMaxVonMises`].
    pub hard_max_mpa: Option<f64>,
    /// Elements with positive stress; the upper bracket factor is
    /// `n_active^(1/p)`. Only set for [`Qoi::SmoothMaxVonMises`].
    pub n_active: Option<usize>,
    /// PCG iterations of the forward solve.
    pub forward_iterations: usize,
    /// PCG iterations of the adjoint solve (0 when self-adjoint).
    pub adjoint_iterations: usize,
}

impl ShapeGradient {
    /// Contract the nodal gradient with a nodal velocity field
    /// `dx/dθ` to get the scalar `dJ/dθ`.
    ///
    /// The velocity field is what a parametric seam
    /// (`vcad-kernel-diff`) produces for a design parameter under the
    /// frozen-tessellation contract. Panics if the lengths disagree —
    /// a velocity field from a different mesh is a programming error,
    /// not a runtime condition.
    pub fn contract(&self, velocity: &[[f64; 3]]) -> f64 {
        assert_eq!(
            velocity.len(),
            self.d_nodes.len(),
            "velocity field has {} nodes but the gradient has {}",
            velocity.len(),
            self.d_nodes.len()
        );
        self.d_nodes
            .iter()
            .zip(velocity)
            .map(|(g, v)| g[0] * v[0] + g[1] * v[1] + g[2] * v[2])
            .sum()
    }
}

/// Solve the static problem and the adjoint, returning the solution and
/// the shape gradient of `qoi`.
///
/// The returned [`Solution`] is the very one that was differentiated —
/// no second forward solve, so the reported QoIs and the gradient can
/// never describe different states.
pub fn shape_gradient(
    mesh: &TetMesh,
    spec: &FeaSpec,
    qoi: &Qoi,
    opts: &SolveOptions,
) -> Result<(Solution, ShapeGradient), GradError> {
    spec.validate().map_err(SolveError::from)?;
    if let Qoi::SmoothMaxVonMises { p, threshold_mpa } = qoi {
        if !p.is_finite() || *p <= 1.0 {
            return Err(GradError::InvalidSmoothingExponent(*p));
        }
        if let Some(t) = threshold_mpa {
            if !t.is_finite() || *t < 0.0 {
                return Err(GradError::InvalidThreshold(*t));
            }
        }
    }

    let elems = build_elements(mesh)?;
    let (lam, mu) = lame(spec.poisson);
    let bc = assemble_bc(mesh, spec)?;
    let ndof = 3 * mesh.nodes.len();
    let e_mod = spec.youngs_modulus_mpa;

    if norm(&bc.f) == 0.0 {
        return Err(GradError::Solve(SolveError::Spec(
            crate::spec::SpecError::Invalid(
                "all load force lands on fixed nodes — nothing to solve".into(),
            ),
        )));
    }

    let (u, forward_iterations, residual_rel) = pcg(&elems, lam, mu, &bc.fixed, &bc.f, opts)?;
    let solution = summarize(
        mesh,
        spec,
        &elems,
        lam,
        mu,
        &bc.f,
        &u,
        forward_iterations,
        residual_rel,
    )
    .summary;

    // Per-QoI: the objective value, the adjoint right-hand side ∂J/∂u,
    // and (stress only) the per-element sensitivity that also carries an
    // explicit ∂J/∂x.
    let mut explicit: Option<Vec<[[f64; 3]; 3]>> = None;
    let mut hard_max_mpa = None;
    let mut n_active = None;

    let (value, rhs, self_adjoint) = match qoi {
        Qoi::Compliance => {
            // J = fᵀu/E; ∂J/∂u = f/E, so λ = u/E exactly.
            (dot(&bc.f, &u) / e_mod, Vec::new(), true)
        }
        Qoi::MeanDisplacement { region, direction } => {
            let dn = (direction[0].powi(2) + direction[1].powi(2) + direction[2].powi(2)).sqrt();
            if dn == 0.0 {
                return Err(GradError::ZeroDirection);
            }
            let dir = [direction[0] / dn, direction[1] / dn, direction[2] / dn];
            let tol_geom = mesh.h * 0.25;
            let sel: Vec<usize> = mesh
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, p)| region.contains(**p, tol_geom))
                .map(|(i, _)| i)
                .collect();
            if sel.is_empty() {
                return Err(GradError::EmptyQoiRegion);
            }
            let per = 1.0 / sel.len() as f64;
            let mut g = vec![0.0f64; ndof];
            for &i in &sel {
                for a in 0..3 {
                    g[3 * i + a] = dir[a] * per;
                }
            }
            let value = dot(&g, &u) / e_mod;
            // ∂J/∂u = g/E.
            let rhs: Vec<f64> = g.iter().map(|v| v / e_mod).collect();
            (value, rhs, false)
        }
        Qoi::SmoothMaxVonMises { p, threshold_mpa } => {
            let p = *p;
            let tau = threshold_mpa.unwrap_or(0.0);
            // Element stresses (already physical: E cancels at unit E).
            let stresses: Vec<[[f64; 3]; 3]> = elems
                .iter()
                .map(|e| stress_of(lam, mu, &grad_of(e, &u)))
                .collect();
            let vms: Vec<f64> = stresses.iter().map(von_mises).collect();
            let hard = vms.iter().cloned().fold(0.0f64, f64::max);
            hard_max_mpa = Some(hard);
            // Peak excess over the threshold; normalizing by it keeps any
            // p from overflowing.
            let m = (hard - tau).max(0.0);
            if m <= 0.0 {
                if tau == 0.0 {
                    // No stress anywhere: the model has no load path, and
                    // the smoothed max has no gradient. Fail closed.
                    return Err(GradError::ZeroStressField);
                }
                // Legitimately below the threshold: J = τ, gradient zero.
                n_active = Some(0);
                return Ok((
                    solution,
                    ShapeGradient {
                        value: tau,
                        d_nodes: vec![[0.0; 3]; mesh.nodes.len()],
                        hard_max_mpa,
                        n_active,
                        forward_iterations,
                        adjoint_iterations: 0,
                    },
                ));
            }
            let mut s = 0.0f64;
            let mut active = 0usize;
            for &vm in &vms {
                let x = vm - tau;
                if x > 0.0 {
                    active += 1;
                    s += (x / m).powf(p);
                }
            }
            let j_excess = m * s.powf(1.0 / p);
            let value = tau + j_excess;
            n_active = Some(active);

            // ∂J/∂vm_e = ((vm_e−τ)₊ / J_excess)^(p−1); the chain through σ
            // gives both the adjoint load and the explicit shape term via
            // the same matrix C(dev σ) scaled by 3/(2·vm).
            let mut rhs = vec![0.0f64; ndof];
            let mut sens = Vec::with_capacity(elems.len());
            for ((e, sig), &vm) in elems.iter().zip(&stresses).zip(&vms) {
                let excess = vm - tau;
                if excess <= 0.0 {
                    sens.push([[0.0; 3]; 3]);
                    continue;
                }
                let dj_dvm = (excess / j_excess).powf(p - 1.0);
                let coeff = dj_dvm * 1.5 / vm;
                let tr = (sig[0][0] + sig[1][1] + sig[2][2]) / 3.0;
                let mut dev = *sig;
                for (a, row) in dev.iter_mut().enumerate() {
                    row[a] -= tr;
                }
                for row in dev.iter_mut() {
                    for v in row.iter_mut() {
                        *v *= coeff;
                    }
                }
                // C applied to the scaled deviator; C is symmetric, so
                // the same matrix serves ∂J/∂u and ∂J/∂x.
                let cd = stress_of(lam, mu, &dev);
                for (i, g) in e.grad.iter().enumerate() {
                    let base = 3 * e.n[i];
                    for a in 0..3 {
                        rhs[base + a] += cd[a][0] * g[0] + cd[a][1] * g[1] + cd[a][2] * g[2];
                    }
                }
                sens.push(cd);
            }
            for (d, v) in bc.fixed.iter().zip(rhs.iter_mut()) {
                if *d {
                    *v = 0.0;
                }
            }
            explicit = Some(sens);
            (value, rhs, false)
        }
    };

    // Adjoint solve (or the free self-adjoint shortcut).
    let (lambda, adjoint_iterations) = if self_adjoint {
        (u.iter().map(|v| v / e_mod).collect::<Vec<_>>(), 0)
    } else {
        let (l, it, _) = pcg(&elems, lam, mu, &bc.fixed, &rhs, opts)?;
        (l, it)
    };

    // One pass over the elements accumulates both terms of dJ/dx.
    let mut d_nodes = vec![[0.0f64; 3]; mesh.nodes.len()];
    accumulate(
        &elems,
        lam,
        mu,
        &u,
        &lambda,
        explicit.as_deref(),
        &mut d_nodes,
    );

    Ok((
        solution,
        ShapeGradient {
            value,
            d_nodes,
            hard_max_mpa,
            n_active,
            forward_iterations,
            adjoint_iterations,
        },
    ))
}

/// Accumulate `dJ/dx = ∂J/∂x|_u − λᵀ(∂K/∂x)u` over the elements.
///
/// `explicit[e]` is the per-element matrix `C(∂J/∂σ_e)` for objectives
/// whose value depends on `x` at fixed `u` (the stress p-norm); `None`
/// for purely displacement-based objectives.
fn accumulate(
    elems: &[Elem],
    lam: f64,
    mu: f64,
    u: &[f64],
    lambda: &[f64],
    explicit: Option<&[[[f64; 3]; 3]]>,
    d_nodes: &mut [[f64; 3]],
) {
    for (ei, e) in elems.iter().enumerate() {
        let hu = grad_of(e, u);
        let hl = grad_of(e, lambda);
        let su = stress_of(lam, mu, &hu);
        let sl = stress_of(lam, mu, &hl);
        // σ(H_u) : H_λ
        let mut suhl = 0.0;
        for a in 0..3 {
            for b in 0..3 {
                suhl += su[a][b] * hl[a][b];
            }
        }
        let cd = explicit.map(|s| &s[ei]);

        for (k, gk) in e.grad.iter().enumerate() {
            let node = e.n[k];
            let su_gk = mat_vec(&su, gk);
            let sl_gk = mat_vec(&sl, gk);
            let cd_gk = cd.map(|m| mat_vec(m, gk));
            for j in 0..3 {
                // Columns j of H_u and H_λ.
                let cu = [hu[0][j], hu[1][j], hu[2][j]];
                let cl = [hl[0][j], hl[1][j], hl[2][j]];
                let dk = e.vol * gk[j] * suhl - e.vol * (dot3(&cu, &sl_gk) + dot3(&cl, &su_gk));
                let mut acc = -dk;
                if let Some(cg) = &cd_gk {
                    // ∂J/∂x|_u for the stress objective.
                    acc -= dot3(&cu, cg);
                }
                d_nodes[node][j] += acc;
            }
        }
    }
}

fn mat_vec(m: &[[f64; 3]; 3], v: &[f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn dot3(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_mesh, tet_fill};
    use crate::spec::{Load, Support};

    const L: f64 = 80.0;
    const B: f64 = 10.0;
    const T: f64 = 8.0;
    const P: f64 = 100.0;

    /// Tight PCG so solver stopping noise sits far below the FD signal.
    fn tight() -> SolveOptions {
        SolveOptions {
            tol: 1e-12,
            max_iters: 200_000,
        }
    }

    /// Cantilever: clamped at x=0, tip-loaded −z at x=L. Regions are
    /// padded in y and z so node selection survives the FD perturbation.
    fn cantilever_spec(res: usize) -> FeaSpec {
        FeaSpec {
            resolution: res,
            youngs_modulus_mpa: 69_000.0,
            poisson: 0.33,
            yield_strength_mpa: None,
            loads: vec![Load {
                region: RegionBox {
                    min: [L, -1.0, -1.0],
                    max: [L, B + 1.0, T + 1.0],
                },
                force: [0.0, 0.0, -P],
            }],
            supports: vec![Support {
                region: RegionBox {
                    min: [0.0, -1.0, -1.0],
                    max: [0.0, B + 1.0, T + 1.0],
                },
                fix: [true, true, true],
            }],
        }
    }

    fn mesh(res: usize) -> TetMesh {
        tet_fill(&box_mesh([0.0; 3], [L, B, T]), res).unwrap()
    }

    /// Uniform thickness scaling: every node's z scales with the
    /// parameter. dx/dt = (0, 0, z/T).
    fn thickness_velocity(m: &TetMesh) -> Vec<[f64; 3]> {
        m.nodes.iter().map(|p| [0.0, 0.0, p[2] / T]).collect()
    }

    /// A taper: thickness grows linearly along the beam. Exercises the
    /// full per-node gradient rather than one global scale.
    fn taper_velocity(m: &TetMesh) -> Vec<[f64; 3]> {
        m.nodes
            .iter()
            .map(|p| [0.0, 0.0, p[2] / T * (p[0] / L)])
            .collect()
    }

    /// Move every node by `s·velocity`, keeping connectivity frozen.
    fn perturbed(m: &TetMesh, vel: &[[f64; 3]], s: f64) -> TetMesh {
        let mut out = m.clone();
        for (p, v) in out.nodes.iter_mut().zip(vel) {
            for a in 0..3 {
                p[a] += s * v[a];
            }
        }
        out
    }

    /// Central finite difference of the QoI along a velocity field, on
    /// the same frozen connectivity.
    fn fd(m: &TetMesh, spec: &FeaSpec, qoi: &Qoi, vel: &[[f64; 3]], eps: f64) -> f64 {
        let plus = shape_gradient(&perturbed(m, vel, eps), spec, qoi, &tight())
            .unwrap()
            .1
            .value;
        let minus = shape_gradient(&perturbed(m, vel, -eps), spec, qoi, &tight())
            .unwrap()
            .1
            .value;
        (plus - minus) / (2.0 * eps)
    }

    /// Adjoint vs central FD for one QoI and one velocity field.
    fn check(qoi: Qoi, vel_of: fn(&TetMesh) -> Vec<[f64; 3]>, tol: f64, label: &str) {
        let m = mesh(40);
        let spec = cantilever_spec(40);
        let vel = vel_of(&m);
        let (_, grad) = shape_gradient(&m, &spec, &qoi, &tight()).unwrap();
        let adj = grad.contract(&vel);
        let num = fd(&m, &spec, &qoi, &vel, 1e-4);
        let rel = (adj - num).abs() / num.abs().max(1e-30);
        assert!(
            rel < tol,
            "{label}: adjoint {adj:.9e} vs FD {num:.9e}, rel {rel:.2e} (tol {tol:.0e})"
        );
        assert!(
            num.abs() > 1e-12,
            "{label}: FD signal is zero — vacuous test"
        );
    }

    #[test]
    fn compliance_gradient_matches_finite_differences() {
        check(
            Qoi::Compliance,
            thickness_velocity,
            1e-5,
            "compliance / thickness",
        );
    }

    #[test]
    fn compliance_gradient_matches_fd_under_a_taper() {
        check(Qoi::Compliance, taper_velocity, 1e-5, "compliance / taper");
    }

    #[test]
    fn mean_displacement_gradient_matches_finite_differences() {
        let qoi = Qoi::MeanDisplacement {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T + 1.0],
            },
            direction: [0.0, 0.0, 1.0],
        };
        check(
            qoi.clone(),
            thickness_velocity,
            1e-5,
            "tip deflection / thickness",
        );
        check(qoi, taper_velocity, 1e-5, "tip deflection / taper");
    }

    #[test]
    fn smooth_max_stress_gradient_matches_finite_differences() {
        check(
            Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: None,
            },
            thickness_velocity,
            1e-4,
            "smooth-max stress / thickness",
        );
        check(
            Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: None,
            },
            taper_velocity,
            1e-4,
            "smooth-max stress / taper",
        );
    }

    #[test]
    fn thresholded_stress_gradient_matches_finite_differences() {
        // The threshold introduces a kink at vm = τ; the gradient is
        // still exact away from a tie, which is what the FD probes.
        check(
            Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: Some(30.0),
            },
            thickness_velocity,
            1e-4,
            "thresholded stress / thickness",
        );
        check(
            Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: Some(30.0),
            },
            taper_velocity,
            1e-4,
            "thresholded stress / taper",
        );
    }

    #[test]
    fn a_threshold_tightens_the_bracket_onto_the_peak() {
        let m = mesh(40);
        let spec = cantilever_spec(40);
        let plain = shape_gradient(
            &m,
            &spec,
            &Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: None,
            },
            &tight(),
        )
        .unwrap()
        .1;
        let hard = plain.hard_max_mpa.unwrap();
        let tuned = shape_gradient(
            &m,
            &spec,
            &Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: Some(hard * 0.75),
            },
            &tight(),
        )
        .unwrap()
        .1;
        // Both bracket the true peak from above; the thresholded one is
        // dramatically closer, on far fewer active elements.
        assert!(plain.value > hard && tuned.value > hard);
        let plain_over = plain.value / hard - 1.0;
        let tuned_over = tuned.value / hard - 1.0;
        assert!(
            tuned_over * 5.0 < plain_over,
            "over-read barely improved: {:.1}% thresholded vs {:.1}% plain",
            tuned_over * 100.0,
            plain_over * 100.0
        );
        assert!(
            tuned.n_active.unwrap() * 10 < plain.n_active.unwrap(),
            "active count barely moved: {:?} vs {:?}",
            tuned.n_active,
            plain.n_active
        );
    }

    #[test]
    fn stress_entirely_below_the_threshold_is_a_satisfied_constraint_not_an_error() {
        let m = mesh(24);
        let spec = cantilever_spec(24);
        let (_, g) = shape_gradient(
            &m,
            &spec,
            &Qoi::SmoothMaxVonMises {
                p: 8.0,
                threshold_mpa: Some(1.0e6),
            },
            &tight(),
        )
        .unwrap();
        assert_eq!(g.value, 1.0e6);
        assert_eq!(g.n_active, Some(0));
        assert!(g.d_nodes.iter().all(|d| d == &[0.0; 3]));
        // The real peak is still reported, so the caller can see how much
        // headroom the constraint has.
        assert!(g.hard_max_mpa.unwrap() > 0.0);
    }

    #[test]
    fn bad_threshold_fails_closed() {
        let m = mesh(8);
        let spec = cantilever_spec(8);
        for t in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                shape_gradient(
                    &m,
                    &spec,
                    &Qoi::SmoothMaxVonMises {
                        p: 8.0,
                        threshold_mpa: Some(t)
                    },
                    &tight()
                ),
                Err(GradError::InvalidThreshold(_))
            ));
        }
    }

    #[test]
    fn deflection_gradient_approaches_the_closed_form() {
        // Finite differences only prove the adjoint matches the discrete
        // model; they cannot catch a discrete model that is wrong. Beam
        // theory gives the physical answer: delta = PL³/(3EI) with
        // I = b·t³/12, so d(delta)/dt = −3·delta_bend/t.
        let qoi = Qoi::MeanDisplacement {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T + 1.0],
            },
            direction: [0.0, 0.0, 1.0],
        };
        let i_zz = B * T.powi(3) / 12.0;
        let e_mod = 69_000.0;
        let d_bend = P * L.powi(3) / (3.0 * e_mod * i_zz);
        let exact = 3.0 * d_bend / T; // sign: +z direction, −z load

        // Constant-strain tets are stiff in bending, so this converges
        // from below — assert both the closeness and the direction of
        // improvement, which a sign or scale error could not fake.
        let mut prev_err = f64::INFINITY;
        for res in [40usize, 80] {
            let m = mesh(res);
            let spec = cantilever_spec(res);
            let (_, g) = shape_gradient(&m, &spec, &qoi, &tight()).unwrap();
            let adj = g.contract(&thickness_velocity(&m));
            let err = (adj - exact).abs() / exact;
            assert!(err < prev_err, "res {res}: error grew, {err} vs {prev_err}");
            prev_err = err;
        }
        assert!(
            prev_err < 0.10,
            "finest gradient still {:.1}% off beam theory",
            prev_err * 100.0
        );
    }

    #[test]
    fn compliance_self_adjoint_shortcut_matches_an_explicit_adjoint_solve() {
        // λ = u/E is claimed, not solved. Verify against the same
        // gradient computed through the general (non-self-adjoint)
        // path by expressing compliance as a linear functional.
        let m = mesh(24);
        let spec = cantilever_spec(24);
        let (_, g) = shape_gradient(&m, &spec, &Qoi::Compliance, &tight()).unwrap();
        assert_eq!(g.adjoint_iterations, 0, "shortcut should skip the solve");

        // Mean tip displacement along −z times the total load equals
        // compliance for this single-load case, so their gradients must
        // agree after scaling.
        let qoi = Qoi::MeanDisplacement {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T + 1.0],
            },
            direction: [0.0, 0.0, -1.0],
        };
        let (_, gd) = shape_gradient(&m, &spec, &qoi, &tight()).unwrap();
        assert!(gd.adjoint_iterations > 0, "general path should solve");
        let vel = thickness_velocity(&m);
        let a = g.contract(&vel);
        let b = gd.contract(&vel) * P;
        let rel = (a - b).abs() / b.abs();
        assert!(
            rel < 1e-8,
            "compliance {a:.9e} vs P·deflection {b:.9e}, rel {rel:.2e}"
        );
    }

    #[test]
    fn smooth_max_brackets_the_hard_max() {
        let m = mesh(24);
        let spec = cantilever_spec(24);
        for p in [4.0, 8.0, 16.0] {
            let (_, g) = shape_gradient(
                &m,
                &spec,
                &Qoi::SmoothMaxVonMises {
                    p,
                    threshold_mpa: None,
                },
                &tight(),
            )
            .unwrap();
            let hard = g.hard_max_mpa.unwrap();
            let n = g.n_active.unwrap() as f64;
            assert!(
                g.value >= hard - 1e-9 && g.value <= hard * n.powf(1.0 / p) + 1e-9,
                "p={p}: J {} outside [{hard}, {}]",
                g.value,
                hard * n.powf(1.0 / p)
            );
        }
    }

    #[test]
    fn smooth_max_tightens_toward_the_hard_max_as_p_grows() {
        let m = mesh(24);
        let spec = cantilever_spec(24);
        let j = |p: f64| {
            shape_gradient(
                &m,
                &spec,
                &Qoi::SmoothMaxVonMises {
                    p,
                    threshold_mpa: None,
                },
                &tight(),
            )
            .unwrap()
            .1
            .value
        };
        let (a, b, c) = (j(4.0), j(16.0), j(64.0));
        assert!(a > b && b > c, "not monotone: {a} {b} {c}");
    }

    #[test]
    fn gradient_reports_the_solution_it_differentiated() {
        let m = mesh(24);
        let spec = cantilever_spec(24);
        let (sol, g) = shape_gradient(&m, &spec, &Qoi::Compliance, &tight()).unwrap();
        assert!((sol.compliance_n_mm - g.value).abs() < 1e-12);
        let direct = crate::solve::solve_static(&m, &spec, &tight()).unwrap();
        assert!((direct.compliance_n_mm - sol.compliance_n_mm).abs() < 1e-12);
    }

    #[test]
    fn bad_smoothing_exponent_fails_closed() {
        let m = mesh(8);
        let spec = cantilever_spec(8);
        for p in [1.0, 0.5, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                shape_gradient(
                    &m,
                    &spec,
                    &Qoi::SmoothMaxVonMises {
                        p,
                        threshold_mpa: None
                    },
                    &tight()
                ),
                Err(GradError::InvalidSmoothingExponent(_))
            ));
        }
    }

    #[test]
    fn empty_qoi_region_fails_closed() {
        let m = mesh(8);
        let spec = cantilever_spec(8);
        let qoi = Qoi::MeanDisplacement {
            region: RegionBox {
                min: [500.0; 3],
                max: [501.0; 3],
            },
            direction: [0.0, 0.0, 1.0],
        };
        assert!(matches!(
            shape_gradient(&m, &spec, &qoi, &tight()),
            Err(GradError::EmptyQoiRegion)
        ));
    }

    #[test]
    fn zero_direction_fails_closed() {
        let m = mesh(8);
        let spec = cantilever_spec(8);
        let qoi = Qoi::MeanDisplacement {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T + 1.0],
            },
            direction: [0.0; 3],
        };
        assert!(matches!(
            shape_gradient(&m, &spec, &qoi, &tight()),
            Err(GradError::ZeroDirection)
        ));
    }

    #[test]
    #[should_panic(expected = "velocity field has")]
    fn contracting_a_mismatched_velocity_field_panics() {
        let m = mesh(8);
        let spec = cantilever_spec(8);
        let (_, g) = shape_gradient(&m, &spec, &Qoi::Compliance, &tight()).unwrap();
        g.contract(&[[1.0, 0.0, 0.0]]);
    }
}
