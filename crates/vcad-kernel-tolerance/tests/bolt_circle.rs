//! The classic non-collinear worked example: a bolt-circle pin fit.
//!
//! Four pins pressed into a base must enter four holes in a plate.
//! Hole positions carry a Ø0.15 position tolerance (RFS at M0; the MMC
//! bonus is milestone M1), hole and pin diameters carry their own
//! bands. A pin enters iff its radial position error ≤ the radial
//! clearance c = (D_hole − d_pin)/2; the plate assembles iff **all**
//! pins enter.
//!
//! This is the everyday case that is NOT a linear chain — the fit
//! condition is on the **magnitude** of a 2-D error — and the test
//! quantifies exactly what the linearized treatment misses:
//!
//! - Exact model: with per-axis position error x, y ~ N(0, σ²), the
//!   radial error is Rayleigh-distributed; P(fit | c) = 1 − e^(−c²/2σ²)
//!   (Rayleigh CDF). Monte Carlo over the true model must land on the
//!   semi-analytic value (Rayleigh CDF integrated over the diameter
//!   distributions) within error bars.
//! - Linearized model: projecting the position error onto one direction
//!   gives P(|e_d| ≤ c) = 2Φ(c/σ) − 1, which is strictly LARGER than
//!   the Rayleigh probability at the same c — a single projection is
//!   optimistic about a radial fit, so the crate's vector-loop module
//!   refuses to be used for it silently (see `loops` module docs).
//! - Worst case: the ASME Y14.5-style virtual-condition check — fit is
//!   guaranteed iff hole MMC − pin MMC ≥ position zone Ø. Here it
//!   fails while the statistical fit rate is ~90%: the entire reason
//!   MMC/statistical analysis exists.
//!
//! Position-tolerance σ convention (stated, as always): the Ø0.15 zone
//! means radial error ≤ 0.075; we model each axis as N(0, σ) with
//! σ = 0.075/3 — the process hits the zone radius at 3σ per axis.

use vcad_kernel_tolerance::capability::phi;
use vcad_kernel_tolerance::rng::Rng;

const N_PINS: usize = 4;
const ZONE_RADIUS: f64 = 0.075; // Ø0.15 position tolerance
const SIGMA_POS: f64 = ZONE_RADIUS / 3.0;
// Hole Ø 6.10 +0.05/0 (uniform), pin Ø 6.00 0/−0.02 (uniform).
const HOLE_LO: f64 = 6.10;
const HOLE_HI: f64 = 6.15;
const PIN_LO: f64 = 5.98;
const PIN_HI: f64 = 6.00;

/// P(one pin fits) integrated over the clearance distribution:
/// E_c[1 − e^(−c²/2σ²)] with c = (D_hole − d_pin)/2, both uniform.
/// Deterministic 2-D midpoint quadrature — the reference value.
fn per_pin_fit_probability_analytic() -> f64 {
    let m = 400;
    let mut acc = 0.0;
    for i in 0..m {
        let hole = HOLE_LO + (HOLE_HI - HOLE_LO) * (i as f64 + 0.5) / m as f64;
        for j in 0..m {
            let pin = PIN_LO + (PIN_HI - PIN_LO) * (j as f64 + 0.5) / m as f64;
            let c = 0.5 * (hole - pin);
            acc += 1.0 - (-c * c / (2.0 * SIGMA_POS * SIGMA_POS)).exp();
        }
    }
    acc / (m * m) as f64
}

#[test]
fn exact_monte_carlo_matches_rayleigh_closed_form() {
    let p_pin = per_pin_fit_probability_analytic();
    let p_all_analytic = p_pin.powi(N_PINS as i32);

    // Monte Carlo over the true 2-D model.
    let mut rng = Rng::new(0xB0117);
    let n = 200_000;
    let mut fits = 0usize;
    for _ in 0..n {
        let mut all = true;
        for _ in 0..N_PINS {
            let hole = HOLE_LO + (HOLE_HI - HOLE_LO) * rng.next_f64();
            let pin = PIN_LO + (PIN_HI - PIN_LO) * rng.next_f64();
            let c = 0.5 * (hole - pin);
            let x = SIGMA_POS * rng.next_normal();
            let y = SIGMA_POS * rng.next_normal();
            if x * x + y * y > c * c {
                all = false;
                break;
            }
        }
        if all {
            fits += 1;
        }
    }
    let p_mc = fits as f64 / n as f64;
    let se = (p_mc * (1.0 - p_mc) / n as f64).sqrt();
    assert!(
        (p_mc - p_all_analytic).abs() < 4.0 * se,
        "MC {p_mc} ± {se} vs Rayleigh-integral {p_all_analytic}"
    );
    // And the regime is the interesting one: real losses, far from 0/1.
    assert!(
        p_all_analytic > 0.85 && p_all_analytic < 0.97,
        "fixture drifted out of the interesting regime: {p_all_analytic}"
    );
}

#[test]
fn linearized_projection_is_optimistic_for_radial_fits() {
    // At any clearance c > 0: P(|e_d| ≤ c) = 2Φ(c/σ)−1 (1-D projection)
    // vs P(√(x²+y²) ≤ c) = 1 − e^(−c²/2σ²) (Rayleigh). The projection
    // must always be larger — quantify at the fixture's mean clearance.
    let c = 0.5 * ((HOLE_LO + HOLE_HI) / 2.0 - (PIN_LO + PIN_HI) / 2.0);
    let p_1d = 2.0 * phi(c / SIGMA_POS) - 1.0;
    let p_2d = 1.0 - (-c * c / (2.0 * SIGMA_POS * SIGMA_POS)).exp();
    // At this fixture's c/σ = 2.7 the gap is ~1.9 percentage points —
    // material for a fit-probability claim.
    assert!(
        p_1d > p_2d + 0.015,
        "projection {p_1d} should exceed Rayleigh {p_2d} materially"
    );
    // Sweep: the inequality is uniform in c (sanity that it's not a
    // fixture accident).
    for i in 1..=20 {
        let c = 0.01 * i as f64;
        let p_1d = 2.0 * phi(c / SIGMA_POS) - 1.0;
        let p_2d = 1.0 - (-c * c / (2.0 * SIGMA_POS * SIGMA_POS)).exp();
        assert!(p_1d >= p_2d, "c = {c}: {p_1d} < {p_2d}");
    }
}

#[test]
fn virtual_condition_worst_case_fails_while_statistics_ship() {
    // Floating-fastener-style virtual condition: guaranteed fit iff
    // (hole MMC − pin MMC) ≥ position zone diameter.
    let hole_mmc = HOLE_LO; // smallest hole
    let pin_mmc = PIN_HI; // largest pin
    let diametral_clearance_mmc = hole_mmc - pin_mmc; // 0.10
    let zone_dia = 2.0 * ZONE_RADIUS; // 0.15
    assert!(
        diametral_clearance_mmc < zone_dia,
        "fixture: WC must fail ({diametral_clearance_mmc} < {zone_dia})"
    );
    // …and yet the statistical fit rate is high — computed above.
    let p_all = per_pin_fit_probability_analytic().powi(N_PINS as i32);
    assert!(p_all > 0.85, "statistical fit rate {p_all}");
}
