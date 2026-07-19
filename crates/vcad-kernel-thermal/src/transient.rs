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

use crate::model::{Boundary, ModelError, ThermalModel};
use crate::solve::{
    assemble_solution, build, pcg, resolve_reference, Solution, SolveError, SolveOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// (`[source][step]`, sources in model order). A zero-power source
    /// doubles as a temperature probe: it injects nothing but its
    /// region's hottest temperature is tracked here every step.
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
    /// |stored − injected| normalized by the gross energy traffic of the
    /// run (Σ|per-step stored change|, Σ|per-step net injection|, and the
    /// net totals — whichever is largest): the conservation identity of
    /// the scheme, honored to CG tolerance. Gross normalization keeps the
    /// audit meaningful for round-trip schedules (heat then cool back)
    /// where the *net* totals both approach zero.
    pub energy_audit_residual_rel: f64,
    /// Total CG iterations across all steps.
    pub cg_iterations_total: usize,
    /// Recorded snapshots (empty when `snapshot_every` = 0).
    pub snapshots: Vec<Snapshot>,
}

/// One piecewise-constant interval of a transient run's drive schedule.
///
/// Within a segment every source power and boundary temperature is held
/// constant; between segments the named overrides switch step-wise. This
/// is exactly the shape of the problems transient conduction gets asked
/// in practice: an RTP recipe is ramp/soak/cool segments of lamp power,
/// an overlay-drift study is an ambient step. Overrides are **numbers
/// only** — a segment may retune a source's watts or a boundary's
/// temperature, never add/remove regions or change which faces convect
/// (structure is provenance; changing it mid-run would silently change
/// the mesh's free set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleSegment {
    /// Time step within this segment, s (> 0).
    pub dt_s: f64,
    /// Number of steps in this segment (≥ 1).
    pub steps: usize,
    /// Source-power overrides by source name, W (fail-closed: an unknown
    /// name is an error). Sources not named keep the model's power.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_power_w: BTreeMap<String, f64>,
    /// Domain-face temperature overrides by BC slot (0..=5 in
    /// `[-x,+x,-y,+y,-z,+z]` order, 6 = the exposed BC). Retunes a
    /// `FixedTemperature`'s temperature or a `Convection`'s ambient;
    /// overriding an `Adiabatic` face is an error (fail-closed — there
    /// is no temperature there to move).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub face_temperature_c: BTreeMap<usize, f64>,
    /// Fixed-region temperature overrides by index into
    /// `ThermalModel::fixed` (fail-closed on out-of-range).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fixed_temperature_c: BTreeMap<usize, f64>,
}

impl ScheduleSegment {
    /// A plain segment with no overrides.
    pub fn plain(dt_s: f64, steps: usize) -> Self {
        Self {
            dt_s,
            steps,
            source_power_w: BTreeMap::new(),
            face_temperature_c: BTreeMap::new(),
            fixed_temperature_c: BTreeMap::new(),
        }
    }

    /// The base model with this segment's overrides applied (fail-closed).
    fn apply(&self, base: &ThermalModel) -> Result<ThermalModel, SolveError> {
        let mut m = base.clone();
        for (name, &power_w) in &self.source_power_w {
            let src = m
                .sources
                .iter_mut()
                .find(|s| &s.name == name)
                .ok_or_else(|| {
                    SolveError::BadScheduleOverride(format!("unknown source {name:?}"))
                })?;
            src.power_w = power_w;
        }
        for (&slot, &temp) in &self.face_temperature_c {
            let bc = if slot < 6 {
                &mut m.domain_faces[slot]
            } else if slot == 6 {
                &mut m.exposed
            } else {
                return Err(SolveError::BadScheduleOverride(format!(
                    "face slot {slot} out of range (0..=5 domain faces, 6 exposed)"
                )));
            };
            match bc {
                Boundary::FixedTemperature { temperature_c } => *temperature_c = temp,
                Boundary::Convection { ambient_c, .. } => *ambient_c = temp,
                Boundary::Adiabatic => {
                    return Err(SolveError::BadScheduleOverride(format!(
                        "face slot {slot} is adiabatic; it has no temperature to schedule"
                    )));
                }
            }
        }
        for (&index, &temp) in &self.fixed_temperature_c {
            let fx = m.fixed.get_mut(index).ok_or_else(|| {
                SolveError::BadScheduleOverride(format!(
                    "fixed region {index} out of range ({} regions)",
                    base.fixed.len()
                ))
            })?;
            fx.temperature_c = temp;
        }
        Ok(m)
    }
}

/// Solve the transient conduction problem by backward Euler with constant
/// drives — the single-segment special case of
/// [`solve_transient_schedule`].
pub fn solve_transient(
    model: &ThermalModel,
    opts: &SolveOptions,
    topts: &TransientOptions,
) -> Result<TransientSolution, SolveError> {
    solve_transient_schedule(
        model,
        opts,
        topts.initial_c,
        topts.snapshot_every,
        &[ScheduleSegment::plain(topts.dt_s, topts.steps)],
    )
}

