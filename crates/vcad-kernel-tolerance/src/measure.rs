//! The measurement pack (M6): measured dimensional scatter becomes the
//! distribution input, and predicted-vs-measured yield closes the loop.
//!
//! Distributions are assumptions until measured. This module is the
//! flip: fit a normal to caliper/CMM samples of a real dimension
//! (Bessel-corrected, with standard errors on both fitted parameters),
//! swap it into the contributor, and mark the provenance
//! [`DistributionSource::Measured`]. The drawing limits stay put — the
//! drawing didn't change, reality did — so worst-case analysis is
//! untouched while RSS/MC re-price on facts.
//!
//! ## Binding to the repo's 3DP print-then-measure loop
//!
//! vcad's `predict_print` tool declares per-part measurables (bounding
//! box dims per print axis, hole diameters, mass) and
//! `record_measurement` binds one printed part's caliper values to
//! them. Print a **batch** of coupons and each measurable id yields a
//! sample vector across coupons; the adapter contract is:
//!
//! 1. one stackup contributor ↔ one measurable id (same physical
//!    dimension, same axis),
//! 2. collect that measurable's value from every coupon's
//!    `record_measurement` call into `samples` (absolute mm, as
//!    calipers read),
//! 3. [`apply_measurement`] fits the deviation distribution against
//!    the contributor's nominal and flips its source to `Measured`
//!    with the coupon count and instrument recorded.
//!
//! Assembly-level truth binds the other end of the loop: an assembly
//! trial (k of n built units fit) becomes a `fit_probability`
//! [`Measurement`] via [`Measurement::from_trial`], and
//! [`crate::receipt::compare`] issues Holds/Violated/Unmeasured — the
//! test suite demonstrates the full circle: an optimistic assumed
//! model is **Violated** by the trial while the coupon-measured model
//! **Holds** on the same data.

use crate::analysis::ProbabilityEstimate;
use crate::dist::{Distribution, DistributionSource};
use crate::receipt::Measurement;
use crate::stackup::{Stackup, StackupError};

/// Minimum samples to fit a distribution. Five is a floor, not a
/// recommendation — at n = 5 the σ standard error is ~35% of σ.
pub const MIN_FIT_SAMPLES: usize = 5;

/// A normal fitted to measured samples, with standard errors.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FittedNormal {
    /// Sample mean, mm.
    pub mean: f64,
    /// Bessel-corrected sample standard deviation, mm.
    pub sigma: f64,
    /// Standard error of the mean: s/√n, mm.
    pub se_mean: f64,
    /// Standard error of σ (normal theory, large-sample):
    /// s/√(2(n−1)), mm.
    pub se_sigma: f64,
    /// Sample count.
    pub n: usize,
}

