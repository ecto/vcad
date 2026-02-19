//! Impedance calculation for PCB traces using IPC-2141 closed-form formulas.
//!
//! Supports microstrip (outer-layer) and stripline (inner-layer) geometries,
//! both single-ended and differential.

use std::f64::consts::{E, PI};

/// Impedance calculation result.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpedanceResult {
    /// Characteristic impedance in ohms.
    pub z0: f64,
    /// Effective dielectric constant.
    pub er_eff: f64,
    /// Propagation delay in ps/mm.
    pub delay_ps_per_mm: f64,
}

/// Calculate microstrip impedance (outer-layer trace) using simplified IPC-2141.
///
/// # Arguments
///
/// * `w` - Trace width in mm
/// * `t` - Copper thickness in mm
/// * `h` - Dielectric height (substrate thickness) in mm
/// * `er` - Relative permittivity of the dielectric
///
/// # Returns
///
/// An [`ImpedanceResult`] with the characteristic impedance, effective dielectric
/// constant, and propagation delay.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn microstrip_impedance(w: f64, t: f64, h: f64, er: f64) -> ImpedanceResult {
    assert!(w > 0.0, "trace width must be positive");
    assert!(t > 0.0, "copper thickness must be positive");
    assert!(h > 0.0, "dielectric height must be positive");
    assert!(er > 0.0, "relative permittivity must be positive");

    // Effective width accounting for copper thickness (Hammerstad–Jensen correction)
    let t_over_h = t / h;
    let t_over_wp = t / (w * PI + 1.1 * t * PI);
    let we = w + (t / PI) * (4.0 * E / (t_over_h * t_over_h + t_over_wp * t_over_wp).sqrt()).ln();

    // Characteristic impedance (IPC-2141 simplified)
    let z0 = (87.0 / (er + 1.41).sqrt()) * (5.98 * h / (0.8 * we + t)).ln();

    // Effective dielectric constant (Hammerstad–Jensen)
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / 2.0 * (1.0 + 12.0 * h / we).powf(-0.5);

    // Propagation delay in ps/mm
    let delay_ps_per_mm = 3.336 * er_eff.sqrt();

    ImpedanceResult {
        z0,
        er_eff,
        delay_ps_per_mm,
    }
}

/// Calculate stripline impedance (inner-layer trace between two ground planes).
///
/// # Arguments
///
/// * `w` - Trace width in mm
/// * `t` - Copper thickness in mm
/// * `h` - Total dielectric height (distance between ground planes) in mm
/// * `er` - Relative permittivity of the dielectric
///
/// # Returns
///
/// An [`ImpedanceResult`] with the characteristic impedance. For stripline,
/// `er_eff` equals `er` since the trace is fully embedded in the dielectric.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn stripline_impedance(w: f64, t: f64, h: f64, er: f64) -> ImpedanceResult {
    assert!(w > 0.0, "trace width must be positive");
    assert!(t > 0.0, "copper thickness must be positive");
    assert!(h > 0.0, "dielectric height must be positive");
    assert!(er > 0.0, "relative permittivity must be positive");

    // Stripline impedance (IPC-2141)
    let z0 = (60.0 / er.sqrt()) * (4.0 * h / (0.67 * PI * (0.8 * w + t))).ln();

    // For stripline, effective dielectric constant equals bulk Er
    let er_eff = er;

    // Propagation delay in ps/mm
    let delay_ps_per_mm = 3.336 * er_eff.sqrt();

    ImpedanceResult {
        z0,
        er_eff,
        delay_ps_per_mm,
    }
}

/// Calculate differential microstrip impedance.
///
/// Uses the single-ended microstrip impedance with a coupling correction
/// factor based on trace spacing.
///
/// # Arguments
///
/// * `w` - Trace width in mm
/// * `s` - Spacing between the two traces in mm
/// * `t` - Copper thickness in mm
/// * `h` - Dielectric height in mm
/// * `er` - Relative permittivity
///
/// # Returns
///
/// Differential impedance in ohms.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn diff_microstrip_impedance(w: f64, s: f64, t: f64, h: f64, er: f64) -> f64 {
    assert!(s > 0.0, "trace spacing must be positive");

    let single = microstrip_impedance(w, t, h, er);

    // Coupling correction: Zdiff = 2 * Z0 * (1 - 0.48 * exp(-0.96 * s/h))
    // This empirical formula accounts for mutual coupling between the pair.
    let correction = 1.0 - 0.48 * (-0.96 * s / h).exp();
    2.0 * single.z0 * correction
}

