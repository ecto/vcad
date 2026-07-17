//! Magnetic material laws beyond constant μ_r.
//!
//! M1 adds isotropic single-valued saturation (no hysteresis — a B–H
//! *curve*, not a loop): the arctangent law
//!
//! ```text
//!   B(H) = μ₀·H + (2·J_s/π)·atan(H/H₀),
//!   H₀ = 2·J_s / (π·(μ_ri − 1)·μ₀)
//! ```
//!
//! which has initial slope `μ₀·μ_ri`, saturation polarization `J_s`
//! (tesla), and the physically correct deep-saturation slope μ₀ (the
//! Fröhlich form saturates flat, which makes ν diverge — this one
//! doesn't). The solver consumes the **secant reluctivity**
//! `ν(B) = H(B)/B`, obtained by Newton inversion of the monotone law.
//!
//! Honesty: single-valued and isotropic — no hysteresis loss, no
//! anisotropy, no temperature dependence. Data sheets quote B_sat at a
//! field strength; `J_s` here is the polarization asymptote, slightly
//! above the quoted B_sat minus μ₀H.

use crate::constants::MU_0;

/// Saturation parameters for the arctangent B–H law.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Saturation {
    /// Saturation polarization J_s, tesla (ferrites ≈ 0.35–0.5, silicon
    /// steel ≈ 1.6–2.0).
    pub js_t: f64,
}

/// B(H) of the arctangent law with initial relative permeability
/// `mu_ri` and saturation `sat`.
pub fn b_of_h(mu_ri: f64, sat: Saturation, h: f64) -> f64 {
    let h0 = 2.0 * sat.js_t / (std::f64::consts::PI * (mu_ri - 1.0) * MU_0);
    MU_0 * h + (2.0 * sat.js_t / std::f64::consts::PI) * (h / h0).atan()
}

/// Secant reluctivity `ν(B) = H(B)/B` of the arctangent law, by Newton
/// inversion (the law is strictly monotone). At `B → 0` this limits to
/// `1/(μ₀·μ_ri)`.
pub fn nu_from_b(mu_ri: f64, sat: Saturation, b: f64) -> f64 {
    let b = b.abs();
    let nu0 = 1.0 / (MU_0 * mu_ri);
    if b < 1e-9 {
        return nu0;
    }
    let h0 = 2.0 * sat.js_t / (std::f64::consts::PI * (mu_ri - 1.0) * MU_0);
    let a = 2.0 * sat.js_t / std::f64::consts::PI;
    // Newton on f(H) = μ₀H + a·atan(H/H₀) − b, warm-started on the
    // initial slope.
    let mut h = b * nu0;
    for _ in 0..60 {
        let f = MU_0 * h + a * (h / h0).atan() - b;
        let df = MU_0 + a / h0 / (1.0 + (h / h0) * (h / h0));
        let step = f / df;
        h -= step;
        if step.abs() < 1e-12 * h.abs().max(1.0) {
            break;
        }
    }
    (h / b).max(1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FERRITE: Saturation = Saturation { js_t: 0.45 };

    #[test]
    fn small_signal_limit_is_the_initial_permeability() {
        let nu = nu_from_b(1000.0, FERRITE, 1e-6);
        let expect = 1.0 / (MU_0 * 1000.0);
        assert!(((nu - expect) / expect).abs() < 1e-6);
    }

    #[test]
    fn inversion_round_trips_the_law() {
        for h in [1.0, 50.0, 500.0, 5_000.0, 200_000.0] {
            let b = b_of_h(1000.0, FERRITE, h);
            let nu = nu_from_b(1000.0, FERRITE, b);
            let h_back = nu * b;
            assert!(
                ((h_back - h) / h).abs() < 1e-9,
                "H = {h}: B = {b}, back {h_back}"
            );
        }
    }

    #[test]
    fn deep_saturation_approaches_vacuum_slope() {
        // Differential slope dB/dH → μ₀; the secant ν rises toward (but
        // never exceeds) 1/μ₀.
        let b1 = b_of_h(1000.0, FERRITE, 1e6);
        let b2 = b_of_h(1000.0, FERRITE, 1.1e6);
        let slope = (b2 - b1) / 0.1e6;
        assert!(((slope - MU_0) / MU_0).abs() < 1e-3, "slope {slope:.3e}");
        let nu = nu_from_b(1000.0, FERRITE, b1);
        assert!(nu < 1.0 / MU_0);
        assert!(nu > 0.5 / MU_0, "deep saturation ν should approach 1/μ₀");
    }

    #[test]
    fn secant_permeability_decreases_monotonically() {
        let mut last = f64::MAX;
        for b in [0.01, 0.1, 0.3, 0.45, 0.6, 1.0] {
            let mu_sec = 1.0 / (nu_from_b(2000.0, FERRITE, b) * MU_0);
            assert!(mu_sec < last, "μ_sec must fall with B: {mu_sec} at {b}");
            last = mu_sec;
        }
    }
}
