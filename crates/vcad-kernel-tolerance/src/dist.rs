//! Statistical models of dimensional deviation, and the tolerance-to-σ
//! convention that links drawing limits to process statistics.
//!
//! Every distribution here describes the **deviation from nominal** of
//! one contributor, in millimeters. The worst-case analysis never looks
//! at the distribution — it uses the drawing limits on the contributor —
//! while RSS and Monte Carlo use only the distribution. The two are tied
//! together by [`SigmaConvention`], and that tie is an *assumption*:
//! see the honesty section in `docs/tolerance-m0.md`.

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// How a ± drawing tolerance maps to the standard deviation of the
/// assumed normal process.
///
/// **This convention buries more products than any solver bug.** The
/// default ±tol = 3σ says the process exactly fills the drawing band at
/// Cp = 1.00 and ships 0.27% of parts outside the limits; a supplier
/// running Cp = 1.33 is at ±tol = 4σ, and a Six Sigma process at ±tol =
/// 6σ (before mean shift). Every yield number downstream inherits
/// whichever convention you pick, so the receipt records it as
/// provenance rather than letting it default silently.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SigmaConvention {
    /// ±tol = 3σ — the industry default assumption for an uncontrolled
    /// but capable process (Cp = 1.00).
    #[default]
    ThreeSigma,
    /// ±tol = k·σ for a stated k (4 for a Cp = 1.33 supplier agreement,
    /// 6 for a Six Sigma process).
    KSigma {
        /// Number of standard deviations covered by the tolerance.
        k: f64,
    },
}

impl SigmaConvention {
    /// The number of σ the ± tolerance spans on each side of nominal.
    pub fn k(self) -> f64 {
        match self {
            SigmaConvention::ThreeSigma => 3.0,
            SigmaConvention::KSigma { k } => k,
        }
    }
}

/// Where a contributor's distribution came from.
///
/// Distributions are **assumptions until measured**. The receipt carries
/// this per contributor so a claimed yield can be audited: a chain of
/// `Assumed` sources is a paper prediction; `Measured` sources tie it to
/// coupon data (see the `measure` module).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DistributionSource {
    /// Derived from the drawing tolerance under a stated convention.
    Assumed {
        /// The tolerance-to-σ convention used.
        convention: SigmaConvention,
    },
    /// Fitted from measured samples (calipers on printed coupons, CMM
    /// data, supplier SPC exports).
    Measured {
        /// Number of samples behind the fit.
        n_samples: usize,
        /// Instrument or data-source provenance.
        instrument: String,
    },
}

impl Default for DistributionSource {
    fn default() -> Self {
        DistributionSource::Assumed {
            convention: SigmaConvention::ThreeSigma,
        }
    }
}

/// Statistical model of one contributor's deviation from nominal (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Distribution {
    /// Normal deviation: `mean` is the process centering error (usually
    /// 0), `sigma` the process standard deviation. Support is unbounded —
    /// under the 3σ convention, 0.27% of parts genuinely fall outside
    /// the drawing limits, and Monte Carlo will sample them.
    Normal {
        /// Process centering error, mm.
        mean: f64,
        /// Process standard deviation, mm (≥ 0; 0 is a fixed offset).
        sigma: f64,
    },
    /// Uniform on [lo, hi] — the honest model when nothing is known
    /// beyond "the vendor ships anywhere in the band" (bearing width
    /// grades, purchased stock).
    Uniform {
        /// Lower deviation bound, mm.
        lo: f64,
        /// Upper deviation bound, mm.
        hi: f64,
    },
    /// Two-state ("Bernoulli-shifted"): deviation `a` with probability
    /// 1 − `p_b`, deviation `b` with probability `p_b`. Models two-state
    /// realities a normal cannot: two suppliers, two mold cavities, a
    /// seated-vs-cocked retaining ring.
    TwoPoint {
        /// First state's deviation, mm.
        a: f64,
        /// Second state's deviation, mm.
        b: f64,
        /// Probability of state `b` (in [0, 1]).
        p_b: f64,
    },
}

impl Distribution {
    /// Mean deviation from nominal, mm.
    pub fn mean(&self) -> f64 {
        match *self {
            Distribution::Normal { mean, .. } => mean,
            Distribution::Uniform { lo, hi } => 0.5 * (lo + hi),
            Distribution::TwoPoint { a, b, p_b } => a * (1.0 - p_b) + b * p_b,
        }
    }

    /// Variance of the deviation, mm².
    pub fn variance(&self) -> f64 {
        match *self {
            Distribution::Normal { sigma, .. } => sigma * sigma,
            Distribution::Uniform { lo, hi } => {
                let w = hi - lo;
                w * w / 12.0
            }
            Distribution::TwoPoint { a, b, p_b } => {
                let d = b - a;
                d * d * p_b * (1.0 - p_b)
            }
        }
    }