/// Fit a normal to samples, fail-closed on tiny or degenerate input.
pub fn fit_normal(samples: &[f64]) -> Result<FittedNormal, StackupError> {
    if samples.len() < MIN_FIT_SAMPLES {
        return Err(StackupError::TooFewSamples {
            n: samples.len(),
            min: MIN_FIT_SAMPLES,
        });
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(StackupError::BadRequirement(
            "non-finite measurement sample".into(),
        ));
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let ss: f64 = samples.iter().map(|s| (s - mean) * (s - mean)).sum();
    let sigma = (ss / (n - 1.0)).sqrt();
    Ok(FittedNormal {
        mean,
        sigma,
        se_mean: sigma / n.sqrt(),
        se_sigma: sigma / (2.0 * (n - 1.0)).sqrt(),
        n: samples.len(),
    })
}

/// Fit the named contributor's deviation distribution from measured
/// **absolute** dimensions (mm, as calipers read them), flip its
/// source to [`DistributionSource::Measured`], and return the fit.
/// Drawing limits are untouched. Fails closed if the fitted mean sits
/// outside the drawing band — measurements saying the process center
/// is out of spec are a finding, not an input to silently accept.
pub fn apply_measurement(
    s: &mut Stackup,
    contributor: &str,
    samples: &[f64],
    instrument: &str,
) -> Result<FittedNormal, StackupError> {
    s.validate()?;
    let c = s
        .contributors
        .iter_mut()
        .find(|c| c.name == contributor)
        .ok_or_else(|| StackupError::NotAllocatable(format!("no contributor {contributor:?}")))?;
    let fit = fit_normal(samples)?;
    let dev_mean = fit.mean - c.nominal;
    if dev_mean < -c.tol_minus || dev_mean > c.tol_plus {
        return Err(StackupError::BadRequirement(format!(
            "measured mean of {contributor:?} deviates {dev_mean:.4} mm from nominal, \
             outside the drawing band [-{}, +{}] — fix the process or the drawing \
             before re-pricing yield",
            c.tol_minus, c.tol_plus
        )));
    }
    c.dist = Distribution::Normal {
        mean: dev_mean,
        sigma: fit.sigma,
    };
    c.source = DistributionSource::Measured {
        n_samples: fit.n,
        instrument: instrument.to_string(),
    };
    Ok(fit)
}

impl Measurement {
    /// A `fit_probability` measurement from an assembly trial: `fits`
    /// of `total` built units met the requirement. The uncertainty is
    /// the Agresti–Coull standard error — an error-bar-free trial
    /// result is as unrepresentable as an error-bar-free prediction.
    pub fn from_trial(fits: usize, total: usize, band_abs: f64, instrument: &str) -> Self {
        let est = ProbabilityEstimate::from_counts(fits, total);
        Measurement {
            name: "fit_probability".into(),
            value: est.p,
            uncertainty: est.standard_error,
            instrument: instrument.to_string(),
            band_abs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{monte_carlo, rss, worst_case, McOptions};
    use crate::dist::SigmaConvention;
    use crate::receipt::{compare, predicted_claims, Verdict};
    use crate::rng::Rng;
    use crate::stackup::{Contributor, Requirement};

    #[test]
    fn fit_normal_recovers_known_parameters() {
        // Samples from Normal(10.05, 0.12), seeded.
        let mut rng = Rng::new(606);
        let n = 400;
        let samples: Vec<f64> = (0..n).map(|_| 10.05 + 0.12 * rng.next_normal()).collect();
        let fit = fit_normal(&samples).unwrap();
        assert!((fit.mean - 10.05).abs() < 4.0 * fit.se_mean, "{fit:?}");
        assert!((fit.sigma - 0.12).abs() < 4.0 * fit.se_sigma, "{fit:?}");
        // SE scaling: 4× samples → half the SE (within tolerance).
        let fit_small = fit_normal(&samples[..100]).unwrap();
        let ratio = fit_small.se_mean / fit.se_mean;
        assert!((1.4..2.9).contains(&ratio), "SE scaling ratio {ratio}");
    }

    #[test]
    fn fit_normal_fails_closed() {
        assert!(matches!(
            fit_normal(&[1.0, 2.0]),
            Err(StackupError::TooFewSamples { .. })
        ));
        assert!(fit_normal(&[1.0, 2.0, f64::NAN, 3.0, 4.0]).is_err());
        // Identical samples: σ = 0 is legal (a very repeatable
        // process), SEs are 0 — honest degenerate.
        let fit = fit_normal(&[5.0; 10]).unwrap();
        assert_eq!(fit.sigma, 0.0);
    }

    fn chain() -> Stackup {
        let conv = SigmaConvention::ThreeSigma;
        Stackup {
            name: "coupon-chain".into(),
            contributors: vec![
                Contributor::normal("printed boss", 1.0, 20.0, 0.3, conv),
                Contributor::normal("mating plate", -1.0, 19.5, 0.15, conv),
            ],
            requirement: Requirement::between("gap", 0.15, 0.85),
        }
    }

    #[test]
    fn apply_measurement_flips_provenance_and_repricing() {
        let mut s = chain();
        // 30 printed coupons, true process shifted +0.08 and wider
        // (σ 0.15) than the ±0.3 = 3σ assumption (σ 0.1).
        let mut rng = Rng::new(707);
        let samples: Vec<f64> = (0..30).map(|_| 20.08 + 0.15 * rng.next_normal()).collect();
        let y_assumed = rss(&s).unwrap().yield_estimate;
        let fit = apply_measurement(&mut s, "printed boss", &samples, "calipers, coupon batch A")
            .unwrap();
        assert_eq!(fit.n, 30);
        // Provenance flipped; drawing limits untouched.
        let c = &s.contributors[0];
        assert!(matches!(
            c.source,
            DistributionSource::Measured { n_samples: 30, .. }
        ));
        assert_eq!(c.tol_minus, 0.3);
        assert_eq!(c.tol_plus, 0.3);
        // The measured model re-prices the yield downward (worse
        // process than assumed).
        let y_measured = rss(&s).unwrap().yield_estimate;
        assert!(
            y_measured < y_assumed - 0.005,
            "measured {y_measured} vs assumed {y_assumed}"
        );
        s.validate().unwrap();
    }

    #[test]
    fn out_of_band_process_center_is_a_finding_not_an_input() {
        let mut s = chain();
        let samples: Vec<f64> = (0..20).map(|i| 20.45 + 0.001 * i as f64).collect();
        let err = apply_measurement(&mut s, "printed boss", &samples, "calipers").unwrap_err();
        assert!(matches!(err, StackupError::BadRequirement(_)), "{err:?}");
        // Untouched on failure.
        assert!(matches!(
            s.contributors[0].source,
            DistributionSource::Assumed { .. }
        ));
    }

    /// The M6 money shot: the full predicted → printed → measured →
    /// verdict circle. An optimistic assumed model is Violated by the
    /// assembly trial; the coupon-measured model Holds on the same
    /// trial data.
    #[test]
    fn closed_loop_assumed_violated_measured_holds() {
        let assumed = chain();
        // The TRUE process for the printed boss: +0.10 shift, σ 0.15.
        let true_boss = Distribution::Normal {
            mean: 0.10,
            sigma: 0.15,
        };

        // Predicted claims from the assumed model.
        let wc = worst_case(&assumed).unwrap();
        let r = rss(&assumed).unwrap();
        let mc = monte_carlo(
            &assumed,
            &McOptions {
                n: 100_000,
                seed: 42_000,
                batches: 16,
            },
        )
        .unwrap();
        let claims_assumed = predicted_claims(&assumed, &wc, &r, &mc).unwrap();

        // Reality: build 400 assemblies from the true process and
        // count fits (seeded, deterministic).
        let mut rng = Rng::new(42_001);
        let mut fits = 0usize;
        let total = 400;
        for _ in 0..total {
            let boss = 20.0 + true_boss.sample(&mut rng);
            let plate = 19.5 + assumed.contributors[1].dist.sample(&mut rng);
            let gap = boss - plate;
            if (0.15..=0.85).contains(&gap) {
                fits += 1;
            }
        }
        let trial = Measurement::from_trial(fits, total, 0.01, "assembly trial, batch A");

        // The assumed receipt is VIOLATED by reality.
        let report = compare(&claims_assumed, std::slice::from_ref(&trial)).unwrap();
        let verdict = |rep: &crate::receipt::ComparisonReport, name: &str| {
            rep.entries.iter().find(|e| e.name == name).unwrap().verdict
        };
        assert_eq!(verdict(&report, "fit_probability"), Verdict::Violated);
        assert!(!report.all_hold);

        // Measure 30 coupons, refit, re-predict: the measured-basis
        // receipt HOLDS against the same trial.
        let mut measured = assumed.clone();
        let mut crng = Rng::new(42_002);
        let coupons: Vec<f64> = (0..30)
            .map(|_| 20.0 + true_boss.sample(&mut crng))
            .collect();
        apply_measurement(&mut measured, "printed boss", &coupons, "calipers").unwrap();
        let wc2 = worst_case(&measured).unwrap();
        let r2 = rss(&measured).unwrap();
        let mc2 = monte_carlo(
            &measured,
            &McOptions {
                n: 100_000,
                seed: 42_003,
                batches: 16,
            },
        )
        .unwrap();
        let claims_measured = predicted_claims(&measured, &wc2, &r2, &mc2).unwrap();
        let report2 = compare(&claims_measured, &[trial]).unwrap();
        assert_eq!(verdict(&report2, "fit_probability"), Verdict::Holds);
        // Provenance on the receipt says which basis this is.
        assert!(claims_measured
            .provenance
            .contributors
            .iter()
            .any(|c| matches!(c.source, DistributionSource::Measured { .. })));

        // Sanity on the magnitudes: assumed predicted ≫ reality;
        // measured predicted ≈ reality.
        let p_assumed = claims_assumed
            .claims
            .iter()
            .find(|c| c.name == "fit_probability")
            .unwrap()
            .value;
        let p_measured = claims_measured
            .claims
            .iter()
            .find(|c| c.name == "fit_probability")
            .unwrap()
            .value;
        let p_real = fits as f64 / total as f64;
        assert!(p_assumed > p_real + 0.03, "{p_assumed} vs real {p_real}");
        assert!(
            (p_measured - p_real).abs() < 0.03,
            "{p_measured} vs {p_real}"
        );
    }

    #[test]
    fn trial_measurements_carry_error_bars() {
        let m = Measurement::from_trial(190, 200, 0.02, "trial");
        assert!((m.value - 0.95).abs() < 1e-12);
        assert!(m.uncertainty > 0.0);
        // Zero-failure trials still carry uncertainty.
        let m = Measurement::from_trial(200, 200, 0.02, "trial");
        assert!(m.uncertainty > 0.0, "200/200 is not certainty");
    }
}
