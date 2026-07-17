//! Figures of merit over trace ensembles.

use crate::device::Device;
use crate::trace::{Fate, TraceOutcome};

/// Ensemble statistics for an electrode design.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnsembleStats {
    /// Ensemble size.
    pub n: usize,
    /// Mean core passes per particle.
    pub mean_passes: f64,
    /// Fraction ending on a wire ring — the loss channel grid shielding
    /// attacks. In hardware this is the cathode interception current.
    pub interception_fraction: f64,
    /// Fraction ending on the chamber wall / end caps.
    pub wall_fraction: f64,
    /// Fraction still alive at the budget (censored).
    pub survivor_fraction: f64,
    /// Per-pass survival probability implied by `mean_passes` under a
    /// geometric-survival model: `m / (m + 1)`. A lower bound when many
    /// traces are censored.
    pub effective_transparency: f64,
    /// Worst energy drift across the ensemble (integration quality).
    pub max_energy_drift_rel: f64,
    /// Mean D(d,n)³He reaction volume per ion, m³ — see
    /// [`crate::trace::TraceOutcome::ddn_sigma_v_m3`]. Multiply by target
    /// deuteron density for expected neutrons per injected ion.
    pub mean_ddn_sigma_v_m3: f64,
}

/// Reduce trace outcomes to [`EnsembleStats`].
pub fn stats(outcomes: &[TraceOutcome]) -> EnsembleStats {
    let n = outcomes.len().max(1);
    let mean_passes = outcomes.iter().map(|o| o.core_passes as f64).sum::<f64>() / n as f64;
    let count = |f: &dyn Fn(&TraceOutcome) -> bool| {
        outcomes.iter().filter(|o| f(o)).count() as f64 / n as f64
    };
    EnsembleStats {
        n: outcomes.len(),
        mean_passes,
        interception_fraction: count(&|o| matches!(o.fate, Fate::Wire(_))),
        wall_fraction: count(&|o| o.fate == Fate::Wall),
        survivor_fraction: count(&|o| o.fate == Fate::Survived),
        effective_transparency: mean_passes / (mean_passes + 1.0),
        max_energy_drift_rel: outcomes
            .iter()
            .map(|o| o.energy_drift_rel)
            .fold(0.0, f64::max),
        mean_ddn_sigma_v_m3: outcomes.iter().map(|o| o.ddn_sigma_v_m3).sum::<f64>() / n as f64,
    }
}

/// Steady-state D-D neutron rate estimate, neutrons/s.
///
/// `(I/e) × n_d × ⟨∫σv dt⟩`: ions injected per second, times expected
/// neutrons per ion against a background deuteron density `n_d` (see
/// [`crate::xsection::d2_deuteron_density_m3`]). Beam-on-background only —
/// beam–beam and fast-neutral (charge-exchange) channels, which matter in
/// real fusors, are not included, so treat this as a floor with ~order-of-
/// magnitude confidence.
pub fn neutron_rate_per_s(
    mean_ddn_sigma_v_m3: f64,
    ion_current_a: f64,
    deuteron_density_m3: f64,
) -> f64 {
    (ion_current_a / crate::constants::ELEMENTARY_CHARGE)
        * deuteron_density_m3
        * mean_ddn_sigma_v_m3
}

/// Thin-wire geometric transparency of the ring cathode: the fraction of
/// the cathode sphere not shadowed by wire, per crossing.
///
/// Each ring at spherical radius `s` blocks a band of area
/// `2πr_ring · 2a`, so the blocked fraction is `Σ r_ring·a / s²`. Purely
/// geometric — no ion optics — which is exactly why traced transparency
/// differs from it (field lensing steers ions into or around wires).
pub fn geometric_transparency(device: &Device) -> f64 {
    let blocked: f64 = device
        .rings
        .iter()
        .map(|ring| {
            let s2 = ring.ring_radius_mm.powi(2) + ring.z_mm.powi(2);
            (ring.ring_radius_mm * ring.wire_radius_mm / s2).max(0.0)
        })
        .sum();
    (1.0 - blocked).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::trace::{Fate, TraceOutcome};

    fn outcome(fate: Fate, passes: u32) -> TraceOutcome {
        TraceOutcome {
            fate,
            core_passes: passes,
            time_s: 1e-6,
            steps: 1000,
            energy_drift_rel: 0.01,
            launch_cos_theta: 0.0,
            ddn_sigma_v_m3: 2.0e-31,
        }
    }

    #[test]
    fn stats_reduce_correctly() {
        let outcomes = vec![
            outcome(Fate::Wire(0), 3),
            outcome(Fate::Wire(1), 5),
            outcome(Fate::Wall, 2),
            outcome(Fate::Survived, 10),
        ];
        let s = stats(&outcomes);
        assert_eq!(s.n, 4);
        assert!((s.mean_passes - 5.0).abs() < 1e-12);
        assert!((s.interception_fraction - 0.5).abs() < 1e-12);
        assert!((s.wall_fraction - 0.25).abs() < 1e-12);
        assert!((s.survivor_fraction - 0.25).abs() < 1e-12);
        assert!((s.effective_transparency - 5.0 / 6.0).abs() < 1e-12);
        assert!((s.mean_ddn_sigma_v_m3 - 2.0e-31).abs() < 1e-40);
    }

    #[test]
    fn neutron_rate_arithmetic() {
        // 10 mA of ions, 1e20 deuterons/m³, 5e-31 m³ per ion:
        // (0.01/1.6e-19) × 1e20 × 5e-31 ≈ 3.1e6 n/s.
        let rate = neutron_rate_per_s(5.0e-31, 0.01, 1.0e20);
        assert!((2.0e6..5.0e6).contains(&rate), "rate = {rate:.3e} n/s");
    }

    #[test]
    fn more_wire_means_less_transparency() {
        let thin = Device::classic_fusor(150.0, 50.0, 4, 0.5, -1.0);
        let thick = Device::classic_fusor(150.0, 50.0, 4, 2.0, -1.0);
        let many = Device::classic_fusor(150.0, 50.0, 8, 0.5, -1.0);
        let t_thin = geometric_transparency(&thin);
        let t_thick = geometric_transparency(&thick);
        let t_many = geometric_transparency(&many);
        assert!(t_thin > t_thick);
        assert!(t_thin > t_many);
        for t in [t_thin, t_thick, t_many] {
            assert!((0.0..=1.0).contains(&t));
        }
    }
}
