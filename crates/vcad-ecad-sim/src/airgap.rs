//! First-order air-gap magnetic field via a magnetic-equivalent-circuit (MEC)
//! reluctance network.
//!
//! The air-gap flux density `B_gap` is the single most important magnetic input
//! to motor performance (it sets the torque constant), yet
//! [`crate::magnetics::motor_torque_constant`] takes it as a *given*. This module
//! closes that loop: it computes `B_gap` from magnet and geometry parameters so
//! the rest of the magnetics stack no longer hardcodes a flux guess.
//!
//! # Model and its limits (read this)
//!
//! This is an explicitly **first-order** lumped reluctance network for the common
//! PM-rotor + soft-iron-stator topology (radial or axial flux). It is deliberately
//! coarse:
//!
//! - **No slotting** — the air gap is treated as smooth; there is no Carter
//!   coefficient correcting for stator teeth/slots.
//! - **No fringing** — flux is assumed to cross the gap straight, with no leakage
//!   spreading at the pole edges.
//! - **No saturation** — by default the soft-iron path is treated as *infinite*
//!   permeability (zero iron reluctance), so the magnet works only against the
//!   air gap. A finite-permeability iron path is available as an optional
//!   refinement but still assumes linear, unsaturated B-H.
//!
//! Use it for sizing intuition and as a differentiable leaf for co-design, not as
//! a substitute for FEA.
//!
//! # The reluctance network
//!
//! A permanent magnet of remanence `Br`, recoil relative permeability `mu_rec`,
//! thickness `l_m` and pole face area `A_m` is modeled as a flux source `phi_r =
//! Br * A_m` behind an internal reluctance `R_m = l_m / (mu0 * mu_rec * A_m)`.
//! That drives flux through the series air-gap reluctance `R_g = g / (mu0 * A_g)`
//! (and, optionally, an iron-path reluctance `R_fe`). With iron taken as infinite
//! permeability (`R_fe = 0`), the gap flux is
//!
//! ```text
//!   phi_g = phi_r * R_m / (R_m + R_g)
//!   B_gap = phi_g / A_g
//!         = Br * (A_m / A_g) / (1 + mu_rec * (g / l_m) * (A_m / A_g))
//! ```
//!
//! For the equal-area case (`A_m == A_g`) this collapses to the familiar
//! `B_gap = Br / (1 + mu_rec * g / l_m)`.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Permeability of free space, T·m/A.
const MU0: f64 = 4.0 * PI * 1e-7;

/// Inputs for the cored (back-iron) air-gap MEC model.
///
/// All lengths in millimetres, areas in mm² (the area *ratio* is what matters,
/// so consistent units cancel). `Br` in tesla. The model returns the operating
/// air-gap flux density via [`airgap_flux_density`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirGapSpec {
    /// Magnet remanent flux density `Br`, tesla (NdFeB ≈ 1.2, ferrite ≈ 0.4).
    pub remanence_tesla: f64,
    /// Magnet thickness in the magnetization direction, mm.
    pub magnet_thickness_mm: f64,
    /// Recoil relative permeability of the magnet (NdFeB ≈ 1.05, ferrite ≈ 1.1).
    pub recoil_mu_rel: f64,
    /// Air-gap length, mm.
    pub airgap_mm: f64,
    /// Magnet pole face area, mm² (cross-section the flux leaves the magnet).
    pub magnet_area_mm2: f64,
    /// Air-gap face area, mm² (cross-section the flux crosses the gap).
    pub gap_area_mm2: f64,
    /// Soft-iron path relative permeability. `None` (or non-finite) treats the
    /// iron as ideal (infinite permeability, zero reluctance) — the first cut.
    /// `Some(mu_r)` adds a finite, *linear* iron reluctance to the loop.
    pub iron_mu_rel: Option<f64>,
    /// Mean soft-iron flux path length (stator + rotor back-iron), mm. Only used
    /// when `iron_mu_rel` is `Some`. Ignored otherwise.
    pub iron_path_mm: f64,
    /// Iron cross-section area carrying the flux, mm². Only used when
    /// `iron_mu_rel` is `Some`. Ignored otherwise.
    pub iron_area_mm2: f64,
}

impl AirGapSpec {
    /// A sensible NdFeB starting point: Br = 1.2 T, 3 mm magnet, 1 mm gap,
    /// recoil permeability 1.05, equal magnet/gap areas, ideal iron.
    ///
    /// Areas are unit (the ratio is 1), so only the length terms matter.
    pub fn ndfeb_default() -> Self {
        Self {
            remanence_tesla: 1.2,
            magnet_thickness_mm: 3.0,
            recoil_mu_rel: 1.05,
            airgap_mm: 1.0,
            magnet_area_mm2: 1.0,
            gap_area_mm2: 1.0,
            iron_mu_rel: None,
            iron_path_mm: 0.0,
            iron_area_mm2: 1.0,
        }
    }
}

