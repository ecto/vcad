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
    let (nr, nz) = (solution.nr, solution.nz);
    let (dr, dz) = (solution.dr, solution.dz);
    let idx = |i: usize, j: usize| i * nz + j;

    // Beam charge density: each simulated ion represents I/(e·N) real
    // ions per second, so a cell's steady population is that rate times
    // the summed dwell. Node volume is the axisymmetric ring around the
    // node (half-cell at the axis and boundaries — the axis node uses the
    // quarter-radius ring, exact enough for a first-order gauge).
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

    // Solve ∇²φ = −ρ/ε₀ with every conductor (wall + electrodes) at 0:
    // the same SOR stencil as the applied-field solve, plus the source.
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
