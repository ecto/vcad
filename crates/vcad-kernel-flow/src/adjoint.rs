//! M4: discrete adjoint of the steady lattice + flow-channel topology
//! optimization.
//!
//! Parameterization: a per-voxel **Brinkman drag** ε ∈ [0, 1] on a
//! design region — the fluid feels an extra acceleration `−α(ε)·u`
//! with `α(ε) = α_max·ε`, so ε = 0 is open channel and ε = 1 is
//! near-solid. The objective is the lattice pressure drop between the
//! port faces; its gradient with respect to every design voxel comes
//! from **one** adjoint solve, however many voxels there are.
//!
//! Method: the steady state is a fixed point `f* = F(f*)` of one
//! lattice step `F = Stream ∘ Collide`. The adjoint of a fixed point
//! needs no checkpointing — iterate the reverse fixed point
//! `λ ← Cᵀ(Sᵀ λ) + ∂J/∂f` to convergence and contract with `∂F/∂ε`.
//! The collision transpose is local and closed-form: with Guo forcing
//! and Brinkman drag the velocity solve is `u = s·(m/ρ + ½a_body)`
//! with `s = 1/(1 + ½α)`, and `λC = (1−ω)Λ + A + (s/ρ)·B·(c − m/ρ)`
//! where A (scalar) and B (3-vector) are single reductions over Λ.
//!
//! **Honesty:** the adjoint covers the isothermal momentum path (the
//! optimizer's use case); the pressure-outlet's quadratic velocity
//! term is frozen at the steady state (its contribution is O(Ma²) and
//! the finite-difference validation gate below bounds the total error
//! at < 1%). The forward model here is the same lattice as
//! [`crate::solve`] plus the Brinkman term; at ε = 0 the two agree and
//! a test asserts it.

use serde::{Deserialize, Serialize};

use crate::lattice::{equilibrium, Scaling, C, OPP, Q, W};
use crate::model::{norm, Cell, FlowModel};
use crate::solve::{SolveError, SolveOptions};

/// A topology-design problem: a flow model plus a design region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignSpace {
    /// Voxel indices (grid layout) that carry a design variable. Must
    /// be fluid cells of the model.
    pub design_cells: Vec<usize>,
    /// Peak Brinkman drag α_max in lattice units (per step). Values
    /// around 0.5–2 make ε = 1 effectively solid at duct scales.
    pub alpha_max_lat: f64,
}

/// Result of one forward+adjoint evaluation.
#[derive(Debug, Clone)]
pub struct Gradient {
    /// Objective: pressure drop, Pa.
    pub pressure_drop_pa: f64,
    /// d(pressure drop)/dε per design cell, Pa per unit ε.
    pub d_dp_d_eps_pa: Vec<f64>,
    /// Forward steps to steadiness.
    pub forward_steps: usize,
    /// Adjoint iterations to convergence.
    pub adjoint_steps: usize,
}

/// Errors from the adjoint path.
#[derive(Debug)]
pub enum AdjointError {
    /// Forward solve failed.
    Forward(SolveError),
    /// Design cell out of range or not fluid.
    BadDesignCell {
        /// The offending index.
        index: usize,
    },
    /// ε vector length mismatch.
    BadEpsilon {
        /// Expected (design cell count).
        expected: usize,
        /// Got.
        actual: usize,
    },
    /// The reverse fixed point did not converge in budget.
    AdjointNotConverged {
        /// Iterations run.
        steps: usize,
        /// Last relative change.
        residual: f64,
    },
    /// The model has no ports (the objective needs them).
    NoPorts,
}

impl std::fmt::Display for AdjointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdjointError::Forward(e) => write!(f, "forward: {e}"),
            AdjointError::BadDesignCell { index } => {
                write!(f, "design cell {index} is out of range or not fluid")
            }
            AdjointError::BadEpsilon { expected, actual } => {
                write!(f, "epsilon length {actual}, expected {expected}")
            }
            AdjointError::AdjointNotConverged { steps, residual } => write!(
                f,
                "adjoint fixed point not converged after {steps} iterations \
                 (residual {residual:.3e})"
            ),
            AdjointError::NoPorts => {
                write!(f, "pressure-drop objective needs inlet and outlet cells")
            }
        }
    }
}

impl std::error::Error for AdjointError {}

