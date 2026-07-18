//! Direct complex linear solves for the Helmholtz operator.
//!
//! The discrete Helmholtz operator `(∇² + k²)` is **indefinite** — its
//! spectrum straddles zero, and it is exactly singular at every resonance.
//! Relaxation methods (SOR, the electrostatic Poisson workhorse next door
//! in `vcad-kernel-particle`) diverge on it. So this crate solves directly.
//!
//! Two pieces:
//!
//! - [`Lu`] — dense complex LU with partial pivoting, the robust kernel for
//!   the small per-slab blocks.
//! - [`solve_block_tridiag`] — block-Thomas for the block-tridiagonal system
//!   the axisymmetric 5-point stencil produces: `n_blocks` slabs of
//!   `block_size` nodes, dense diagonal blocks, **diagonal** off-diagonal
//!   coupling (the axial flux couples node `i` only to node `i` in the
//!   neighbouring slab). Cost `O(n_blocks · block_size³)`, storage
//!   `O(n_blocks · block_size²)`. A direct solve of the *symmetric* assembled
//!   matrix, so field reciprocity holds to round-off.

use crate::complex::Cplx;

/// A dense complex LU factorization with partial pivoting.
#[derive(Debug, Clone)]
pub struct Lu {
    /// Combined L\U factors, row-major `n×n`.
    lu: Vec<Cplx>,
    /// Row permutation (pivot for each elimination step).
    piv: Vec<usize>,
    /// Dimension.
    n: usize,
}

/// A near-zero pivot: the block is numerically singular (a resonance sits on
/// this frequency). Fail-closed — the caller reports it rather than returning
/// a garbage field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Singular;

impl Lu {
    /// Factor the dense row-major `n×n` matrix `a` in place-ish (a copy is
    /// taken). Returns [`Singular`] if any pivot underflows.
    pub fn factor(a: &[Cplx], n: usize) -> Result<Self, Singular> {
        assert_eq!(a.len(), n * n);
        let mut lu = a.to_vec();
        let mut piv = vec![0usize; n];
        // Pivot scale: the largest magnitude in the matrix sets the floor
        // for "this pivot is effectively zero".
        let amax = lu.iter().map(|z| z.abs()).fold(0.0_f64, f64::max);
        let floor = 1e-14 * amax.max(1.0);
        for k in 0..n {
            // Partial pivot: largest |a[i][k]| for i ≥ k.
            let mut p = k;
            let mut best = lu[k * n + k].abs();
            for i in (k + 1)..n {
                let v = lu[i * n + k].abs();
                if v > best {
                    best = v;
                    p = i;
                }
            }
            if best <= floor {
                return Err(Singular);
            }
            piv[k] = p;
            if p != k {
                for j in 0..n {
                    lu.swap(k * n + j, p * n + j);
                }
            }
            let akk = lu[k * n + k];
            for i in (k + 1)..n {
                let f = lu[i * n + k] / akk;
                lu[i * n + k] = f;
                for j in (k + 1)..n {
                    let t = f * lu[k * n + j];
                    lu[i * n + j] -= t;
                }
            }
        }
        Ok(Self { lu, piv, n })
    }

    /// Solve `A x = b` for the factored `A`. `b` is consumed length-`n`.
    pub fn solve(&self, b: &[Cplx]) -> Vec<Cplx> {
        let n = self.n;
        assert_eq!(b.len(), n);
        let mut x = b.to_vec();
        // Apply the row permutation.
        for k in 0..n {
            x.swap(k, self.piv[k]);
        }
        // Forward substitution (unit lower L).
        for i in 0..n {
            let mut s = x[i];
            for (j, &xj) in x[..i].iter().enumerate() {
                s -= self.lu[i * n + j] * xj;
            }
            x[i] = s;
        }
        // Back substitution (upper U).
        for i in (0..n).rev() {
            let mut s = x[i];
            for (j, &xj) in x.iter().enumerate().skip(i + 1) {
                s -= self.lu[i * n + j] * xj;
            }
            x[i] = s / self.lu[i * n + i];
        }
        x
    }
}

