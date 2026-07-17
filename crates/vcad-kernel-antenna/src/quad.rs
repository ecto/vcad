//! Gauss–Legendre quadrature, hand-rolled.
//!
//! Nodes are the roots of the Legendre polynomial `P_n`, found by Newton
//! iteration from the Chebyshev-based initial guess; weights follow from
//! the derivative. Exact for polynomials of degree `2n − 1`. Used for the
//! EFIE matrix fill (outer/inner segment integrals), the far-field
//! radiation integral, and the power integration over the sphere.

/// Gauss–Legendre nodes and weights on `[-1, 1]`.
///
/// Returns `(nodes, weights)`. Panics only on `n == 0`, which would be a
/// programming error (quadrature orders are crate-internal constants).
pub fn gauss_legendre(n: usize) -> (Vec<f64>, Vec<f64>) {
    assert!(n > 0, "quadrature order must be positive");
    let mut nodes = vec![0.0; n];
    let mut weights = vec![0.0; n];
    // Roots are symmetric; compute the upper half and mirror.
    let m = n.div_ceil(2);
    for i in 0..m {
        // Chebyshev-like initial guess for the i-th root (descending x).
        let mut x = (std::f64::consts::PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        let mut dp = 0.0;
        for _ in 0..100 {
            // Evaluate P_n(x) and P_{n-1}(x) by the three-term recurrence.
            let mut p0 = 1.0;
            let mut p1 = x;
            for k in 2..=n {
                let kf = k as f64;
                let p2 = ((2.0 * kf - 1.0) * x * p1 - (kf - 1.0) * p0) / kf;
                p0 = p1;
                p1 = p2;
            }
            // P'_n(x) = n (x P_n − P_{n−1}) / (x² − 1)
            dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
            let dx = p1 / dp;
            x -= dx;
            if dx.abs() < 1e-15 {
                break;
            }
        }
        let w = 2.0 / ((1.0 - x * x) * dp * dp);
        nodes[i] = -x;
        nodes[n - 1 - i] = x;
        weights[i] = w;
        weights[n - 1 - i] = w;
    }
    if n % 2 == 1 {
        nodes[n / 2] = 0.0;
    }
    (nodes, weights)
}

/// Nodes and weights mapped to `[0, len]`.
pub fn gauss_legendre_scaled(n: usize, len: f64) -> (Vec<f64>, Vec<f64>) {
    let (x, w) = gauss_legendre(n);
    let half = 0.5 * len;
    (
        x.iter().map(|&t| half * (t + 1.0)).collect(),
        w.iter().map(|&wi| wi * half).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_interval_length() {
        for n in [1, 2, 4, 7, 16, 33, 48] {
            let (_, w) = gauss_legendre(n);
            let s: f64 = w.iter().sum();
            assert!((s - 2.0).abs() < 1e-13, "n={n}: sum {s}");
        }
    }

    #[test]
    fn integrates_polynomials_exactly() {
        // GL(6) is exact through degree 11: check x^10 on [-1,1] = 2/11.
        let (x, w) = gauss_legendre(6);
        let s: f64 = x.iter().zip(&w).map(|(&xi, &wi)| wi * xi.powi(10)).sum();
        assert!((s - 2.0 / 11.0).abs() < 1e-13, "got {s}");
    }

    #[test]
    fn scaled_rule_integrates_a_cosine() {
        // ∫_0^L cos(t) dt = sin(L)
        let l = 1.3;
        let (x, w) = gauss_legendre_scaled(8, l);
        let s: f64 = x.iter().zip(&w).map(|(&xi, &wi)| wi * xi.cos()).sum();
        assert!((s - l.sin()).abs() < 1e-12);
    }
}
