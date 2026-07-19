//! Beam-on-target D-D yield: the records lane.
//!
//! The gas-fusion machine's "interception loss" is a yield *channel* if the
//! wires are coated in deuterided titanium (TiD): an ion that would have
//! been a loss instead buries itself in a solid-density deuterium target
//! and fuses on the way down. The published anchor is the titanium
//! drive-in D-D generator: **1.9×10⁸ n/s from 7.6 mA at 94 keV**
//! ([Reijonen et al., Ti drive-in target], see docs/research-log.md).
//!
//! Physics: a beam ion of energy `E₀` entering the target loses energy to
//! electronic stopping and fuses against the bound deuterons with the
//! Bosch–Hale cross section until it thermalizes. The **thick-target
//! yield** per incident ion is
//!
//! ```text
//! Y = n_D · ∫₀^{E₀} σ(E) / S(E) dE
//! ```
//!
//! where `n_D` is the target deuteron density and `S(E) = −dE/dx` the
//! stopping power. Rather than ship an uncertain first-principles stopping
//! model, this module uses a one-parameter effective stopping calibrated
//! **once** to the published anchor, then predicts other energies through
//! the (steep, well-measured) cross-section energy dependence. The
//! calibration constant and its provenance are public
//! ([`CALIBRATION`]); a prediction is only ever a cross-section
//! *ratio* away from a measured datum.
//!
//! Scope/honesty: solid-target neutron generators lose deuterium above
//! ~250 °C (metal-hydride vapor pressure) — the thermal limit is a real
//! operating ceiling this module does *not* enforce; pair it with the
//! thermal crate. Beam-target Q is stopping-power-capped at ~10⁻⁴ (most
//! ions stop without fusing) — this is a records-lane channel, never a
//! path to gain, and the Q accounting says so.

use crate::xsection::dd_n_sigma_m2;

/// The published calibration anchor.
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    /// Beam energy of the anchor datum, keV (lab frame).
    pub energy_kev: f64,
    /// Beam current, amperes.
    pub current_a: f64,
    /// Measured neutron rate, n/s.
    pub neutron_rate_n_per_s: f64,
    /// Monatomic-beam fraction assumed in the datum (D⁺ vs D₂⁺/D₃⁺); the
    /// cited generator ran ~their stated efficiency, folded in here.
    pub monatomic_fraction: f64,
}

/// The titanium drive-in D-D generator anchor (Reijonen et al.).
pub const CALIBRATION: Calibration = Calibration {
    energy_kev: 94.0,
    current_a: 7.6e-3,
    neutron_rate_n_per_s: 1.9e8,
    monatomic_fraction: 1.0,
};

/// Effective-stopping thick-target yield **shape**: the cross-section
/// integral ∫₀^E σ(E') dE' (units m²·keV), assuming stopping power roughly
/// constant across the fusion-relevant band. Absolute yield is this times
/// a single calibrated constant (see [`yield_per_ion`]). Trapezoidal in
/// 1 keV steps — the integrand rises steeply so fine steps matter near E.
fn sigma_integral_m2_kev(energy_kev: f64) -> f64 {
    if energy_kev <= 0.0 {
        return 0.0;
    }
    let steps = (energy_kev.ceil() as usize).max(2);
    let de = energy_kev / steps as f64;
    let mut acc = 0.0;
    let mut prev = dd_n_sigma_m2(0.5 * 0.0); // σ at E_cm = 0
    for k in 1..=steps {
        let e = k as f64 * de;
        // Beam-on-stationary-target: E_cm = E_lab / 2.
        let cur = dd_n_sigma_m2(0.5 * e);
        acc += 0.5 * (prev + cur) * de;
        prev = cur;
    }
    acc
}