/// Solve a block-tridiagonal complex system by block elimination.
///
/// - `diag[j]` is the `bs×bs` row-major diagonal block of slab `j`.
/// - `lower[j]` (length `bs`) is the **diagonal** coupling from slab `j` to
///   slab `j−1`; `lower[0]` is ignored.
/// - `upper[j]` (length `bs`) is the diagonal coupling from slab `j` to slab
///   `j+1`; `upper[nb−1]` is ignored.
/// - `rhs` is length `nb·bs`, slab-major (`j·bs + i`).
///
/// Returns the solution in the same layout, or [`Singular`] if any reduced
/// block is singular (a resonance sits on this frequency).
pub fn solve_block_tridiag(
    nb: usize,
    bs: usize,
    diag: &[Vec<Cplx>],
    lower: &[Vec<Cplx>],
    upper: &[Vec<Cplx>],
    rhs: &[Cplx],
) -> Result<Vec<Cplx>, Singular> {
    assert_eq!(diag.len(), nb);
    assert_eq!(lower.len(), nb);
    assert_eq!(upper.len(), nb);
    assert_eq!(rhs.len(), nb * bs);

    // Reduced diagonal blocks, factored, plus the modified RHS.
    let mut fact: Vec<Lu> = Vec::with_capacity(nb);
    let mut rmod: Vec<Vec<Cplx>> = Vec::with_capacity(nb);

    // Slab 0.
    fact.push(Lu::factor(&diag[0], bs)?);
    rmod.push(rhs[0..bs].to_vec());

    for j in 1..nb {
        // X = (D'_{j-1})^{-1} · diag(upper[j-1]): solve column by column.
        // Column q of diag(upper[j-1]) is upper[j-1][q]·e_q, so
        // X[:,q] = upper[j-1][q] · (D'_{j-1})^{-1} e_q.
        let prev = &fact[j - 1];
        // D'_j = D_j − diag(lower[j]) · X.
        let mut dprime = diag[j].clone();
        let mut ecol = vec![Cplx::ZERO; bs];
        for q in 0..bs {
            let scale = upper[j - 1][q];
            if scale == Cplx::ZERO {
                continue;
            }
            for e in ecol.iter_mut() {
                *e = Cplx::ZERO;
            }
            ecol[q] = Cplx::ONE;
            let col = prev.solve(&ecol); // (D'_{j-1})^{-1} e_q
            for p in 0..bs {
                // subtract lower[j][p] · scale · col[p] from D'_j[p][q]
                let t = lower[j][p] * (scale * col[p]);
                dprime[p * bs + q] -= t;
            }
        }
        // RHS: b'_j = b_j − diag(lower[j]) · (D'_{j-1})^{-1} b'_{j-1}.
        let y = prev.solve(&rmod[j - 1]);
        let mut bj = rhs[j * bs..(j + 1) * bs].to_vec();
        for p in 0..bs {
            bj[p] -= lower[j][p] * y[p];
        }
        fact.push(Lu::factor(&dprime, bs)?);
        rmod.push(bj);
    }

    // Back substitution.
    let mut x = vec![Cplx::ZERO; nb * bs];
    let last = nb - 1;
    let xl = fact[last].solve(&rmod[last]);
    x[last * bs..(last + 1) * bs].copy_from_slice(&xl);
    for j in (0..last).rev() {
        // x_j = (D'_j)^{-1} ( b'_j − diag(upper[j]) · x_{j+1} ).
        let mut r = rmod[j].clone();
        for p in 0..bs {
            r[p] -= upper[j][p] * x[(j + 1) * bs + p];
        }
        let xj = fact[j].solve(&r);
        x[j * bs..(j + 1) * bs].copy_from_slice(&xj);
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(re: f64, im: f64) -> Cplx {
        Cplx::new(re, im)
    }

    #[test]
    fn dense_lu_solves_a_known_complex_system() {
        // A = [[2, j], [1, 3]], b = [1+0j, 0+1j]; solve and multiply back.
        let a = vec![cx(2.0, 0.0), cx(0.0, 1.0), cx(1.0, 0.0), cx(3.0, 0.0)];
        let lu = Lu::factor(&a, 2).unwrap();
        let b = vec![cx(1.0, 0.0), cx(0.0, 1.0)];
        let x = lu.solve(&b);
        // Residual A x − b.
        let r0 = a[0] * x[0] + a[1] * x[1] - b[0];
        let r1 = a[2] * x[0] + a[3] * x[1] - b[1];
        assert!(
            r0.abs() < 1e-12 && r1.abs() < 1e-12,
            "residual {r0:?} {r1:?}"
        );
    }

    #[test]
    fn dense_lu_needs_pivoting() {
        // Zero leading pivot forces a row swap.
        let a = vec![cx(0.0, 0.0), cx(1.0, 0.0), cx(1.0, 0.0), cx(1.0, 0.0)];
        let lu = Lu::factor(&a, 2).unwrap();
        let b = vec![cx(2.0, 0.0), cx(3.0, 0.0)];
        let x = lu.solve(&b);
        // A x = b: [x1, x0+x1] = [2,3] → x1=2, x0=1.
        assert!((x[0] - cx(1.0, 0.0)).abs() < 1e-12);
        assert!((x[1] - cx(2.0, 0.0)).abs() < 1e-12);
    }

    #[test]
    fn singular_matrix_is_rejected() {
        let a = vec![cx(1.0, 0.0), cx(2.0, 0.0), cx(2.0, 0.0), cx(4.0, 0.0)];
        assert!(matches!(Lu::factor(&a, 2), Err(Singular)));
    }

    #[test]
    fn block_tridiag_matches_a_dense_reference() {
        // 3 blocks of size 2. Build a symmetric-structure block-tridiagonal
        // system, solve it both ways, compare.
        let bs = 2;
        let nb = 3;
        let d = |a, b, c, d| vec![cx(a, 0.0), cx(b, 0.0), cx(b, 0.0), cx(c, 0.0), cx(d, 0.0)];
        let _ = d; // (unused helper placeholder)
        let diag = vec![
            vec![cx(6.0, 0.0), cx(-1.0, 0.0), cx(-1.0, 0.0), cx(6.0, 0.0)],
            vec![cx(7.0, 0.0), cx(-1.0, 0.0), cx(-1.0, 0.0), cx(7.0, 0.0)],
            vec![cx(6.0, 0.0), cx(-1.0, 0.0), cx(-1.0, 0.0), cx(6.0, 0.0)],
        ];
        // Diagonal axial coupling of −2 between consecutive slabs.
        let upper = vec![
            vec![cx(-2.0, 0.0), cx(-2.0, 0.0)],
            vec![cx(-2.0, 0.0), cx(-2.0, 0.0)],
            vec![cx(0.0, 0.0), cx(0.0, 0.0)],
        ];
        let lower = vec![
            vec![cx(0.0, 0.0), cx(0.0, 0.0)],
            vec![cx(-2.0, 0.0), cx(-2.0, 0.0)],
            vec![cx(-2.0, 0.0), cx(-2.0, 0.0)],
        ];
        let rhs: Vec<Cplx> = (0..nb * bs).map(|i| cx(i as f64 + 1.0, 0.0)).collect();
        let x = solve_block_tridiag(nb, bs, &diag, &lower, &upper, &rhs).unwrap();

        // Assemble the dense equivalent and solve with the dense LU.
        let n = nb * bs;
        let mut a = vec![Cplx::ZERO; n * n];
        for j in 0..nb {
            for p in 0..bs {
                for q in 0..bs {
                    a[(j * bs + p) * n + (j * bs + q)] = diag[j][p * bs + q];
                }
            }
            if j + 1 < nb {
                for p in 0..bs {
                    a[(j * bs + p) * n + ((j + 1) * bs + p)] = upper[j][p];
                    a[((j + 1) * bs + p) * n + (j * bs + p)] = lower[j + 1][p];
                }
            }
        }
        let xd = Lu::factor(&a, n).unwrap().solve(&rhs);
        for i in 0..n {
            assert!((x[i] - xd[i]).abs() < 1e-10, "block vs dense at {i}");
        }
    }
}
