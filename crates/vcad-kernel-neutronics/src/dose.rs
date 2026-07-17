//! Flux-to-ambient-dose-equivalent conversion, H*(10).
//!
//! Point coefficients transcribed at design-estimate precision from the
//! ICRP Publication 74 (1996) neutron fluence-to-H*(10) table (the same
//! family as NCRP-38's NCRP values); log-log interpolated between points.
//! Group factors are the 1/E-weighted average of the point curve across
//! each group ([`group_dose_factors_psv_cm2`]) — the weighting is stated
//! because it matters: the 1 keV–100 keV group spans an 8× swing in
//! h(E), and a different intra-group spectrum assumption moves that
//! group's factor by ~2×. That sensitivity is a caveat carried by every
//! dose claim (the fast groups, which dominate shield design, are far
//! less sensitive).

use crate::groups::{GROUP_BOUNDS_EV, N_GROUPS};

/// ICRP-74-style H*(10) per unit fluence, (energy MeV, pSv·cm²).
/// Transcribed at design-estimate precision — see module docs.
pub const H10_POINTS_MEV_PSV_CM2: [(f64, f64); 22] = [
    (1.0e-9, 6.6),
    (1.0e-8, 9.0),
    (2.53e-8, 10.6),
    (1.0e-7, 12.9),
    (2.0e-7, 13.5),
    (5.0e-7, 13.6),
    (1.0e-6, 13.3),
    (1.0e-5, 11.3),
    (1.0e-4, 9.4),
    (1.0e-3, 7.9),
    (1.0e-2, 10.5),
    (2.0e-2, 16.6),
    (5.0e-2, 41.1),
    (1.0e-1, 88.0),
    (2.0e-1, 170.0),
    (5.0e-1, 322.0),
    (1.0, 416.0),
    (1.2, 425.0),
    (2.0, 420.0),
    (3.0, 412.0),
    (5.0, 405.0),
    (10.0, 420.0),
];

/// H*(10) per unit fluence at energy `e_ev`, pSv·cm², log-log
/// interpolated; clamped to the table ends.
pub fn h10_psv_cm2(e_ev: f64) -> f64 {
    let e_mev = e_ev * 1.0e-6;
    let pts = &H10_POINTS_MEV_PSV_CM2;
    if e_mev <= pts[0].0 {
        return pts[0].1;
    }
    if e_mev >= pts[pts.len() - 1].0 {
        return pts[pts.len() - 1].1;
    }
    let i = pts.partition_point(|p| p.0 < e_mev).max(1);
    let (e0, h0) = pts[i - 1];
    let (e1, h1) = pts[i];
    let t = (e_mev / e0).ln() / (e1 / e0).ln();
    h0 * (h1 / h0).powf(t)
}

/// Group dose factors, pSv·cm² per group: 1/E-weighted average of
/// [`h10_psv_cm2`] over each group (64-point log quadrature).
pub fn group_dose_factors_psv_cm2() -> [f64; N_GROUPS] {
    let mut out = [0.0; N_GROUPS];
    const M: usize = 64;
    for (g, o) in out.iter_mut().enumerate() {
        let (hi, lo) = (GROUP_BOUNDS_EV[g], GROUP_BOUNDS_EV[g + 1]);
        let mut sum = 0.0;
        for k in 0..M {
            let e = hi * (lo / hi).powf((k as f64 + 0.5) / M as f64);
            sum += h10_psv_cm2(e);
        }
        *o = sum / M as f64;
    }
    out
}

/// Convert a per-group fluence rate (n/cm²/s) into an ambient dose
/// equivalent rate in µSv/h.
pub fn dose_rate_usv_per_h(flux_per_group_cm2_s: &[f64; N_GROUPS]) -> f64 {
    let h = group_dose_factors_psv_cm2();
    let psv_per_s: f64 = flux_per_group_cm2_s
        .iter()
        .zip(h.iter())
        .map(|(f, h)| f * h)
        .sum();
    psv_per_s * 3600.0 * 1.0e-6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::SOURCE_GROUP;

    #[test]
    fn point_curve_hits_anchor_values() {
        assert!((h10_psv_cm2(0.0253) - 10.6).abs() < 0.2);
        assert!((h10_psv_cm2(2.45e6) - 419.0).abs() < 10.0);
        // Monotone rise through the fast range.
        assert!(h10_psv_cm2(1.0e5) > h10_psv_cm2(1.0e4));
    }

    #[test]
    fn group_factors_ordered_and_sane() {
        let h = group_dose_factors_psv_cm2();
        // Source group ≈ the 2.45 MeV plateau.
        assert!(h[SOURCE_GROUP] > 400.0 && h[SOURCE_GROUP] < 430.0);
        // Fast dose factor dwarfs thermal — the reason moderation alone
        // (without absorption) already buys dose.
        assert!(h[SOURCE_GROUP] > 30.0 * h[N_GROUPS - 1]);
        for f in h {
            assert!(f > 5.0 && f < 500.0);
        }
    }

    #[test]
    fn unshielded_point_source_dose_hand_check() {
        // S = 1e6 n/s at 1 m: φ = S/4πr² = 7.96 n/cm²/s, all in the
        // source group → ≈ 12 µSv/h. The classic bench number.
        let mut flux = [0.0; N_GROUPS];
        flux[SOURCE_GROUP] = 1.0e6 / (4.0 * std::f64::consts::PI * 100.0_f64.powi(2));
        let d = dose_rate_usv_per_h(&flux);
        assert!((d - 12.0).abs() < 1.0, "unshielded 1 m dose = {d} µSv/h");
    }
}
