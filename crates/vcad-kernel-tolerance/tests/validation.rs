//! The M0 validation ladder (see `docs/tolerance-m0.md`).
//!
//! 1. RSS agrees with Monte Carlo within MC error bars on a
//!    5-contributor chain (self-consistency of the two paths).
//! 2. Worst-case bounds contain every MC sample (fixed seeds —
//!    deterministic, never flaky).
//! 3. A textbook-style chain with hand-computed answers asserted
//!    exactly.
//! 4. Φ/erf-based yield against known normal-table values.
//! 5. Monte Carlo error scales as 1/√N (empirically, across disjoint
//!    seeds).
//!
//! Every test uses fixed seeds and wide, stated acceptance bands: a
//! failure here is a real regression, not sampling noise.

use vcad_kernel_tolerance::analysis::{monte_carlo, rss, worst_case, McOptions};
use vcad_kernel_tolerance::capability::yield_within;
use vcad_kernel_tolerance::dist::{Distribution, SigmaConvention};
use vcad_kernel_tolerance::stackup::{Contributor, Requirement, Stackup};

/// A 5-contributor all-normal chain: RSS yield is exact under the
/// model, so MC must agree within its own error bars.
fn five_normal_chain() -> Stackup {
    let conv = SigmaConvention::ThreeSigma;
    Stackup {
        name: "five-normal".into(),
        contributors: vec![
            Contributor::normal("a", 1.0, 40.0, 0.30, conv),
            Contributor::normal("b", -1.0, 12.0, 0.20, conv),
            Contributor::normal("c", -1.0, 10.0, 0.15, conv),
            Contributor::normal("d", -1.0, 8.0, 0.15, conv),
            Contributor::normal("e", -1.0, 9.5, 0.10, conv),
        ],
        requirement: Requirement::between("gap", 0.20, 0.85),
    }
}

/// Mixed-distribution chain (normal + uniform + two-point), the shape
/// real BOMs have.
fn mixed_chain() -> Stackup {
    Stackup {
        name: "mixed".into(),
        contributors: vec![
            Contributor::normal("plate", 1.0, 25.0, 0.2, SigmaConvention::ThreeSigma),
            Contributor::uniform("bushing", -1.0, 10.0, 0.1, 0.0),
            Contributor::with_dist(
                "washer",
                -1.0,
                2.0,
                0.05,
                0.05,
                Distribution::TwoPoint {
                    a: -0.02,
                    b: 0.03,
                    p_b: 0.3,
                },
            ),
            Contributor::uniform("ring", -1.0, 12.4, 0.15, 0.15),
        ],
        requirement: Requirement::between("gap", 0.30, 1.05),
    }
}

#[test]
fn ladder_1_rss_agrees_with_mc_within_error_bars() {
    let s = five_normal_chain();
    let r = rss(&s).unwrap();
    assert!(r.all_normal, "this rung requires the exact-normal case");
    let mc = monte_carlo(
        &s,
        &McOptions {
            n: 200_000,
            seed: 1001,
            batches: 16,
        },
    )
    .unwrap();
    // Yield: |p_mc − p_rss| within 4 standard errors (fixed seed, so
    // this is a deterministic regression bound, not a flaky assertion;
    // the erf approximation contributes ≤ 1.5e-7 of the gap).
    let diff = (mc.fit.p - r.yield_estimate).abs();
    assert!(
        diff <= 4.0 * mc.fit.standard_error,
        "RSS yield {} vs MC {} ± {} (diff {diff})",
        r.yield_estimate,
        mc.fit.p,
        mc.fit.standard_error
    );
    // Moments agree within their own error bars.
    assert!((mc.mean_gap - r.mean_gap).abs() <= 4.0 * mc.mean_gap_se);
    assert!((mc.sigma_gap - r.sigma_gap).abs() <= 4.0 * mc.sigma_gap_se);

    // The mixed chain leans on the CLT for the RSS yield: hold it to a
    // wider, stated band (absolute 0.005 at these variance ratios).
    let s = mixed_chain();
    let r = rss(&s).unwrap();
    assert!(!r.all_normal);
    let mc = monte_carlo(
        &s,
        &McOptions {
            n: 200_000,
            seed: 1002,
            batches: 16,
        },
    )
    .unwrap();
    assert!(
        (mc.fit.p - r.yield_estimate).abs() < 5e-3,
        "CLT band: RSS {} vs MC {}",
        r.yield_estimate,
        mc.fit.p
    );
}

#[test]
fn ladder_2_worst_case_bounds_contain_every_mc_sample() {
    for (s, seed) in [(five_normal_chain(), 7u64), (mixed_chain(), 8u64)] {
        let wc = worst_case(&s).unwrap();
        let mc = monte_carlo(
            &s,
            &McOptions {
                n: 200_000,
                seed,
                batches: 16,
            },
        )
        .unwrap();
        // For bounded distributions this is a theorem; for normal
        // contributors the WC interval sits ~6–7 σ_G out, so a fixed
        // seed makes the assertion deterministic and effectively
        // certain (P(violation) ~ 1e-11 per sample).
        assert!(
            mc.min_sample >= wc.min_gap && mc.max_sample <= wc.max_gap,
            "{}: samples [{}, {}] escaped WC [{}, {}]",
            s.name,
            mc.min_sample,
            mc.max_sample,
            wc.min_gap,
            wc.max_gap
        );
    }
}

