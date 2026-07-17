//! The three stackup analyses: worst-case, RSS, and Monte Carlo.
//!
//! - **Worst-case** is interval arithmetic over the drawing limits:
//!   every part simultaneously at its worst extreme. Guaranteed bounds,
//!   brutally pessimistic for chains of more than a few contributors
//!   (the width grows as Σtᵢ while real scatter grows as √Σtᵢ²).
//! - **RSS** (root-sum-square) is exact linear variance propagation:
//!   σ_G² = Σ aᵢ²σᵢ². The *yield* derived from it assumes the gap is
//!   normal — exact when every contributor is normal, a central-limit
//!   approximation otherwise (flagged on the result).
//! - **Monte Carlo** samples whole virtual assemblies with a seeded
//!   deterministic generator. It is the check on RSS's normality
//!   assumption, and every probability it reports carries a standard
//!   error — the [`ProbabilityEstimate`] type has no error-bar-free
//!   constructor, deliberately.

use serde::{Deserialize, Serialize};

use crate::capability;
use crate::rng::Rng;
use crate::stackup::{Stackup, StackupError};

/// Minimum Monte Carlo sample count (below this, standard errors on
/// interesting yields are so wide the number is noise).
pub const MIN_MC_SAMPLES: usize = 100;

/// A probability with its standard error. There is no way to build one
/// without the error bar: fit probabilities without uncertainty are
/// unrepresentable in this API.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProbabilityEstimate {
    /// Point estimate p̂ = successes/n.
    pub p: f64,
    /// Standard error of p̂, from the Agresti–Coull adjusted count
    /// (Agresti & Coull, *The American Statistician* 52(2), 1998):
    /// p̃ = (k+2)/(n+4), SE = √(p̃(1−p̃)/(n+4)). Unlike the raw binomial
    /// formula it never reports SE = 0 at k = 0 or k = n — "we saw no
    /// failures in n samples" is not certainty.
    pub standard_error: f64,
    /// Sample count.
    pub n: usize,
    /// Success count.
    pub successes: usize,
}

impl ProbabilityEstimate {
    /// Build from counts; the standard error is computed here and
    /// cannot be omitted.
    pub fn from_counts(successes: usize, n: usize) -> Self {
        assert!(n > 0, "probability of zero samples");
        assert!(successes <= n);
        let p = successes as f64 / n as f64;
        let n_adj = n as f64 + 4.0;
        let p_adj = (successes as f64 + 2.0) / n_adj;
        let standard_error = (p_adj * (1.0 - p_adj) / n_adj).sqrt();
        Self {
            p,
            standard_error,
            n,
            successes,
        }
    }
}

/// Worst-case (interval) analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorstCaseAnalysis {
    /// Smallest possible gap with every part at its worst limit, mm.
    pub min_gap: f64,
    /// Largest possible gap, mm.
    pub max_gap: f64,
    /// `min_gap − lower` when a lower bound exists (negative = the
    /// worst-case assembly violates it), mm.
    pub margin_lower: Option<f64>,
    /// `upper − max_gap` when an upper bound exists, mm.
    pub margin_upper: Option<f64>,
    /// True iff every present margin is ≥ 0.
    pub passes: bool,
}

impl WorstCaseAnalysis {
    /// The binding margin: the smallest of the present margins, mm.
    pub fn worst_margin(&self) -> f64 {
        match (self.margin_lower, self.margin_upper) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => f64::NAN, // unreachable post-validate
        }
    }
}

/// Worst-case analysis: interval arithmetic over drawing limits.
pub fn worst_case(s: &Stackup) -> Result<WorstCaseAnalysis, StackupError> {
    s.validate()?;
    let (mut min_gap, mut max_gap) = (0.0, 0.0);
    for c in &s.contributors {
        let e1 = c.coeff * (c.nominal - c.tol_minus);
        let e2 = c.coeff * (c.nominal + c.tol_plus);
        min_gap += e1.min(e2);
        max_gap += e1.max(e2);
    }
    let margin_lower = s.requirement.lower_mm.map(|l| min_gap - l);
    let margin_upper = s.requirement.upper_mm.map(|u| u - max_gap);
    let passes = margin_lower.unwrap_or(0.0) >= 0.0 && margin_upper.unwrap_or(0.0) >= 0.0;
    Ok(WorstCaseAnalysis {
        min_gap,
        max_gap,
        margin_lower,
        margin_upper,
        passes,
    })
}

