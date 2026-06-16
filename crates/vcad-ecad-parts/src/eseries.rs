//! E-series (preferred number) snapping for passive values.

/// E24 mantissas (±5%).
pub const E24: [f64; 24] = [
    1.0, 1.1, 1.2, 1.3, 1.5, 1.6, 1.8, 2.0, 2.2, 2.4, 2.7, 3.0, 3.3, 3.6, 3.9, 4.3, 4.7, 5.1, 5.6,
    6.2, 6.8, 7.5, 8.2, 9.1,
];

/// E96 mantissas (±1%).
pub const E96: [f64; 96] = [
    1.00, 1.02, 1.05, 1.07, 1.10, 1.13, 1.15, 1.18, 1.21, 1.24, 1.27, 1.30, 1.33, 1.37, 1.40, 1.43,
    1.47, 1.50, 1.54, 1.58, 1.62, 1.65, 1.69, 1.74, 1.78, 1.82, 1.87, 1.91, 1.96, 2.00, 2.05, 2.10,
    2.15, 2.21, 2.26, 2.32, 2.37, 2.43, 2.49, 2.55, 2.61, 2.67, 2.74, 2.80, 2.87, 2.94, 3.01, 3.09,
    3.16, 3.24, 3.32, 3.40, 3.48, 3.57, 3.65, 3.74, 3.83, 3.92, 4.02, 4.12, 4.22, 4.32, 4.42, 4.53,
    4.64, 4.75, 4.87, 4.99, 5.11, 5.23, 5.36, 5.49, 5.62, 5.76, 5.90, 6.04, 6.19, 6.34, 6.49, 6.65,
    6.81, 6.98, 7.15, 7.32, 7.50, 7.68, 7.87, 8.06, 8.25, 8.45, 8.66, 8.87, 9.09, 9.31, 9.53, 9.76,
];

/// A preferred-number series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ESeries {
    /// ±5% (24 values/decade).
    E24,
    /// ±1% (96 values/decade).
    E96,
}

impl ESeries {
    fn mantissas(&self) -> &'static [f64] {
        match self {
            ESeries::E24 => &E24,
            ESeries::E96 => &E96,
        }
    }
}

/// Snap a positive value to the nearest preferred number in `series`.
/// Non-positive values are returned unchanged.
pub fn snap(value: f64, series: ESeries) -> f64 {
    if value <= 0.0 || !value.is_finite() {
        return value;
    }
    let decade = 10f64.powf(value.log10().floor());
    let m = series.mantissas();
    // Candidates can straddle a decade boundary (e.g. 9.8 → 10.0), so also
    // consider the first mantissa of the next decade.
    let mut best = m[0] * decade;
    let mut best_err = (best - value).abs();
    for &cand_m in m.iter() {
        for &mult in &[decade, decade * 10.0] {
            let cand = cand_m * mult;
            let err = (cand - value).abs();
            if err < best_err {
                best = cand;
                best_err = err;
            }
        }
    }
    best
}

/// The `n` nearest preferred values to `value` (including the snapped value),
/// sorted by closeness — used to offer E-series neighbours as alternatives.
pub fn neighbors(value: f64, series: ESeries, n: usize) -> Vec<f64> {
    if value <= 0.0 || n == 0 {
        return vec![];
    }
    let decade = 10f64.powf(value.log10().floor());
    let m = series.mantissas();
    let mut cands: Vec<f64> = Vec::new();
    for mult in [decade / 10.0, decade, decade * 10.0] {
        for &cand_m in m.iter() {
            cands.push(cand_m * mult);
        }
    }
    cands.sort_by(|a, b| {
        (a - value)
            .abs()
            .partial_cmp(&(b - value).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cands.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
    cands.truncate(n);
    cands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_e24() {
        assert!((snap(10_500.0, ESeries::E24) - 10_000.0).abs() < 1.0);
        assert!((snap(4600.0, ESeries::E24) - 4700.0).abs() < 1.0);
        assert!((snap(33_500.0, ESeries::E24) - 33_000.0).abs() < 1.0);
    }

    #[test]
    fn snaps_to_e96() {
        // 10.1k → nearest E96 is 10.0k; 10.3k → 10.2k.
        assert!((snap(10_100.0, ESeries::E96) - 10_000.0).abs() < 1.0);
        assert!((snap(10_300.0, ESeries::E96) - 10_200.0).abs() < 1.0);
    }

    #[test]
    fn neighbors_sorted_by_closeness() {
        let n = neighbors(10_000.0, ESeries::E24, 3);
        assert_eq!(n.len(), 3);
        assert!((n[0] - 10_000.0).abs() < 1.0);
    }
}