/// Internal steady lattice state kept for the adjoint.
struct Steady {
    rho: Vec<f64>,
    vel: Vec<[f64; 3]>,
    steps: usize,
    scaling: Scaling,
    /// Cells adjacent to inlet / outlet (for the objective).
    inlet_adj: Vec<usize>,
    outlet_adj: Vec<usize>,
}

/// One forward run of the Brinkman-augmented lattice to steady state.
/// Mirrors [`crate::solve::solve_steady`]'s isothermal path exactly,
/// plus the per-cell drag; a test pins the ε = 0 agreement.
#[allow(clippy::needless_range_loop)]
fn forward(
    model: &FlowModel,
    opts: &SolveOptions,
    alpha_lat: &[f64],
) -> Result<Steady, SolveError> {
    model.validate()?;
    let (nx, ny, nz) = (
        model.divisions[0] as isize,
        model.divisions[1] as isize,
        model.divisions[2] as isize,
    );
    let n = (nx * ny * nz) as usize;
    let dx_m = model.voxel_mm() / 1000.0;
    let inlet_speed = norm(model.inlet_velocity_m_s);
    let u_ref = match opts.u_ref_m_s {
        Some(u) if u > 0.0 => u,
        _ if inlet_speed > 0.0 => inlet_speed,
        _ => return Err(SolveError::NoReference),
    };
    let scaling = Scaling::derive(dx_m, model.fluid.kinematic_viscosity_m2_s(), u_ref)?;
    let omega = 1.0 / scaling.tau;
    let phi = 1.0 - 0.5 * omega;
    let u_in_lat = [
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[0]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[1]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[2]),
    ];
    let c_lat = scaling.dx_m / scaling.dt_s;
    let rho_out_lat = 1.0
        + model.outlet_gauge_pa / (crate::lattice::CS2 * model.fluid.density_kg_m3 * c_lat * c_lat);
    let a_body = [
        scaling.accel_to_lattice(model.body_force_n_m3[0] / model.fluid.density_kg_m3),
        scaling.accel_to_lattice(model.body_force_n_m3[1] / model.fluid.density_kg_m3),
        scaling.accel_to_lattice(model.body_force_n_m3[2] / model.fluid.density_kg_m3),
    ];

    let cells = &model.cells;
    let idx = |i: isize, j: isize, k: isize| -> usize { ((k * ny + j) * nx + i) as usize };
    let feq0 = equilibrium(1.0, [0.0; 3]);
    let mut f = vec![0.0; n * Q];
    for x in 0..n {
        f[x * Q..(x + 1) * Q].copy_from_slice(&feq0);
    }
    let mut f_new = f.clone();
    let mut rho = vec![1.0f64; n];
    let mut vel = vec![[0.0f64; 3]; n];
    let mut vel_prev = vel.clone();
    let u_floor = scaling.u_lattice.max(1e-12);
    let mut steps = 0usize;
    let mut residual;

    loop {
        if steps >= opts.max_steps {
            return Err(SolveError::NotConverged {
                steps,
                residual: f64::INFINITY,
                tol: opts.steady_tol,
            });
        }
        let s_ramp = if opts.ramp_steps == 0 {
            1.0
        } else {
            let t = ((steps + 1) as f64 / opts.ramp_steps as f64).min(1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let u_in_now = [
            u_in_lat[0] * s_ramp,
            u_in_lat[1] * s_ramp,
            u_in_lat[2] * s_ramp,
        ];

        // Collision with Brinkman drag.
        for x in 0..n {
            if cells[x] != Cell::Fluid {
                continue;
            }
            let alpha = alpha_lat[x];
            let s = 1.0 / (1.0 + 0.5 * alpha);
            let fx = &mut f[x * Q..(x + 1) * Q];
            let mut r = 0.0;
            let mut m = [0.0f64; 3];
            for (i, fi) in fx.iter().enumerate() {
                r += fi;
                m[0] += fi * C[i][0] as f64;
                m[1] += fi * C[i][1] as f64;
                m[2] += fi * C[i][2] as f64;
            }
            let u = [
                s * (m[0] / r + 0.5 * a_body[0]),
                s * (m[1] / r + 0.5 * a_body[1]),
                s * (m[2] / r + 0.5 * a_body[2]),
            ];
            let a_tot = [
                a_body[0] - alpha * u[0],
                a_body[1] - alpha * u[1],
                a_body[2] - alpha * u[2],
            ];
            rho[x] = r;
            vel[x] = u;
            let feq = equilibrium(r, u);
            for i in 0..Q {
                let cu = C[i][0] as f64 * u[0] + C[i][1] as f64 * u[1] + C[i][2] as f64 * u[2];
                let ca = C[i][0] as f64 * a_tot[0]
                    + C[i][1] as f64 * a_tot[1]
                    + C[i][2] as f64 * a_tot[2];
                let ua = u[0] * a_tot[0] + u[1] * a_tot[1] + u[2] * a_tot[2];
                let guo = phi * W[i] * r * (3.0 * (ca - ua) + 9.0 * cu * ca);
                fx[i] += -omega * (fx[i] - feq[i]) + guo;
            }
        }

        // Streaming (identical to solve.rs).
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = idx(i, j, k);
                    if cells[x] != Cell::Fluid {
                        continue;
                    }
                    for q in 0..Q {
                        let (mut si, mut sj, mut sk) = (
                            i - C[q][0] as isize,
                            j - C[q][1] as isize,
                            k - C[q][2] as isize,
                        );
                        let mut outside = false;
                        for (axis, sc, nmax) in
                            [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                        {
                            if *sc < 0 || *sc >= nmax {
                                if model.periodic[axis] {
                                    *sc = (*sc + nmax) % nmax;
                                } else {
                                    outside = true;
                                }
                            }
                        }
                        let src_kind = if outside {
                            Cell::Solid
                        } else {
                            cells[idx(si, sj, sk)]
                        };
                        f_new[x * Q + q] = match src_kind {
                            Cell::Fluid => f[idx(si, sj, sk) * Q + q],
                            Cell::Solid => f[x * Q + OPP[q]],
                            Cell::Inlet => {
                                let cu = C[q][0] as f64 * u_in_now[0]
                                    + C[q][1] as f64 * u_in_now[1]
                                    + C[q][2] as f64 * u_in_now[2];
                                f[x * Q + OPP[q]] + 6.0 * W[q] * cu
                            }
                            Cell::Outlet => {
                                let u = vel[x];
                                let cu = C[q][0] as f64 * u[0]
                                    + C[q][1] as f64 * u[1]
                                    + C[q][2] as f64 * u[2];
                                let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
                                -f[x * Q + OPP[q]]
                                    + 2.0 * W[q] * rho_out_lat * (1.0 + 4.5 * cu * cu - 1.5 * uu)
                            }
                        };
                    }
                }
            }
        }
        std::mem::swap(&mut f, &mut f_new);
        steps += 1;
        if steps.is_multiple_of(opts.check_every) && steps >= opts.ramp_steps {
            let mut max_delta = 0.0f64;
            let mut finite = true;
            for x in 0..n {
                if cells[x] != Cell::Fluid {
                    continue;
                }
                for a in 0..3 {
                    if !vel[x][a].is_finite() {
                        finite = false;
                    }
                    max_delta = max_delta.max((vel[x][a] - vel_prev[x][a]).abs());
                }
            }
            if !finite {
                return Err(SolveError::Diverged { step: steps });
            }
            residual = max_delta / u_floor;
            vel_prev.copy_from_slice(&vel);
            if residual < opts.steady_tol {
                break;
            }
        }
    }

    // Port-adjacent cells for the objective.
    let dirs: [(isize, isize, isize); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    let mut inlet_adj = Vec::new();
    let mut outlet_adj = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = idx(i, j, k);
                if cells[x] != Cell::Fluid {
                    continue;
                }
                for (di, dj, dk) in dirs {
                    let (si, sj, sk) = (i + di, j + dj, k + dk);
                    if si < 0 || sj < 0 || sk < 0 || si >= nx || sj >= ny || sk >= nz {
                        continue;
                    }
                    match cells[idx(si, sj, sk)] {
                        Cell::Inlet => inlet_adj.push(x),
                        Cell::Outlet => outlet_adj.push(x),
                        _ => {}
                    }
                }
            }
        }
    }
    inlet_adj.dedup();
    outlet_adj.dedup();

    let _ = f;
    Ok(Steady {
        rho,
        vel,
        steps,
        scaling,
        inlet_adj,
        outlet_adj,
    })
}