/// RSS (linear variance propagation) analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RssAnalysis {
    /// Mean gap (nominals + process centering errors), mm.
    pub mean_gap: f64,
    /// Gap standard deviation √(Σ aᵢ²σᵢ²), mm. Exact under
    /// independence — no normality needed for the moments.
    pub sigma_gap: f64,
    /// Φ-based fit probability. Exact under the model when
    /// `all_normal`, a central-limit approximation otherwise.
    pub yield_estimate: f64,
    /// Cp = (USL−LSL)/(6σ_G); `None` for one-sided requirements.
    pub cp: Option<f64>,
    /// Cpk = min distance-to-limit/(3σ_G); `None` only when σ_G = 0
    /// (rejected earlier as [`StackupError::DegenerateChain`]).
    pub cpk: Option<f64>,
    /// Whether every contributor is normal (yield exact under the
    /// model) or not (yield leans on the CLT).
    pub all_normal: bool,
}

/// RSS analysis: exact second-moment propagation, Φ-based yield.
pub fn rss(s: &Stackup) -> Result<RssAnalysis, StackupError> {
    s.validate()?;
    let variance = s.variance_gap();
    if variance == 0.0 {
        return Err(StackupError::DegenerateChain);
    }
    let mean_gap = s.mean_gap();
    let sigma_gap = variance.sqrt();
    let lower = s.requirement.lower_mm;
    let upper = s.requirement.upper_mm;
    let yield_estimate = capability::yield_within(mean_gap, sigma_gap, lower, upper);
    let all_normal = s
        .contributors
        .iter()
        .all(|c| matches!(c.dist, crate::dist::Distribution::Normal { .. }));
    Ok(RssAnalysis {
        mean_gap,
        sigma_gap,
        yield_estimate,
        cp: capability::cp(sigma_gap, lower, upper),
        cpk: capability::cpk(mean_gap, sigma_gap, lower, upper),
        all_normal,
    })
}

/// Monte Carlo analysis result. Every probability carries its standard
/// error; the batch cross-check exposes estimator health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonteCarloAnalysis {
    /// Sample count.
    pub n: usize,
    /// PRNG seed (xoshiro256++ via SplitMix64) — reproducibility
    /// provenance.
    pub seed: u64,
    /// Fit probability with Agresti–Coull standard error.
    pub fit: ProbabilityEstimate,
    /// Cross-check: standard error of the fit probability from batch
    /// means (spread of per-batch p̂ over [`Self::batches`] disjoint
    /// batches). Should agree with `fit.standard_error` within a small
    /// factor for healthy i.i.d. sampling.
    pub fit_se_batch: f64,
    /// Number of batches behind `fit_se_batch`.
    pub batches: usize,
    /// Sample mean gap, mm.
    pub mean_gap: f64,
    /// Standard error of the mean gap: s/√n, mm.
    pub mean_gap_se: f64,
    /// Bessel-corrected sample standard deviation of the gap, mm.
    pub sigma_gap: f64,
    /// Standard error of `sigma_gap` under normal theory,
    /// ≈ s/√(2(n−1)) (large-sample; e.g. Kenney & Keeping,
    /// *Mathematics of Statistics*), mm.
    pub sigma_gap_se: f64,
    /// Smallest sampled gap, mm.
    pub min_sample: f64,
    /// Largest sampled gap, mm.
    pub max_sample: f64,
}

/// Options for [`monte_carlo`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct McOptions {
    /// Number of virtual assemblies to build.
    pub n: usize,
    /// PRNG seed. Same seed + same stackup = bit-identical result.
    pub seed: u64,
    /// Batches for the batch-mean SE cross-check.
    pub batches: usize,
}

impl Default for McOptions {
    fn default() -> Self {
        Self {
            n: 100_000,
            seed: 0x5EED_7015,
            batches: 16,
        }
    }
}

