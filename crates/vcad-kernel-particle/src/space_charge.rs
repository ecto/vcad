//! Space-charge validity estimator: where does the linear-current
//! assumption break?
//!
//! Every yield claim in this crate scales linearly with injected ion
//! current because trajectories are traced in the **vacuum** field — the
//! beam's own charge is ignored. This module prices that assumption
//! instead of hoping about it:
//!
//! 1. trace an ensemble with dwell-time recording
//!    ([`crate::trace::Tracer::launch_ensemble_dwell`]),
//! 2. convert dwell to a steady-state beam charge density (each simulated
//!    ion carries `I / (e·N)` real ions per second),
//! 3. solve ∇²φ = −ρ/ε₀ on the same axisymmetric stencil with **every
//!    conductor grounded** (the perturbation potential),
//! 4. report the peak beam potential against the applied well depth.
//!
//! `ratio = φ_beam,peak / |well|` is first-order: the estimate is itself
//! computed from unperturbed trajectories, so it is a validity *gauge*,
//! not a self-consistent solution. Read it as: below ~10% the linear
//! claims stand; approaching ~100% the machine is space-charge-limited
//! and a self-consistent (PIC) treatment is required. Both thresholds
//! ride on the emitted diagnostics rather than being silently assumed.

use crate::constants::{ELEMENTARY_CHARGE, EPSILON_0};
use crate::poisson::{Solution, SolveError, SolveOptions};

/// The space-charge validity report for one traced configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpaceChargeReport {
    /// Peak beam-induced potential (conductors grounded), volts.
    pub phi_beam_peak_v: f64,
    /// Applied well depth |min φ|, volts.
    pub well_depth_v: f64,
    /// `phi_beam_peak_v / well_depth_v` — the validity gauge.
    pub ratio: f64,
    /// The ion current the estimate was priced at, amperes.
    pub ion_current_a: f64,
    /// Total beam charge in flight, coulombs (diagnostic).
    pub beam_charge_c: f64,
}

impl SpaceChargeReport {
    /// The current at which the gauge would reach `ratio_limit`
    /// (exactly linear: ρ, and thus φ_beam, scale with I).
    pub fn current_at_ratio_a(&self, ratio_limit: f64) -> f64 {
        if self.ratio <= 0.0 {
            f64::INFINITY
        } else {
            self.ion_current_a * ratio_limit / self.ratio
        }
    }
}

/// Dwell map → steady-state beam charge density (and total charge).
fn beam_rho(
    solution: &Solution,
    dwell: &[f64],
    n_particles: usize,
    ion_current_a: f64,
) -> (Vec<f64>, f64) {
    let (nr, nz) = (solution.nr, solution.nz);
    let (dr, dz) = (solution.dr, solution.dz);
    let idx = |i: usize, j: usize| i * nz + j;
    let ions_per_s_per_sim = ion_current_a / ELEMENTARY_CHARGE / n_particles as f64;
    let mut rho = vec![0.0_f64; nr * nz];
    let mut beam_charge_c = 0.0;
    for i in 0..nr {
        let r_eff = if i == 0 { 0.25 * dr } else { i as f64 * dr };
        let vol = 2.0 * std::f64::consts::PI * r_eff * dr * dz;
        for j in 0..nz {
            let population = ions_per_s_per_sim * dwell[idx(i, j)];
            let q = population * ELEMENTARY_CHARGE;
            beam_charge_c += q;
            rho[idx(i, j)] = q / vol;
        }
    }
    (rho, beam_charge_c)
}

