//! Binned jackknife statistics.
//!
//! Monte Carlo time series are autocorrelated; naive standard errors
//! lie low. The standard defense is to average the series into bins
//! longer than the autocorrelation time and jackknife over the bins.
//! Every observable this crate reports is an [`Estimate`] produced
//! this way — the API has no path that yields a bare mean.

use serde::{Deserialize, Serialize};

/// A mean with its jackknife standard error and the binning that
/// produced it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// Sample mean.
    pub mean: f64,
    /// Jackknife standard error over bins.
    pub err: f64,
    /// Number of bins the error was estimated from.
    pub n_bins: usize,
    /// Measurements per bin.
    pub bin_size: usize,
}

/// Errors from statistics construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StatsError {
    /// Fewer than 2 complete bins — no error bar is estimable.
    TooFewBins {
        /// Complete bins available.
        available: usize,
    },
    /// Bin size must be at least 1.
    ZeroBinSize,
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatsError::TooFewBins { available } => {
                write!(f, "jackknife needs >= 2 complete bins, have {available}")
            }
            StatsError::ZeroBinSize => write!(f, "bin size must be >= 1"),
        }
    }
}

impl std::error::Error for StatsError {}

/// Binned jackknife estimate of the mean of `series`. Trailing samples
/// that do not fill a complete bin are dropped (documented, standard).
pub fn jackknife(series: &[f64], bin_size: usize) -> Result<Estimate, StatsError> {
    if bin_size == 0 {
        return Err(StatsError::ZeroBinSize);
    }
    let n_bins = series.len() / bin_size;
    if n_bins < 2 {
        return Err(StatsError::TooFewBins { available: n_bins });
    }
    let bins: Vec<f64> = (0..n_bins)
        .map(|b| series[b * bin_size..(b + 1) * bin_size].iter().sum::<f64>() / bin_size as f64)
        .collect();
    let total: f64 = bins.iter().sum();
    let mean = total / n_bins as f64;
    // Leave-one-out means; jackknife variance of the mean.
    let var: f64 = bins
        .iter()
        .map(|&b| {
            let loo = (total - b) / (n_bins - 1) as f64;
            (loo - mean) * (loo - mean)
        })
        .sum::<f64>()
        * (n_bins - 1) as f64
        / n_bins as f64;
    Ok(Estimate {
        mean,
        err: var.sqrt(),
        n_bins,
        bin_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn rejects_starved_input() {
        assert_eq!(
            jackknife(&[1.0], 1),
            Err(StatsError::TooFewBins { available: 1 })
        );
        assert_eq!(jackknife(&[1.0, 2.0], 0), Err(StatsError::ZeroBinSize));
    }

    #[test]
    fn matches_naive_error_for_iid() {
        // For iid data with bin_size 1, jackknife error = naive standard
        // error of the mean.
        let mut rng = Rng::seeded(31);
        let xs: Vec<f64> = (0..1000).map(|_| rng.uniform()).collect();
        let e = jackknife(&xs, 1).unwrap();
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (xs.len() - 1) as f64;
        let naive = (var / xs.len() as f64).sqrt();
        assert!((e.mean - mean).abs() < 1e-12);
        assert!((e.err - naive).abs() < 1e-12, "{} vs {naive}", e.err);
    }

    #[test]
    fn error_scales_like_inverse_sqrt_n() {
        let mut rng = Rng::seeded(32);
        let xs: Vec<f64> = (0..4000).map(|_| rng.uniform()).collect();
        let e_small = jackknife(&xs[..1000], 10).unwrap();
        let e_big = jackknife(&xs, 10).unwrap();
        let ratio = e_small.err / e_big.err;
        assert!((ratio - 2.0).abs() < 0.5, "1/sqrt(N) ratio {ratio}");
    }
}