/// Monte Carlo analysis: seeded, deterministic, error-barred.
pub fn monte_carlo(s: &Stackup, opts: &McOptions) -> Result<MonteCarloAnalysis, StackupError> {
    s.validate()?;
    if s.variance_gap() == 0.0 {
        return Err(StackupError::DegenerateChain);
    }
    if opts.n < MIN_MC_SAMPLES {
        return Err(StackupError::TooFewSamples {
            n: opts.n,
            min: MIN_MC_SAMPLES,
        });
    }
    let batches = opts.batches.max(2).min(opts.n);
    let mut rng = Rng::new(opts.seed);
    let lower = s.requirement.lower_mm;
    let upper = s.requirement.upper_mm;

    // Welford one-pass moments + extrema + per-batch fit counts.
    let mut mean = 0.0f64;
    let mut m2 = 0.0f64;
    let mut fits = 0usize;
    let (mut min_sample, mut max_sample) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut batch_fit = vec![0usize; batches];
    let mut batch_n = vec![0usize; batches];
    for i in 0..opts.n {
        let g = s.sample_gap(&mut rng);
        let k = (i + 1) as f64;
        let delta = g - mean;
        mean += delta / k;
        m2 += delta * (g - mean);
        min_sample = min_sample.min(g);
        max_sample = max_sample.max(g);
        let ok = lower.is_none_or(|l| g >= l) && upper.is_none_or(|u| g <= u);
        let b = i * batches / opts.n; // contiguous batches of ~equal size
        batch_n[b] += 1;
        if ok {
            fits += 1;
            batch_fit[b] += 1;
        }
    }
    let n_f = opts.n as f64;
    let var = m2 / (n_f - 1.0);
    let sigma = var.sqrt();

    // Batch-mean SE of the fit probability: sd of per-batch p̂ / √B.
    let batch_ps: Vec<f64> = batch_fit
        .iter()
        .zip(&batch_n)
        .filter(|&(_, &bn)| bn > 0)
        .map(|(&bf, &bn)| bf as f64 / bn as f64)
        .collect();
    let nb = batch_ps.len() as f64;
    let bp_mean = batch_ps.iter().sum::<f64>() / nb;
    let bp_var = batch_ps.iter().map(|p| (p - bp_mean).powi(2)).sum::<f64>() / (nb - 1.0);
    let fit_se_batch = (bp_var / nb).sqrt();

    Ok(MonteCarloAnalysis {
        n: opts.n,
        seed: opts.seed,
        fit: ProbabilityEstimate::from_counts(fits, opts.n),
        fit_se_batch,
        batches,
        mean_gap: mean,
        mean_gap_se: sigma / n_f.sqrt(),
        sigma_gap: sigma,
        sigma_gap_se: sigma / (2.0 * (n_f - 1.0)).sqrt(),
        min_sample,
        max_sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::SigmaConvention;
    use crate::stackup::{Contributor, Requirement};

    fn two_block() -> Stackup {
        // Slot 30 ±0.3, block 29 ±0.4 → gap 1.0, WC [0.3, 1.7],
        // σ_G = √(0.01 + 0.4²/9)… with 3σ: σ = 0.1 and 0.1333…
        Stackup {
            name: "two-block".into(),
            contributors: vec![
                Contributor::normal("slot", 1.0, 30.0, 0.3, SigmaConvention::ThreeSigma),
                Contributor::normal("block", -1.0, 29.0, 0.4, SigmaConvention::ThreeSigma),
            ],
            requirement: Requirement::between("clearance", 0.2, 1.8),
        }
    }

    #[test]
    fn worst_case_interval_arithmetic_exact() {
        let wc = worst_case(&two_block()).unwrap();
        assert!((wc.min_gap - 0.3).abs() < 1e-12);
        assert!((wc.max_gap - 1.7).abs() < 1e-12);
        assert!((wc.margin_lower.unwrap() - 0.1).abs() < 1e-12);
        assert!((wc.margin_upper.unwrap() - 0.1).abs() < 1e-12);
        assert!(wc.passes);
        assert!((wc.worst_margin() - 0.1).abs() < 1e-12);

        // Tighten the requirement: fails, margins go negative.
        let mut s = two_block();
        s.requirement = Requirement::between("tight", 0.4, 1.6);
        let wc = worst_case(&s).unwrap();
        assert!(!wc.passes);
        assert!((wc.worst_margin() + 0.1).abs() < 1e-12);
    }

    #[test]
    fn worst_case_handles_negative_and_scaled_coefficients() {
        // A lever-ratio contributor (coeff 2.5) and a negative-nominal
        // direction: interval endpoints must be taken per-sign.
        let s = Stackup {
            name: "levers".into(),
            contributors: vec![
                Contributor::normal("a", 2.5, 10.0, 0.1, SigmaConvention::ThreeSigma),
                Contributor::normal("b", -2.5, 9.0, 0.1, SigmaConvention::ThreeSigma),
            ],
            requirement: Requirement::at_least("gap", 0.0),
        };
        let wc = worst_case(&s).unwrap();
        assert!((wc.min_gap - (2.5 * 9.9 - 2.5 * 9.1)).abs() < 1e-12);
        assert!((wc.max_gap - (2.5 * 10.1 - 2.5 * 8.9)).abs() < 1e-12);
    }

    #[test]
    fn rss_moments_and_capability() {
        let r = rss(&two_block()).unwrap();
        assert!((r.mean_gap - 1.0).abs() < 1e-12);
        let sigma = (0.1f64.powi(2) + (0.4f64 / 3.0).powi(2)).sqrt();
        assert!((r.sigma_gap - sigma).abs() < 1e-12);
        assert!(r.all_normal);
        // Cp = 1.6/(6σ), Cpk = 0.8/(3σ) — symmetric here.
        assert!((r.cp.unwrap() - 1.6 / (6.0 * sigma)).abs() < 1e-12);
        assert!((r.cpk.unwrap() - 0.8 / (3.0 * sigma)).abs() < 1e-12);
        // Yield: symmetric two-sided at z = 0.8/σ = 4.8 → ~1 − 2Φ(−4.8).
        assert!(r.yield_estimate > 0.999_99);
    }

    #[test]
    fn monte_carlo_is_deterministic_and_error_barred() {
        let s = two_block();
        let opts = McOptions {
            n: 50_000,
            seed: 42,
            batches: 16,
        };
        let a = monte_carlo(&s, &opts).unwrap();
        let b = monte_carlo(&s, &opts).unwrap();
        assert_eq!(a, b, "same seed must be bit-identical");

        // Moments agree with RSS (exact under the model) within SE.
        let r = rss(&s).unwrap();
        assert!((a.mean_gap - r.mean_gap).abs() < 5.0 * a.mean_gap_se);
        assert!((a.sigma_gap - r.sigma_gap).abs() < 5.0 * a.sigma_gap_se);

        // The two SE estimates for the fit probability agree in order.
        assert!(a.fit_se_batch < 5.0 * a.fit.standard_error + 1e-9);

        // Different seed differs (statistically certain at n=50k).
        let c = monte_carlo(&s, &McOptions { seed: 43, ..opts }).unwrap();
        assert_ne!(a.mean_gap.to_bits(), c.mean_gap.to_bits());
    }

    #[test]
    fn probability_estimate_never_reports_zero_se() {
        let p = ProbabilityEstimate::from_counts(0, 1000);
        assert_eq!(p.p, 0.0);
        assert!(p.standard_error > 0.0, "0/1000 is not certainty");
        let p = ProbabilityEstimate::from_counts(1000, 1000);
        assert_eq!(p.p, 1.0);
        assert!(p.standard_error > 0.0);
        // And the SE shrinks with n.
        let big = ProbabilityEstimate::from_counts(0, 100_000);
        assert!(big.standard_error < p.standard_error);
    }

    #[test]
    fn fail_closed_paths() {
        let s = two_block();
        assert!(matches!(
            monte_carlo(
                &s,
                &McOptions {
                    n: 10,
                    ..Default::default()
                }
            ),
            Err(StackupError::TooFewSamples { .. })
        ));

        // All-zero-variance chain: statistical analyses refuse.
        let mut d = two_block();
        for c in &mut d.contributors {
            c.dist = crate::dist::Distribution::Normal {
                mean: 0.0,
                sigma: 0.0,
            };
        }
        assert_eq!(rss(&d), Err(StackupError::DegenerateChain));
        assert!(matches!(
            monte_carlo(&d, &McOptions::default()),
            Err(StackupError::DegenerateChain)
        ));
        // Worst case still works — that's its job.
        assert!(worst_case(&d).is_ok());
    }
}