/// Estimate the space-charge validity gauge from a dwell map.
///
/// `dwell` is the node-indexed dwell-time map from
/// [`crate::trace::Tracer::launch_ensemble_dwell`] over `n_particles`
/// simulated ions, and `ion_current_a` the physical injected current the
/// configuration claims.
pub fn estimate(
    solution: &Solution,
    dwell: &[f64],
    n_particles: usize,
    ion_current_a: f64,
    opts: &SolveOptions,
) -> Result<SpaceChargeReport, SolveError> {
    assert_eq!(dwell.len(), solution.nr * solution.nz, "dwell map size");

    // Beam charge density: each simulated ion represents I/(e·N) real
    // ions per second, so a cell's steady population is that rate times
    // the summed dwell. Node volume is the axisymmetric ring around the
    // node (the axis node uses the quarter-radius ring — exact enough for
    // a first-order gauge).
    let (rho, beam_charge_c) = beam_rho(solution, dwell, n_particles, ion_current_a);

    // Solve ∇²φ = −ρ/ε₀ with every conductor (wall + electrodes) at 0:
    // the same SOR stencil as the applied-field solve, plus the source.
    let phi = solve_grounded_source(solution, &rho, opts)?;

    let phi_beam_peak_v = phi.iter().fold(0.0_f64, |a, b| a.max(b.abs()));
    let well_depth_v = solution
        .phi
        .iter()
        .fold(0.0_f64, |a, b| a.max(-b))
        .max(1e-30);
    Ok(SpaceChargeReport {
        phi_beam_peak_v,
        well_depth_v,
        ratio: phi_beam_peak_v / well_depth_v,
        ion_current_a,
        beam_charge_c,
    })
}

/// Solve ∇²φ = −ρ/ε₀ on `solution`'s grid with every Dirichlet node
/// (wall + electrodes) grounded — the beam's perturbation potential.
fn solve_grounded_source(
    solution: &Solution,
    rho: &[f64],
    opts: &SolveOptions,
) -> Result<Vec<f64>, SolveError> {
    let (nr, nz) = (solution.nr, solution.nz);
    let (dr, dz) = (solution.dr, solution.dz);
    let idx = |i: usize, j: usize| i * nz + j;
    let omega = if opts.omega > 0.0 {
        opts.omega
    } else {
        let n = nr.max(nz) as f64;
        2.0 / (1.0 + (std::f64::consts::PI / n).sin())
    };
    let dr2 = dr * dr;
    let dz2 = dz * dz;
    let mut phi = vec![0.0_f64; nr * nz];
    // Convergence scale: the largest single-cell source contribution.
    let src_scale = rho
        .iter()
        .map(|r| r / EPSILON_0 * 0.25 * dr2.min(dz2))
        .fold(0.0_f64, f64::max)
        .max(1e-30);

    let mut residual = f64::MAX;
    let mut sweeps = 0;
    while sweeps < opts.max_sweeps {
        residual = 0.0;
        for i in 0..nr - 1 {
            for j in 1..nz - 1 {
                let id = idx(i, j);
                if solution.fixed[id] {
                    continue;
                }
                let pz = (phi[idx(i, j + 1)] + phi[idx(i, j - 1)]) / dz2;
                let (num_r, den_r) = if i == 0 {
                    (4.0 * phi[idx(1, j)] / dr2, 4.0 / dr2)
                } else {
                    let r = i as f64 * dr;
                    let rp = r + 0.5 * dr;
                    let rm = r - 0.5 * dr;
                    (
                        (rp * phi[idx(i + 1, j)] + rm * phi[idx(i - 1, j)]) / (r * dr2),
                        (rp + rm) / (r * dr2),
                    )
                };
                let updated = (num_r + pz + rho[id] / EPSILON_0) / (den_r + 2.0 / dz2);
                let delta = updated - phi[id];
                phi[id] += omega * delta;
                let ad = delta.abs();
                if ad > residual {
                    residual = ad;
                }
            }
        }
        sweeps += 1;
        if residual < opts.tol * src_scale.max(phi.iter().fold(0.0_f64, |a, b| a.max(b.abs()))) {
            break;
        }
    }
    if sweeps >= opts.max_sweeps {
        return Err(SolveError::NotConverged { residual, sweeps });
    }
    Ok(phi)
}

/// Options for [`self_consistent`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfConsistentOptions {
    /// Maximum deposit → re-solve → re-trace iterations.
    pub iterations: usize,
    /// Under-relaxation on the charge-density update (0 < relax ≤ 1).
    pub relax: f64,
    /// Trace ensemble size per iteration.
    pub particles: usize,
    /// Converged when max|Δρ|/max|ρ| falls below this.
    pub rho_tol: f64,
}

impl Default for SelfConsistentOptions {
    fn default() -> Self {
        Self {
            iterations: 8,
            relax: 0.3,
            particles: 96,
            rho_tol: 0.05,
        }
    }
}

