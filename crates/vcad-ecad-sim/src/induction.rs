//! Thin-sheet axial induction machine model (drag-cup / PCB-cage rotors).
//!
//! [`crate::motor`] models PM machines: torque comes from magnet flux crossing
//! the gap (Kt/Ke). An axial *induction* machine has no magnets — the stator's
//! rotating MMF induces eddy currents in a thin conductive rotor sheet (a
//! drag-cup wall, a copper-clad PCB disc), and torque comes from those currents
//! reacting against the travelling field. This module gives that machine the
//! same closed-form, first-order treatment.
//!
//! # Model (read the limits)
//!
//! **Stator field.** A balanced m=3 phase winding with `N` series turns per
//! phase, winding factor `kw`, and `p` pole pairs carrying peak current `I_pk`
//! produces a rotating MMF fundamental of amplitude
//!
//! ```text
//!   F1 = (3/2) · (4/π) · (kw·N / (2p)) · I_pk        [A·turns]
//! ```
//!
//! Driving that MMF across the total non-ferromagnetic gap `g` (mechanical air
//! gaps + rotor sheet + any PCB/adhesive in the flux path — everything that is
//! not iron) gives the fundamental gap field
//!
//! ```text
//!   B1 = μ0 · F1 / g                                  [T]
//! ```
//!
//! **Rotor torque.** The rotor is a thin sheet characterized only by its
//! surface conductance `σs = σ · t` (S) — e.g. two 2 oz copper layers:
//! `5.8e7 S/m × 0.14e-3 m ≈ 8120 S`. At slip `s` the field sweeps past the
//! sheet at slip speed `s·ωsync` (mechanical, `ωsync = ωe/p`), inducing a
//! current density `J = σs·(v × B)`; integrating `r × (J × B)` over the active
//! annulus `r1..r2` with the θ-average `⟨B²⟩ = B1²/2` gives a torque *linear*
//! in slip:
//!
//! ```text
//!   T(s) = k_ee · π · σs · s · (ωe/p) · B1² · (r2⁴ − r1⁴) / 4   [N·m]
//! ```
//!
//! `k_ee` is the Russell–Norsworthy end-effect factor (≈ 0.5–0.8, default
//! 0.65): the eddy-current return paths close *outside* the active annulus
//! through sheet resistance the ideal integral ignores, so real torque is a
//! constant fraction of the ideal value.
//!
//! **Honesty.** First-order only: linear-in-slip torque (no peak/breakdown —
//! valid while the sheet's skin depth at slip frequency exceeds its thickness,
//! true for thin sheets at small `s·f`), no stator leakage or magnetizing
//! reactance (B1 is impressed, not solved from a T-equivalent circuit), no
//! slotting, no saturation, no temperature dependence of σ. Use it for
//! will-it-spin sizing of drag-cup and PCB-rotor machines, not as an FEA
//! substitute.
//!
//! Reference for the end-effect treatment: Russell & Norsworthy, "Eddy
//! currents and wall losses in screened-rotor induction motors", Proc. IEE,
//! 1958.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Permeability of free space, T·m/A.
const MU0: f64 = 4.0 * PI * 1e-7;

/// Inputs for the thin-sheet axial induction model.
///
/// Lengths in millimetres, current in amps RMS, conductance in siemens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinSheetInductionSpec {
    /// Pole pairs `p` (electrical periods per mechanical revolution).
    pub pole_pairs: f64,
    /// Series turns per phase `N`.
    pub turns_per_phase: f64,
    /// Winding factor `kw` (distribution × pitch).
    pub winding_factor: f64,
    /// Phase current, amps RMS (balanced 3-phase drive assumed).
    pub phase_current_a_rms: f64,
    /// Electrical drive frequency, Hz.
    pub electrical_freq_hz: f64,
    /// Total non-ferromagnetic flux path `g`, mm — every air gap plus the
    /// rotor sheet and any PCB substrate the flux crosses between back-irons.
    pub effective_gap_mm: f64,
    /// Rotor sheet surface conductance `σs = σ·thickness`, siemens.
    pub sheet_conductance_s: f64,
    /// Inner radius of the active (field-swept) annulus, mm.
    pub inner_radius_mm: f64,
    /// Outer radius of the active annulus, mm.
    pub outer_radius_mm: f64,
    /// Russell–Norsworthy end-effect factor `k_ee` (0..1]. Fraction of the
    /// ideal torque that survives the eddy-current return paths closing
    /// outside the active annulus. Typical 0.5–0.8; 0.65 is a sane default.
    pub end_effect_factor: f64,
}