    /// Standard deviation of the deviation, mm.
    pub fn sigma(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Support bounds, when the distribution is bounded. `None` for
    /// [`Distribution::Normal`] — its tails are infinite, which is
    /// exactly why worst-case analysis uses drawing limits instead.
    pub fn support(&self) -> Option<(f64, f64)> {
        match *self {
            Distribution::Normal { .. } => None,
            Distribution::Uniform { lo, hi } => Some((lo, hi)),
            Distribution::TwoPoint { a, b, .. } => Some((a.min(b), a.max(b))),
        }
    }

    /// Draw one deviation sample.
    pub fn sample(&self, rng: &mut Rng) -> f64 {
        match *self {
            Distribution::Normal { mean, sigma } => mean + sigma * rng.next_normal(),
            Distribution::Uniform { lo, hi } => lo + (hi - lo) * rng.next_f64(),
            Distribution::TwoPoint { a, b, p_b } => {
                if rng.next_f64() < p_b {
                    b
                } else {
                    a
                }
            }
        }
    }

    /// Structural validity: finite parameters, σ ≥ 0, lo ≤ hi,
    /// p_b ∈ [0, 1]. Returns a human-readable reason on failure.
    pub(crate) fn check(&self) -> Result<(), String> {
        match *self {
            Distribution::Normal { mean, sigma } => {
                if !mean.is_finite() || !sigma.is_finite() {
                    return Err("normal parameters must be finite".into());
                }
                if sigma < 0.0 {
                    return Err(format!("sigma must be >= 0, got {sigma}"));
                }
            }
            Distribution::Uniform { lo, hi } => {
                if !lo.is_finite() || !hi.is_finite() {
                    return Err("uniform bounds must be finite".into());
                }
                if lo > hi {
                    return Err(format!("uniform requires lo <= hi, got [{lo}, {hi}]"));
                }
            }
            Distribution::TwoPoint { a, b, p_b } => {
                if !a.is_finite() || !b.is_finite() || !p_b.is_finite() {
                    return Err("two-point parameters must be finite".into());
                }
                if !(0.0..=1.0).contains(&p_b) {
                    return Err(format!("p_b must be in [0, 1], got {p_b}"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_match_closed_forms() {
        let n = Distribution::Normal {
            mean: 0.1,
            sigma: 0.05,
        };
        assert_eq!(n.mean(), 0.1);
        assert!((n.variance() - 0.0025).abs() < 1e-15);

        let u = Distribution::Uniform { lo: -0.12, hi: 0.0 };
        assert!((u.mean() + 0.06).abs() < 1e-15);
        assert!((u.variance() - 0.12 * 0.12 / 12.0).abs() < 1e-15);

        let t = Distribution::TwoPoint {
            a: -0.03,
            b: 0.04,
            p_b: 0.4,
        };
        assert!((t.mean() - (-0.03 * 0.6 + 0.04 * 0.4)).abs() < 1e-15);
        assert!((t.variance() - 0.07 * 0.07 * 0.4 * 0.6).abs() < 1e-15);
    }

    #[test]
    fn sample_means_converge_to_analytic_means() {
        let dists = [
            Distribution::Normal {
                mean: 0.02,
                sigma: 0.1,
            },
            Distribution::Uniform { lo: -0.2, hi: 0.1 },
            Distribution::TwoPoint {
                a: -1.0,
                b: 2.0,
                p_b: 0.25,
            },
        ];
        let mut rng = Rng::new(2024);
        for d in dists {
            let n = 100_000;
            let mut sum = 0.0;
            let mut sum_sq = 0.0;
            for _ in 0..n {
                let x = d.sample(&mut rng);
                sum += x;
                sum_sq += x * x;
            }
            let mean = sum / n as f64;
            let var = sum_sq / n as f64 - mean * mean;
            let se_mean = d.sigma() / (n as f64).sqrt();
            assert!(
                (mean - d.mean()).abs() < 5.0 * se_mean,
                "{d:?}: mean {mean} vs {}",
                d.mean()
            );
            assert!(
                (var - d.variance()).abs() / d.variance() < 0.05,
                "{d:?}: var {var} vs {}",
                d.variance()
            );
        }
    }

    #[test]
    fn support_is_honest_about_normal_tails() {
        assert_eq!(
            Distribution::Normal {
                mean: 0.0,
                sigma: 1.0
            }
            .support(),
            None
        );
        assert_eq!(
            Distribution::Uniform { lo: -1.0, hi: 2.0 }.support(),
            Some((-1.0, 2.0))
        );
        assert_eq!(
            Distribution::TwoPoint {
                a: 3.0,
                b: -1.0,
                p_b: 0.5
            }
            .support(),
            Some((-1.0, 3.0))
        );
    }

    #[test]
    fn check_rejects_bad_parameters() {
        assert!(Distribution::Normal {
            mean: 0.0,
            sigma: -1.0
        }
        .check()
        .is_err());
        assert!(Distribution::Uniform { lo: 1.0, hi: 0.0 }.check().is_err());
        assert!(Distribution::TwoPoint {
            a: 0.0,
            b: 1.0,
            p_b: 1.5
        }
        .check()
        .is_err());
        assert!(Distribution::Normal {
            mean: f64::NAN,
            sigma: 1.0
        }
        .check()
        .is_err());
    }

    #[test]
    fn sigma_convention_k() {
        assert_eq!(SigmaConvention::ThreeSigma.k(), 3.0);
        assert_eq!(SigmaConvention::KSigma { k: 4.5 }.k(), 4.5);
        assert_eq!(SigmaConvention::default(), SigmaConvention::ThreeSigma);
    }

    #[test]
    fn serde_round_trip() {
        let d = Distribution::TwoPoint {
            a: -0.03,
            b: 0.04,
            p_b: 0.4,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("two_point"), "{json}");
        let back: Distribution = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
}
