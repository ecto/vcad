//! Steady-state D3Q19 lattice-Boltzmann solve.
//!
//! BGK collision with Guo forcing (second-order body-force coupling),
//! half-way bounce-back walls, moving-wall bounce-back velocity inlets,
//! anti-bounce-back pressure outlets. Steadiness is *detected* — the
//! velocity field's relative L∞ change per check interval must fall
//! below tolerance — and a run that never gets there is an error, not a
//! result.

use serde::{Deserialize, Serialize};

use crate::lattice::{equilibrium, Scaling, ScalingError, C, CS2, OPP, Q, W};
use crate::model::{norm, Cell, FlowModel, ModelError};

/// Solve options. Defaults are the validated M0 settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SolveOptions {
    /// Reference speed for unit scaling, m/s. Defaults to the inlet
    /// speed; body-force-driven cases (no inlet) must supply it — the
    /// solver refuses to guess.
    pub u_ref_m_s: Option<f64>,
    /// Hard step budget before the solve fails as not converged.
    pub max_steps: usize,
    /// Steadiness is checked every this many steps.
    pub check_every: usize,
    /// Steady tolerance: relative L∞ velocity change per check interval.
    pub steady_tol: f64,
    /// Inlet velocity ramp length, steps. An impulsive start launches a
    /// pressure shock through the lattice that can destabilize marginal
    /// τ; the inlet velocity rises smoothly (smoothstep) over this many
    /// steps instead. Steadiness is only checked after the ramp.
    pub ramp_steps: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions {
            u_ref_m_s: None,
            max_steps: 400_000,
            check_every: 200,
            steady_tol: 1e-6,
            ramp_steps: 1000,
        }
    }
}

/// Why a solve failed. Fail-closed: no partial fields are returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SolveError {
    /// Model validation failed.
    Model(ModelError),
    /// Unit scaling could not be derived for this grid/fluid/speed.
    Scaling(ScalingError),
    /// No reference speed: body-force drive without `u_ref_m_s`.
    NoReference,
    /// The step budget ran out before the field went steady.
    NotConverged {
        /// Steps taken.
        steps: usize,
        /// Last steadiness residual observed.
        residual: f64,
        /// The tolerance it failed to meet.
        tol: f64,
    },
    /// The field produced non-finite values (lattice instability).
    Diverged {
        /// Step at which non-finite values appeared.
        step: usize,
    },
    /// The scalar relaxation time τ_g = 3·α·dt/dx² + ½ falls outside
    /// the validated lattice window at this scaling.
    ThermalUnstable {
        /// α·dt/dx² of the run.
        alpha_lat: f64,
    },
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::Model(e) => write!(f, "model: {e}"),
            SolveError::Scaling(e) => write!(f, "scaling: {e}"),
            SolveError::NoReference => write!(
                f,
                "body-force drive needs u_ref_m_s in SolveOptions; the solver does not guess \
                 reference speeds"
            ),
            SolveError::NotConverged {
                steps,
                residual,
                tol,
            } => write!(
                f,
                "not steady after {steps} steps: residual {residual:.3e} > tol {tol:.1e}"
            ),
            SolveError::Diverged { step } => {
                write!(f, "lattice diverged (non-finite field) at step {step}")
            }
            SolveError::ThermalUnstable { alpha_lat } => write!(
                f,
                "scalar relaxation time tau_g = {:.4} (alpha_lat = {alpha_lat:.4}) is \
                 outside the validated lattice window; refine the grid or adjust u_ref",
                3.0 * alpha_lat + 0.5
            ),
        }
    }
}

impl std::error::Error for SolveError {}

impl From<ModelError> for SolveError {
    fn from(e: ModelError) -> Self {
        SolveError::Model(e)
    }
}

impl From<ScalingError> for SolveError {
    fn from(e: ScalingError) -> Self {
        SolveError::Scaling(e)
    }
}

/// A converged steady solution, all SI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solution {
    /// The unit mapping the solve ran at (provenance).
    pub scaling: Scaling,
    /// Steps taken to steadiness.
    pub steps: usize,
    /// Final steadiness residual.
    pub steady_residual: f64,
    /// Velocity per voxel, m/s (zero for non-fluid voxels), layout
    /// `(k·ny + j)·nx + i`.
    pub velocity_m_s: Vec<[f64; 3]>,
    /// Gauge pressure per voxel, Pa (zero for non-fluid voxels).
    pub gauge_pressure_pa: Vec<f64>,
    /// Volumetric flow the inlet boundary actually injects, m³/s —
    /// summed over the moving-wall bounce-back links, which is less than
    /// plug-flow `U·A` when the patch touches walls (no-slip eats the
    /// edge links; the deficit is physical, not lost mass).
    pub inlet_flow_m3_s: f64,
    /// Volumetric flow measured leaving through the outlet, m³/s.
    pub outlet_flow_m3_s: f64,
    /// `|Q_in − Q_out| / max(|Q_in|, |Q_out|)` — the mass audit. Closes
    /// to solver tolerance or the solution is wrong.
    pub mass_balance_residual: f64,
    /// Mean gauge pressure of fluid adjacent to the inlet minus fluid
    /// adjacent to the outlet, Pa. Zero when the model has no ports.
    pub pressure_drop_pa: f64,
    /// Largest fluid speed, m/s.
    pub max_speed_m_s: f64,
    /// Fluid temperature per voxel, °C (thermal runs only; non-fluid
    /// voxels hold the initial temperature).
    pub temperature_c: Option<Vec<f64>>,
    /// Flux-weighted mean outlet temperature, °C (thermal + ports).
    pub outlet_temp_c: Option<f64>,
    /// Heat picked up by the fluid between inlet and outlet, W:
    /// `ρ·c_p·(Σ_out u_n·A·T − Q_in·T_in)`.
    pub heat_pickup_w: Option<f64>,
    /// Heat entering the fluid through Dirichlet boundaries, W
    /// (isothermal walls plus conduction through the inlet ghost).
    /// Steady state closes `wall_heat_w ≈ heat_pickup_w` for ported
    /// runs — the thermal energy audit.
    pub wall_heat_w: Option<f64>,
}

