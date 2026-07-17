//! Process capability: Φ-based yield, Cp/Cpk, and the hand-rolled erf
//! they stand on.
//!
//! The error function uses the Abramowitz & Stegun rational
//! approximation 7.1.26 (Handbook of Mathematical Functions, 1964;
//! originally Hastings 1955), with stated maximum absolute error
//! **1.5×10⁻⁷**. That error propagates directly into yield estimates:
//! a claimed yield of 0.9973000 is really 0.9973000 ± 1.5e-7 from the
//! approximation alone — far below Monte Carlo error at any practical
//! sample count, which is why the RSS and MC paths can check each other.

/// Error function via Abramowitz & Stegun 7.1.26. Max |error| 1.5e-7.
pub fn erf(x: f64) -> f64 {
    // Constants from A&S 7.1.26.
    const P: f64 = 0.327_591_1;
    const A1: f64 = 0.254_829_592;
    const A2: f64 = -0.284_496_736;
    const A3: f64 = 1.421_413_741;
    const A4: f64 = -1.453_152_027;
    const A5: f64 = 1.061_405_429;
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + P * x);
    let poly = ((((A5 * t + A4) * t + A3) * t + A2) * t + A1) * t;
    sign * (1.0 - poly * (-x * x).exp())
}

/// Standard normal CDF: Φ(z) = ½(1 + erf(z/√2)).
pub fn phi(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Standard normal PDF: φ(z) = e^(−z²/2)/√(2π). Exact (no
/// approximation); used by the exact yield sensitivities.
pub fn phi_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// Probability that a Normal(mean, sigma) falls within the requirement
/// bounds. With `sigma == 0` this degenerates honestly to a 0/1
/// indicator on the mean.
pub fn yield_within(mean: f64, sigma: f64, lower: Option<f64>, upper: Option<f64>) -> f64 {
    if sigma == 0.0 {
        let ok_lower = lower.is_none_or(|l| mean >= l);
        let ok_upper = upper.is_none_or(|u| mean <= u);
        return if ok_lower && ok_upper { 1.0 } else { 0.0 };
    }
    let hi = upper.map_or(1.0, |u| phi((u - mean) / sigma));
    let lo = lower.map_or(0.0, |l| phi((l - mean) / sigma));
    (hi - lo).max(0.0)
}

/// Process capability Cp = (USL − LSL)/(6σ). Needs both limits; a
/// one-sided requirement has no Cp (that's what Cpk is for).
pub fn cp(sigma: f64, lower: Option<f64>, upper: Option<f64>) -> Option<f64> {
    match (lower, upper) {
        (Some(l), Some(u)) if sigma > 0.0 => Some((u - l) / (6.0 * sigma)),
        _ => None,
    }
}

/// Process capability index Cpk = min over present limits of
/// (distance from mean to limit)/(3σ). `None` when σ = 0 (capability
/// of a deterministic chain is not a meaningful ratio).
pub fn cpk(mean: f64, sigma: f64, lower: Option<f64>, upper: Option<f64>) -> Option<f64> {
    if sigma <= 0.0 {
        return None;
    }
    let from_lower = lower.map(|l| (mean - l) / (3.0 * sigma));
    let from_upper = upper.map(|u| (u - mean) / (3.0 * sigma));
    match (from_lower, from_upper) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_matches_reference_values() {
        // Reference values (A&S Table 7.1 / standard math libraries),
        // checked to the approximation's stated 1.5e-7 bound.
        let cases = [
            (0.0, 0.0),
            (0.5, 0.520_499_877_8),
            (1.0, 0.842_700_792_9),
            (1.5, 0.966_105_146_5),
            (2.0, 0.995_322_265_0),
            (3.0, 0.999_977_909_5),
        ];
        for (x, want) in cases {
            assert!(
                (erf(x) - want).abs() <= 1.5e-7,
                "erf({x}) = {} want {want}",
                erf(x)
            );
            // Odd symmetry.
            assert!((erf(-x) + want).abs() <= 1.5e-7);
        }
    }

    #[test]
    fn phi_matches_reference_values() {
        let cases = [
            (0.0, 0.5),
            (1.0, 0.841_344_746_1),
            (2.0, 0.977_249_868_1),
            (3.0, 0.998_650_101_968),
            (-1.0, 0.158_655_253_9),
        ];
        for (z, want) in cases {
            assert!(
                (phi(z) - want).abs() <= 1.5e-7,
                "phi({z}) = {} want {want}",
                phi(z)
            );
        }
    }

    #[test]
    fn phi_pdf_is_exact() {
        assert!((phi_pdf(0.0) - 0.398_942_280_401).abs() < 1e-12);
        assert!((phi_pdf(1.0) - 0.241_970_724_519).abs() < 1e-12);
    }

    #[test]
    fn yield_and_capability_closed_forms() {
        // Centered process filling ±3σ: yield 99.73%, Cp = Cpk = 1.
        let y = yield_within(0.0, 1.0, Some(-3.0), Some(3.0));
        assert!((y - 0.997_300_2).abs() < 1e-6, "{y}");
        assert_eq!(cp(1.0, Some(-3.0), Some(3.0)), Some(1.0));
        assert_eq!(cpk(0.0, 1.0, Some(-3.0), Some(3.0)), Some(1.0));

        // Shifted process: Cpk reflects the nearer limit.
        let k = cpk(1.0, 1.0, Some(-3.0), Some(3.0)).unwrap();
        assert!((k - 2.0 / 3.0).abs() < 1e-12);

        // One-sided: no Cp, Cpk from the single limit.
        assert_eq!(cp(1.0, Some(0.0), None), None);
        assert_eq!(cpk(3.0, 1.0, Some(0.0), None), Some(1.0));

        // Degenerate σ = 0: yield is an indicator, capability is None.
        assert_eq!(yield_within(0.5, 0.0, Some(0.0), Some(1.0)), 1.0);
        assert_eq!(yield_within(2.0, 0.0, Some(0.0), Some(1.0)), 0.0);
        assert_eq!(cpk(0.5, 0.0, Some(0.0), Some(1.0)), None);
    }
}