/// Solve the reluctance network for the operating air-gap flux density (tesla).
///
/// Returns 0.0 for any non-physical input that would zero or break the loop
/// (`Br <= 0`, `magnet_thickness <= 0`, or any area `<= 0`). A non-positive air
/// gap is treated as zero gap (`B_gap == Br * A_m / A_g`, no gap reluctance).
///
/// First-order only: no slotting, no fringing, no saturation. See the module
/// docs for the full reluctance derivation and assumptions.
pub fn airgap_flux_density(spec: &AirGapSpec) -> f64 {
    let br = spec.remanence_tesla;
    let l_m = spec.magnet_thickness_mm;
    let mu_rec = spec.recoil_mu_rel;
    let g = spec.airgap_mm.max(0.0);
    let a_m = spec.magnet_area_mm2;
    let a_g = spec.gap_area_mm2;

    // Non-physical guards: collapse to zero rather than NaN/inf.
    if br <= 0.0 || l_m <= 0.0 || mu_rec <= 0.0 || a_m <= 0.0 || a_g <= 0.0 {
        return 0.0;
    }

    // Reluctances. Lengths in mm, areas in mm² — the 1e-3 / 1e-6 factors are a
    // common scale across every reluctance, so they cancel in the flux divider.
    // We keep them explicit for readability (and so the iron term, which has a
    // different geometry, scales correctly relative to the gap and magnet).
    let scale = 1e-3 / 1e-6; // mm / mm² -> 1/m, applied uniformly below
    let r_m = l_m / (MU0 * mu_rec * a_m) * scale; // magnet internal reluctance
    let r_g = g / (MU0 * a_g) * scale; // air-gap reluctance

    // Optional finite, linear iron path (still no saturation).
    let r_fe = match spec.iron_mu_rel {
        Some(mu_fe) if mu_fe.is_finite() && mu_fe > 0.0 && spec.iron_area_mm2 > 0.0 => {
            spec.iron_path_mm / (MU0 * mu_fe * spec.iron_area_mm2) * scale
        }
        _ => 0.0, // ideal iron: zero reluctance
    };

    // Magnet as a flux source phi_r = Br * A_m behind R_m, driving the series
    // gap (+ iron) reluctance. phi_g = phi_r * R_m / (R_m + R_g + R_fe).
    let phi_r = br * (a_m * 1e-6); // Wb (A_m converted to m²)
    let phi_g = phi_r * r_m / (r_m + r_g + r_fe);
    // B_gap = phi_g / A_g (A_g in m²).
    phi_g / (a_g * 1e-6)
}

/// First-order Carter-like fringing derate for the MEC gap field.
///
/// [`airgap_flux_density`] assumes flux crosses the gap straight — no
/// spreading at the pole edges. Real flux fringes outward by roughly one gap
/// length per pole edge (the classical straight-line-plus-quarter-circle
/// fringe-tube estimate), so the same total flux crosses an effectively wider
/// pole and the density *under* the pole face drops. Modeling the widening in
/// the one dimension that matters (across the pole width `w`, gap `g`):
///
/// ```text
///   B_derated = B_raw · w / (w + 2g)  =  B_raw · ρ / (ρ + 2),   ρ = w/g
/// ```
///
/// This is the fringing analogue of Carter's slotting coefficient — a pure
/// geometry ratio, first-order in `g/w`. It is honest only while the pole is
/// wide compared to the gap (`ρ ≳ 2`); below that the fringe tubes overlap
/// and the closed form under-predicts the field, so treat small-`ρ` results
/// as a lower bound. Returns 1.0 (no derate) for a non-positive gap and 0.0
/// for a non-positive pole width.
pub fn fringing_derate(pole_width_mm: f64, airgap_mm: f64) -> f64 {
    if airgap_mm <= 0.0 {
        return 1.0;
    }
    if pole_width_mm <= 0.0 {
        return 0.0;
    }
    pole_width_mm / (pole_width_mm + 2.0 * airgap_mm)
}