/// Evaluate the pressure-drop objective and its adjoint gradient.
///
/// `eps` has one entry per `space.design_cells`; the returned gradient
/// is `d(Δp)/dε` in Pa per unit ε, exact for the discrete forward model
/// up to the frozen outlet term (finite-difference validated).
pub fn pressure_drop_gradient(
    model: &FlowModel,
    space: &DesignSpace,
    eps: &[f64],
    opts: &SolveOptions,
) -> Result<Gradient, AdjointError> {
    let n = model.cells.len();
    if eps.len() != space.design_cells.len() {
        return Err(AdjointError::BadEpsilon {
            expected: space.design_cells.len(),
            actual: eps.len(),
        });
    }
    for &dc in &space.design_cells {
        if dc >= n || model.cells[dc] != Cell::Fluid {
            return Err(AdjointError::BadDesignCell { index: dc });
        }
    }
    let mut alpha = vec![0.0f64; n];
    for (&dc, &e) in space.design_cells.iter().zip(eps) {
        alpha[dc] = space.alpha_max_lat * e.clamp(0.0, 1.0);
    }

    let steady = forward(model, opts, &alpha).map_err(AdjointError::Forward)?;
    if steady.inlet_adj.is_empty() || steady.outlet_adj.is_empty() {
        return Err(AdjointError::NoPorts);
    }

    // Objective in lattice units: J = mean(rho at inlet-adjacent) −
    // mean(rho at outlet-adjacent); Δp = cs²·(J)·ρ_phys·(dx/dt)².
    let p_scale = crate::lattice::CS2
        * model.fluid.density_kg_m3
        * (steady.scaling.dx_m / steady.scaling.dt_s).powi(2);
    let j_lat: f64 = steady.inlet_adj.iter().map(|&x| steady.rho[x]).sum::<f64>()
        / steady.inlet_adj.len() as f64
        - steady
            .outlet_adj
            .iter()
            .map(|&x| steady.rho[x])
            .sum::<f64>()
            / steady.outlet_adj.len() as f64;

    // ∂J/∂f: ±1/N on every direction of the port-adjacent cells.
    let mut gj = vec![0.0f64; n * Q];
    for &x in &steady.inlet_adj {
        for q in 0..Q {
            gj[x * Q + q] += 1.0 / steady.inlet_adj.len() as f64;
        }
    }
    for &x in &steady.outlet_adj {
        for q in 0..Q {
            gj[x * Q + q] -= 1.0 / steady.outlet_adj.len() as f64;
        }
    }

    // Reverse fixed point: λ ← Cᵀ(Sᵀ λ) + gJ.
    let (nx, ny, nz) = (
        model.divisions[0] as isize,
        model.divisions[1] as isize,
        model.divisions[2] as isize,
    );
    let idx = |i: isize, j: isize, k: isize| -> usize { ((k * ny + j) * nx + i) as usize };
    let omega = 1.0 / steady.scaling.tau;
    let phi = 1.0 - 0.5 * omega;
    let a_body = {
        let s = &steady.scaling;
        [
            s.accel_to_lattice(model.body_force_n_m3[0] / model.fluid.density_kg_m3),
            s.accel_to_lattice(model.body_force_n_m3[1] / model.fluid.density_kg_m3),
            s.accel_to_lattice(model.body_force_n_m3[2] / model.fluid.density_kg_m3),
        ]
    };
    let cells = &model.cells;
    let c_lat2 = steady.scaling.dx_m / steady.scaling.dt_s;
    let rho_out_lat = 1.0
        + model.outlet_gauge_pa
            / (crate::lattice::CS2 * model.fluid.density_kg_m3 * c_lat2 * c_lat2);
    let mut lam = gj.clone();
    let mut lam_tilde = vec![0.0f64; n * Q];
    // Outlet velocity coupling: the anti-bounce-back equilibrium term
    // depends on the cell's own collision velocity; freezing it makes
    // the transpose operator unstable (the u-coupling is what damps the
    // -1 self-link). Accumulate d(outlet term)/du per cell and fold it
    // into the collision transpose via du/df.
    let mut d_out = vec![[0.0f64; 3]; n];
    let mut lam_new = vec![0.0f64; n * Q];
    let max_adj = 4 * opts.max_steps;
    let mut adj_steps = 0usize;
    let mut adj_res = f64::INFINITY;

    while adj_steps < max_adj {
        // Sᵀ: scatter λ back through the streaming/boundary rules.
        lam_tilde.iter_mut().for_each(|v| *v = 0.0);
        d_out.iter_mut().for_each(|v| *v = [0.0; 3]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = idx(i, j, k);
                    if cells[x] != Cell::Fluid {
                        continue;
                    }
                    for q in 0..Q {
                        let l = lam[x * Q + q];
                        if l == 0.0 {
                            continue;
                        }
                        let (mut si, mut sj, mut sk) = (
                            i - C[q][0] as isize,
                            j - C[q][1] as isize,
                            k - C[q][2] as isize,
                        );
                        let mut outside = false;
                        for (axis, sc, nmax) in
                            [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                        {
                            if *sc < 0 || *sc >= nmax {
                                if model.periodic[axis] {
                                    *sc = (*sc + nmax) % nmax;
                                } else {
                                    outside = true;
                                }
                            }
                        }
                        let src_kind = if outside {
                            Cell::Solid
                        } else {
                            cells[idx(si, sj, sk)]
                        };
                        match src_kind {
                            Cell::Fluid => lam_tilde[idx(si, sj, sk) * Q + q] += l,
                            Cell::Solid | Cell::Inlet => lam_tilde[x * Q + OPP[q]] += l,
                            Cell::Outlet => {
                                lam_tilde[x * Q + OPP[q]] -= l;
                                let u = steady.vel[x];
                                let cq = [C[q][0] as f64, C[q][1] as f64, C[q][2] as f64];
                                let cu = cq[0] * u[0] + cq[1] * u[1] + cq[2] * u[2];
                                for a in 0..3 {
                                    d_out[x][a] += l
                                        * 2.0
                                        * W[q]
                                        * rho_out_lat
                                        * (9.0 * cu * cq[a] - 3.0 * u[a]);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Cᵀ per cell + add gJ.
        let mut max_delta = 0.0f64;
        let mut max_abs = 0.0f64;
        for x in 0..(n) {
            if cells[x] != Cell::Fluid {
                continue;
            }
            let lt = &lam_tilde[x * Q..(x + 1) * Q];
            let r = steady.rho[x];
            let u = steady.vel[x];
            let alpha_x = alpha[x];
            let s = 1.0 / (1.0 + 0.5 * alpha_x);
            let a_tot = [
                a_body[0] - alpha_x * u[0],
                a_body[1] - alpha_x * u[1],
                a_body[2] - alpha_x * u[2],
            ];
            let m_over_rho = [
                u[0] / s - 0.5 * a_body[0],
                u[1] / s - 0.5 * a_body[1],
                u[2] / s - 0.5 * a_body[2],
            ];
            let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
            let mut a_scal = 0.0f64;
            let mut b = d_out[x];
            for i in 0..Q {
                let li = lt[i];
                if li == 0.0 {
                    continue;
                }
                let ci = [C[i][0] as f64, C[i][1] as f64, C[i][2] as f64];
                let cu = ci[0] * u[0] + ci[1] * u[1] + ci[2] * u[2];
                let e_i = 1.0 + 3.0 * cu + 4.5 * cu * cu - 1.5 * uu;
                let ca = ci[0] * a_tot[0] + ci[1] * a_tot[1] + ci[2] * a_tot[2];
                let ua = u[0] * a_tot[0] + u[1] * a_tot[1] + u[2] * a_tot[2];
                let g_over_rho = phi * W[i] * (3.0 * (ca - ua) + 9.0 * cu * ca);
                a_scal += li * (omega * W[i] * e_i + g_over_rho);
                for a in 0..3 {
                    let dfeq = W[i] * r * (3.0 * ci[a] + 9.0 * cu * ci[a] - 3.0 * u[a]);
                    // ∇_u of the Guo bracket · a_tot, plus the explicit
                    // −α chain from a_tot = a_body − α·u.
                    let dbracket = -3.0 * a_tot[a] + 9.0 * ci[a] * ca
                        - alpha_x * (3.0 * (ci[a] - u[a]) + 9.0 * cu * ci[a]);
                    let dg = phi * W[i] * r * dbracket;
                    b[a] += li * (omega * dfeq + dg);
                }
            }
            for q in 0..Q {
                let cq = [C[q][0] as f64, C[q][1] as f64, C[q][2] as f64];
                let dot = (cq[0] - m_over_rho[0]) * b[0]
                    + (cq[1] - m_over_rho[1]) * b[1]
                    + (cq[2] - m_over_rho[2]) * b[2];
                let v = (1.0 - omega) * lt[q] + a_scal + (s / r) * dot + gj[x * Q + q];
                let old = lam[x * Q + q];
                lam_new[x * Q + q] = v;
                max_delta = max_delta.max((v - old).abs());
                max_abs = max_abs.max(v.abs());
            }
        }
        std::mem::swap(&mut lam, &mut lam_new);
        adj_steps += 1;
        adj_res = max_delta / max_abs.max(1e-300);
        if adj_res < opts.steady_tol {
            break;
        }
    }
    if adj_res >= opts.steady_tol {
        return Err(AdjointError::AdjointNotConverged {
            steps: adj_steps,
            residual: adj_res,
        });
    }

    // Final Sᵀ pass so λ̃ matches the converged λ, then contract with
    // ∂f̃/∂α at each design cell.
    lam_tilde.iter_mut().for_each(|v| *v = 0.0);
    d_out.iter_mut().for_each(|v| *v = [0.0; 3]);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = idx(i, j, k);
                if cells[x] != Cell::Fluid {
                    continue;
                }
                for q in 0..Q {
                    let l = lam[x * Q + q];
                    if l == 0.0 {
                        continue;
                    }
                    let (mut si, mut sj, mut sk) = (
                        i - C[q][0] as isize,
                        j - C[q][1] as isize,
                        k - C[q][2] as isize,
                    );
                    let mut outside = false;
                    for (axis, sc, nmax) in
                        [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                    {
                        if *sc < 0 || *sc >= nmax {
                            if model.periodic[axis] {
                                *sc = (*sc + nmax) % nmax;
                            } else {
                                outside = true;
                            }
                        }
                    }
                    let src_kind = if outside {
                        Cell::Solid
                    } else {
                        cells[idx(si, sj, sk)]
                    };
                    match src_kind {
                        Cell::Fluid => lam_tilde[idx(si, sj, sk) * Q + q] += l,
                        Cell::Solid | Cell::Inlet => lam_tilde[x * Q + OPP[q]] += l,
                        Cell::Outlet => {
                            lam_tilde[x * Q + OPP[q]] -= l;
                            let u = steady.vel[x];
                            let cq = [C[q][0] as f64, C[q][1] as f64, C[q][2] as f64];
                            let cu = cq[0] * u[0] + cq[1] * u[1] + cq[2] * u[2];
                            for a in 0..3 {
                                d_out[x][a] +=
                                    l * 2.0 * W[q] * rho_out_lat * (9.0 * cu * cq[a] - 3.0 * u[a]);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut grad = Vec::with_capacity(space.design_cells.len());
    for &x in &space.design_cells {
        let lt = &lam_tilde[x * Q..(x + 1) * Q];
        let r = steady.rho[x];
        let u = steady.vel[x];
        let alpha_x = alpha[x];
        let s = 1.0 / (1.0 + 0.5 * alpha_x);
        let a_tot = [
            a_body[0] - alpha_x * u[0],
            a_body[1] - alpha_x * u[1],
            a_body[2] - alpha_x * u[2],
        ];
        // du/dα = −½·s·u ; da_tot/dα = −u − α·du/dα.
        let du = [-0.5 * s * u[0], -0.5 * s * u[1], -0.5 * s * u[2]];
        let da = [
            -u[0] - alpha_x * du[0],
            -u[1] - alpha_x * du[1],
            -u[2] - alpha_x * du[2],
        ];
        let mut g = d_out[x][0] * du[0] + d_out[x][1] * du[1] + d_out[x][2] * du[2];
        for i in 0..Q {
            let li = lt[i];
            if li == 0.0 {
                continue;
            }
            let ci = [C[i][0] as f64, C[i][1] as f64, C[i][2] as f64];
            let cu = ci[0] * u[0] + ci[1] * u[1] + ci[2] * u[2];
            let c_du = ci[0] * du[0] + ci[1] * du[1] + ci[2] * du[2];
            let u_du = u[0] * du[0] + u[1] * du[1] + u[2] * du[2];
            // dfeq/dα = ∇_u feq · du
            let dfeq = W[i] * r * (3.0 * c_du + 9.0 * cu * c_du - 3.0 * u_du);
            // dG/dα: product rule over the bracket and a_tot.
            let ca = ci[0] * a_tot[0] + ci[1] * a_tot[1] + ci[2] * a_tot[2];
            let c_da = ci[0] * da[0] + ci[1] * da[1] + ci[2] * da[2];
            let u_da = u[0] * da[0] + u[1] * da[1] + u[2] * da[2];
            let du_a = du[0] * a_tot[0] + du[1] * a_tot[1] + du[2] * a_tot[2];
            let dbracket = 3.0 * (c_da - u_da - du_a) + 9.0 * (c_du * ca + cu * c_da);
            let dg = phi * W[i] * r * dbracket;
            g += li * (omega * dfeq + dg);
        }
        grad.push(g * space.alpha_max_lat * p_scale);
    }

    Ok(Gradient {
        pressure_drop_pa: j_lat * p_scale,
        d_dp_d_eps_pa: grad,
        forward_steps: steady.steps,
        adjoint_steps: adj_steps,
    })
}

/// Result of a channel optimization.
#[derive(Debug, Clone)]
pub struct OptimizeResult {
    /// Final ε per design cell.
    pub eps: Vec<f64>,
    /// Pressure drop at the start, Pa.
    pub dp_initial_pa: f64,
    /// Pressure drop at the end, Pa.
    pub dp_final_pa: f64,
    /// Gradient evaluations used.
    pub evaluations: usize,
}

/// Minimize pressure drop over the design region subject to a minimum
/// solid fraction `Σε / N ≥ solid_fraction` (without it the optimum is
/// trivially all-open). Projected gradient descent with a bisected
/// volume multiplier; each iteration costs one forward + one adjoint
/// solve regardless of design size — the point of M4.
pub fn optimize_channel(
    model: &FlowModel,
    space: &DesignSpace,
    solid_fraction: f64,
    iterations: usize,
    step: f64,
    opts: &SolveOptions,
) -> Result<OptimizeResult, AdjointError> {
    let nd = space.design_cells.len();
    let mut eps = vec![solid_fraction.clamp(0.0, 1.0); nd];
    let mut dp_initial = None;
    let mut dp = 0.0;
    let mut evals = 0usize;
    for _ in 0..iterations {
        let g = pressure_drop_gradient(model, space, &eps, opts)?;
        evals += 1;
        dp = g.pressure_drop_pa;
        if dp_initial.is_none() {
            dp_initial = Some(dp);
        }
        let gmax = g
            .d_dp_d_eps_pa
            .iter()
            .fold(0.0f64, |m, v| m.max(v.abs()))
            .max(1e-300);
        // Project: eps − step·g/|g|max, then bisect a shift μ so the
        // volume constraint holds with equality (or is slack).
        let raw: Vec<f64> = eps
            .iter()
            .zip(&g.d_dp_d_eps_pa)
            .map(|(e, gr)| e - step * gr / gmax)
            .collect();
        let volume = |mu: f64| -> f64 {
            raw.iter().map(|r| (r + mu).clamp(0.0, 1.0)).sum::<f64>() / nd as f64
        };
        let target = solid_fraction.clamp(0.0, 1.0);
        let mut mu = 0.0;
        if volume(0.0) < target {
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..60 {
                mu = 0.5 * (lo + hi);
                if volume(mu) < target {
                    lo = mu;
                } else {
                    hi = mu;
                }
            }
        }
        for (e, r) in eps.iter_mut().zip(&raw) {
            *e = (r + mu).clamp(0.0, 1.0);
        }
    }
    // Final evaluation at the projected point.
    let g = pressure_drop_gradient(model, space, &eps, opts)?;
    evals += 1;
    Ok(OptimizeResult {
        eps,
        dp_initial_pa: dp_initial.unwrap_or(dp),
        dp_final_pa: g.pressure_drop_pa,
        evaluations: evals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fluid;
    use crate::solve::solve_steady;

    fn duct(nx: usize, w: usize, u: f64) -> FlowModel {
        let mut m = FlowModel::new([0.0; 3], [nx as f64, w as f64, w as f64], [nx, w, w]);
        for k in 0..w {
            for j in 0..w {
                for i in 0..nx {
                    let x = m.index(i, j, k);
                    m.cells[x] = if i == 0 {
                        Cell::Inlet
                    } else if i == nx - 1 {
                        Cell::Outlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        m.fluid = Fluid::AIR_20C;
        m.inlet_velocity_m_s = [u, 0.0, 0.0];
        m
    }

    fn blob_space(m: &FlowModel) -> DesignSpace {
        // A 4x3x3 design blob mid-duct.
        let mut cells = Vec::new();
        for k in 2..5 {
            for j in 2..5 {
                for i in 8..12 {
                    cells.push(m.index(i, j, k));
                }
            }
        }
        DesignSpace {
            design_cells: cells,
            alpha_max_lat: 1.0,
        }
    }

    #[test]
    fn epsilon_zero_matches_plain_solver() {
        let m = duct(20, 7, 0.08);
        let space = blob_space(&m);
        let eps = vec![0.0; space.design_cells.len()];
        let opts = SolveOptions {
            steady_tol: 1e-8,
            ..Default::default()
        };
        let g = pressure_drop_gradient(&m, &space, &eps, &opts).expect("adjoint eval");
        let plain = solve_steady(&m, &opts).expect("plain solve");
        let rel = (g.pressure_drop_pa - plain.pressure_drop_pa).abs()
            / plain.pressure_drop_pa.abs().max(1e-300);
        assert!(
            rel < 1e-6,
            "eps=0 dp {:.6e} vs solve.rs {:.6e} ({rel:.2e} rel)",
            g.pressure_drop_pa,
            plain.pressure_drop_pa
        );
    }

    #[test]
    fn drag_raises_pressure_drop() {
        let m = duct(20, 7, 0.08);
        let space = blob_space(&m);
        let opts = SolveOptions {
            steady_tol: 1e-8,
            ..Default::default()
        };
        let g0 = pressure_drop_gradient(&m, &space, &vec![0.0; space.design_cells.len()], &opts)
            .unwrap();
        let g1 = pressure_drop_gradient(&m, &space, &vec![0.8; space.design_cells.len()], &opts)
            .unwrap();
        assert!(
            g1.pressure_drop_pa > g0.pressure_drop_pa * 1.05,
            "blocking the blob must cost pressure: {} vs {}",
            g1.pressure_drop_pa,
            g0.pressure_drop_pa
        );
        // And the gradient must know it: at eps=0 every component is
        // positive (more drag -> more pressure drop).
        assert!(g0.d_dp_d_eps_pa.iter().all(|v| *v > 0.0));
    }

    /// The M4 gate: adjoint gradient vs central finite differences on a
    /// handful of design cells, < 1% (particle-crate precedent).
    #[test]
    fn adjoint_matches_finite_differences() {
        let m = duct(16, 5, 0.06);
        let mut cells = Vec::new();
        for k in 1..4 {
            for j in 1..4 {
                for i in 6..9 {
                    cells.push(m.index(i, j, k));
                }
            }
        }
        let space = DesignSpace {
            design_cells: cells,
            alpha_max_lat: 1.0,
        };
        let eps0 = vec![0.3; space.design_cells.len()];
        let opts = SolveOptions {
            steady_tol: 1e-10,
            max_steps: 800_000,
            ..Default::default()
        };
        let g = pressure_drop_gradient(&m, &space, &eps0, &opts).expect("adjoint");

        let h = 1e-4;
        for probe in [0usize, 13, 26] {
            let mut ep = eps0.clone();
            ep[probe] += h;
            let jp = pressure_drop_gradient(&m, &space, &ep, &opts)
                .unwrap()
                .pressure_drop_pa;
            ep[probe] -= 2.0 * h;
            let jm = pressure_drop_gradient(&m, &space, &ep, &opts)
                .unwrap()
                .pressure_drop_pa;
            let fd = (jp - jm) / (2.0 * h);
            let ad = g.d_dp_d_eps_pa[probe];
            let rel = (fd - ad).abs() / fd.abs().max(1e-300);
            assert!(
                rel < 0.01,
                "design cell {probe}: adjoint {ad:.6e} vs FD {fd:.6e} ({:.2}% off)",
                rel * 100.0
            );
        }
    }

    /// Grown-channel demo: a half-blocked design region; the optimizer
    /// must strictly beat the blocked start at equal solid fraction.
    /// Release-ladder rung.
    #[test]
    #[ignore = "release-ladder rung: several forward+adjoint solves"]
    fn optimizer_beats_naive_block() {
        let m = duct(24, 7, 0.06);
        // Design region: full cross-section slab, i in [8, 16).
        let mut cells = Vec::new();
        for k in 0..7 {
            for j in 0..7 {
                for i in 8..16 {
                    cells.push(m.index(i, j, k));
                }
            }
        }
        let space = DesignSpace {
            design_cells: cells,
            alpha_max_lat: 1.0,
        };
        let opts = SolveOptions {
            steady_tol: 1e-7,
            ..Default::default()
        };
        let r = optimize_channel(&m, &space, 0.4, 8, 0.15, &opts).expect("optimize");
        assert!(
            r.dp_final_pa < r.dp_initial_pa * 0.9,
            "optimizer should beat the uniform 40%-solid start: {:.4e} -> {:.4e} Pa",
            r.dp_initial_pa,
            r.dp_final_pa
        );
        // Volume constraint held.
        let vol: f64 = r.eps.iter().sum::<f64>() / r.eps.len() as f64;
        assert!(vol >= 0.39, "solid fraction {vol:.3} < target");
    }
}
