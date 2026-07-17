//! Transient conduction: backward (implicit) Euler time stepping.
//!
//! Discretization: with per-voxel thermal mass C = ρc_p·V (J/K),
//!
//! ```text
//! (C/Δt + A) · Tⁿ⁺¹ = (C/Δt) · Tⁿ + b
//! ```
//!
//! where A and b are the steady conduction operator and drive from
//! [`crate::solve`]. Backward Euler is chosen deliberately: conduction is
//! stiff (the stable explicit step scales with Δx², which is brutal at
//! fine grids), and implicit stepping is unconditionally stable, letting
//! Δt follow the *physics* time scale instead of the grid. The cost is
//! first-order accuracy in Δt — halve the step to check convergence in
//! time, exactly like halving the grid checks convergence in space.
//!
//! Each step is one SPD solve with the same operator plus a positive
//! diagonal (C/Δt), so it reuses the steady PCG, warm-started from the
//! previous step (a handful of iterations per step in practice).
//!
//! **The energy audit is the conscience.** Summing the discrete update
//! over free voxels makes every internal face cancel (the scheme is
//! conservative), leaving *exactly*
//!
//! ```text
//! ΣC·(Tⁿ⁺¹ − Tⁿ) = Δt · (P_source − P_out(Tⁿ⁺¹))
//! ```
//!
//! so stored-energy change must equal net injected energy to CG tolerance
//! — not as an approximation but as an identity of the discretization.
//! [`TransientSolution::energy_audit_residual_rel`] reports how well the
//! computed trajectory honors it over the whole run.

use crate::model::{ModelError, ThermalModel};
use crate::solve::{
    assemble_solution, build, pcg, resolve_reference, Solution, SolveError, SolveOptions,
};
use serde::Serialize;

/// Options for [`solve_transient`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransientOptions {
    /// Time step, seconds (must be > 0). Backward Euler is stable for any
    /// Δt; accuracy is first order, so Δt should resolve the fastest time
    /// constant you care about.
    pub dt_s: f64,
    /// Number of steps (must be ≥ 1).
    pub steps: usize,
    /// Uniform initial temperature of all free voxels, °C. (Pinned
    /// reservoirs hold their fixed temperatures from t = 0.)
    pub initial_c: f64,
    /// Record a full field snapshot every this many steps (0 = none; the
    /// final state is always available).
    pub snapshot_every: usize,
}

/// A recorded temperature field at one instant.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// Simulation time, s.
    pub time_s: f64,
    /// Temperature per voxel, °C (`NaN` for void), same layout as
    /// [`Solution::t_c`].
    pub t_c: Vec<f64>,
}

/// The transient run: per-step series, the final state, and the audit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransientSolution {
    /// Time after each step, s (length = steps).
    pub times_s: Vec<f64>,
    /// Hottest solid-voxel temperature after each step, °C.
    pub t_max_c: Vec<f64>,
    /// Per-source hottest temperature after each step, °C
    /// (`[source][step]`, sources in model order).
    pub source_t_max_c: Vec<Vec<f64>>,
    /// Full report at the final time. Its `energy.residual_rel` is the
    /// steady-state balance residual of the *final field* — i.e. the
    /// distance from steady state (it approaches solver tolerance as the
    /// run converges to steady).
    pub final_state: Solution,
    /// Total stored-energy change ΣC·(T_end − T_0), J.
    pub stored_delta_j: f64,
    /// Net injected energy ∫(P_source − P_out) dt over the run, J.
    pub injected_j: f64,
    /// |stored − injected| / max(|stored|, |injected|): the conservation
    /// identity of the scheme, honored to CG tolerance.
    pub energy_audit_residual_rel: f64,
    /// Total CG iterations across all steps.
    pub cg_iterations_total: usize,
    /// Recorded snapshots (empty when `snapshot_every` = 0).
    pub snapshots: Vec<Snapshot>,
}