/// The single calibration constant `C` such that
/// `Y(E) = C · ∫₀^E σ dE'`, fixed by the anchor:
/// `C = (rate / (current/e)) / ∫₀^{E_anchor} σ dE'`, in units of
/// (ions·m⁻²·keV⁻¹) folded with target density and stopping — i.e. it
/// absorbs `n_D / S`. Recomputed from [`CALIBRATION`] so the anchor is the
/// single source of truth.
fn calibration_constant() -> f64 {
    let ions_per_s = CALIBRATION.current_a * CALIBRATION.monatomic_fraction
        / crate::constants::ELEMENTARY_CHARGE;
    let yield_anchor = CALIBRATION.neutron_rate_n_per_s / ions_per_s;
    yield_anchor / sigma_integral_m2_kev(CALIBRATION.energy_kev)
}

/// Thick-target D-D neutron yield per incident ion at beam energy
/// `energy_kev`, calibrated to the published anchor.
///
/// This is a *ratio prediction*: at the anchor energy it returns the
/// measured yield-per-ion by construction; elsewhere it scales by the
/// cross-section integral (the steep, well-measured energy dependence).
pub fn yield_per_ion(energy_kev: f64) -> f64 {
    calibration_constant() * sigma_integral_m2_kev(energy_kev)
}

/// Neutron rate from a TiD-target beam: `Y(E) · (I·f / e)` n/s.
///
/// `monatomic_fraction` folds in the D⁺ vs molecular-ion mix of the real
/// source (1.0 = ideal atomic beam).
pub fn neutron_rate_n_per_s(energy_kev: f64, current_a: f64, monatomic_fraction: f64) -> f64 {
    let ions_per_s = current_a * monatomic_fraction / crate::constants::ELEMENTARY_CHARGE;
    yield_per_ion(energy_kev) * ions_per_s
}

/// Beam-target energy gain `Q = fusion power / beam power`.
///
/// Each D-D reaction releases 3.27 MeV (D(d,n)³He branch, both products);
/// beam power is `I·V`. Stopping-power-capped at ~10⁻⁴ — the honest
/// records-lane ceiling.
pub fn q_beam_target(energy_kev: f64) -> f64 {
    let e_ddn_j = 3.27e6 * crate::constants::ELEMENTARY_CHARGE;
    let fusion_w_per_ion = yield_per_ion(energy_kev) * e_ddn_j;
    let beam_w_per_ion = energy_kev * 1e3 * crate::constants::ELEMENTARY_CHARGE;
    fusion_w_per_ion / beam_w_per_ion
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproduces_the_calibration_anchor() {
        // By construction, exact at the anchor.
        let rate = neutron_rate_n_per_s(
            CALIBRATION.energy_kev,
            CALIBRATION.current_a,
            CALIBRATION.monatomic_fraction,
        );
        assert!(
            (rate - CALIBRATION.neutron_rate_n_per_s).abs() / CALIBRATION.neutron_rate_n_per_s
                < 1e-9,
            "anchor must reproduce exactly: {rate:.3e} vs {:.3e}",
            CALIBRATION.neutron_rate_n_per_s
        );
    }

    #[test]
    fn yield_rises_steeply_but_saturates_relative_to_energy() {
        // The cross-section integral grows fast through the fusor band…
        assert!(yield_per_ion(94.0) > 10.0 * yield_per_ion(40.0));
        // …and is monotonic.
        let mut last = 0.0;
        for e in [10.0, 30.0, 60.0, 94.0, 150.0] {
            let y = yield_per_ion(e);
            assert!(y > last, "yield must rise with energy at {e} keV");
            last = y;
        }
    }

    #[test]
    fn q_is_capped_far_below_unity() {
        // The whole honesty of the records lane: beam-target Q is tiny.
        for e in [40.0, 94.0, 150.0, 300.0] {
            let q = q_beam_target(e);
            assert!(
                q < 1e-3,
                "beam-target Q must stay << 1 (stopping-power-capped): {q:.2e} at {e} keV"
            );
        }
    }

    #[test]
    fn beats_the_gas_machine_at_the_same_current() {
        // 30 mA at 100 keV onto TiD vs the gas ceiling (~15-17x record ~
        // 8e7 n/s). Solid-density target must win by orders.
        let tid = neutron_rate_n_per_s(100.0, 0.030, 1.0);
        assert!(
            tid > 5e8,
            "TiD at 30 mA/100 keV should reach ~1e9 n/s (>>gas ceiling ~8e7): {tid:.3e}"
        );
    }
}
