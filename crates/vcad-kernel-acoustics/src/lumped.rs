//! Lumped-element acoustics: the closed-form spine of loudspeaker design.
//!
//! Below the first cross-mode, a duct behaves as an acoustic **mass** and a
//! cavity as an acoustic **compliance** (a spring). Their resonance is the
//! Helmholtz frequency — the tuning of a bass-reflex port, the note of a
//! blown bottle. These formulas are simultaneously **features** (a designer
//! wants the number) and the **oracles** that validate the field solver.
//!
//! References: Beranek & Mellow, *Acoustics: Sound Fields and Transducers*,
//! §4 (lumped acoustic elements); Kinsler & Frey, *Fundamentals of
//! Acoustics*, 4th ed., §10.5 (the Helmholtz resonator) and §7.5 (end
//! corrections). All SI unless a name says `_mm`.

use crate::medium::Medium;

/// End correction for a circular open pipe end, meters, given tube radius
/// `a` (m). Flanged (baffled) ends load more air than free ones.
///
/// - flanged: `Δ = 8a/3π ≈ 0.8488·a` (a piston in an infinite baffle);
/// - unflanged: `Δ ≈ 0.6133·a` (a pipe radiating into free space).
///
/// (Kinsler & Frey §7.5; Levine & Schwinger 1948 for the unflanged value.)
pub fn end_correction(radius_m: f64, flanged: bool) -> f64 {
    if flanged {
        8.0 * radius_m / (3.0 * std::f64::consts::PI)
    } else {
        0.6133 * radius_m
    }
}

/// Effective acoustic length of a neck/port, meters: the physical length plus
/// the end corrections at each end (`inner`/`outer` = flanged or not).
pub fn effective_length(
    length_m: f64,
    radius_m: f64,
    inner_flanged: bool,
    outer_flanged: bool,
) -> f64 {
    length_m + end_correction(radius_m, inner_flanged) + end_correction(radius_m, outer_flanged)
}

/// Acoustic mass of a duct, kg/m⁴: `M_A = ρ·L_eff / S`.
/// The port's inertia — the air plug that has to be shoved back and forth.
pub fn duct_acoustic_mass(medium: &Medium, l_eff_m: f64, area_m2: f64) -> f64 {
    medium.rho * l_eff_m / area_m2
}

/// Acoustic compliance of a closed cavity, m⁵/N (= m³/Pa):
/// `C_A = V / (ρc²)`. The box's springiness.
pub fn cavity_compliance(medium: &Medium, volume_m3: f64) -> f64 {
    volume_m3 / (medium.rho * medium.c * medium.c)
}

/// Helmholtz resonance from lumped mass and compliance, Hz:
/// `f = 1 / (2π·√(M_A·C_A))`. Fail-soft: non-positive arguments give 0.
pub fn helmholtz_from_lumped(m_a: f64, c_a: f64) -> f64 {
    let mc = m_a * c_a;
    if mc <= 0.0 {
        return 0.0;
    }
    1.0 / (std::f64::consts::TAU * mc.sqrt())
}

/// The Helmholtz resonator frequency in the compact form
/// `f = (c/2π)·√(S / (V·L_eff))`, Hz. `l_eff_m` already includes the end
/// corrections. Equivalent to composing [`duct_acoustic_mass`],
/// [`cavity_compliance`] and [`helmholtz_from_lumped`].
pub fn helmholtz_frequency(medium: &Medium, area_m2: f64, volume_m3: f64, l_eff_m: f64) -> f64 {
    if volume_m3 <= 0.0 || l_eff_m <= 0.0 || area_m2 <= 0.0 {
        return 0.0;
    }
    (medium.c / std::f64::consts::TAU) * (area_m2 / (volume_m3 * l_eff_m)).sqrt()
}

/// A predicted resonance with the end-correction uncertainty made explicit:
/// the pressure-release field model omits the exterior radiation mass, so the
/// field-solved tuning lands between `f_min` (both ends fully flanged, longest
/// L_eff) and `f_max` (interior end only). `f_nominal` uses interior-flanged +
/// exterior-unflanged, the honest best estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuningBand {
    /// Lowest plausible tuning, Hz (longest effective length).
    pub f_min_hz: f64,
    /// Best-estimate tuning, Hz.
    pub f_nominal_hz: f64,
    /// Highest plausible tuning, Hz (shortest effective length).
    pub f_max_hz: f64,
}

impl TuningBand {
    /// True if `f` lies within `[f_min, f_max]` (inclusive, small tolerance).
    pub fn contains(&self, f_hz: f64) -> bool {
        f_hz >= self.f_min_hz * (1.0 - 1e-9) && f_hz <= self.f_max_hz * (1.0 + 1e-9)
    }
}

