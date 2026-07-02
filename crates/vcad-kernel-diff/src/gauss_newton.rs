//! M10 — Gauss–Newton curvature for least-squares QoI objectives.
//!
//! For an objective assembled as a sum of squared residuals
//!
//! ```text
//! J(θ) = Σ_q r_q(θ)²
//! ```
//!
//! (the shape every mass-property recovery objective in this crate takes —
//! `VolumeMatch`, the M9 five-QoI `QoiMatch`, each residual a *relative* miss
//! `r_q = (Q_q − t_q)/t_q`), the exact gradient and Hessian are
//!
//! ```text
//! g   = 2 Jᵀ r
//! H   = 2 Jᵀ J  +  2 Σ_q r_q ∇²r_q ,     J = ∂r/∂θ  (m residuals × n params)
//! ```
//!
//! The **Gauss–Newton** approximation keeps only the first Hessian term,
//! `H_GN = 2 JᵀJ`. That term needs nothing but the residual Jacobian `J` —
//! which the differentiable seam already produces exactly: each row
//! `∂r_q/∂θ` is `(1/t_q)` times the QoI derivative the seam computes in
//! forward mode (one seam pass per parameter, [`crate::objective_gradient`])
//! or reverse mode (one pullback per residual QoI,
//! [`crate::evaluate_with_pullback`]). No node accelerations, no second-order
//! seeding — the curvature that *is* recoverable from first derivatives
//! alone, priced honestly.
//!
//! # What these functions compute, exactly
//!
//! They compute `H_GN = 2 JᵀJ` and products with it. They **drop** the
//! residual-curvature term `2 Σ_q r_q ∇²r_q`. The two agree exactly when
//! every residual is zero (at a perfect fit) and to `O(‖r‖)` near it, so the
//! Gauss–Newton Hessian is the trusted curvature model at and near an
//! optimum — the regime a Newton-type step is taken in — and is a positive
//! semidefinite lower model everywhere else. It is **not** the full Hessian
//! of `J`; a caller wanting the exact second derivative of a *single* QoI (as
//! opposed to the least-squares assembly) wants
//! [`crate::volume_with_second_derivative`], which carries the dropped
//! node-acceleration term.

/// The Gauss–Newton Hessian `H_GN = 2 JᵀJ` of a least-squares objective
/// `J = Σ_q r_q²`, formed explicitly (`n × n`, row-major).
///
/// `residual_jacobian` is `J = ∂r/∂θ`: one row per residual `r_q`, each of
/// length `n` (the number of parameters). Every row must have the same
/// length; the result is `n × n` symmetric.
///
/// Exact-at-zero-residual: this is the full Hessian of `J` only where all
/// residuals vanish (see the module docs). Prefer [`gauss_newton_hvp`] when
/// `n` is large and only Hessian–vector products are needed.
pub fn gauss_newton_hessian(residual_jacobian: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = residual_jacobian.first().map(Vec::len).unwrap_or(0);
    debug_assert!(
        residual_jacobian.iter().all(|row| row.len() == n),
        "every residual-Jacobian row must have the same parameter count"
    );
    let mut h = vec![vec![0.0; n]; n];
    for row in residual_jacobian {
        // Rank-1 update H += 2 · (row ⊗ row); the factor 2 is the derivative
        // of the square in J = Σ r².
        for i in 0..n {
            let ri = row[i];
            if ri == 0.0 {
                continue;
            }
            let hi = &mut h[i];
            for (j, &rj) in row.iter().enumerate() {
                hi[j] += 2.0 * ri * rj;
            }
        }
    }
    h
}

/// A Gauss–Newton Hessian–vector product `H_GN · v = 2 Jᵀ(Jv)`, formed
/// **without** materializing the `n × n` Hessian.
///
/// `residual_jacobian` is `J = ∂r/∂θ` (one row per residual). `v` has length
/// `n` (the parameter count). Cost is `O(m·n)` — one pass to form `Jv`
/// (length `m`), one to accumulate `Jᵀ(Jv)` — so this is the form to use in a
/// truncated-Newton / conjugate-gradient inner loop where `H_GN` is only ever
/// touched through products.
///
/// Computes exactly the same operator as [`gauss_newton_hessian`] applied to
/// `v`; same exact-at-zero-residual contract.
pub fn gauss_newton_hvp(residual_jacobian: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    let n = v.len();
    // Jv: one entry per residual.
    let jv: Vec<f64> = residual_jacobian
        .iter()
        .map(|row| {
            debug_assert_eq!(row.len(), n, "Jacobian row / vector length mismatch");
            row.iter().zip(v).map(|(a, b)| a * b).sum::<f64>()
        })
        .collect();
    // 2 Jᵀ(Jv).
    let mut out = vec![0.0; n];
    for (row, &s) in residual_jacobian.iter().zip(&jv) {
        let two_s = 2.0 * s;
        if two_s == 0.0 {
            continue;
        }
        for (o, &a) in out.iter_mut().zip(row) {
            *o += two_s * a;
        }
    }
    out
}