/// Solve the transient conduction problem by backward Euler.
pub fn solve_transient(
    model: &ThermalModel,
    opts: &SolveOptions,
    topts: &TransientOptions,
) -> Result<TransientSolution, SolveError> {
    if topts.dt_s <= 0.0 || !topts.dt_s.is_finite() || topts.steps == 0 {
        return Err(SolveError::InvalidTimeStep);
    }
    let sys = build(model)?;
    let reference_c = resolve_reference(&sys, model)?;
    let nvox = sys.b.len();

    // Thermal mass per voxel; every solid voxel must have one (fail
    // closed, naming the offending material region).
    let mut cap = vec![0.0_f64; nvox];
    for (p, c) in cap.iter_mut().enumerate() {
        if sys.solid[p] {
            if sys.rc[p] <= 0.0 {
                return Err(ModelError::MissingHeatCapacity {
                    index: sys.mat_id[p],
                }
                .into());
            }
            *c = sys.rc[p] * sys.cell_volume_m3;
        }
    }
    let cdt: Vec<f64> = cap.iter().map(|c| c / topts.dt_s).collect();
    let diag_t: Vec<f64> = sys.diag.iter().zip(&cdt).map(|(d, c)| d + c).collect();

    let mut t: Vec<f64> = (0..nvox)
        .map(|p| if sys.free[p] { topts.initial_c } else { 0.0 })
        .collect();
    let fixed_t_max = (0..nvox)
        .filter(|&p| sys.solid[p] && !sys.free[p])
        .map(|p| sys.tfix[p])
        .fold(f64::NEG_INFINITY, f64::max);

    let mut times_s = Vec::with_capacity(topts.steps);
    let mut t_max_series = Vec::with_capacity(topts.steps);
    let mut source_series: Vec<Vec<f64>> = sys
        .sources
        .iter()
        .map(|_| Vec::with_capacity(topts.steps))
        .collect();
    let mut snapshots = Vec::new();
    let mut rhs = vec![0.0_f64; nvox];
    let mut stored_delta_j = 0.0;
    let mut injected_j = 0.0;
    let mut cg_iterations_total = 0usize;
    let mut last_iters = 0usize;
    let mut last_resid = 0.0_f64;

    for step in 1..=topts.steps {
        for p in 0..nvox {
            rhs[p] = if sys.free[p] {
                sys.b[p] + cdt[p] * t[p]
            } else {
                0.0
            };
        }
        let (x, iters, resid) = pcg(&sys, &diag_t, &rhs, &t, opts)?;
        cg_iterations_total += iters;
        last_iters = iters;
        last_resid = resid;

        // Conservation audit of this step (an identity of the scheme).
        let mut de = 0.0;
        for p in 0..nvox {
            if sys.free[p] {
                de += cap[p] * (x[p] - t[p]);
            }
        }
        stored_delta_j += de;
        injected_j += topts.dt_s * (sys.source_w_total - sys.boundary_outflow_w(&x));

        t = x;
        let time = step as f64 * topts.dt_s;
        times_s.push(time);
        let mut tmax = fixed_t_max;
        for (p, &tv) in t.iter().enumerate() {
            if sys.free[p] && tv > tmax {
                tmax = tv;
            }
        }
        t_max_series.push(tmax);
        for (series, (_, _, ids)) in source_series.iter_mut().zip(&sys.sources) {
            let mut m = f64::NEG_INFINITY;
            for &p in ids {
                if t[p] > m {
                    m = t[p];
                }
            }
            series.push(m);
        }
        if topts.snapshot_every > 0 && step % topts.snapshot_every == 0 {
            snapshots.push(Snapshot {
                time_s: time,
                t_c: full_field(&sys, &t),
            });
        }
    }

    let audit_scale = stored_delta_j.abs().max(injected_j.abs()).max(1e-30);
    let final_state = assemble_solution(&sys, model, &t, last_iters, last_resid, reference_c);
    Ok(TransientSolution {
        times_s,
        t_max_c: t_max_series,
        source_t_max_c: source_series,
        final_state,
        stored_delta_j,
        injected_j,
        energy_audit_residual_rel: (stored_delta_j - injected_j).abs() / audit_scale,
        cg_iterations_total,
        snapshots,
    })
}