/// One iteration of the self-consistent loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfConsistentIteration {
    /// Ensemble statistics traced in the field of the previous iterate.
    pub stats: crate::fom::EnsembleStats,
    /// φ_beam,peak / well at this iterate.
    pub ratio: f64,
    /// Relative charge-density change max|Δρ|/max|ρ| after this iterate.
    pub rho_delta_rel: f64,
}

/// Result of [`self_consistent`].
#[derive(Debug, Clone, PartialEq)]
pub struct SelfConsistentReport {
    /// Vacuum-field (iteration-0) statistics — the linear-claim baseline.
    pub vacuum_stats: crate::fom::EnsembleStats,
    /// Per-iteration history.
    pub iterations: Vec<SelfConsistentIteration>,
    /// Whether the density update converged within the budget.
    pub converged: bool,
    /// Whether the *physical observables* went stationary: relative
    /// spread of the beam-potential ratio over the last three iterations
    /// below 5%. Node-level density deltas floor at ensemble shot noise
    /// even at stationarity, so this is the flag that tracks physics;
    /// `converged` stays the stricter density criterion.
    pub observably_converged: bool,
    /// The final (relaxed) beam charge density, node-indexed — input to
    /// two-species neutralization.
    pub final_rho: Vec<f64>,
}

impl SelfConsistentReport {
    /// The last iterate's statistics (the self-consistent answer when
    /// `converged`; fail-closed callers must check).
    pub fn final_stats(&self) -> crate::fom::EnsembleStats {
        self.iterations
            .last()
            .map(|i| i.stats)
            .unwrap_or(self.vacuum_stats)
    }
}

/// PIC-lite self-consistent space charge: iterate deposit → grounded
/// source solve → re-trace in `applied + beam` fields with under-relaxed
/// density updates, until the density stops moving.
///
/// This prices what the [`estimate`] gauge only flags: the actual
/// degradation (or not) of confinement and yield once the beam pushes
/// back on its own well. Steady-state, single species, and inherits every
/// M0 scope caveat; censored traces under-weight the longest-lived charge,
/// so treat converged densities as floors.
#[allow(clippy::too_many_arguments)]
pub fn self_consistent(
    device: &crate::device::Device,
    nr: usize,
    nz: usize,
    sopts: &SolveOptions,
    topts: &crate::trace::TraceOptions,
    species: crate::trace::Species,
    ion_current_a: f64,
    opts: &SelfConsistentOptions,
) -> Result<SelfConsistentReport, SolveError> {
    let base = crate::poisson::solve(device, nr, nz, sopts)?;
    let well = base.phi.iter().fold(0.0_f64, |a, b| a.max(-b)).max(1e-30);

    // Iteration 0: vacuum fields.
    let fields = crate::field::FieldMap::new(device, &base);
    let tracer = crate::trace::Tracer::new(device, &fields, &base, *topts);
    let (outcomes, dwell) = tracer.launch_ensemble_dwell(species, opts.particles);
    let vacuum_stats = crate::fom::stats(&outcomes);
    let (mut rho, _) = beam_rho(&base, &dwell, opts.particles, ion_current_a);

    let mut iterations = Vec::new();
    let mut converged = false;
    for _ in 0..opts.iterations {
        let phi_sc = solve_grounded_source(&base, &rho, sopts)?;
        let ratio = phi_sc.iter().fold(0.0_f64, |a, b| a.max(b.abs())) / well;

        let mut total = base.clone();
        for (t, s) in total.phi.iter_mut().zip(&phi_sc) {
            *t += s;
        }
        let fields = crate::field::FieldMap::new(device, &total);
        let tracer = crate::trace::Tracer::new(device, &fields, &total, *topts);
        let (outcomes, dwell) = tracer.launch_ensemble_dwell(species, opts.particles);
        let stats = crate::fom::stats(&outcomes);
        let (rho_new, _) = beam_rho(&base, &dwell, opts.particles, ion_current_a);

        // Convergence is judged on the *relaxed* update — the raw
        // iterate-vs-iterate delta compares two noisy ensemble samples and
        // never settles; the damped state is what the loop actually
        // propagates, so its movement is the honest convergence signal.
        let scale = rho.iter().fold(0.0_f64, |a, b| a.max(b.abs())).max(1e-300);
        let mut delta = 0.0_f64;
        for (r, n) in rho.iter_mut().zip(&rho_new) {
            let updated = (1.0 - opts.relax) * *r + opts.relax * *n;
            let d = (updated - *r).abs();
            if d > delta {
                delta = d;
            }
            *r = updated;
        }
        let delta = delta / scale;
        iterations.push(SelfConsistentIteration {
            stats,
            ratio,
            rho_delta_rel: delta,
        });
        if delta < opts.rho_tol {
            converged = true;
            break;
        }
    }

    let observably_converged = iterations.len() >= 3 && {
        let tail: Vec<f64> = iterations.iter().rev().take(3).map(|i| i.ratio).collect();
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        let spread = tail.iter().fold(0.0_f64, |a, b| a.max((b - mean).abs()));
        mean > 0.0 && spread / mean < 0.05
    };
    Ok(SelfConsistentReport {
        vacuum_stats,
        iterations,
        converged,
        observably_converged,
        final_rho: rho,
    })
}

