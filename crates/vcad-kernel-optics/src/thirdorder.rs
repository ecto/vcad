//! Third-order (Seidel) spherical aberration of a thin lens — the
//! validation reference for the exact tracer, not a design tool.
//!
//! For a thin lens in air with shape factor q = (c₁ + c₂)/(c₁ − c₂) and
//! position factor p (= −1 for an object at infinity), the marginal ray
//! at height h focuses short of the paraxial focus by (Jenkins & White,
//! *Fundamentals of Optics*, 4th ed., §9.5; also Hecht §6.3.1):
//!
//! ```text
//! 1/s′_m − 1/s′_p = h²/(8f³) · 1/(n(n−1)) ·
//!     [ (n+2)/(n−1)·q² + 4(n+1)·p·q + (3n+2)(n−1)·p² + n³/(n−1) ]
//! ```
//!
//! The bracket is a parabola in q — the classic **U-curve** — with
//! minimum at q = −2(n²−1)p/(n+2) (≈ +0.714 for n = 1.5, object at
//! infinity: the "best-form" lens, more strongly curved side toward the
//! object). The sign conventions here were pinned against the exact
//! tracer (`tests/analytic.rs`), which is the arbiter.

/// Δ(1/s′) = 1/s′_marginal − 1/s′_paraxial for a thin lens in air
/// (1/mm). `h` is the marginal ray height at the lens (mm), `f` the
/// focal length (mm), `q` the shape factor, `p` the position factor.
pub fn thin_lens_delta_inv_s(n: f64, f: f64, q: f64, p: f64, h: f64) -> f64 {
    let bracket = (n + 2.0) / (n - 1.0) * q * q
        + 4.0 * (n + 1.0) * p * q
        + (3.0 * n + 2.0) * (n - 1.0) * p * p
        + n.powi(3) / (n - 1.0);
    h * h / (8.0 * f.powi(3)) * bracket / (n * (n - 1.0))
}

/// Longitudinal spherical aberration s′_p − s′_m (mm, positive =
/// undercorrected) for an object at infinity, computed exactly from the
/// Δ(1/s′) form (no small-aberration linearization).
pub fn thin_lens_lsa_infinity(n: f64, f: f64, q: f64, h: f64) -> f64 {
    let delta = thin_lens_delta_inv_s(n, f, q, -1.0, h);
    f - 1.0 / (1.0 / f + delta)
}

/// The best-form shape factor q minimizing third-order spherical
/// aberration: q = −2(n²−1)p/(n+2).
pub fn best_form_q(n: f64, p: f64) -> f64 {
    -2.0 * (n * n - 1.0) * p / (n + 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_form_matches_textbook_value() {
        // n = 1.5, object at infinity: q = 2(n²−1)/(n+2) = 5/7 ≈ 0.714.
        let q = best_form_q(1.5, -1.0);
        assert!((q - 5.0 / 7.0).abs() < 1e-12, "{q}");
    }

    #[test]
    fn ucurve_is_a_parabola_with_positive_curvature() {
        // Second difference in q is constant and positive: marginal focus
        // always shorter than paraxial (undercorrected) for a positive
        // thin lens at any bending.
        let f = |q: f64| thin_lens_delta_inv_s(1.5, 100.0, q, -1.0, 5.0);
        let d2 = f(1.0) - 2.0 * f(0.0) + f(-1.0);
        assert!(d2 > 0.0);
        for q in [-2.0, -1.0, 0.0, 0.714, 2.0] {
            assert!(f(q) > 0.0, "q = {q}");
        }
    }

    #[test]
    fn equiconvex_vs_best_form_ratio() {
        // Textbook check: at n = 1.5, object at ∞, the equiconvex lens
        // (q = 0) has ≈1.56× the SA of the best-form lens.
        let best = thin_lens_lsa_infinity(1.5, 100.0, best_form_q(1.5, -1.0), 5.0);
        let equi = thin_lens_lsa_infinity(1.5, 100.0, 0.0, 5.0);
        let ratio = equi / best;
        assert!((ratio - 1.556).abs() < 0.01, "ratio = {ratio}");
    }

    #[test]
    fn lsa_scales_as_h_squared() {
        let a = thin_lens_lsa_infinity(1.5, 100.0, 0.7, 2.0);
        let b = thin_lens_lsa_infinity(1.5, 100.0, 0.7, 4.0);
        assert!((b / a - 4.0).abs() < 0.02, "ratio = {}", b / a);
    }
}