fn full_field(sys: &crate::solve::System, x: &[f64]) -> Vec<f64> {
    (0..x.len())
        .map(|p| {
            if sys.free[p] {
                x[p]
            } else if sys.solid[p] {
                sys.tfix[p]
            } else {
                f64::NAN
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};

    /// Lumped capacitance: a 10 mm aluminum-ish cube (k = 300,
    /// ρc = 2.43e6 J/m³K) convecting from all six faces with h = 20 from
    /// 100 °C into 25 °C ambient. Biot = h(V/A)/k ≈ 1e-4 ≪ 1, so the
    /// exact solution is the single-exponential decay
    /// T(t) = T∞ + (T₀−T∞)·exp(−t/τ), τ = ρcV/(hA) ≈ 202.6 s
    /// (with the half-cell correction, h_eff differs from h by 8e-5 —
    /// negligible). Backward Euler at Δt = τ/200 tracks it to ~0.25%.
    #[test]
    fn lumped_capacitance_decay_matches_the_exponential() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [10.0, 10.0, 10.0], [4, 4, 4]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [10.0, 10.0, 10.0],
                },
                300.0,
            )
            .with_heat_capacity(2.43e6),
        );
        let conv = Boundary::Convection {
            h_w_m2k: 20.0,
            ambient_c: 25.0,
        };
        m.domain_faces = [conv; 6];
        let sol = solve_transient(
            &m,
            &SolveOptions::default(),
            &TransientOptions {
                dt_s: 1.0,
                steps: 400,
                initial_c: 100.0,
                snapshot_every: 0,
            },
        )
        .unwrap();

        let tau = 2.43e6 * 1e-6 / (20.0 * 6.0e-4);
        for &probe in &[100usize, 200, 400] {
            let t_num = sol.t_max_c[probe - 1];
            let time = probe as f64;
            let exact = 25.0 + 75.0 * (-time / tau).exp();
            assert!(
                (t_num - exact).abs() < 0.005 * 75.0,
                "t = {time} s: computed {t_num:.3}, lumped exact {exact:.3}"
            );
        }
        // The conservation identity holds to solver tolerance.
        assert!(
            sol.energy_audit_residual_rel < 1e-6,
            "energy audit residual {}",
            sol.energy_audit_residual_rel
        );
        // Cooling: stored energy fell, boundaries carried it out.
        assert!(sol.stored_delta_j < 0.0);
    }

    /// Semi-infinite solid: face stepped to 100 °C at t = 0, body
    /// initially at 0 °C. Exact: T(x,t) = T_s·erfc(x / 2√(αt)) (Carslaw &
    /// Jaeger). α = k/ρc = 1e-6 m²/s here; at t = 100 s the thermal
    /// penetration √(αt) = 10 mm, and the 50 mm domain's far end sits at
    /// η = 2.5 where erfc ≈ 4e-4 — effectively still semi-infinite.
    #[test]
    fn semi_infinite_solid_follows_erfc() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [50.0, 5.0, 5.0], [100, 1, 1]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [50.0, 5.0, 5.0],
                },
                1.0,
            )
            .with_heat_capacity(1.0e6),
        );
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 100.0,
        };
        let sol = solve_transient(
            &m,
            &SolveOptions::default(),
            &TransientOptions {
                dt_s: 0.25,
                steps: 400,
                initial_c: 0.0,
                snapshot_every: 400,
            },
        )
        .unwrap();
        let field = &sol.snapshots[0].t_c;

        let alpha: f64 = 1.0 / 1.0e6;
        let sqrt_at = (alpha * 100.0).sqrt();
        for &(x_mm, i) in &[(5.25_f64, 10usize), (10.25, 20), (20.25, 40)] {
            let eta = x_mm * 1e-3 / (2.0 * sqrt_at);
            let exact = 100.0 * erfc(eta);
            let got = field[i];
            assert!(
                (got - exact).abs() < 1.5,
                "x = {x_mm} mm: computed {got:.3}, erfc exact {exact:.3}"
            );
        }
        assert!(sol.energy_audit_residual_rel < 1e-6);
    }

    /// A long transient run lands on the steady solution: same
    /// chip-on-plate as the steady energy-balance test, run ~16 time
    /// constants. The final field must match `solve_steady` and its
    /// steady balance residual (= distance from steady) must be small.
    #[test]
    fn transient_relaxes_to_the_steady_state() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [60.0, 60.0, 2.0], [30, 30, 2]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [60.0, 60.0, 2.0],
                },
                15.0,
            )
            .with_heat_capacity(1.8e6),
        );
        m.sources.push(PowerSource {
            name: "die".into(),
            shape: Shape::Box {
                min_mm: [25.0, 25.0, 0.0],
                size_mm: [10.0, 10.0, 2.0],
            },
            power_w: 2.0,
        });
        let conv = Boundary::Convection {
            h_w_m2k: 15.0,
            ambient_c: 25.0,
        };
        m.domain_faces[4] = conv;
        m.domain_faces[5] = conv;

        let steady = crate::solve::solve_steady(&m, &SolveOptions::default()).unwrap();
        let trans = solve_transient(
            &m,
            &SolveOptions::default(),
            &TransientOptions {
                dt_s: 10.0,
                steps: 200,
                initial_c: 25.0,
                snapshot_every: 0,
            },
        )
        .unwrap();

        let mut max_diff = 0.0_f64;
        for (a, b) in trans.final_state.t_c.iter().zip(&steady.t_c) {
            if !a.is_nan() {
                max_diff = max_diff.max((a - b).abs());
            }
        }
        assert!(
            max_diff < 0.01,
            "final transient field differs from steady by {max_diff} K"
        );
        assert!(
            trans.final_state.energy.residual_rel < 1e-3,
            "distance from steady: {}",
            trans.final_state.energy.residual_rel
        );
        // Monotone approach: T_max never overshoots steady from below.
        let steady_max = steady.t_max_c;
        for &t in &trans.t_max_c {
            assert!(t <= steady_max + 1e-6);
        }
        assert!(trans.energy_audit_residual_rel < 1e-6);
    }

    #[test]
    fn missing_heat_capacity_fails_closed() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [10.0, 5.0, 5.0], [4, 1, 1]);
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [10.0, 5.0, 5.0],
            },
            50.0,
        ));
        m.domain_faces[0] = Boundary::FixedTemperature { temperature_c: 0.0 };
        let err = solve_transient(
            &m,
            &SolveOptions::default(),
            &TransientOptions {
                dt_s: 1.0,
                steps: 1,
                initial_c: 20.0,
                snapshot_every: 0,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SolveError::Model(ModelError::MissingHeatCapacity { index: 0 })
        ));
    }

    #[test]
    fn bad_time_step_fails_closed() {
        let m = ThermalModel::new([0.0, 0.0, 0.0], [10.0, 5.0, 5.0], [4, 1, 1]);
        for (dt, steps) in [(0.0, 10), (-1.0, 10), (1.0, 0)] {
            let err = solve_transient(
                &m,
                &SolveOptions::default(),
                &TransientOptions {
                    dt_s: dt,
                    steps,
                    initial_c: 0.0,
                    snapshot_every: 0,
                },
            )
            .unwrap_err();
            assert!(matches!(err, SolveError::InvalidTimeStep));
        }
    }

    /// Abramowitz & Stegun 7.1.26 rational approximation (|ε| ≤ 1.5e-7),
    /// plenty for a 1.5% validation tolerance.
    fn erfc(x: f64) -> f64 {
        let t = 1.0 / (1.0 + 0.327_591_1 * x);
        let poly = t
            * (0.254_829_592
                + t * (-0.284_496_736
                    + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
        poly * (-x * x).exp()
    }
}