/// Run the lattice to steady state.
pub fn solve_steady(model: &FlowModel, opts: &SolveOptions) -> Result<Solution, SolveError> {
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
    let tau = scaling.tau;
    let omega = 1.0 / tau;
    let force_prefactor = 1.0 - 0.5 * omega;

    let u_in_lat = [
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[0]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[1]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[2]),
    ];
    let c_lat = scaling.dx_m / scaling.dt_s;
    let rho_out_lat =
        1.0 + model.outlet_gauge_pa / (CS2 * model.fluid.density_kg_m3 * c_lat * c_lat);
    let a_lat = [
        scaling.accel_to_lattice(model.body_force_n_m3[0] / model.fluid.density_kg_m3),
        scaling.accel_to_lattice(model.body_force_n_m3[1] / model.fluid.density_kg_m3),
        scaling.accel_to_lattice(model.body_force_n_m3[2] / model.fluid.density_kg_m3),
    ];

    // Thermal transport setup (M1/M3).
    let thermal = model.thermal;
    let (buoy_lat_per_k, t_ref_buoy, t_scale) = if let Some(t) = &thermal {
        let alpha_lat = t.diffusivity_m2_s * scaling.dt_s / (scaling.dx_m * scaling.dx_m);
        // Scalar relaxation time must sit in the same validated window
        // as the flow lattice.
        let tau_g = 3.0 * alpha_lat + 0.5;
        if !(crate::lattice::TAU_MIN..=crate::lattice::TAU_MAX).contains(&tau_g) {
            return Err(SolveError::ThermalUnstable { alpha_lat });
        }
        // Buoyant acceleration per kelvin: a = −g·β·(T − T_ref).
        let (bl, tref) = match t.buoyancy {
            Some(b) => (
                [
                    scaling.accel_to_lattice(-b.gravity_m_s2[0] * b.beta_per_k),
                    scaling.accel_to_lattice(-b.gravity_m_s2[1] * b.beta_per_k),
                    scaling.accel_to_lattice(-b.gravity_m_s2[2] * b.beta_per_k),
                ],
                b.t_ref_c,
            ),
            None => ([0.0; 3], 0.0),
        };
        let mut spread = (t.inlet_temp_c - t.initial_temp_c).abs();
        if let Some(w) = t.wall_temp_c {
            spread = spread
                .max((w - t.initial_temp_c).abs())
                .max((w - t.inlet_temp_c).abs());
        }
        if let Some(st) = &model.solid_temp_c {
            for v in st.iter().filter(|v| v.is_finite()) {
                spread = spread
                    .max((v - t.initial_temp_c).abs())
                    .max((v - t.inlet_temp_c).abs());
            }
        }
        (bl, tref, spread.max(1e-9))
    } else {
        ([0.0; 3], 0.0, 1.0)
    };
    let has_buoyancy = buoy_lat_per_k.iter().any(|b| *b != 0.0);
    let mut temp: Vec<f64> = match &thermal {
        Some(t) => vec![t.initial_temp_c; n],
        None => Vec::new(),
    };
    let mut temp_prev = temp.clone();
    // Second distribution for the temperature scalar (the FV route was
    // tried and rejected: cell-centered upwind advection on the LBM
    // velocity field violates the maximum principle near the inlet
    // because its discrete divergence differs from the lattice's; the
    // double-distribution scalar inherits the lattice's continuity).
    // g carries θ = T − T_inlet so boundary fluxes are baseline-free.
    let omega_g = thermal.map(|t| {
        1.0 / (3.0 * t.diffusivity_m2_s * scaling.dt_s / (scaling.dx_m * scaling.dx_m) + 0.5)
    });
    let mut g: Vec<f64> = match &thermal {
        Some(t) => {
            let theta0 = t.initial_temp_c - t.inlet_temp_c;
            let mut g = vec![0.0; n * Q];
            for x in 0..n {
                for q in 0..Q {
                    g[x * Q + q] = W[q] * theta0;
                }
            }
            g
        }
        None => Vec::new(),
    };
    let mut g_new = g.clone();
    // Boundary θ-exchange accumulators (per step, lattice units):
    // Dirichlet walls, inlet, outlet — the audit's second route.
    let mut q_wall_lat = 0.0f64;
    let mut q_inlet_lat = 0.0f64;
    let mut q_outlet_lat = 0.0f64;

    let cells = &model.cells;
    let mut f: Vec<f64> = vec![0.0; n * Q];
    let feq0 = equilibrium(1.0, [0.0; 3]);
    for x in 0..n {
        f[x * Q..(x + 1) * Q].copy_from_slice(&feq0);
    }
    let mut f_new = f.clone();
    let mut vel = vec![[0.0f64; 3]; n];
    let mut rho = vec![1.0f64; n];
    let mut vel_prev = vel.clone();

    let idx = |i: isize, j: isize, k: isize| -> usize { ((k * ny + j) * nx + i) as usize };

    let mut steps = 0usize;
    let mut residual = f64::INFINITY;
    let u_floor = scaling.u_lattice.max(1e-12);

    while steps < opts.max_steps {
        // Smoothstep inlet ramp: avoids the impulsive-start shock.
        let s = if opts.ramp_steps == 0 {
            1.0
        } else {
            let t = ((steps + 1) as f64 / opts.ramp_steps as f64).min(1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let u_in_now = [u_in_lat[0] * s, u_in_lat[1] * s, u_in_lat[2] * s];

        // Collision (fluid cells only), macroscopic update included.
        for x in 0..n {
            if cells[x] != Cell::Fluid {
                continue;
            }
            // Scalar moment first: θ from g, so buoyancy sees this
            // step's temperature.
            if let (Some(t), Some(_)) = (&thermal, omega_g) {
                let gx = &g[x * Q..(x + 1) * Q];
                let theta: f64 = gx.iter().sum();
                temp[x] = theta + t.inlet_temp_c;
            }
            let fx = &mut f[x * Q..(x + 1) * Q];
            let mut r = 0.0;
            let mut m = [0.0f64; 3];
            for (i, fi) in fx.iter().enumerate() {
                r += fi;
                m[0] += fi * C[i][0] as f64;
                m[1] += fi * C[i][1] as f64;
                m[2] += fi * C[i][2] as f64;
            }
            // Per-cell acceleration: global body force plus Boussinesq
            // buoyancy when a temperature field is coupled in.
            let a_cell = if has_buoyancy {
                let dtc = temp[x] - t_ref_buoy;
                [
                    a_lat[0] + buoy_lat_per_k[0] * dtc,
                    a_lat[1] + buoy_lat_per_k[1] * dtc,
                    a_lat[2] + buoy_lat_per_k[2] * dtc,
                ]
            } else {
                a_lat
            };
            let u = [
                (m[0] + 0.5 * a_cell[0] * r) / r,
                (m[1] + 0.5 * a_cell[1] * r) / r,
                (m[2] + 0.5 * a_cell[2] * r) / r,
            ];
            rho[x] = r;
            vel[x] = u;
            let feq = equilibrium(r, u);
            for i in 0..Q {
                let cu = C[i][0] as f64 * u[0] + C[i][1] as f64 * u[1] + C[i][2] as f64 * u[2];
                let ca = C[i][0] as f64 * a_cell[0]
                    + C[i][1] as f64 * a_cell[1]
                    + C[i][2] as f64 * a_cell[2];
                let ua = u[0] * a_cell[0] + u[1] * a_cell[1] + u[2] * a_cell[2];
                let guo = force_prefactor * W[i] * r * (3.0 * (ca - ua) + 9.0 * cu * ca);
                fx[i] += -omega * (fx[i] - feq[i]) + guo;
            }
            // Scalar BGK collision toward w_i·θ·(1 + 3c·u).
            if let (Some(t), Some(og)) = (&thermal, omega_g) {
                let gx = &mut g[x * Q..(x + 1) * Q];
                let theta = temp[x] - t.inlet_temp_c;
                for (i, gi) in gx.iter_mut().enumerate() {
                    let cu = C[i][0] as f64 * u[0] + C[i][1] as f64 * u[1] + C[i][2] as f64 * u[2];
                    let geq = W[i] * theta * (1.0 + 3.0 * cu);
                    *gi += -og * (*gi - geq);
                }
            }
        }

        // Streaming (pull) with boundary rules.
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = idx(i, j, k);
                    if cells[x] != Cell::Fluid {
                        continue;
                    }
                    let dst = &mut f_new[x * Q..(x + 1) * Q];
                    for q in 0..Q {
                        let (mut si, mut sj, mut sk) = (
                            i - C[q][0] as isize,
                            j - C[q][1] as isize,
                            k - C[q][2] as isize,
                        );
                        let mut outside = false;
                        for (axis, s, nmax) in
                            [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                        {
                            if *s < 0 || *s >= nmax {
                                if model.periodic[axis] {
                                    *s = (*s + nmax) % nmax;
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
                        dst[q] = match src_kind {
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

        // Thermal scalar streaming (M1): pull the g distribution with
        // boundary rules mirroring the flow lattice — bounce-back =
        // adiabatic wall, anti-bounce-back = Dirichlet surface (inlet
        // theta = 0, isothermal walls theta_w), copy-own = zero-gradient
        // outlet. Per-step boundary theta-exchange is accumulated as the
        // audit's link-level route.
        if let Some(t) = &thermal {
            q_wall_lat = 0.0;
            q_inlet_lat = 0.0;
            q_outlet_lat = 0.0;
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
                            for (axis, s, nmax) in
                                [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                            {
                                if *s < 0 || *s >= nmax {
                                    if model.periodic[axis] {
                                        *s = (*s + nmax) % nmax;
                                    } else {
                                        outside = true;
                                    }
                                }
                            }
                            let g_out = g[x * Q + OPP[q]];
                            let src_kind = if outside {
                                Cell::Solid
                            } else {
                                cells[idx(si, sj, sk)]
                            };
                            let val = match src_kind {
                                Cell::Fluid => g[idx(si, sj, sk) * Q + q],
                                Cell::Solid => {
                                    let wall_theta = if outside {
                                        t.wall_temp_c.map(|w| w - t.inlet_temp_c)
                                    } else {
                                        let sx = idx(si, sj, sk);
                                        model
                                            .solid_temp_c
                                            .as_ref()
                                            .map(|st| st[sx])
                                            .filter(|v| v.is_finite())
                                            .or(t.wall_temp_c)
                                            .map(|w| w - t.inlet_temp_c)
                                    };
                                    match wall_theta {
                                        // Dirichlet: anti-bounce-back.
                                        Some(tw) => {
                                            let v = -g_out + 2.0 * W[q] * tw;
                                            q_wall_lat += v - g_out;
                                            v
                                        }
                                        // Adiabatic: bounce-back (link
                                        // nets exactly zero).
                                        None => g_out,
                                    }
                                }
                                Cell::Inlet => {
                                    // Dirichlet theta = 0 at the inlet
                                    // surface, moving at the (ramped)
                                    // inlet velocity.
                                    let cu = C[q][0] as f64 * u_in_now[0]
                                        + C[q][1] as f64 * u_in_now[1]
                                        + C[q][2] as f64 * u_in_now[2];
                                    let v = -g_out + 2.0 * W[q] * 0.0 * (1.0 + 3.0 * cu);
                                    q_inlet_lat += v - g_out;
                                    v
                                }
                                Cell::Outlet => {
                                    // Zero-gradient: copy own
                                    // post-collision value.
                                    let v = g[x * Q + q];
                                    q_outlet_lat += g_out - v;
                                    v
                                }
                            };
                            g_new[x * Q + q] = val;
                        }
                    }
                }
            }
            std::mem::swap(&mut g, &mut g_new);
        }
        steps += 1;

        if steps.is_multiple_of(opts.check_every) && steps >= opts.ramp_steps {
            let mut max_delta = 0.0f64;
            let mut max_tdelta = 0.0f64;
            let mut finite = true;
            for x in 0..n {
                if cells[x] != Cell::Fluid {
                    continue;
                }
                let (u, p) = (vel[x], vel_prev[x]);
                for a in 0..3 {
                    if !u[a].is_finite() {
                        finite = false;
                    }
                    max_delta = max_delta.max((u[a] - p[a]).abs());
                }
                if thermal.is_some() {
                    if !temp[x].is_finite() {
                        finite = false;
                    }
                    max_tdelta = max_tdelta.max((temp[x] - temp_prev[x]).abs());
                }
            }
            if !finite {
                return Err(SolveError::Diverged { step: steps });
            }
            residual = (max_delta / u_floor).max(max_tdelta / t_scale);
            vel_prev.copy_from_slice(&vel);
            if thermal.is_some() {
                temp_prev.copy_from_slice(&temp);
            }
            if residual < opts.steady_tol {
                return Ok(finish(
                    model,
                    &scaling,
                    steps,
                    residual,
                    &vel,
                    &rho,
                    &temp,
                    (q_wall_lat, q_inlet_lat, q_outlet_lat),
                    dx_m,
                ));
            }
        }
    }

    Err(SolveError::NotConverged {
        steps,
        residual,
        tol: opts.steady_tol,
    })
}

/// Assemble the SI solution from the converged lattice fields.
#[allow(clippy::too_many_arguments)]
fn finish(
    model: &FlowModel,
    scaling: &Scaling,
    steps: usize,
    residual: f64,
    vel: &[[f64; 3]],
    rho: &[f64],
    temp: &[f64],
    q_boundary_lat: (f64, f64, f64),
    dx_m: f64,
) -> Solution {
    let (nx, ny, nz) = (
        model.divisions[0] as isize,
        model.divisions[1] as isize,
        model.divisions[2] as isize,
    );
    let n = vel.len();
    let idx = |i: isize, j: isize, k: isize| -> usize { ((k * ny + j) * nx + i) as usize };
    let mut velocity_m_s = vec![[0.0f64; 3]; n];
    let mut gauge_pressure_pa = vec![0.0f64; n];
    let mut max_speed: f64 = 0.0;
    for x in 0..n {
        if model.cells[x] != Cell::Fluid {
            continue;
        }
        let u = [
            scaling.velocity_to_si(vel[x][0]),
            scaling.velocity_to_si(vel[x][1]),
            scaling.velocity_to_si(vel[x][2]),
        ];
        velocity_m_s[x] = u;
        gauge_pressure_pa[x] = scaling.pressure_to_si(rho[x], model.fluid.density_kg_m3);
        max_speed = max_speed.max(norm(u));
    }

    // Inlet flow: the flux the moving-wall bounce-back actually injects,
    // summed over the boundary links — not the plug-flow U·A, which
    // over-counts patch edges (diagonal links into side walls inject
    // nothing; the honest number is the link sum).
    let u_in_lat = [
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[0]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[1]),
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[2]),
    ];
    let mut inlet_flux_lat = 0.0f64;
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = idx(i, j, k);
                if model.cells[x] != Cell::Fluid {
                    continue;
                }
                for q in 1..crate::lattice::Q {
                    let (mut si, mut sj, mut sk) = (
                        i - crate::lattice::C[q][0] as isize,
                        j - crate::lattice::C[q][1] as isize,
                        k - crate::lattice::C[q][2] as isize,
                    );
                    let mut outside = false;
                    for (axis, s, nmax) in
                        [(0usize, &mut si, nx), (1, &mut sj, ny), (2, &mut sk, nz)]
                    {
                        if *s < 0 || *s >= nmax {
                            if model.periodic[axis] {
                                *s = (*s + nmax) % nmax;
                            } else {
                                outside = true;
                            }
                        }
                    }
                    if outside {
                        continue;
                    }
                    if model.cells[idx(si, sj, sk)] == Cell::Inlet {
                        let cu = crate::lattice::C[q][0] as f64 * u_in_lat[0]
                            + crate::lattice::C[q][1] as f64 * u_in_lat[1]
                            + crate::lattice::C[q][2] as f64 * u_in_lat[2];
                        inlet_flux_lat += 6.0 * crate::lattice::W[q] * cu;
                    }
                }
            }
        }
    }
    let inlet_flow = inlet_flux_lat * dx_m * dx_m * dx_m / scaling.dt_s;

    // Port bookkeeping: exposed port faces are (port cell -> fluid
    // neighbor) pairs; the face normal points into the fluid.
    let face_area = dx_m * dx_m;
    let mut outlet_heat_flux = 0.0f64;
    let mut inlet_adv_flux = 0.0f64;
    let mut outlet_flow = 0.0f64;
    let mut inlet_p = (0.0f64, 0usize);
    let mut outlet_p = (0.0f64, 0usize);
    let dirs: [(isize, isize, isize); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = idx(i, j, k);
                let kind = model.cells[x];
                if kind != Cell::Inlet && kind != Cell::Outlet {
                    continue;
                }
                for (di, dj, dk) in dirs {
                    let (fi, fj, fk) = (i + di, j + dj, k + dk);
                    if fi < 0 || fj < 0 || fk < 0 || fi >= nx || fj >= ny || fk >= nz {
                        continue;
                    }
                    let fx = idx(fi, fj, fk);
                    if model.cells[fx] != Cell::Fluid {
                        continue;
                    }
                    // Normal from port cell into the fluid.
                    let nrm = [di as f64, dj as f64, dk as f64];
                    match kind {
                        Cell::Inlet => {
                            inlet_p.0 += gauge_pressure_pa[fx];
                            inlet_p.1 += 1;
                            if model.thermal.is_some() {
                                // Advective enthalpy influx as the
                                // scalar scheme sees it: cell-center
                                // velocity, inlet ghost temperature.
                                let u = velocity_m_s[fx];
                                let un = u[0] * nrm[0] + u[1] * nrm[1] + u[2] * nrm[2];
                                inlet_adv_flux += un * face_area;
                            }
                        }
                        Cell::Outlet => {
                            // Flow leaving the fluid through this face:
                            // fluid velocity against the inward normal.
                            let u = velocity_m_s[fx];
                            let un = -(u[0] * nrm[0] + u[1] * nrm[1] + u[2] * nrm[2]);
                            outlet_flow += un * face_area;
                            outlet_p.0 += gauge_pressure_pa[fx];
                            outlet_p.1 += 1;
                            if model.thermal.is_some() {
                                outlet_heat_flux += un * face_area * temp[fx];
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
    let mass_balance_residual = if inlet_flow.abs().max(outlet_flow.abs()) > 0.0 {
        (inlet_flow - outlet_flow).abs() / inlet_flow.abs().max(outlet_flow.abs())
    } else {
        0.0
    };
    let pressure_drop_pa = if inlet_p.1 > 0 && outlet_p.1 > 0 {
        inlet_p.0 / inlet_p.1 as f64 - outlet_p.0 / outlet_p.1 as f64
    } else {
        0.0
    };

    // Thermal outputs (M1).
    let (temperature_c, outlet_temp_c, heat_pickup_w, wall_heat_w) = match &model.thermal {
        None => (None, None, None, None),
        Some(t) => {
            let rho_cp = model.fluid.density_kg_m3 * t.heat_capacity_j_kg_k;
            let outlet_temp = if outlet_flow.abs() > f64::MIN_POSITIVE {
                Some(outlet_heat_flux / outlet_flow)
            } else {
                None
            };
            let heat_pickup = if outlet_flow.abs() > f64::MIN_POSITIVE {
                Some(rho_cp * (outlet_heat_flux - inlet_adv_flux * t.inlet_temp_c))
            } else {
                None
            };
            // Boundary heat, link route: the per-step θ-exchange the
            // scalar lattice actually performed through its Dirichlet
            // boundaries (isothermal walls + inlet exchange beyond the
            // advected baseline), converted to watts. The field-route
            // `heat_pickup_w` above is measured independently from the
            // outlet velocity/temperature fields — the thermal audit is
            // the gap between the two.
            let (qw, qi, _qo) = q_boundary_lat;
            let scale_w = rho_cp * dx_m * dx_m * dx_m / scaling.dt_s;
            let wall_heat = Some((qw + qi) * scale_w);
            (Some(temp.to_vec()), outlet_temp, heat_pickup, wall_heat)
        }
    };

    Solution {
        scaling: *scaling,
        steps,
        steady_residual: residual,
        velocity_m_s,
        gauge_pressure_pa,
        inlet_flow_m3_s: inlet_flow,
        outlet_flow_m3_s: outlet_flow,
        mass_balance_residual,
        pressure_drop_pa,
        max_speed_m_s: max_speed,
        temperature_c,
        outlet_temp_c,
        heat_pickup_w,
        wall_heat_w,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fluid;

    /// Body-force-driven Poiseuille flow between parallel plates:
    /// periodic in x and y, walls at ±z. Analytic:
    /// u(z) = (g/2ν)·z·(H−z), u_max = g·H²/(8ν).
    fn poiseuille_error(nz: usize) -> f64 {
        let nx = 4;
        let ny = 4;
        let mm = 1.0;
        let mut m = FlowModel::new(
            [0.0; 3],
            [nx as f64 * mm, ny as f64 * mm, nz as f64 * mm],
            [nx, ny, nz],
        );
        m.cells = vec![Cell::Fluid; nx * ny * nz];
        m.periodic = [true, true, false];
        m.fluid = Fluid::AIR_20C;
        let h = nz as f64 * mm / 1000.0;
        let nu = m.fluid.kinematic_viscosity_m2_s();
        // Target u_max ~ 0.05 m/s (laminar, Re_h ~ 20 for nz=21).
        let u_max = 0.05;
        let g = u_max * 8.0 * nu / (h * h);
        m.body_force_n_m3 = [g * m.fluid.density_kg_m3, 0.0, 0.0];

        let opts = SolveOptions {
            u_ref_m_s: Some(u_max),
            steady_tol: 1e-7,
            ..Default::default()
        };
        let sol = solve_steady(&m, &opts).expect("poiseuille solve");

        // Compare the u_x profile along z at (i=0, j=0) against the
        // analytic parabola at cell centers (walls half-way outside the
        // first/last cells -> channel exactly H wide).
        let dx = mm / 1000.0;
        let mut num = 0.0;
        let mut den = 0.0;
        for k in 0..nz {
            let z = (k as f64 + 0.5) * dx;
            let exact = g / (2.0 * nu) * z * (h - z);
            let got = sol.velocity_m_s[m.index(0, 0, k)][0];
            num += (got - exact) * (got - exact);
            den += exact * exact;
        }
        (num / den).sqrt()
    }

    #[test]
    fn poiseuille_profile_under_one_percent() {
        let err = poiseuille_error(21);
        assert!(err < 0.01, "L2 profile error {err:.4} >= 1%");
    }

    #[test]
    fn poiseuille_second_order_convergence() {
        let coarse = poiseuille_error(11);
        let fine = poiseuille_error(21);
        // Grid ratio ~1.9; second order predicts error ratio ~3.6. Allow
        // margin for the BGK slip term, require >= 2.0.
        assert!(
            coarse / fine >= 2.0,
            "convergence order too low: coarse {coarse:.5} / fine {fine:.5} = {:.2}",
            coarse / fine
        );
    }

    /// Straight square duct with inlet/outlet: mass must balance and the
    /// pressure drop must agree with the laminar rectangular-duct oracle.
    #[test]
    fn duct_mass_balance_and_pressure_drop() {
        let (nx, ny, nz) = (40usize, 9usize, 9usize);
        let mm = 1.0;
        let mut m = FlowModel::new(
            [0.0; 3],
            [nx as f64 * mm, ny as f64 * mm, nz as f64 * mm],
            [nx, ny, nz],
        );
        for k in 0..nz {
            for j in 0..ny {
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
        // Slow enough that the laminar entrance length (0.05·Re·D_h)
        // fits well before the measurement window — the oracle knows
        // only developed flow.
        m.inlet_velocity_m_s = [0.02, 0.0, 0.0];

        let re = m.reynolds().unwrap();
        let dh = m.inlet_hydraulic_diameter_mm().unwrap() / 1000.0;
        let entrance = crate::lumped::entrance_length_m(re, dh);
        let window_start = (nx / 4) as f64 * 1.0e-3;
        assert!(
            entrance < window_start,
            "test precondition: entrance {entrance:.4} m must fit before {window_start:.4} m"
        );

        let sol = solve_steady(&m, &SolveOptions::default()).expect("duct solve");

        assert!(
            sol.mass_balance_residual < 0.01,
            "mass balance {:.4}",
            sol.mass_balance_residual
        );

        // Oracle: developed laminar square-duct pressure gradient over
        // the developed core. Compare gradient (Pa/m) computed from the
        // interior pressure field against the oracle to sidestep
        // entrance effects at the ports.
        let w = ny as f64 * mm / 1000.0;
        let q = sol.inlet_flow_m3_s;
        let dpdx_oracle =
            crate::lumped::rect_duct_pressure_gradient_pa_m(q, w, w, m.fluid.viscosity_pa_s);
        let dx_m = mm / 1000.0;
        let (jm, km) = (ny / 2, nz / 2);
        let (i0, i1) = (nx / 4, 3 * nx / 4);
        let p0 = sol.gauge_pressure_pa[m.index(i0, jm, km)];
        let p1 = sol.gauge_pressure_pa[m.index(i1, jm, km)];
        let dpdx_field = (p0 - p1) / ((i1 - i0) as f64 * dx_m);
        let rel = (dpdx_field - dpdx_oracle).abs() / dpdx_oracle.abs();
        assert!(
            rel < 0.08,
            "duct dp/dx field {dpdx_field:.4} vs oracle {dpdx_oracle:.4} Pa/m: {:.1}% off",
            rel * 100.0
        );
    }

    /// Lid-driven cavity at Re = 100 (quasi-2D: one periodic-y layer)
    /// against the Ghia, Ghia & Shin (1982) 129² reference: u_x along
    /// the vertical centerline. Release-ladder rung — run with
    /// `cargo test --release -p vcad-kernel-flow -- --ignored`.
    #[test]
    #[ignore = "release-ladder rung: ~seconds in release, minutes in debug"]
    fn lid_driven_cavity_re100_matches_ghia() {
        let n = 33usize;
        let mm = 1.0;
        let (nx, ny, nz) = (n, 1usize, n + 1);
        let mut m = FlowModel::new(
            [0.0; 3],
            [nx as f64 * mm, ny as f64 * mm, nz as f64 * mm],
            [nx, ny, nz],
        );
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = m.index(i, j, k);
                    m.cells[x] = if k == nz - 1 {
                        Cell::Inlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        m.periodic = [false, true, false];
        let l = n as f64 * mm / 1000.0;
        let u_lid = 0.05;
        let re = 100.0;
        // Synthetic fluid with exactly Re = U·L/ν.
        m.fluid = Fluid {
            density_kg_m3: 1.0,
            viscosity_pa_s: u_lid * l / re,
        };
        m.inlet_velocity_m_s = [u_lid, 0.0, 0.0];

        let opts = SolveOptions {
            steady_tol: 1e-7,
            ..Default::default()
        };
        let sol = solve_steady(&m, &opts).expect("cavity solve");

        // Ghia et al., table I, Re = 100: (y/L, u/U_lid) on the vertical
        // centerline through the cavity center.
        let ghia: [(f64, f64); 11] = [
            (0.9766, 0.84123),
            (0.9531, 0.68717),
            (0.8516, 0.23151),
            (0.7344, 0.00332),
            (0.6172, -0.13641),
            (0.5000, -0.20581),
            (0.4531, -0.21090),
            (0.2813, -0.15662),
            (0.1719, -0.10150),
            (0.1016, -0.06434),
            (0.0625, -0.04192),
        ];
        // Sample u_x at the centerline column (linear interpolation
        // between cell centers), normalized by the lid speed.
        let ic = nx / 2;
        let sample = |y_frac: f64| -> f64 {
            let z = y_frac * n as f64 - 0.5;
            let k0 = (z.floor().max(0.0) as usize).min(n - 1);
            let k1 = (k0 + 1).min(n - 1);
            let t = (z - k0 as f64).clamp(0.0, 1.0);
            let u0 = sol.velocity_m_s[m.index(ic, 0, k0)][0];
            let u1 = sol.velocity_m_s[m.index(ic, 0, k1)][0];
            (u0 * (1.0 - t) + u1 * t) / u_lid
        };
        let mut max_err = 0.0f64;
        let mut sq = 0.0f64;
        for (y, u_ref) in ghia {
            let u = sample(y);
            let err = (u - u_ref).abs();
            max_err = max_err.max(err);
            sq += err * err;
        }
        let rms = (sq / ghia.len() as f64).sqrt();
        assert!(
            max_err < 0.05 && rms < 0.03,
            "cavity vs Ghia: max err {max_err:.4}, rms {rms:.4} (33^2 grid)"
        );
    }

    fn thermal_duct(nx: usize, w: usize, u: f64) -> FlowModel {
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
        m.thermal = Some(crate::model::ThermalTransport::AIR_20C);
        m
    }

    /// M1: adiabatic duct conserves enthalpy — outlet temperature equals
    /// inlet temperature to solver tolerance.
    #[test]
    fn adiabatic_duct_conserves_temperature() {
        let mut m = thermal_duct(30, 7, 0.08);
        let t = m.thermal.as_mut().unwrap();
        t.inlet_temp_c = 47.0;
        t.initial_temp_c = 20.0;
        t.wall_temp_c = None;
        let sol = solve_steady(&m, &SolveOptions::default()).expect("adiabatic duct");
        let t_out = sol.outlet_temp_c.unwrap();
        assert!(
            (t_out - 47.0).abs() < 0.2,
            "adiabatic outlet temp {t_out:.3} C, expected 47"
        );
    }

    /// M1: isothermal-wall duct — the thermal audit closes (heat picked
    /// up by the fluid equals heat in through the walls) and the outlet
    /// temperature sits strictly between inlet and wall.
    #[test]
    fn heated_wall_duct_energy_audit_closes() {
        let mut m = thermal_duct(40, 7, 0.06);
        let t = m.thermal.as_mut().unwrap();
        t.inlet_temp_c = 20.0;
        t.initial_temp_c = 20.0;
        t.wall_temp_c = Some(60.0);
        let opts = SolveOptions {
            steady_tol: 1e-7,
            ..Default::default()
        };
        let sol = solve_steady(&m, &opts).expect("heated duct");
        let t_out = sol.outlet_temp_c.unwrap();
        assert!(
            t_out > 21.0 && t_out < 60.0,
            "outlet temp {t_out:.2} C out of (20, 60)"
        );
        let q = sol.heat_pickup_w.unwrap();
        let w = sol.wall_heat_w.unwrap();
        assert!(q > 0.0 && w > 0.0);
        // The two routes discretize differently (cell-center fields at
        // the outlet vs lattice link exchange at the boundaries); at
        // 7x7 across they agree to ~4%. This is a cross-route
        // consistency check, not an energy-conservation check — the
        // scalar lattice conserves theta exactly by construction.
        let resid = (q - w).abs() / q.abs().max(w.abs());
        assert!(
            resid < 0.05,
            "thermal audit: pickup {q:.4e} W vs wall {w:.4e} W ({:.1}% off)",
            resid * 100.0
        );
    }

    /// M1: slower flow spends longer against hot walls and leaves
    /// hotter — the qualitative Graetz trend.
    #[test]
    fn slower_flow_leaves_hotter() {
        let mut temps = Vec::new();
        for u in [0.03f64, 0.09] {
            let mut m = thermal_duct(40, 7, u);
            let t = m.thermal.as_mut().unwrap();
            t.wall_temp_c = Some(60.0);
            let sol = solve_steady(&m, &SolveOptions::default()).expect("duct");
            temps.push(sol.outlet_temp_c.unwrap());
        }
        assert!(
            temps[0] > temps[1] + 1.0,
            "expected slower flow hotter: {temps:?}"
        );
    }

    /// M1: refusal when the explicit scalar scheme would be unstable.
    #[test]
    fn thermal_instability_is_refused() {
        let mut m = thermal_duct(30, 7, 0.05);
        m.thermal.as_mut().unwrap().diffusivity_m2_s = 1.0;
        assert!(matches!(
            solve_steady(&m, &SolveOptions::default()),
            Err(SolveError::ThermalUnstable { .. })
        ));
    }

    /// M3: differentially heated square cavity, Ra = 10³, Pr = 0.71 vs
    /// de Vahl Davis: mean hot-wall Nusselt = 1.118. Release-ladder
    /// rung: `cargo test --release -p vcad-kernel-flow -- --ignored`.
    #[test]
    #[ignore = "release-ladder rung: ~seconds in release, minutes in debug"]
    fn heated_cavity_ra1e3_matches_de_vahl_davis() {
        let n = 33usize;
        // Fluid cavity n x n with one solid column on each side (hot
        // left, cold right); top/bottom out-of-domain walls (adiabatic).
        let (nx, ny, nz) = (n + 2, 1usize, n);
        let mut m = FlowModel::new([0.0; 3], [nx as f64, ny as f64, nz as f64], [nx, ny, nz]);
        for k in 0..nz {
            for i in 0..nx {
                let x = m.index(i, 0, k);
                m.cells[x] = if i == 0 || i == nx - 1 {
                    Cell::Solid
                } else {
                    Cell::Fluid
                };
            }
        }
        m.periodic = [false, true, false];

        let (t_hot, t_cold) = (30.0, 20.0);
        let dt = t_hot - t_cold;
        let l = n as f64 / 1000.0;
        let pr = 0.71;
        let ra = 1.0e3;
        // Pick nu, derive alpha from Pr and beta*g from Ra.
        let nu = 1.5e-5;
        let alpha = nu / pr;
        let g_beta = ra * nu * alpha / (dt * l * l * l);
        m.fluid = Fluid {
            density_kg_m3: 1.0,
            viscosity_pa_s: nu,
        };
        m.thermal = Some(crate::model::ThermalTransport {
            inlet_temp_c: 0.5 * (t_hot + t_cold),
            initial_temp_c: 0.5 * (t_hot + t_cold),
            wall_temp_c: None,
            diffusivity_m2_s: alpha,
            heat_capacity_j_kg_k: 1000.0,
            buoyancy: Some(crate::model::Boussinesq {
                beta_per_k: g_beta / 9.81,
                t_ref_c: 0.5 * (t_hot + t_cold),
                gravity_m_s2: [0.0, 0.0, -9.81],
            }),
        });
        let mut st = vec![f64::NAN; nx * ny * nz];
        for k in 0..nz {
            st[m.index(0, 0, k)] = t_hot;
            st[m.index(nx - 1, 0, k)] = t_cold;
        }
        m.solid_temp_c = Some(st);

        // Characteristic buoyancy velocity for the scaling.
        let u_ref = (g_beta * dt * l).sqrt();
        let opts = SolveOptions {
            u_ref_m_s: Some(u_ref),
            steady_tol: 1e-7,
            max_steps: 2_000_000,
            ..Default::default()
        };
        let sol = solve_steady(&m, &opts).expect("cavity solve");
        let temp = sol.temperature_c.as_ref().unwrap();

        // Mean hot-wall Nusselt from the half-cell fluxes:
        // Nu = 2·Σ(T_hot − T_adjacent) / ΔT · (1/n) · n_width-normalized
        // — conduction-only linear profile gives exactly 1.
        let mut sum = 0.0;
        for k in 0..nz {
            sum += t_hot - temp[m.index(1, 0, k)];
        }
        // Reference: conduction flux ΔT/n per face; actual half-cell
        // flux 2(Th−T_adj) per face; both share alpha and face count.
        // Sanity: a linear conduction profile (T_adj = Th − 0.5·ΔT/n)
        // gives exactly Nu = 1.
        let nu_avg = 2.0 * sum * n as f64 / (nz as f64 * dt);
        assert!(
            (nu_avg - 1.118).abs() < 0.08,
            "cavity Nu = {nu_avg:.4}, de Vahl Davis 1.118"
        );
    }

    #[test]
    fn unconverged_budget_is_an_error() {
        let (nx, ny, nz) = (30usize, 7usize, 7usize);
        let mut m = FlowModel::new([0.0; 3], [30.0, 7.0, 7.0], [nx, ny, nz]);
        for k in 0..nz {
            for j in 0..ny {
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
        m.inlet_velocity_m_s = [0.1, 0.0, 0.0];
        let opts = SolveOptions {
            max_steps: 400,
            check_every: 200,
            steady_tol: 1e-14,
            ..Default::default()
        };
        assert!(matches!(
            solve_steady(&m, &opts),
            Err(SolveError::NotConverged { .. })
        ));
    }
}