/// Solve a transient run driven by a piecewise-constant schedule.
///
/// The temperature field is continuous across segment boundaries; the
/// drives (source powers, boundary temperatures) switch step-wise at
/// them. Each segment's operator is reassembled from the overridden
/// model — number-only overrides guarantee the voxel structure (solid,
/// free, pinned sets) is identical across segments, so the field carries
/// over index-for-index. The energy audit integrates over the whole run,
/// segment switches included.
pub fn solve_transient_schedule(
    model: &ThermalModel,
    opts: &SolveOptions,
    initial_c: f64,
    snapshot_every: usize,
    segments: &[ScheduleSegment],
) -> Result<TransientSolution, SolveError> {
    if segments.is_empty()
        || segments
            .iter()
            .any(|s| s.dt_s <= 0.0 || !s.dt_s.is_finite() || s.steps == 0)
    {
        return Err(SolveError::InvalidTimeStep);
    }
    let total_steps: usize = segments.iter().map(|s| s.steps).sum();

    // θ reference comes from the base model (the schedule may move
    // ambients; the reference is the stated baseline, not a moving one).
    let base_sys = build(model)?;
    let reference_c = resolve_reference(&base_sys, model)?;
    let nvox = base_sys.b.len();

    // Thermal mass per voxel; every solid voxel must have one (fail
    // closed, naming the offending material region).
    let mut cap = vec![0.0_f64; nvox];
    for (p, c) in cap.iter_mut().enumerate() {
        if base_sys.solid[p] {
            if base_sys.rc[p] <= 0.0 {
                return Err(ModelError::MissingHeatCapacity {
                    index: base_sys.mat_id[p],
                }
                .into());
            }
            *c = base_sys.rc[p] * base_sys.cell_volume_m3;
        }
    }

    let mut t: Vec<f64> = (0..nvox)
        .map(|p| if base_sys.free[p] { initial_c } else { 0.0 })
        .collect();

    let mut times_s = Vec::with_capacity(total_steps);
    let mut t_max_series = Vec::with_capacity(total_steps);
    let mut source_series: Vec<Vec<f64>> = base_sys
        .sources
        .iter()
        .map(|_| Vec::with_capacity(total_steps))
        .collect();
    let mut snapshots = Vec::new();
    let mut rhs = vec![0.0_f64; nvox];
    let mut stored_delta_j = 0.0;
    let mut injected_j = 0.0;
    let mut stored_gross_j = 0.0;
    let mut injected_gross_j = 0.0;
    let mut cg_iterations_total = 0usize;
    let mut last_iters = 0usize;
    let mut last_resid = 0.0_f64;
    let mut time = 0.0_f64;
    let mut global_step = 0usize;

    // Reused for the final report: the last segment's system and model.
    let mut seg_model = model.clone();
    let mut sys = base_sys;

    for (si, seg) in segments.iter().enumerate() {
        if si > 0 || !seg_is_plain(seg) {
            seg_model = seg.apply(model)?;
            sys = build(&seg_model)?;
        }
        let cdt: Vec<f64> = cap.iter().map(|c| c / seg.dt_s).collect();
        let diag_t: Vec<f64> = sys.diag.iter().zip(&cdt).map(|(d, c)| d + c).collect();
        let fixed_t_max = (0..nvox)
            .filter(|&p| sys.solid[p] && !sys.free[p])
            .map(|p| sys.tfix[p])
            .fold(f64::NEG_INFINITY, f64::max);

        for _ in 0..seg.steps {
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
            stored_gross_j += de.abs();
            let net_in = seg.dt_s * (sys.source_w_total - sys.boundary_outflow_w(&x));
            injected_j += net_in;
            injected_gross_j += net_in.abs();

            t = x;
            time += seg.dt_s;
            global_step += 1;
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
            if snapshot_every > 0 && global_step.is_multiple_of(snapshot_every) {
                snapshots.push(Snapshot {
                    time_s: time,
                    t_c: full_field(&sys, &t),
                });
            }
        }
    }

    let audit_scale = stored_delta_j
        .abs()
        .max(injected_j.abs())
        .max(stored_gross_j)
        .max(injected_gross_j)
        .max(1e-30);
    let final_state = assemble_solution(&sys, &seg_model, &t, last_iters, last_resid, reference_c);
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

fn seg_is_plain(seg: &ScheduleSegment) -> bool {
    seg.source_power_w.is_empty()
        && seg.face_temperature_c.is_empty()
        && seg.fixed_temperature_c.is_empty()
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

    /// Lumped cube (Biot ≪ 1) equilibrated at ambient, then the ambient
    /// steps +2 °C — the overlay-drift question. The response after the
    /// switch is the same single exponential toward the *new* ambient:
    /// T(t') = 27 − 2·exp(−t'/τ).
    #[test]
    fn ambient_step_schedule_relaxes_toward_the_new_ambient() {
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
        m.domain_faces = [Boundary::Convection {
            h_w_m2k: 20.0,
            ambient_c: 25.0,
        }; 6];

        let tau = 2.43e6 * 1e-6 / (20.0 * 6.0e-4); // ≈ 202.6 s
        let hold = ScheduleSegment::plain(1.0, 50);
        let mut step = ScheduleSegment::plain(1.0, 400);
        for slot in 0..6 {
            step.face_temperature_c.insert(slot, 27.0);
        }
        let sol =
            solve_transient_schedule(&m, &SolveOptions::default(), 25.0, 0, &[hold, step.clone()])
                .unwrap();

        // Held segment stays put at ambient.
        assert!((sol.t_max_c[49] - 25.0).abs() < 1e-9);
        // After the step, exponential approach to 27 °C.
        for &after in &[100usize, 200, 400] {
            let got = sol.t_max_c[50 + after - 1];
            let exact = 27.0 - 2.0 * (-(after as f64) / tau).exp();
            assert!(
                (got - exact).abs() < 0.005 * 2.0,
                "t' = {after} s: computed {got:.4}, exact {exact:.4}"
            );
        }
        assert!(sol.energy_audit_residual_rel < 1e-6);
    }

    /// RTP-shaped source schedule: heat / soak / lamp-off. T_max peaks at
    /// the end of the soak, then decays back toward the reservoir; the
    /// energy audit integrates cleanly across the power switches.
    #[test]
    fn source_schedule_heats_soaks_and_cools() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [20.0, 20.0, 4.0], [10, 10, 2]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [20.0, 20.0, 4.0],
                },
                150.0,
            )
            .with_heat_capacity(1.7e6),
        );
        m.sources.push(PowerSource {
            name: "lamp".into(),
            shape: Shape::Box {
                min_mm: [5.0, 5.0, 2.0],
                size_mm: [10.0, 10.0, 2.0],
            },
            power_w: 5.0,
        });
        m.domain_faces[4] = Boundary::FixedTemperature {
            temperature_c: 25.0,
        };
        m.reference_c = Some(25.0);

        let mut soak = ScheduleSegment::plain(0.5, 40);
        soak.source_power_w.insert("lamp".into(), 5.0);
        let mut off = ScheduleSegment::plain(0.5, 80);
        off.source_power_w.insert("lamp".into(), 0.0);
        let sol =
            solve_transient_schedule(&m, &SolveOptions::default(), 25.0, 0, &[soak, off]).unwrap();

        let peak = sol
            .t_max_c
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        // Peak is at the end of the powered segment...
        assert!((sol.t_max_c[39] - peak).abs() < 1e-9);
        assert!(peak > 25.5);
        // ...and the lamp-off tail decays monotonically toward the chuck.
        for w in sol.t_max_c[40..].windows(2) {
            assert!(w[1] <= w[0] + 1e-9);
        }
        assert!(sol.t_max_c.last().unwrap() - 25.0 < 0.5 * (peak - 25.0));
        // Gross-normalized audit: the net totals both round-trip toward
        // zero here, so the bound reflects per-step CG noise against the
        // gross traffic (‖rhs‖ carries the 25 °C offset).
        assert!(
            sol.energy_audit_residual_rel < 1e-3,
            "audit residual {}",
            sol.energy_audit_residual_rel
        );
        // The source series follows the schedule too (its own probe).
        assert_eq!(sol.source_t_max_c[0].len(), 120);
    }

    #[test]
    fn schedule_overrides_fail_closed() {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [10.0, 5.0, 5.0], [4, 1, 1]);
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [0.0, 0.0, 0.0],
                    size_mm: [10.0, 5.0, 5.0],
                },
                50.0,
            )
            .with_heat_capacity(2.0e6),
        );
        m.domain_faces[0] = Boundary::FixedTemperature { temperature_c: 0.0 };

        let run = |seg: ScheduleSegment| {
            solve_transient_schedule(&m, &SolveOptions::default(), 0.0, 0, &[seg]).unwrap_err()
        };

        let mut bad_source = ScheduleSegment::plain(1.0, 1);
        bad_source.source_power_w.insert("ghost".into(), 1.0);
        assert!(
            matches!(run(bad_source), SolveError::BadScheduleOverride(w) if w.contains("ghost"))
        );

        let mut adiabatic_face = ScheduleSegment::plain(1.0, 1);
        adiabatic_face.face_temperature_c.insert(3, 40.0);
        assert!(matches!(
            run(adiabatic_face),
            SolveError::BadScheduleOverride(w) if w.contains("adiabatic")
        ));

        let mut bad_slot = ScheduleSegment::plain(1.0, 1);
        bad_slot.face_temperature_c.insert(7, 40.0);
        assert!(matches!(
            run(bad_slot),
            SolveError::BadScheduleOverride(w) if w.contains("out of range")
        ));

        let mut bad_fixed = ScheduleSegment::plain(1.0, 1);
        bad_fixed.fixed_temperature_c.insert(0, 40.0);
        assert!(matches!(
            run(bad_fixed),
            SolveError::BadScheduleOverride(w) if w.contains("out of range")
        ));

        assert!(matches!(
            solve_transient_schedule(&m, &SolveOptions::default(), 0.0, 0, &[]),
            Err(SolveError::InvalidTimeStep)
        ));
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
