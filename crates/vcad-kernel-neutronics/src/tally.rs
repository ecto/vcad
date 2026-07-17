//! Batch statistics: every tally is a mean **with** its uncertainty.
//!
//! The run is divided into `B` independent batches (independent RNG
//! streams); every scored quantity is reduced per batch, and the
//! reported value is the mean of batch means with its relative standard
//! error `s/(x̄·√B)`. [`Estimate`] is the only number type the transport
//! API exposes — **a result without an error bar is unrepresentable.**
//!
//! Fail-closed zero rule: a tally that scored nothing in any batch
//! reports `mean = 0` with `rse = ∞`. Zero events is a *statistics
//! floor*, not a measured zero — an infinite relative error is the
//! honest encoding, and it poisons any acceptance test that forgets to
//! check it.

/// A batch-statistics estimate: mean ± relative standard error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Mean of batch means.
    pub mean: f64,
    /// Relative standard error of the mean (σ_mean / |mean|); `∞` when
    /// nothing was scored.
    pub rse: f64,
    /// Number of batches behind the estimate.
    pub batches: usize,
}

impl Estimate {
    /// Reduce per-batch values to an estimate. Panics on fewer than two
    /// batches — a single batch has no variance information, and this
    /// crate refuses to report numbers it cannot put error bars on.
    pub fn from_batches(batch_means: &[f64]) -> Estimate {
        assert!(
            batch_means.len() >= 2,
            "batch statistics need ≥ 2 batches (got {})",
            batch_means.len()
        );
        let b = batch_means.len() as f64;
        let mean = batch_means.iter().sum::<f64>() / b;
        if mean == 0.0 {
            return Estimate {
                mean: 0.0,
                rse: f64::INFINITY,
                batches: batch_means.len(),
            };
        }
        let var_mean =
            batch_means.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (b * (b - 1.0));
        Estimate {
            mean,
            rse: var_mean.sqrt() / mean.abs(),
            batches: batch_means.len(),
        }
    }

    /// One absolute standard error, `|mean|·rse`.
    pub fn abs_sigma(&self) -> f64 {
        if self.rse.is_finite() {
            self.mean.abs() * self.rse
        } else {
            f64::INFINITY
        }
    }

    /// Scale by a constant (source-rate normalization): relative error
    /// is unchanged.
    pub fn scaled(&self, k: f64) -> Estimate {
        Estimate {
            mean: self.mean * k,
            rse: self.rse,
            batches: self.batches,
        }
    }

    /// True when `other` lies within `n_sigma` combined standard errors
    /// of `self` (both must be finite — an all-zero tally never agrees
    /// with anything, fail-closed).
    pub fn consistent_with(&self, expected: f64, n_sigma: f64) -> bool {
        self.rse.is_finite() && (self.mean - expected).abs() <= n_sigma * self.abs_sigma()
    }
}

impl std::fmt::Display for Estimate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.rse.is_finite() {
            write!(f, "{:.4e} ± {:.1}%", self.mean, self.rse * 100.0)
        } else {
            write!(f, "0 (nothing scored — statistics floor, not a zero)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_rse() {
        let e = Estimate::from_batches(&[1.0, 2.0, 3.0, 2.0]);
        assert!((e.mean - 2.0).abs() < 1.0e-12);
        // s² of batch means = ((1)²+0+1+0)/3 = 2/3; var_mean = (2/3)/4;
        // σ_mean = 0.408; rse = 0.204.
        assert!((e.rse - 0.2041).abs() < 1.0e-3);
        assert_eq!(e.batches, 4);
    }

    #[test]
    fn zero_tally_is_infinite_rse() {
        let e = Estimate::from_batches(&[0.0, 0.0, 0.0]);
        assert_eq!(e.mean, 0.0);
        assert!(e.rse.is_infinite());
        assert!(!e.consistent_with(0.0, 3.0), "all-zero must fail closed");
    }

    #[test]
    #[should_panic(expected = "batch statistics need")]
    fn single_batch_refused() {
        let _ = Estimate::from_batches(&[1.0]);
    }
}
