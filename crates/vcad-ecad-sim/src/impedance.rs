//! Impedance calculation for PCB traces using IPC-2141 closed-form formulas.
//!
//! Supports microstrip (outer-layer) and stripline (inner-layer) geometries,
//! both single-ended and differential.

use std::f64::consts::{E, PI};
use tang::Scalar;

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

// ---------------------------------------------------------------------------
// Scalar-generic closed-form leaves.
//
// Written over `tang::Scalar` so the same code computes f64 (forward / verify)
// and, when instantiated with `tang_expr::ExprId`, builds an expression graph
// that differentiates symbolically — the foundation for solving trace geometry
// by gradient rather than by search. Every constant is lifted with
// `S::from_f64`; π is `S::PI`.
// ---------------------------------------------------------------------------

/// Effective microstrip width (Hammerstad–Jensen copper-thickness correction).
pub fn microstrip_we<S: Scalar>(w: S, t: S, h: S) -> S {
    // NB: lift every constant with from_f64 — ExprId's associated consts
    // (PI, ONE, HALF, …) are graph sentinels, not real nodes.
    let pi = S::from_f64(PI);
    let t_over_h = t / h;
    let t_over_wp = t / (w * pi + S::from_f64(1.1) * t * pi);
    w + (t / pi)
        * (S::from_f64(4.0 * E) / (t_over_h * t_over_h + t_over_wp * t_over_wp).sqrt()).ln()
}

/// Single-ended microstrip characteristic impedance (Ω). Monotonic ↓ in `w`.
pub fn microstrip_z0<S: Scalar>(w: S, t: S, h: S, er: S) -> S {
    let we = microstrip_we(w, t, h);
    (S::from_f64(87.0) / (er + S::from_f64(1.41)).sqrt())
        * (S::from_f64(5.98) * h / (S::from_f64(0.8) * we + t)).ln()
}

/// Microstrip effective permittivity (Hammerstad–Jensen).
pub fn microstrip_er_eff<S: Scalar>(w: S, t: S, h: S, er: S) -> S {
    let we = microstrip_we(w, t, h);
    let one = S::from_f64(1.0);
    let half = S::from_f64(0.5);
    (er + one) * half + (er - one) * half * (one + S::from_f64(12.0) * h / we).sqrt().recip()
}

/// Single-ended stripline characteristic impedance (Ω). Monotonic ↓ in `w`.
pub fn stripline_z0<S: Scalar>(w: S, t: S, h: S, er: S) -> S {
    (S::from_f64(60.0) / er.sqrt())
        * (S::from_f64(4.0) * h / (S::from_f64(0.67) * S::from_f64(PI) * (S::from_f64(0.8) * w + t)))
            .ln()
}

/// Edge-coupling factor `k` for a differential pair; `Zdiff = 2·Z0·k`. `a` and
/// `b` are the empirical microstrip (0.48, 0.96) or stripline (0.347, 2.9)
/// constants.
pub fn diff_coupling_k<S: Scalar>(s: S, h: S, a: S, b: S) -> S {
    S::from_f64(1.0) - a * (-(b * s / h)).exp()
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

    // Shared Scalar-generic leaves (identical math to the differentiable path).
    let z0 = microstrip_z0(w, t, h, er);
    let er_eff = microstrip_er_eff(w, t, h, er);
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

    let z0 = stripline_z0(w, t, h, er);
    // For stripline, effective dielectric constant equals bulk Er.
    let er_eff = er;
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
    2.0 * single.z0 * diff_coupling_k(s, h, 0.48, 0.96)
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
    2.0 * single.z0 * diff_coupling_k(s, h, 0.347, 2.9)
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

    /// The differentiable foothold: trace z0(w) through tang-expr, differentiate
    /// symbolically, and confirm dz0/dw matches a central finite difference of
    /// the f64 path. This is what makes solving geometry by gradient possible.
    #[test]
    fn microstrip_z0_symbolic_gradient_matches_finite_difference() {
        use tang_expr::{trace, ExprId};
        let (t, h, er) = (0.035_f64, 0.20_f64, 4.3_f64);
        let w0 = 0.30_f64;

        // Trace z0 as a function of var(0) = w; t, h, er are baked constants.
        let (mut graph, z0_expr) = trace(|| {
            microstrip_z0(
                ExprId::var(0),
                ExprId::from_f64(t),
                ExprId::from_f64(h),
                ExprId::from_f64(er),
            )
        });

        // The traced graph evaluates to the same number as the direct f64 path.
        let traced = graph.eval(z0_expr, &[w0]);
        assert!(
            (traced - microstrip_z0(w0, t, h, er)).abs() < 1e-9,
            "traced {traced} vs direct {}",
            microstrip_z0(w0, t, h, er)
        );

        // The SYMBOLIC derivative matches a central finite difference.
        let dexpr = graph.diff(z0_expr, 0);
        let grad = graph.eval(dexpr, &[w0]);
        let eps = 1e-6;
        let fd = (microstrip_z0(w0 + eps, t, h, er) - microstrip_z0(w0 - eps, t, h, er)) / (2.0 * eps);
        assert!(
            (grad - fd).abs() < 1e-4,
            "symbolic dz0/dw {grad} vs finite-difference {fd}"
        );
        // Wider trace → lower impedance, so the gradient is negative.
        assert!(grad < 0.0, "dz0/dw should be negative, got {grad}");
    }
}
