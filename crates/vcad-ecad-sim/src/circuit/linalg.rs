//! Tiny dense linear solver (Gaussian elimination with partial pivoting).
//!
//! Circuit MNA systems are small (tens of unknowns for typical boards), so a
//! dense `O(n³)` solve is plenty and keeps the crate dependency-free and
//! WASM-friendly. If we ever need large sparse systems, this is the seam to
//! swap in a sparse factorization.

/// Solve `a · x = b` in place via Gaussian elimination with partial pivoting.
///
/// `a` is a row-major `n×n` matrix, `b` is length `n`. Both are consumed
/// (overwritten). Returns the solution `x`, or `None` if the system is singular
/// (a pivot falls below a small epsilon).
pub fn solve_dense(a: &mut [f64], b: &mut [f64], n: usize) -> Option<Vec<f64>> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Some(Vec::new());
    }

    for col in 0..n {
        // Partial pivot: largest-magnitude entry in this column, at/below the diagonal.
        let mut pivot_row = col;
        let mut pivot_mag = a[col * n + col].abs();
        for r in (col + 1)..n {
            let mag = a[r * n + col].abs();
            if mag > pivot_mag {
                pivot_mag = mag;
                pivot_row = r;
            }
        }
        if pivot_mag < 1e-14 {
            return None; // singular / structurally disconnected
        }
        if pivot_row != col {
            for c in 0..n {
                a.swap(pivot_row * n + c, col * n + c);
            }
            b.swap(pivot_row, col);
        }

        // Eliminate entries below the pivot.
        let pivot = a[col * n + col];
        for r in (col + 1)..n {
            let factor = a[r * n + col] / pivot;
            if factor != 0.0 {
                for c in col..n {
                    a[r * n + c] -= factor * a[col * n + c];
                }
                b[r] -= factor * b[col];
            }
        }
    }

    // Back-substitution.
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut sum = b[r];
        for c in (r + 1)..n {
            sum -= a[r * n + c] * x[c];
        }
        x[r] = sum / a[r * n + r];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_identity() {
        let mut a = vec![1.0, 0.0, 0.0, 1.0];
        let mut b = vec![3.0, 4.0];
        let x = solve_dense(&mut a, &mut b, 2).unwrap();
        assert!((x[0] - 3.0).abs() < 1e-12);
        assert!((x[1] - 4.0).abs() < 1e-12);
    }

    #[test]
    fn solves_2x2() {
        // [2 1; 1 3] x = [5; 10]  ->  x = [1; 3]
        let mut a = vec![2.0, 1.0, 1.0, 3.0];
        let mut b = vec![5.0, 10.0];
        let x = solve_dense(&mut a, &mut b, 2).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-9, "x0 = {}", x[0]);
        assert!((x[1] - 3.0).abs() < 1e-9, "x1 = {}", x[1]);
    }

    #[test]
    fn needs_pivoting() {
        // Zero pivot in the first position forces a row swap.
        // [0 1; 1 0] x = [2; 3] -> x = [3; 2]
        let mut a = vec![0.0, 1.0, 1.0, 0.0];
        let mut b = vec![2.0, 3.0];
        let x = solve_dense(&mut a, &mut b, 2).unwrap();
        assert!((x[0] - 3.0).abs() < 1e-9);
        assert!((x[1] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn singular_returns_none() {
        // Rank-deficient.
        let mut a = vec![1.0, 2.0, 2.0, 4.0];
        let mut b = vec![1.0, 2.0];
        assert!(solve_dense(&mut a, &mut b, 2).is_none());
    }

    #[test]
    fn solves_3x3() {
        // [1 1 1; 0 2 5; 2 5 -1] x = [6; -4; 27] -> x = [5; 3; -2]
        let mut a = vec![1.0, 1.0, 1.0, 0.0, 2.0, 5.0, 2.0, 5.0, -1.0];
        let mut b = vec![6.0, -4.0, 27.0];
        let x = solve_dense(&mut a, &mut b, 3).unwrap();
        assert!((x[0] - 5.0).abs() < 1e-9, "x0={}", x[0]);
        assert!((x[1] - 3.0).abs() < 1e-9, "x1={}", x[1]);
        assert!((x[2] + 2.0).abs() < 1e-9, "x2={}", x[2]);
    }
}