/// Closed-form performance of a [`ThinSheetInductionSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinSheetInductionPerformance {
    /// Fundamental air-gap flux density amplitude `B1`, tesla.
    pub b1_tesla: f64,
    /// Torque per unit slip `K` (with end effect applied): `T(s) = K·s`, N·m.
    pub torque_per_unit_slip_nm: f64,
    /// Locked-rotor torque `T(s=1)` with end effect applied, N·m. Equal to
    /// `torque_per_unit_slip_nm` in this linear model.
    pub locked_rotor_torque_nm: f64,
    /// Locked-rotor torque *before* the end-effect factor, N·m — the ideal
    /// annulus integral, for grading against the factored value.
    pub locked_rotor_torque_raw_nm: f64,
    /// Synchronous mechanical speed, RPM (`60·f/p`).
    pub sync_rpm: f64,
    /// Rotor sheet dissipation at locked rotor, W. At `s = 1` all air-gap
    /// power is dissipated in the sheet: `P = T(1) · ωsync` (end effect
    /// applied). Stator copper loss is *not* included — the model has no
    /// phase resistance input.
    pub copper_loss_w: f64,
}

/// Evaluate the thin-sheet axial induction closed form.
///
/// Returns all-zero performance (rather than NaN/inf) when any input that
/// would break the closed form is non-positive: pole pairs, turns, winding
/// factor, current, frequency, gap, conductance, or a non-annulus
/// (`r2 <= r1`). The end-effect factor is clamped to (0, 1].
pub fn evaluate_thin_sheet_induction(
    spec: &ThinSheetInductionSpec,
) -> ThinSheetInductionPerformance {
    let zero = ThinSheetInductionPerformance {
        b1_tesla: 0.0,
        torque_per_unit_slip_nm: 0.0,
        locked_rotor_torque_nm: 0.0,
        locked_rotor_torque_raw_nm: 0.0,
        sync_rpm: 0.0,
        copper_loss_w: 0.0,
    };
    let p = spec.pole_pairs;
    if p <= 0.0
        || spec.turns_per_phase <= 0.0
        || spec.winding_factor <= 0.0
        || spec.phase_current_a_rms <= 0.0
        || spec.electrical_freq_hz <= 0.0
        || spec.effective_gap_mm <= 0.0
        || spec.sheet_conductance_s <= 0.0
        || spec.outer_radius_mm <= spec.inner_radius_mm
        || spec.inner_radius_mm < 0.0
    {
        return zero;
    }
    let k_ee = if spec.end_effect_factor > 0.0 {
        spec.end_effect_factor.min(1.0)
    } else {
        return zero;
    };

    // Rotating MMF fundamental (3-phase): F1 = (3/2)·(4/π)·(kw·N/(2p))·I_pk.
    let i_pk = spec.phase_current_a_rms * std::f64::consts::SQRT_2;
    let f1 = 1.5 * (4.0 / PI) * (spec.winding_factor * spec.turns_per_phase / (2.0 * p)) * i_pk;
    let g_m = spec.effective_gap_mm * 1e-3;
    let b1 = MU0 * f1 / g_m;

    // Ideal linear slip torque: T(s) = π·σs·s·(ωe/p)·B1²·(r2⁴ − r1⁴)/4.
    let omega_e = 2.0 * PI * spec.electrical_freq_hz;
    let omega_sync = omega_e / p;
    let r1_m = spec.inner_radius_mm * 1e-3;
    let r2_m = spec.outer_radius_mm * 1e-3;
    let annulus = (r2_m.powi(4) - r1_m.powi(4)) / 4.0;
    let k_raw = PI * spec.sheet_conductance_s * omega_sync * b1 * b1 * annulus;
    let k = k_ee * k_raw;

    ThinSheetInductionPerformance {
        b1_tesla: b1,
        torque_per_unit_slip_nm: k,
        locked_rotor_torque_nm: k,
        locked_rotor_torque_raw_nm: k_raw,
        sync_rpm: 60.0 * spec.electrical_freq_hz / p,
        // s = 1: air-gap power T·ωsync all burns in the sheet.
        copper_loss_w: k * omega_sync,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The validation reference from the tool spec: N=30, kw=0.866, p=3,
    /// I=1.5 A rms, f=100 Hz, g=4.7 mm, σs=8120 S, r1=15.3, r2=28.5,
    /// end effect 0.65.
    fn reference_spec() -> ThinSheetInductionSpec {
        ThinSheetInductionSpec {
            pole_pairs: 3.0,
            turns_per_phase: 30.0,
            winding_factor: 0.866,
            phase_current_a_rms: 1.5,
            electrical_freq_hz: 100.0,
            effective_gap_mm: 4.7,
            sheet_conductance_s: 8120.0,
            inner_radius_mm: 15.3,
            outer_radius_mm: 28.5,
            end_effect_factor: 0.65,
        }
    }

    #[test]
    fn reference_b1_is_about_4_7_mt() {
        let perf = evaluate_thin_sheet_induction(&reference_spec());
        assert!(
            (perf.b1_tesla - 4.7e-3).abs() < 0.1e-3,
            "B1 {} T should be ≈ 4.7 mT",
            perf.b1_tesla
        );
    }

    #[test]
    fn reference_locked_rotor_raw_is_about_18_unm() {
        let perf = evaluate_thin_sheet_induction(&reference_spec());
        assert!(
            (perf.locked_rotor_torque_raw_nm - 17.8e-6).abs() < 0.5e-6,
            "raw locked-rotor {} N·m should be ≈ 17.8 µN·m",
            perf.locked_rotor_torque_raw_nm
        );
        // End effect scales the delivered torque.
        assert!(
            (perf.locked_rotor_torque_nm - 0.65 * perf.locked_rotor_torque_raw_nm).abs() < 1e-12
        );
    }

    #[test]
    fn reference_sync_speed_is_2000_rpm() {
        let perf = evaluate_thin_sheet_induction(&reference_spec());
        assert!((perf.sync_rpm - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn torque_is_linear_in_slip_and_loss_matches_airgap_power() {
        let perf = evaluate_thin_sheet_induction(&reference_spec());
        assert_eq!(perf.torque_per_unit_slip_nm, perf.locked_rotor_torque_nm);
        let omega_sync = 2.0 * PI * 100.0 / 3.0;
        assert!(
            (perf.copper_loss_w - perf.locked_rotor_torque_nm * omega_sync).abs() < 1e-12,
            "locked-rotor sheet loss should be T·ωsync"
        );
    }

    #[test]
    fn torque_scales_with_conductance_and_current_squared() {
        let base = evaluate_thin_sheet_induction(&reference_spec());
        let mut spec = reference_spec();
        spec.sheet_conductance_s *= 2.0;
        let thick = evaluate_thin_sheet_induction(&spec);
        assert!((thick.locked_rotor_torque_nm / base.locked_rotor_torque_nm - 2.0).abs() < 1e-9);

        let mut spec = reference_spec();
        spec.phase_current_a_rms *= 2.0;
        let hot = evaluate_thin_sheet_induction(&spec);
        // B1 ∝ I, T ∝ B1² → 4×.
        assert!((hot.locked_rotor_torque_nm / base.locked_rotor_torque_nm - 4.0).abs() < 1e-9);
    }

    #[test]
    fn bigger_gap_weakens_field_and_torque() {
        let base = evaluate_thin_sheet_induction(&reference_spec());
        let mut spec = reference_spec();
        spec.effective_gap_mm *= 2.0;
        let far = evaluate_thin_sheet_induction(&spec);
        assert!((far.b1_tesla / base.b1_tesla - 0.5).abs() < 1e-9);
        assert!((far.locked_rotor_torque_nm / base.locked_rotor_torque_nm - 0.25).abs() < 1e-9);
    }

    #[test]
    fn non_physical_inputs_collapse_to_zero() {
        for mutate in [
            (|s: &mut ThinSheetInductionSpec| s.pole_pairs = 0.0) as fn(&mut _),
            |s: &mut ThinSheetInductionSpec| s.effective_gap_mm = 0.0,
            |s: &mut ThinSheetInductionSpec| s.sheet_conductance_s = -1.0,
            |s: &mut ThinSheetInductionSpec| s.outer_radius_mm = s.inner_radius_mm,
            |s: &mut ThinSheetInductionSpec| s.end_effect_factor = 0.0,
        ] {
            let mut spec = reference_spec();
            mutate(&mut spec);
            let perf = evaluate_thin_sheet_induction(&spec);
            assert_eq!(perf.locked_rotor_torque_nm, 0.0);
            assert_eq!(perf.b1_tesla, 0.0);
        }
    }
}