/// Coarse coreless / air-cored air-gap flux density (tesla) — **no back-iron**.
///
/// With no soft-iron return path the field is set directly by the coil MMF
/// (`N * I` ampere-turns) driving flux across the gap, with permeability `mu0`
/// everywhere:
///
/// ```text
///   B_gap ≈ mu0 * N * I / g
/// ```
///
/// Be honest: this is a *very* coarse estimate. A real air-cored machine has the
/// MMF distributed around the winding, large fringing, and a path length that is
/// not simply the mechanical gap — so this systematically over-predicts the field
/// in the gap centre. Treat it as an order-of-magnitude figure for coreless
/// (e.g. PCB-stator / Halbach-less) layouts, not a design value.
///
/// `turns` and `current_amps` are the per-pole ampere-turns; `airgap_mm` is the
/// effective magnetic gap (coil-to-coil or coil-to-rotor spacing). Returns 0.0
/// for a non-positive gap.
pub fn aircored_airgap_flux_density(turns: f64, current_amps: f64, airgap_mm: f64) -> f64 {
    if airgap_mm <= 0.0 {
        return 0.0;
    }
    let g = airgap_mm * 1e-3; // m
    MU0 * turns * current_amps / g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndfeb_default_is_physically_plausible() {
        // NdFeB Br=1.2, 3mm magnet across 1mm gap -> ~0.4..0.9 T.
        let b = airgap_flux_density(&AirGapSpec::ndfeb_default());
        assert!(
            (0.4..=0.9).contains(&b),
            "B_gap {b} out of plausible NdFeB range"
        );
        // Closed form for equal areas: Br / (1 + mu_rec * g / l_m).
        let expected = 1.2 / (1.0 + 1.05 * 1.0 / 3.0);
        assert!(
            (b - expected).abs() < 1e-9,
            "B_gap {b} vs closed form {expected}"
        );
    }

    #[test]
    fn larger_gap_means_smaller_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        let b_small = airgap_flux_density(&spec);
        spec.airgap_mm = 3.0; // triple the gap
        let b_large = airgap_flux_density(&spec);
        assert!(
            b_large < b_small,
            "bigger gap should drop B_gap: {b_large} !< {b_small}"
        );
    }

    #[test]
    fn zero_remanence_gives_zero_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        spec.remanence_tesla = 0.0;
        assert_eq!(airgap_flux_density(&spec), 0.0);
    }

    #[test]
    fn thicker_magnet_means_stronger_field() {
        let mut spec = AirGapSpec::ndfeb_default();
        let b_thin = airgap_flux_density(&spec);
        spec.magnet_thickness_mm = 6.0; // thicker magnet, same gap
        let b_thick = airgap_flux_density(&spec);
        assert!(
            b_thick > b_thin,
            "thicker magnet should raise B_gap: {b_thick} !> {b_thin}"
        );
        // Asymptote: B_gap -> Br * (A_m/A_g) as l_m -> infinity. Stay below it.
        assert!(b_thick < spec.remanence_tesla);
    }

    #[test]
    fn unequal_areas_concentrate_flux() {
        // Magnet bigger than the gap face concentrates flux -> higher B_gap.
        let mut spec = AirGapSpec::ndfeb_default();
        spec.magnet_area_mm2 = 2.0;
        spec.gap_area_mm2 = 1.0;
        let b_focus = airgap_flux_density(&spec);
        let b_equal = airgap_flux_density(&AirGapSpec::ndfeb_default());
        assert!(
            b_focus > b_equal,
            "flux focusing should raise B_gap: {b_focus} !> {b_equal}"
        );
    }

    #[test]
    fn finite_iron_reluctance_lowers_field_vs_ideal() {
        let ideal = AirGapSpec::ndfeb_default();
        let b_ideal = airgap_flux_density(&ideal);

        let mut with_iron = ideal;
        with_iron.iron_mu_rel = Some(1000.0); // good but finite silicon steel
        with_iron.iron_path_mm = 50.0;
        with_iron.iron_area_mm2 = 1.0;
        let b_iron = airgap_flux_density(&with_iron);

        assert!(
            b_iron < b_ideal,
            "finite iron reluctance should drop B_gap below ideal: {b_iron} !< {b_ideal}"
        );
        // But only slightly, since mu_fe is large.
        assert!(
            b_iron > 0.95 * b_ideal,
            "iron drop should be small: {b_iron}"
        );
    }

    #[test]
    fn fringing_derate_behaves_like_a_carter_factor() {
        // Wide pole, small gap: barely any derate.
        assert!(fringing_derate(20.0, 0.5) > 0.95);
        // ρ = w/g = 2 → w/(w+2g) = 0.5.
        assert!((fringing_derate(2.0, 1.0) - 0.5).abs() < 1e-12);
        // Monotonic: bigger gap, more fringing, lower B.
        assert!(fringing_derate(10.0, 2.0) < fringing_derate(10.0, 1.0));
        // Degenerate guards.
        assert_eq!(fringing_derate(10.0, 0.0), 1.0);
        assert_eq!(fringing_derate(0.0, 1.0), 0.0);
        // Always a derate, never a boost.
        assert!(fringing_derate(5.0, 1.0) < 1.0);
    }

    #[test]
    fn aircored_is_small_and_scales_as_expected() {
        // 100 ampere-turns across a 1 mm gap: tiny field (no iron to amplify).
        let b = aircored_airgap_flux_density(100.0, 1.0, 1.0);
        let expected = MU0 * 100.0 / 1e-3;
        assert!((b - expected).abs() < 1e-12);
        assert!(b < 0.2, "air-cored field should be small: {b}");

        // More ampere-turns -> more field; bigger gap -> less.
        assert!(aircored_airgap_flux_density(200.0, 1.0, 1.0) > b);
        assert!(aircored_airgap_flux_density(100.0, 1.0, 2.0) < b);
        // Non-positive gap -> 0.
        assert_eq!(aircored_airgap_flux_density(100.0, 1.0, 0.0), 0.0);
    }
}