#[test]
fn ladder_3_textbook_chain_hand_computed() {
    // Shaft in a bushing pocket: pocket depth 20.00 ±0.15, bushing
    // 12.00 ±0.10, shim 7.50 ±0.05, all normal at ±tol = 3σ.
    // Requirement: protrusion gap ∈ [0.20, 0.80].
    //
    // Hand computation:
    //   nominal gap  = 20 − 12 − 7.5              = 0.5
    //   σ            = √((0.05)² + (0.0333…)² + (0.0166…)²)
    //                = √(0.0025 + 0.001111… + 0.000277…)
    //                = √0.0038888… = 0.06236095644623235…
    //   WC           = [0.5 − 0.30, 0.5 + 0.30] = [0.20, 0.80]  (exact)
    //   Cp = Cpk     = 0.30/(3σ) = 1.603567451474546…
    //   z            = 0.30/σ = 4.810702354423638
    //   yield        = Φ(z) − Φ(−z) = 1 − 2Φ(−4.8107…) ≈ 0.99999849…
    let conv = SigmaConvention::ThreeSigma;
    let s = Stackup {
        name: "textbook".into(),
        contributors: vec![
            Contributor::normal("pocket", 1.0, 20.0, 0.15, conv),
            Contributor::normal("bushing", -1.0, 12.0, 0.10, conv),
            Contributor::normal("shim", -1.0, 7.5, 0.05, conv),
        ],
        requirement: Requirement::between("protrusion", 0.20, 0.80),
    };
    let sigma = (0.0025f64 + 0.1f64 * 0.1 / 9.0 + 0.05f64 * 0.05 / 9.0).sqrt();

    let wc = worst_case(&s).unwrap();
    assert!((wc.min_gap - 0.20).abs() < 1e-12);
    assert!((wc.max_gap - 0.80).abs() < 1e-12);
    assert!(wc.passes, "WC lands exactly on the limits");

    let r = rss(&s).unwrap();
    assert!((r.mean_gap - 0.5).abs() < 1e-12);
    assert!((r.sigma_gap - sigma).abs() < 1e-15);
    assert!((r.cp.unwrap() - 0.6 / (6.0 * sigma)).abs() < 1e-12);
    assert!((r.cpk.unwrap() - 0.3 / (3.0 * sigma)).abs() < 1e-12);
    assert!((r.cp.unwrap() - r.cpk.unwrap()).abs() < 1e-12, "centered");
    // Yield to the erf approximation's honesty band.
    let z = 0.3 / sigma;
    let want = yield_within(0.0, 1.0, Some(-z), Some(z));
    assert!((r.yield_estimate - want).abs() < 1e-12);
    assert!(r.yield_estimate > 0.999_997 && r.yield_estimate < 0.999_999_5);
}

#[test]
fn ladder_4_yield_matches_normal_table() {
    // Symmetric two-sided intervals at textbook z values.
    let cases = [
        (1.0, 0.682_689_49),
        (1.959_964, 0.95),
        (2.0, 0.954_499_74),
        (2.575_829, 0.99),
        (3.0, 0.997_300_20),
    ];
    for (z, want) in cases {
        let y = yield_within(0.0, 1.0, Some(-z), Some(z));
        assert!(
            (y - want).abs() < 1e-6,
            "±{z}σ: yield {y} want {want} (erf approx bound 1.5e-7 per tail)"
        );
    }
    // One-sided: Φ(3) tail.
    let y = yield_within(0.0, 1.0, Some(-3.0), None);
    assert!((y - 0.998_650_10).abs() < 1e-6);
}

#[test]
fn ladder_5_mc_error_scales_as_inverse_sqrt_n() {
    // Empirical 1/√N: run K disjoint seeds at N and 4N; the spread of
    // p̂ across seeds must halve (within a stated band), and the
    // reported per-run SE must match the empirical spread.
    let s = five_normal_chain();
    let k = 24;
    let spread = |n: usize, seed0: u64| -> (f64, f64) {
        let ps: Vec<f64> = (0..k)
            .map(|i| {
                monte_carlo(
                    &s,
                    &McOptions {
                        n,
                        seed: seed0 + i as u64,
                        batches: 8,
                    },
                )
                .unwrap()
            })
            .map(|m| m.fit.p)
            .collect();
        let mean = ps.iter().sum::<f64>() / k as f64;
        let var = ps.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / (k as f64 - 1.0);
        // Also return one run's reported SE for the match check.
        let reported = monte_carlo(
            &s,
            &McOptions {
                n,
                seed: seed0,
                batches: 8,
            },
        )
        .unwrap()
        .fit
        .standard_error;
        (var.sqrt(), reported)
    };
    let (sd_small, se_small) = spread(4_000, 100);
    let (sd_big, se_big) = spread(16_000, 200);
    let ratio = sd_small / sd_big;
    assert!(
        (1.4..=2.9).contains(&ratio),
        "4× samples should ≈ halve the spread: {sd_small} → {sd_big} (ratio {ratio})"
    );
    // Reported SE tracks the empirical spread within a factor of 1.8
    // (24 seeds estimate the spread itself to ~±15%).
    for (sd, se) in [(sd_small, se_small), (sd_big, se_big)] {
        let f = sd / se;
        assert!(
            (1.0 / 1.8..=1.8).contains(&f),
            "reported SE {se} vs empirical sd {sd} (factor {f})"
        );
    }
}