/// Calculate differential stripline impedance.
///
/// Uses the single-ended stripline impedance with a coupling correction
/// factor based on trace spacing.
///
/// # Arguments
///
/// * `w` - Trace width in mm
/// * `s` - Spacing between the two traces in mm
/// * `t` - Copper thickness in mm
/// * `h` - Total dielectric height in mm
/// * `er` - Relative permittivity
///
/// # Returns
///
/// Differential impedance in ohms.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn diff_stripline_impedance(w: f64, s: f64, t: f64, h: f64, er: f64) -> f64 {
    assert!(s > 0.0, "trace spacing must be positive");

    let single = stripline_impedance(w, t, h, er);

    // Coupling correction: Zdiff = 2 * Z0 * (1 - 0.347 * exp(-2.9 * s/h))
    let correction = 1.0 - 0.347 * (-2.9 * s / h).exp();
    2.0 * single.z0 * correction
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert that a value is within a percentage tolerance of expected.
    fn assert_within_pct(actual: f64, expected: f64, pct: f64, label: &str) {
        let diff = ((actual - expected) / expected).abs() * 100.0;
        assert!(
            diff < pct,
            "{label}: expected ~{expected:.2}, got {actual:.2} ({diff:.1}% off, tolerance {pct}%)"
        );
    }

    #[test]
    fn test_microstrip_50_ohm() {
        // FR-4 stackup targeting ~50 ohm with simplified IPC-2141:
        // w=0.30mm, t=0.035mm (1oz copper), h=0.20mm, er=4.3
        // The simplified formula gives ~49-51 ohm for these parameters.
        let result = microstrip_impedance(0.30, 0.035, 0.20, 4.3);
        assert_within_pct(result.z0, 50.0, 10.0, "microstrip Z0");
        assert!(result.er_eff > 1.0, "er_eff must be > 1");
        assert!(result.er_eff < 4.3, "er_eff must be < er for microstrip");
        assert!(result.delay_ps_per_mm > 0.0, "delay must be positive");
    }

    #[test]
    fn test_microstrip_er_eff_bounds() {
        let result = microstrip_impedance(0.15, 0.035, 0.10, 4.5);
        // For microstrip, er_eff should be between 1 and er
        assert!(result.er_eff > 1.0);
        assert!(result.er_eff < 4.5);
    }

    #[test]
    fn test_microstrip_wider_trace_lower_impedance() {
        let narrow = microstrip_impedance(0.10, 0.035, 0.20, 4.3);
        let wide = microstrip_impedance(0.40, 0.035, 0.20, 4.3);
        assert!(
            wide.z0 < narrow.z0,
            "wider trace should have lower impedance"
        );
    }

    #[test]
    fn test_microstrip_thicker_dielectric_higher_impedance() {
        let thin = microstrip_impedance(0.20, 0.035, 0.10, 4.3);
        let thick = microstrip_impedance(0.20, 0.035, 0.30, 4.3);
        assert!(
            thick.z0 > thin.z0,
            "thicker dielectric should give higher impedance"
        );
    }

    #[test]
    fn test_microstrip_higher_er_lower_impedance() {
        let low_er = microstrip_impedance(0.20, 0.035, 0.20, 3.0);
        let high_er = microstrip_impedance(0.20, 0.035, 0.20, 6.0);
        assert!(
            high_er.z0 < low_er.z0,
            "higher permittivity should give lower impedance"
        );
    }

    #[test]
    fn test_stripline_50_ohm() {
        // Stripline targeting ~50 ohm:
        // w=0.15mm, t=0.035mm, h=0.40mm (total between planes), er=4.3
        let result = stripline_impedance(0.15, 0.035, 0.40, 4.3);
        assert_within_pct(result.z0, 50.0, 20.0, "stripline Z0");
        // For stripline, er_eff == er
        assert!(
            (result.er_eff - 4.3).abs() < 1e-10,
            "stripline er_eff should equal er"
        );
    }

    #[test]
    fn test_stripline_wider_trace_lower_impedance() {
        let narrow = stripline_impedance(0.10, 0.035, 0.40, 4.3);
        let wide = stripline_impedance(0.30, 0.035, 0.40, 4.3);
        assert!(wide.z0 < narrow.z0, "wider trace should have lower Z0");
    }

    #[test]
    fn test_stripline_delay() {
        let result = stripline_impedance(0.15, 0.035, 0.40, 4.3);
        // Stripline delay should be ~6.9 ps/mm for er=4.3
        let expected_delay = 3.336 * 4.3_f64.sqrt();
        assert!(
            (result.delay_ps_per_mm - expected_delay).abs() < 1e-10,
            "stripline delay should be 3.336 * sqrt(er)"
        );
    }

    #[test]
    fn test_diff_microstrip() {
        let single = microstrip_impedance(0.15, 0.035, 0.15, 4.3);
        let diff = diff_microstrip_impedance(0.15, 0.15, 0.035, 0.15, 4.3);

        // Differential impedance should be less than 2 * single-ended
        // (coupling reduces the effective impedance)
        assert!(diff < 2.0 * single.z0, "diff Z should be < 2 * Z0");
        assert!(diff > single.z0, "diff Z should be > Z0");

        // With wide spacing, coupling vanishes and diff -> 2 * Z0
        let wide_diff = diff_microstrip_impedance(0.15, 5.0, 0.035, 0.15, 4.3);
        assert_within_pct(wide_diff, 2.0 * single.z0, 2.0, "wide-spaced diff");
    }

    #[test]
    fn test_diff_stripline() {
        let single = stripline_impedance(0.15, 0.035, 0.40, 4.3);
        let diff = diff_stripline_impedance(0.15, 0.20, 0.035, 0.40, 4.3);

        assert!(diff < 2.0 * single.z0, "diff Z should be < 2 * Z0");
        assert!(diff > single.z0, "diff Z should be > Z0");

        // Wide spacing -> 2 * Z0
        let wide_diff = diff_stripline_impedance(0.15, 10.0, 0.035, 0.40, 4.3);
        assert_within_pct(
            wide_diff,
            2.0 * single.z0,
            1.0,
            "wide-spaced stripline diff",
        );
    }

    #[test]
    #[should_panic(expected = "trace width must be positive")]
    fn test_microstrip_zero_width_panics() {
        microstrip_impedance(0.0, 0.035, 0.20, 4.3);
    }

    #[test]
    #[should_panic(expected = "trace spacing must be positive")]
    fn test_diff_microstrip_zero_spacing_panics() {
        diff_microstrip_impedance(0.15, 0.0, 0.035, 0.15, 4.3);
    }
}