/// Result of [`neutralized`]: the perfect-injection electron-cloud bound.
#[derive(Debug, Clone, PartialEq)]
pub struct TwoSpeciesReport {
    /// The single-species (ion) self-consistent run this builds on.
    pub ion_only: SelfConsistentReport,
    /// Electron ensemble statistics (traced in applied + ion-beam field).
    pub electron_stats: crate::fom::EnsembleStats,
    /// In-flight electron charge / in-flight ion charge.
    pub neutralization_fraction: f64,
    /// Peak |net beam potential| / well after the electron cloud.
    pub net_ratio: f64,
    /// Electron contribution to the on-axis core potential (r=0, z=0),
    /// volts. **Negative deepens the ion well** — the virtual-cathode /
    /// neutralization signal that actually matters for the fusing core, as
    /// opposed to `net_ratio` which a peripheral electron pile-up can move
    /// without helping the center.
    pub core_potential_change_v: f64,
    /// Fraction of electrons still confined at the flight budget — the
    /// trapped cloud that does the neutralizing.
    pub electron_survivor_fraction: f64,
    /// Ion statistics re-traced in the neutralized field — compare with
    /// `ion_only.final_stats()` (taxed) and `ion_only.vacuum_stats`.
    pub recovered_stats: crate::fom::EnsembleStats,
}

