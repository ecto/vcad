//! Dense complex linear algebra: matrix storage and LU with partial
//! pivoting.
//!
//! Hand-rolled on [`Complex`] pairs — no external dependencies. The MoM
//! systems here are small (N ≲ a few hundred), dense, and complex
//! symmetric; O(N³) Doolittle LU with partial pivoting is exactly right.
//! The factorization is kept so multiple right-hand sides (multi-port
//! excitation, and the M2 adjoint solve — the matrix is symmetric, so the
//! transpose system shares the factorization) cost O(N²) each.

use crate::complex::Complex;
use crate::error::AntennaError;

/// Dense square complex matrix, row-major.
#[derive(Debug, Clone)]
pub struct CMatrix {
    n: usize,
    a: Vec<Complex>,
}

impl CMatrix {
    /// Zero matrix of side `n`.
    pub fn zeros(n: usize) -> Self {
        CMatrix {
            n,
            a: vec![Complex::ZERO; n * n],
        }
    }

    /// Matrix side.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Element (i, j).
    pub fn at(&self, i: usize, j: usize) -> Complex {
        self.a[i * self.n + j]
    }

    /// Mutable element (i, j).
    pub fn at_mut(&mut self, i: usize, j: usize) -> &mut Complex {
        &mut self.a[i * self.n + j]
    }

    /// Largest element magnitude (scale for singularity thresholds).
    pub fn max_abs(&self) -> f64 {
        self.a.iter().map(|z| z.abs()).fold(0.0, f64::max)
    }
}

/// LU factorization with row pivoting.
#[derive(Debug, Clone)]
pub struct LuFactors {
    n: usize,
    lu: Vec<Complex>,
    piv: Vec<usize>,
}

/// Factor `m` in place (Doolittle, partial pivoting). Fails closed on a
/// pivot smaller than `1e-13` of the matrix scale.
pub fn lu_decompose(m: CMatrix) -> Result<LuFactors, AntennaError> {
    let n = m.n;
    let scale = m.max_abs();
    if scale <= 0.0 || !scale.is_finite() {
        return Err(AntennaError::SingularSystem);
    }
    let tiny2 = (1e-13 * scale) * (1e-13 * scale);
    let mut lu = m.a;
    let mut piv: Vec<usize> = (0..n).collect();

    for k in 0..n {
        // Pivot: largest |a_ik| for i ≥ k.
        let (mut best_i, mut best) = (k, lu[k * n + k].norm_sqr());
        for i in (k + 1)..n {
            let v = lu[i * n + k].norm_sqr();
            if v > best {
                best = v;
                best_i = i;
            }
        }
        if best <= tiny2 {
            return Err(AntennaError::SingularSystem);
        }
        if best_i != k {
            for j in 0..n {
                lu.swap(k * n + j, best_i * n + j);
            }
            piv.swap(k, best_i);
        }
        let pivot = lu[k * n + k];
        for i in (k + 1)..n {
            let f = lu[i * n + k] / pivot;
            lu[i * n + k] = f;
            for j in (k + 1)..n {
                let sub = f * lu[k * n + j];
                lu[i * n + j] -= sub;
            }
        }
    }
    Ok(LuFactors { n, lu, piv })
}

impl LuFactors {
    /// Solve `A x = b` for the factored `A`.
    pub fn solve(&self, b: &[Complex]) -> Vec<Complex> {
        let n = self.n;
        assert_eq!(b.len(), n, "rhs length mismatch");
        // Apply the row permutation, then forward/back substitution.
        let mut x: Vec<Complex> = self.piv.iter().map(|&p| b[p]).collect();
        for i in 1..n {
            let mut s = x[i];
            for (&lij, &xj) in self.lu[i * n..i * n + i].iter().zip(&x[..i]) {
                let d = lij * xj;
                s -= d;
            }
            x[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for (&lij, &xj) in self.lu[i * n + i + 1..i * n + n].iter().zip(&x[i + 1..]) {
                let d = lij * xj;
                s -= d;
            }
            x[i] = s / self.lu[i * n + i];
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_known_2x2_system() {
        // [1+j, 2; 3, 4-j] x = [3+j, 7-j]  →  hand-checkable
        let mut m = CMatrix::zeros(2);
        *m.at_mut(0, 0) = Complex::new(1.0, 1.0);
        *m.at_mut(0, 1) = Complex::new(2.0, 0.0);
        *m.at_mut(1, 0) = Complex::new(3.0, 0.0);
        *m.at_mut(1, 1) = Complex::new(4.0, -1.0);
        let a = m.clone();
        let b = [Complex::new(3.0, 1.0), Complex::new(7.0, -1.0)];
        let x = lu_decompose(m).unwrap().solve(&b);
        // Residual check.
        for (i, &bi) in b.iter().enumerate() {
            let mut r = Complex::ZERO;
            for (j, &xj) in x.iter().enumerate() {
                r += a.at(i, j) * xj;
            }
            assert!((r - bi).abs() < 1e-12);
        }
    }

    #[test]
    fn residual_is_tiny_on_a_deterministic_dense_system() {
        // Deterministic LCG fill; diagonally nudged for conditioning.
        let n = 24;
        let mut state = 0x2545F491_u64;
        let mut rng = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        };
        let mut m = CMatrix::zeros(n);
        for i in 0..n {
            for j in 0..n {
                *m.at_mut(i, j) = Complex::new(rng(), rng());
            }
            *m.at_mut(i, i) += Complex::new(4.0, 2.0);
        }
        let a = m.clone();
        let b: Vec<Complex> = (0..n).map(|_| Complex::new(rng(), rng())).collect();
        let x = lu_decompose(m).unwrap().solve(&b);
        let mut worst: f64 = 0.0;
        for (i, &bi) in b.iter().enumerate() {
            let mut r = Complex::ZERO;
            for (j, &xj) in x.iter().enumerate() {
                r += a.at(i, j) * xj;
            }
            worst = worst.max((r - bi).abs());
        }
        assert!(worst < 1e-11, "residual {worst}");
    }

    #[test]
    fn singular_matrix_fails_closed() {
        let mut m = CMatrix::zeros(3);
        for j in 0..3 {
            *m.at_mut(0, j) = Complex::new(1.0, j as f64);
            *m.at_mut(1, j) = Complex::new(2.0, 2.0 * j as f64); // 2 × row 0
            *m.at_mut(2, j) = Complex::new(0.5, -1.0);
        }
        assert!(matches!(lu_decompose(m), Err(AntennaError::SingularSystem)));
    }
}
