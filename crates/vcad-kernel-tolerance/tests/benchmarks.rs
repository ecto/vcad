//! M5 benchmarks: exact analytic ground truth.
//!
//! Published worked examples get transcription errors; closed forms
//! don't. These benchmarks pin the engine against mathematics that can
//! be checked by hand:
//!
//! - **Irwin–Hall**: the sum of n standard uniforms has the exact CDF
//!   F_n(x) = (1/n!) Σ_{k≤x} (−1)^k C(n,k)(x−k)^n (Irwin 1927; Hall
//!   1927). A chain of equal-width uniform contributors maps affinely
//!   onto it, giving an EXACT yield to compare all three analyses
//!   against — including the size and sign of the RSS/CLT error.
//! - **The √n law**: for n equal contributors, the worst-case
//!   half-width over the RSS 3σ half-width is exactly √n. This ratio
//!   is the entire economic argument for statistical tolerancing, so
//!   it gets asserted to 1e-12, not just talked about.
//!
//! (External published-fixture cross-validation — e.g. the classic
//! Fortini/Chase–Greenwood one-way clutch — needs the source tables in
//! hand and is flagged in the paper draft, not faked from memory.)

use vcad_kernel_tolerance::analysis::{monte_carlo, rss, worst_case, McOptions};
use vcad_kernel_tolerance::dist::SigmaConvention;
use vcad_kernel_tolerance::stackup::{Contributor, Requirement, Stackup};

/// Exact CDF of the sum of `n` independent U(0,1) (Irwin–Hall).
fn irwin_hall_cdf(x: f64, n: u32) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= n as f64 {
        return 1.0;
    }
    let fact = |m: u32| -> f64 { (1..=m).map(|v| v as f64).product::<f64>().max(1.0) };
    let choose = |n: u32, k: u32| fact(n) / (fact(k) * fact(n - k));
    let mut acc = 0.0;
    for k in 0..=(x.floor() as u32) {
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        acc += sign * choose(n, k) * (x - k as f64).powi(n as i32);
    }
    acc / fact(n)
}

/// A chain of `n` uniform contributors, each deviation U(−w/2, w/2),
/// consuming from an opening dimension so the nominal gap is `gap0`.
fn uniform_chain(n: usize, w: f64, gap0: f64, lower: f64, upper: f64) -> Stackup {
    let mut contributors = vec![Contributor::with_dist(
        "opening",
        1.0,
        gap0,
        0.0,
        0.0,
        vcad_kernel_tolerance::dist::Distribution::Normal {
            mean: 0.0,
            sigma: 0.0,
        },
    )];
    for i in 0..n {
        contributors.push(Contributor::uniform(
            &format!("u{i}"),
            -1.0,
            0.0,
            w / 2.0,
            w / 2.0,
        ));
    }
    Stackup {
        name: format!("irwin-hall-{n}"),
        contributors,
        requirement: Requirement::between("gap", lower, upper),
    }
}

/// Exact yield of `uniform_chain` via Irwin–Hall: the gap is
/// gap0 − Σ devᵢ with devᵢ ~ U(−w/2, w/2), so
/// gap = gap0 + n·w/2 − w·IHₙ and
/// P(L ≤ gap ≤ U) = F((gap0 + n·w/2 − L)/w) − F((gap0 + n·w/2 − U)/w).
fn exact_yield(n: u32, w: f64, gap0: f64, lower: f64, upper: f64) -> f64 {
    let hi = (gap0 + n as f64 * w / 2.0 - lower) / w;
    let lo = (gap0 + n as f64 * w / 2.0 - upper) / w;
    irwin_hall_cdf(hi, n) - irwin_hall_cdf(lo, n)
}

#[test]
fn irwin_hall_cdf_sanity() {
    // n = 1: uniform CDF. n = 2: triangular — F(1) = 1/2 by symmetry,
    // F(0.5) = 0.5²/2 = 0.125. n = 3: F(1.5) = 1/2, and
    // F(1) = 1/6 (the unit simplex volume x³/6 at x = 1).
    assert!((irwin_hall_cdf(0.3, 1) - 0.3).abs() < 1e-12);
    assert!((irwin_hall_cdf(1.0, 2) - 0.5).abs() < 1e-12);
    assert!((irwin_hall_cdf(0.5, 2) - 0.125).abs() < 1e-12);
    assert!((irwin_hall_cdf(1.5, 3) - 0.5).abs() < 1e-12);
    assert!((irwin_hall_cdf(1.0, 3) - 1.0 / 6.0).abs() < 1e-12);
    // Symmetry: F(x) + F(n − x) = 1.
    for n in [2u32, 3, 5] {
        for x in [0.3, 0.9, 1.4] {
            let s = irwin_hall_cdf(x, n) + irwin_hall_cdf(n as f64 - x, n);
            assert!((s - 1.0).abs() < 1e-12, "n={n} x={x}: {s}");
        }
    }
}

