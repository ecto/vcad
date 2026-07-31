//! The power ledger: an honest Q for the neutralized machine.
//!
//! The virtual-cathode result (ion yield ×8.4 at 10 A electron current)
//! is a *yield* claim. A *Q* claim needs the full input-power ledger, and
//! the electron cloud is not free: in steady state the injector must
//! replace every electron lost through a cusp, and each carries out ~the
//! well energy it fell through. This module prices every term so the
//! neutralization lane can never quote a yield gain without its power cost.
//!
//! All terms are explicit and signed by their source (measured trace
//! quantities or supplied hardware numbers) — nothing is defaulted. The
//! electron sustaining current is derived from the **measured** loss rate
//! (trapped charge / confinement time); because confinement times here are
//! budget-capped *lower bounds*, the loss rate is an upper bound and the
//! resulting Q is **conservative** (the honest direction).

/// Inputs to the power ledger. Every field is a measured or supplied
/// number with a stated origin — see `PowerLedger::evaluate`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedgerInputs {
    /// D-D neutron rate of the machine, n/s (from the ion traces).
    pub neutron_rate_n_per_s: f64,
    /// Ion beam current, A.
    pub ion_current_a: f64,
    /// Accelerating voltage, V (absolute).
    pub voltage_v: f64,
    /// Trapped electron charge in flight, C (from the electron dwell).
    pub trapped_electron_charge_c: f64,
    /// Mean electron confinement time, s (budget-capped lower bound →
    /// conservative loss rate).
    pub electron_confinement_time_s: f64,
    /// Mean energy a lost electron carries out, eV (≈ well depth it fell
    /// through). If 0, defaults to the full `voltage_v`.
    pub electron_loss_energy_ev: f64,
    /// Ohmic/cryo power to sustain the shield magnet, W. The dominant term
    /// at MA·turn scale — supply it from the em + thermal crates; 0 means
    /// "not yet priced" and the ledger says so.
    pub magnet_power_w: f64,
}

/// The evaluated ledger.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerLedger {
    /// Fusion power, W (both D-D branches implied by the n-branch rate:
    /// the p-branch adds ~equal energy, folded in at 7.3 MeV/neutron).
    pub fusion_power_w: f64,
    /// Ion beam power, W.
    pub ion_beam_power_w: f64,
    /// Electron sustaining power, W (loss current × loss energy).
    pub electron_sustain_power_w: f64,
    /// Magnet power, W (as supplied).
    pub magnet_power_w: f64,
    /// Total input power, W.
    pub input_power_w: f64,
    /// `fusion_power_w / input_power_w`.
    pub q: f64,
    /// Whether the magnet term was priced (false = Q is an over-estimate
    /// missing the dominant cost).
    pub magnet_priced: bool,
}

/// Energy per D-D reaction counting both branches, joules. The measured
/// quantity is the *neutron* rate (n-branch); the p-branch (T + p, 4.03
/// MeV) fires at ~equal probability, so total fusion energy ≈ 7.3 MeV per
/// detected neutron.
const E_PER_NEUTRON_J: f64 = 7.3e6 * crate::constants::ELEMENTARY_CHARGE;

impl LedgerInputs {
    /// Evaluate the ledger.
    pub fn evaluate(&self) -> PowerLedger {
        let fusion_power_w = self.neutron_rate_n_per_s * E_PER_NEUTRON_J;
        let ion_beam_power_w = self.ion_current_a * self.voltage_v;

        let loss_energy_ev = if self.electron_loss_energy_ev > 0.0 {
            self.electron_loss_energy_ev
        } else {
            self.voltage_v
        };
        let electron_sustain_power_w = if self.electron_confinement_time_s > 0.0 {
            let loss_current_a =
                self.trapped_electron_charge_c.abs() / self.electron_confinement_time_s;
            loss_current_a * loss_energy_ev
        } else {
            0.0
        };

        let input_power_w = ion_beam_power_w + electron_sustain_power_w + self.magnet_power_w;
        PowerLedger {
            fusion_power_w,
            ion_beam_power_w,
            electron_sustain_power_w,
            magnet_power_w: self.magnet_power_w,
            input_power_w,
            q: if input_power_w > 0.0 {
                fusion_power_w / input_power_w
            } else {
                0.0
            },
            magnet_priced: self.magnet_power_w > 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> LedgerInputs {
        LedgerInputs {
            neutron_rate_n_per_s: 8.0e7,
            ion_current_a: 0.030,
            voltage_v: 100_000.0,
            trapped_electron_charge_c: 1.0e-9,
            electron_confinement_time_s: 1.0e-7,
            electron_loss_energy_ev: 0.0,
            magnet_power_w: 0.0,
        }
    }

    #[test]
    fn q_is_tiny_and_input_dominated_by_beam() {
        let l = base().evaluate();
        assert!(
            l.q > 0.0 && l.q < 1e-6,
            "gas-machine Q must be tiny: {:.2e}",
            l.q
        );
        assert!(l.ion_beam_power_w > 0.0);
        assert!(!l.magnet_priced, "unpriced magnet must be flagged");
    }

    #[test]
    fn electron_sustain_scales_with_loss_rate() {
        let mut a = base();
        a.trapped_electron_charge_c = 1.0e-6; // heavy cloud
        a.electron_confinement_time_s = 1.0e-8; // leaky
        let l = a.evaluate();
        // Loss current 1e-6/1e-8 = 100 A, × 100 kV = 10 MW sustain.
        assert!(
            (l.electron_sustain_power_w - 1.0e7).abs() / 1.0e7 < 1e-6,
            "sustain power = loss current × well: {:.3e}",
            l.electron_sustain_power_w
        );
        // A leaky heavy cloud tanks Q vs the beam-only case.
        assert!(l.q < base().evaluate().q);
    }

    #[test]
    fn magnet_power_is_counted_when_priced() {
        let mut a = base();
        a.magnet_power_w = 5.0e4;
        let l = a.evaluate();
        assert!(l.magnet_priced);
        assert!(l.input_power_w >= 5.0e4);
        assert!(l.q < base().evaluate().q, "magnet cost must lower Q");
    }

    #[test]
    fn confinement_time_governs_the_penalty() {
        // Better confinement (longer τ) → lower loss current → higher Q.
        let mut leaky = base();
        leaky.electron_confinement_time_s = 1.0e-8;
        let mut tight = base();
        tight.electron_confinement_time_s = 1.0e-6;
        assert!(tight.evaluate().q > leaky.evaluate().q);
    }
}