/// Idealized two-species neutralization — the **perfect-injection upper
/// bound**.
///
/// Electrons are born at rest on a shell *inside* the ion cloud (as if an
/// injector delivered them there losslessly), traced in the applied +
/// ion-beam field, and their charge is subtracted from the beam density;
/// ions are then re-traced in the neutralized field. Real injectors,
/// cusp electron losses, and electron thermalization — the polywell's
/// actual demons — are not modeled: this brackets the best case, nothing
/// more, and claims built on it must say so.
#[allow(clippy::too_many_arguments)]
pub fn neutralized(
    device: &crate::device::Device,
    nr: usize,
    nz: usize,
    sopts: &SolveOptions,
    ion_topts: &crate::trace::TraceOptions,
    ion_current_a: f64,
    electron_current_a: f64,
    electron_shell_fraction: f64,
    opts: &SelfConsistentOptions,
) -> Result<TwoSpeciesReport, SolveError> {
    let ion_only = self_consistent(
        device,
        nr,
        nz,
        sopts,
        ion_topts,
        crate::trace::DEUTERON,
        ion_current_a,
        opts,
    )?;
    let base = crate::poisson::solve(device, nr, nz, sopts)?;
    let well = base.phi.iter().fold(0.0_f64, |a, b| a.max(-b)).max(1e-30);

    // Field the electrons live in: applied + converged ion beam.
    let phi_ion = solve_grounded_source(&base, &ion_only.final_rho, sopts)?;
    let mut with_ions = base.clone();
    for (t, s) in with_ions.phi.iter_mut().zip(&phi_ion) {
        *t += s;
    }

    // Electrons: born at rest on the launch shell (in the applied + ion
    // field, so the positive ion beam pulls them inward). Launch location
    // is decisive — electrons born deep on-axis escape the point cusp
    // before magnetizing; electrons born near the rings trap in the strong
    // cusp field (see `confinement`). A generous budget lets the trapped
    // ones accumulate real dwell rather than censoring early.
    let e_topts = crate::trace::TraceOptions {
        launch_shell_fraction: electron_shell_fraction,
        time_budget_factor: ion_topts.time_budget_factor.max(30.0),
        ..*ion_topts
    };
    let fields = crate::field::FieldMap::new(device, &with_ions);
    let tracer = crate::trace::Tracer::new(device, &fields, &with_ions, e_topts);
    let (e_outcomes, e_dwell) =
        tracer.launch_ensemble_dwell(crate::trace::ELECTRON, opts.particles);
    let electron_stats = crate::fom::stats(&e_outcomes);
    let electron_survivor_fraction = e_outcomes
        .iter()
        .filter(|o| o.fate == crate::trace::Fate::Survived)
        .count() as f64
        / e_outcomes.len().max(1) as f64;
    let (rho_e, q_e) = beam_rho(&base, &e_dwell, opts.particles, electron_current_a);

    let q_i: f64 = {
        // In-flight ion charge from the converged density and node volumes.
        let (dr, dz) = (base.dr, base.dz);
        let mut q = 0.0;
        for i in 0..nr {
            let r_eff = if i == 0 { 0.25 * dr } else { i as f64 * dr };
            let vol = 2.0 * std::f64::consts::PI * r_eff * dr * dz;
            for j in 0..nz {
                q += ion_only.final_rho[i * nz + j] * vol;
            }
        }
        q
    };

    // Net density and the neutralized field; ions re-traced in it.
    let net_rho: Vec<f64> = ion_only
        .final_rho
        .iter()
        .zip(&rho_e)
        .map(|(i, e)| i - e)
        .collect();
    let phi_net = solve_grounded_source(&base, &net_rho, sopts)?;
    let net_ratio = phi_net.iter().fold(0.0_f64, |a, b| a.max(b.abs())) / well;
    // Electron-only contribution at the on-axis core (node r=0, z=0): the
    // change to the ion well where fusion happens. Negative = deepened.
    let phi_e = solve_grounded_source(&base, &rho_e, sopts)?;
    let core_j = nz / 2;
    let core_potential_change_v = -phi_e[core_j];
    let mut neutralized_sol = base.clone();
    for (t, s) in neutralized_sol.phi.iter_mut().zip(&phi_net) {
        *t += s;
    }
    let fields = crate::field::FieldMap::new(device, &neutralized_sol);
    let tracer = crate::trace::Tracer::new(device, &fields, &neutralized_sol, *ion_topts);
    let (i_outcomes, _) = tracer.launch_ensemble_dwell(crate::trace::DEUTERON, opts.particles);
    let recovered_stats = crate::fom::stats(&i_outcomes);

    Ok(TwoSpeciesReport {
        ion_only,
        electron_stats,
        neutralization_fraction: q_e / q_i.max(1e-300),
        net_ratio,
        core_potential_change_v,
        electron_survivor_fraction,
        recovered_stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::field::FieldMap;
    use crate::poisson::solve;
    use crate::trace::{TraceOptions, Tracer, DEUTERON};

    fn traced() -> (Solution, Vec<f64>, usize) {
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 0.0);
        let sol = solve(&device, 61, 121, &SolveOptions::default()).unwrap();
        let fields = FieldMap::new(&device, &sol);
        let opts = TraceOptions {
            max_passes: 8,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, opts);
        let (_, dwell) = tracer.launch_ensemble_dwell(DEUTERON, 12);
        (sol, dwell, 12)
    }

    #[test]
    fn dwell_is_recorded_and_finite() {
        let (_, dwell, _) = traced();
        let total: f64 = dwell.iter().sum();
        assert!(total > 0.0, "traces must deposit dwell time");
        assert!(dwell.iter().all(|d| d.is_finite() && *d >= 0.0));
    }

    #[test]
    fn gauge_is_exactly_linear_in_current() {
        let (sol, dwell, n) = traced();
        let opts = SolveOptions::default();
        let a = estimate(&sol, &dwell, n, 0.010, &opts).unwrap();
        let b = estimate(&sol, &dwell, n, 0.020, &opts).unwrap();
        assert!(a.ratio > 0.0);
        assert!(
            (b.ratio / a.ratio - 2.0).abs() < 1e-6,
            "rho and phi are linear in I: {} vs {}",
            a.ratio,
            b.ratio
        );
        assert!((a.current_at_ratio_a(0.1) * 2.0 - b.current_at_ratio_a(0.1) * 2.0).abs() < 1e-12);
    }

    #[test]
    fn self_consistent_is_benign_at_low_current_and_bites_at_high() {
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -2_000.0, 0.0);
        let topts = TraceOptions {
            max_passes: 6,
            ..TraceOptions::default()
        };
        let sc_opts = SelfConsistentOptions {
            iterations: 3,
            particles: 12,
            ..SelfConsistentOptions::default()
        };
        let low = self_consistent(
            &device,
            61,
            121,
            &SolveOptions::default(),
            &topts,
            crate::trace::DEUTERON,
            1e-4,
            &sc_opts,
        )
        .unwrap();
        let l_final = low.final_stats();
        assert!(low.iterations.iter().all(|i| i.ratio < 0.02));
        let rel = (l_final.mean_passes - low.vacuum_stats.mean_passes).abs()
            / low.vacuum_stats.mean_passes.max(1e-9);
        assert!(rel < 0.15, "0.1 mA must be near-vacuum, drifted {rel:.3}");

        let high = self_consistent(
            &device,
            61,
            121,
            &SolveOptions::default(),
            &topts,
            crate::trace::DEUTERON,
            0.5,
            &sc_opts,
        )
        .unwrap();
        assert!(
            high.iterations[0].ratio > 0.2,
            "500 mA must be deep in space charge: {:?}",
            high.iterations[0].ratio
        );
        let h_final = high.final_stats();
        assert!(
            (h_final.mean_passes - high.vacuum_stats.mean_passes).abs()
                / high.vacuum_stats.mean_passes.max(1e-9)
                > 0.05,
            "space charge this strong must move confinement: vacuum {} vs {}",
            high.vacuum_stats.mean_passes,
            h_final.mean_passes
        );
    }

    #[test]
    fn neutralization_reduces_the_net_ratio_and_recovers_yield() {
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -2_000.0, 0.0);
        let topts = TraceOptions {
            max_passes: 6,
            ..TraceOptions::default()
        };
        let sc_opts = SelfConsistentOptions {
            iterations: 2,
            particles: 12,
            ..SelfConsistentOptions::default()
        };
        let heavy_ma = 0.3; // deep space charge at 2 kV
        let r = neutralized(
            &device,
            61,
            121,
            &SolveOptions::default(),
            &topts,
            heavy_ma,
            heavy_ma,
            0.5,
            &sc_opts,
        )
        .unwrap();
        let taxed_ratio = r.ion_only.iterations.last().unwrap().ratio;
        assert!(
            r.net_ratio < taxed_ratio,
            "electron cloud must reduce the net beam potential: {} vs {}",
            r.net_ratio,
            taxed_ratio
        );
        assert!(r.neutralization_fraction > 0.0);
        assert!(r.electron_stats.n > 0);
        // Recovered confinement moves back toward vacuum relative to taxed.
        let vac = r.ion_only.vacuum_stats.mean_passes;
        let taxed = r.ion_only.final_stats().mean_passes;
        let rec = r.recovered_stats.mean_passes;
        assert!(
            (rec - vac).abs() <= (taxed - vac).abs() + 1e-9,
            "neutralized ions must sit no farther from vacuum than taxed: vac {vac}, taxed {taxed}, recovered {rec}"
        );
    }

    #[test]
    fn gauge_magnitude_is_physical() {
        let (sol, dwell, n) = traced();
        let r = estimate(&sol, &dwell, n, 0.030, &SolveOptions::default()).unwrap();
        // 30 mA in a 20 kV well: the gauge must be a sane fraction —
        // neither vanishing (deposit bug) nor absurd (volume bug).
        assert!(
            r.ratio > 1e-5 && r.ratio < 10.0,
            "implausible space-charge ratio {:.3e} (phi_beam {:.3e} V, well {:.3e} V)",
            r.ratio,
            r.phi_beam_peak_v,
            r.well_depth_v
        );
        assert!(r.beam_charge_c > 0.0 && r.beam_charge_c.is_finite());
        let i_max = r.current_at_ratio_a(0.10);
        assert!(i_max > 0.0 && i_max.is_finite());
    }
}