/// The exact least-squares gradient `g = 2 Jᵀ r` of `J = Σ_q r_q²`.
///
/// Provided alongside the curvature so a caller assembling a Gauss–Newton
/// step `H_GN Δ = −g` has both halves from the same convention (`J = ∂r/∂θ`,
/// `r` the residual vector). This gradient is **exact** — no approximation —
/// unlike the Hessian; it is here only so the factor-of-two bookkeeping lives
/// in one place.
pub fn gauss_newton_gradient(residual_jacobian: &[Vec<f64>], residual: &[f64]) -> Vec<f64> {
    debug_assert_eq!(
        residual_jacobian.len(),
        residual.len(),
        "one residual per Jacobian row"
    );
    let n = residual_jacobian.first().map(Vec::len).unwrap_or(0);
    let mut g = vec![0.0; n];
    for (row, &r) in residual_jacobian.iter().zip(residual) {
        let two_r = 2.0 * r;
        if two_r == 0.0 {
            continue;
        }
        for (gi, &a) in g.iter_mut().zip(row) {
            *gi += two_r * a;
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `H_GN v` via the matrix-free product must equal the explicit-Hessian
    /// multiply for a hand Jacobian.
    #[test]
    fn hvp_matches_explicit_hessian() {
        let jac = vec![
            vec![1.0, 2.0, -1.0],
            vec![0.0, 1.0, 3.0],
            vec![2.0, -1.0, 0.5],
        ];
        let h = gauss_newton_hessian(&jac);
        // Symmetry.
        for (i, row) in h.iter().enumerate() {
            for (j, &hij) in row.iter().enumerate() {
                assert!((hij - h[j][i]).abs() < 1e-14);
            }
        }
        for v in [[1.0, 0.0, 0.0], [1.0, -2.0, 3.0], [0.5, 0.5, 0.5]] {
            let hv_free = gauss_newton_hvp(&jac, &v);
            let hv_mat: Vec<f64> = (0..3)
                .map(|i| (0..3).map(|j| h[i][j] * v[j]).sum())
                .collect();
            for k in 0..3 {
                assert!(
                    (hv_free[k] - hv_mat[k]).abs() < 1e-12,
                    "row {k}: {} vs {}",
                    hv_free[k],
                    hv_mat[k]
                );
            }
        }
    }

    /// For a *linear* residual `r(θ) = Jθ − b`, every `∇²r_q = 0`, so the
    /// Gauss–Newton Hessian is the exact Hessian and one Newton step
    /// `H Δ = −g` lands on the least-squares minimizer.
    #[test]
    fn linear_residual_newton_step_is_exact() {
        // r = J θ − b, over-determined but consistent at θ* = (1, −1).
        let jmat = [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let theta_star = [1.0, -1.0];
        let b: Vec<f64> = jmat
            .iter()
            .map(|row| row[0] * theta_star[0] + row[1] * theta_star[1])
            .collect();

        let theta0 = [0.0, 0.0];
        let residual: Vec<f64> = jmat
            .iter()
            .zip(&b)
            .map(|(row, bi)| row[0] * theta0[0] + row[1] * theta0[1] - bi)
            .collect();
        let jac: Vec<Vec<f64>> = jmat.iter().map(|r| r.to_vec()).collect();

        let g = gauss_newton_gradient(&jac, &residual);
        let h = gauss_newton_hessian(&jac);
        // Solve H Δ = −g (2×2).
        let det = h[0][0] * h[1][1] - h[0][1] * h[1][0];
        let dx = (-g[0] * h[1][1] + g[1] * h[0][1]) / det;
        let dy = (-g[1] * h[0][0] + g[0] * h[1][0]) / det;
        let theta1 = [theta0[0] + dx, theta0[1] + dy];
        assert!((theta1[0] - theta_star[0]).abs() < 1e-12);
        assert!((theta1[1] - theta_star[1]).abs() < 1e-12);
    }
}
