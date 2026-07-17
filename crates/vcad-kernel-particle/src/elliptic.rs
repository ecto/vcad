//! Complete elliptic integrals K(m) and E(m) via the arithmetic–geometric
//! mean, in the parameter convention `m = k²`:
//!
//! K(m) = ∫₀^{π/2} dθ / √(1 − m sin²θ),  E(m) = ∫₀^{π/2} √(1 − m sin²θ) dθ.
//!
//! Needed for the exact off-axis magnetic field of a circular current loop.

/// Complete elliptic integrals `(K(m), E(m))` for `m ∈ [0, 1)`.
///
/// Accuracy is limited only by the AGM iteration (converges quadratically;
/// ~5 iterations to machine precision). `m` is clamped just below 1 to keep
/// K finite for callers that graze the singular point.
pub fn ellip_ke(m: f64) -> (f64, f64) {
    let m = m.clamp(0.0, 1.0 - 1e-15);
    let mut a = 1.0_f64;
    let mut b = (1.0 - m).sqrt();
    let mut c = m.sqrt();
    let mut sum = 0.5 * c * c; // 2^{-1} c₀²
    let mut pow = 0.5;
    for _ in 0..60 {
        if c.abs() < 1e-17 {
            break;
        }
        let an = 0.5 * (a + b);
        let bn = (a * b).sqrt();
        c = 0.5 * (a - b);
        a = an;
        b = bn;
        pow *= 2.0;
        sum += pow * c * c;
    }
    let k = std::f64::consts::FRAC_PI_2 / a;
    let e = k * (1.0 - sum);
    (k, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_values() {
        let (k0, e0) = ellip_ke(0.0);
        assert!((k0 - std::f64::consts::FRAC_PI_2).abs() < 1e-14);
        assert!((e0 - std::f64::consts::FRAC_PI_2).abs() < 1e-14);

        // Abramowitz & Stegun: K(m=0.5) = 1.854074677..., E(m=0.5) = 1.350643881...
        let (k, e) = ellip_ke(0.5);
        assert!((k - 1.854_074_677_301_372).abs() < 1e-12, "K(0.5) = {k}");
        assert!((e - 1.350_643_881_047_675).abs() < 1e-12, "E(0.5) = {e}");
    }

    #[test]
    fn near_singular_is_finite_and_ordered() {
        let (k, e) = ellip_ke(0.999_999);
        assert!(k.is_finite() && e.is_finite());
        assert!(k > e, "K must exceed E for m > 0");
        assert!(e >= 1.0, "E(m→1) → 1");
    }
}