/// Ported-box (bass-reflex) tuning `f_b` and its end-correction band, from
/// geometry in **millimeters**. The box + port form a Helmholtz resonator:
/// the port air mass resonates against the box compliance. At `f_b` the cone
/// motion is minimal and the port carries the output — the defining
/// bass-reflex behaviour.
pub fn ported_box_tuning_mm(
    medium: &Medium,
    box_volume_mm3: f64,
    port_radius_mm: f64,
    port_length_mm: f64,
) -> TuningBand {
    let v = box_volume_mm3 * 1e-9; // mm³ → m³
    let a = port_radius_mm * 1e-3;
    let l = port_length_mm * 1e-3;
    let s = std::f64::consts::PI * a * a;
    // Longest L_eff → lowest f: both ends flanged.
    let l_max = effective_length(l, a, true, true);
    // Shortest L_eff → highest f: interior flanged only (the pressure-release
    // mouth omits the exterior mass).
    let l_min = l + end_correction(a, true);
    // Nominal: interior flanged, exterior unflanged.
    let l_nom = effective_length(l, a, true, false);
    TuningBand {
        f_min_hz: helmholtz_frequency(medium, s, v, l_max),
        f_nominal_hz: helmholtz_frequency(medium, s, v, l_nom),
        f_max_hz: helmholtz_frequency(medium, s, v, l_min),
    }
}

/// Rigid closed-cylinder axial mode `n` (both ends rigid), Hz: `fₙ = n·c/2L`.
/// The exact eigenvalue the field solver must reproduce.
pub fn closed_cylinder_axial_hz(medium: &Medium, length_mm: f64, n: usize) -> f64 {
    n as f64 * medium.c / (2.0 * length_mm * 1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compliance_and_mass_compose_to_the_compact_form() {
        let air = Medium::air(20.0);
        let s = std::f64::consts::PI * 0.01 * 0.01; // 10 mm radius
        let v = 1e-3; // 1 L
        let l_eff = 0.05;
        let m = duct_acoustic_mass(&air, l_eff, s);
        let c = cavity_compliance(&air, v);
        let f_lumped = helmholtz_from_lumped(m, c);
        let f_compact = helmholtz_frequency(&air, s, v, l_eff);
        assert!(
            (f_lumped - f_compact).abs() < 1e-9,
            "{f_lumped} vs {f_compact}"
        );
    }

    #[test]
    fn helmholtz_scales_as_the_physics_says() {
        let air = Medium::air(20.0);
        let f0 = helmholtz_frequency(&air, 1e-4, 1e-3, 0.05);
        // Quadrupling the volume halves the frequency (∝ 1/√V).
        let f1 = helmholtz_frequency(&air, 1e-4, 4e-3, 0.05);
        assert!((f1 - f0 / 2.0).abs() / f0 < 1e-9);
        // Quadrupling the neck area doubles it (∝ √S).
        let f2 = helmholtz_frequency(&air, 4e-4, 1e-3, 0.05);
        assert!((f2 - 2.0 * f0).abs() / f0 < 1e-9);
    }

    #[test]
    fn textbook_bottle_lands_in_the_audible_bass() {
        // A 1-litre cavity, 20 mm-diameter × 50 mm neck: tens of Hz, and the
        // hand computation f = (c/2π)√(S/(V·L_eff)).
        let air = Medium::air(20.0);
        let a = 0.01;
        let s = std::f64::consts::PI * a * a;
        let l_eff = effective_length(0.05, a, false, false);
        let f = helmholtz_frequency(&air, s, 1e-3, l_eff);
        let by_hand = (air.c / std::f64::consts::TAU) * (s / (1e-3 * l_eff)).sqrt();
        assert!((f - by_hand).abs() < 1e-9);
        assert!((90.0..140.0).contains(&f), "bottle f = {f}");
    }

    #[test]
    fn ported_box_band_is_ordered_and_brackets_nominal() {
        let air = Medium::air(20.0);
        // 20 L box, 50 mm-dia port, 120 mm long — a typical subwoofer vent.
        let band = ported_box_tuning_mm(&air, 20e6, 25.0, 120.0);
        assert!(band.f_min_hz < band.f_nominal_hz);
        assert!(band.f_nominal_hz < band.f_max_hz);
        assert!(band.contains(band.f_nominal_hz));
        // Bass-reflex vents land in the deep bass (tens of Hz).
        assert!(
            (20.0..60.0).contains(&band.f_nominal_hz),
            "f_b = {}",
            band.f_nominal_hz
        );
    }

    #[test]
    fn axial_modes_are_the_harmonic_ladder() {
        let air = Medium::air(20.0);
        let f1 = closed_cylinder_axial_hz(&air, 340.0, 1);
        let f2 = closed_cylinder_axial_hz(&air, 340.0, 2);
        assert!((f2 - 2.0 * f1).abs() < 1e-9);
        // c/2L for L = 0.34 m ≈ 504.7 Hz.
        assert!((f1 - air.c / 0.68).abs() < 1e-9);
    }
}