#[test]
fn three_uniform_chain_exact_vs_mc_vs_rss() {
    // 3 contributors, w = 0.2 (±0.1), nominal gap 1.0, requirement
    // [0.75, 1.25] — cuts into the Irwin–Hall body where the CLT
    // error is visible.
    let (n, w, gap0, lo, hi) = (3usize, 0.2, 1.0, 0.75, 1.25);
    let s = uniform_chain(n, w, gap0, lo, hi);
    let exact = exact_yield(n as u32, w, gap0, lo, hi);

    // Monte Carlo lands on the exact value within error bars.
    let mc = monte_carlo(
        &s,
        &McOptions {
            n: 400_000,
            seed: 27_1828,
            batches: 16,
        },
    )
    .unwrap();
    assert!(
        (mc.fit.p - exact).abs() <= 4.0 * mc.fit.standard_error,
        "MC {} ± {} vs exact {exact}",
        mc.fit.p,
        mc.fit.standard_error
    );

    // RSS moments are exact for any distribution mix…
    let r = rss(&s).unwrap();
    let sigma_exact = (n as f64 * w * w / 12.0).sqrt();
    assert!((r.sigma_gap - sigma_exact).abs() < 1e-12);
    assert!((r.mean_gap - 1.0).abs() < 1e-12);
    // …but the Φ yield is a CLT approximation: the error must be real
    // (this is a benchmark, not a tautology) and bounded. With n = 3
    // uniforms at a ±2.5σ requirement the CLT error is ~7e-3, and its
    // SIGN is fixed: the bounded uniform sum has no mass beyond ±2.5σ
    // worth of normal tail, so the Φ model UNDERestimates the yield —
    // RSS is conservative in the tails of bounded chains.
    let clt_err = (r.yield_estimate - exact).abs();
    assert!(clt_err < 1.2e-2, "CLT error too large: {clt_err}");
    assert!(clt_err > 2e-4, "CLT error suspiciously small: {clt_err}");
    assert!(
        r.yield_estimate < exact,
        "Φ must under-read a bounded chain here: {} vs {exact}",
        r.yield_estimate
    );
    // Beyond the exact support the uniform sum has NO tail: a
    // requirement at the worst-case bounds has exact yield 1 while the
    // normal model still leaks — RSS is conservative there.
    let wc = worst_case(&s).unwrap();
    let exact_at_wc = exact_yield(n as u32, w, gap0, wc.min_gap, wc.max_gap);
    assert!((exact_at_wc - 1.0).abs() < 1e-12);
    let mut s_wc = s.clone();
    s_wc.requirement = Requirement::between("wc", wc.min_gap, wc.max_gap);
    let r_wc = rss(&s_wc).unwrap();
    assert!(
        r_wc.yield_estimate < 1.0 - 1e-4,
        "normal tails leak: {}",
        r_wc.yield_estimate
    );
}

#[test]
fn two_uniform_triangular_hand_computed() {
    // n = 2, w = 0.2: the gap is triangular on [0.8, 1.2]. Requirement
    // [0.9, 1.3]: P = 1 − P(gap < 0.9) − P(gap > 1.3) = 1 − 0.125 − 0
    // = 0.875 by the triangle-corner area, hand-computed.
    let s = uniform_chain(2, 0.2, 1.0, 0.9, 1.3);
    let exact = exact_yield(2, 0.2, 1.0, 0.9, 1.3);
    assert!((exact - 0.875).abs() < 1e-12, "hand computation: {exact}");
    let mc = monte_carlo(
        &s,
        &McOptions {
            n: 200_000,
            seed: 31_4159,
            batches: 16,
        },
    )
    .unwrap();
    assert!((mc.fit.p - 0.875).abs() <= 4.0 * mc.fit.standard_error);
}

#[test]
fn sqrt_n_law_exact() {
    // n equal normal contributors ±t at 3σ: WC half-width = n·t,
    // RSS 3σ half-width = 3·√n·(t/3) = √n·t. Ratio = √n exactly.
    let t = 0.1;
    for n in [4usize, 9, 16, 25] {
        let contributors: Vec<Contributor> = (0..n)
            .map(|i| {
                Contributor::normal(&format!("c{i}"), -1.0, 1.0, t, SigmaConvention::ThreeSigma)
            })
            .chain(std::iter::once(Contributor::with_dist(
                "opening",
                1.0,
                n as f64 + 1.0,
                0.0,
                0.0,
                vcad_kernel_tolerance::dist::Distribution::Normal {
                    mean: 0.0,
                    sigma: 0.0,
                },
            )))
            .collect();
        let s = Stackup {
            name: format!("sqrt-{n}"),
            contributors,
            requirement: Requirement::between("gap", 0.5, 1.5),
        };
        let wc = worst_case(&s).unwrap();
        let r = rss(&s).unwrap();
        let wc_half = 0.5 * (wc.max_gap - wc.min_gap);
        let rss_half = 3.0 * r.sigma_gap;
        assert!((wc_half - n as f64 * t).abs() < 1e-12);
        assert!(
            (wc_half / rss_half - (n as f64).sqrt()).abs() < 1e-12,
            "n={n}: ratio {} vs √n {}",
            wc_half / rss_half,
            (n as f64).sqrt()
        );
    }
}
