//! Sequential convex optimization over compiled claims ("tang-route" M2).
//!
//! TrajOpt-style replacement for the Adam loop in [`super::descent`]: the
//! claim set is compiled ONCE into a residual vector `ρ(θ)` (each row the
//! least-squares form of one claim, so `Σ ρ² =` the descent loss) plus its
//! sparse Jacobian — only the partials a row actually depends on are
//! differentiated, found by a dependency walk over the graph
//! ([`super::claims_ir::var_deps`]). Both are emitted as one
//! `tang_expr::compile_many` closure, so each step is a single straight-line
//! evaluation instead of an interpreted graph walk.
//!
//! Each inner step solves the damped Gauss-Newton (Levenberg-Marquardt)
//! subproblem `(JᵀJ + λ·diag) δ = −Jᵀρ`, clipped to a trust radius, and is
//! accepted only if the TRUE merit falls (ratio test against the model's
//! predicted decrease; the radius grows on good agreement and shrinks on
//! bad). The outer loop then judges every HARD claim with its exact
//! predicate and updates an augmented-Lagrangian shift `σ_i` (an input of
//! the compiled residual, `hinge(r + σ)`) plus grows the penalty scale of
//! each violated one, until all hold or the budget is spent. A plain
//! quadratic penalty is only feasible in the limit; the shift makes the
//! exact predicate reachable at finite weight (the fixed point sits
//! `1e-6` inside the boundary). The final shifts are multiplier estimates —
//! the input M4 would turn into history costs.

use tang_expr::compile::CompiledMany;
use tang_expr::{ExprGraph, ExprId};

use super::claims_ir::{var_deps, ClaimSet, Penalty, SparseDiff};

/// Solver knobs.
#[derive(Debug, Clone)]
pub struct ScoOptions {
    /// Max inner (LM) iterations summed over all outer rounds.
    pub max_iters: usize,
    /// Max penalty-escalation rounds.
    pub max_outer: usize,
    /// Factor applied to a violated hard claim's residual scale per round
    /// (its loss weight grows by the square).
    pub penalty_growth: f64,
    /// Initial trust radius (θ units, mm).
    pub init_radius: f64,
}

impl Default for ScoOptions {
    fn default() -> Self {
        Self {
            max_iters: 300,
            max_outer: 10,
            penalty_growth: 2.0,
            init_radius: 0.5,
        }
    }
}

/// What a solve did.
#[derive(Debug, Clone, Default)]
pub struct ScoReport {
    /// Inner iterations (accepted + rejected steps).
    pub iters: usize,
    /// Outer rounds run.
    pub outer: usize,
    /// Final merit (with escalated penalty scales).
    pub merit: f64,
    /// Every hard claim's exact predicate holds at the result.
    pub hard_ok: bool,
}

/// A claim set compiled to residuals + sparse Jacobian.
pub struct ResidualSystem {
    n_vars: usize,
    rows: usize,
    /// Outputs: `rows` residuals, then one value per Jacobian nonzero.
    eval: CompiledMany,
    /// Raw claim residuals (the exact predicates), one per row.
    raw: CompiledMany,
    /// (row, col) of each Jacobian nonzero, in output order.
    pattern: Vec<(usize, usize)>,
    hard: Vec<bool>,
    /// Input slot of each row's AL shift (`None` for soft rows).
    shift_slot: Vec<Option<usize>>,
    /// Linear-objective rows (wirelength) are NOT Gauss-Newton rows: a
    /// length is all second-order (straightening tension lives in its
    /// Hessian, which GN would drop — that stalled LM at a 0.03 mm trust
    /// radius on the long fixture). They contribute `w·(∇L, ∇²L)` exactly:
    /// (row, col) gradient and (row, i, j) Hessian entries, output order.
    lin_grad: Vec<(usize, usize)>,
    lin_hess: Vec<(usize, usize, usize)>,
    /// Per-row linear weight (`0` for GN rows).
    lin_weight: Vec<f64>,
}

impl ResidualSystem {
    /// Compile `claims` (over `n_vars` variables) into a residual system.
    /// Appends derivative nodes to `g`.
    pub fn compile(g: &mut ExprGraph, claims: &ClaimSet, n_vars: usize) -> Self {
        // Hard claims get an AL shift input appended after θ — when the var
        // id space (u16) has room; otherwise they fall back to pure penalty.
        let n_hard = claims.claims.iter().filter(|c| c.hard).count();
        let use_shift = n_vars + n_hard <= u16::MAX as usize;
        let mut shift_slot = Vec::with_capacity(claims.claims.len());
        let mut next = n_vars;
        let rho: Vec<ExprId> = claims
            .claims
            .iter()
            .map(|c| {
                if c.hard && use_shift {
                    let s = g.var(next as u16);
                    shift_slot.push(Some(next));
                    next += 1;
                    ClaimSet::gn_residual_shifted(g, c, s)
                } else {
                    shift_slot.push(None);
                    ClaimSet::gn_residual(g, c)
                }
            })
            .collect();
        let deps = var_deps(g, &rho);
        let mut sd = SparseDiff::new();
        let lin_weight: Vec<f64> = claims
            .claims
            .iter()
            .map(|c| match c.penalty {
                Penalty::Linear => c.weight,
                Penalty::SquaredHinge => 0.0,
            })
            .collect();
        let mut outs: Vec<ExprId> = rho.clone();
        let mut pattern = Vec::new();
        for (row, (&r, vars)) in rho.iter().zip(&deps).enumerate() {
            if lin_weight[row] != 0.0 {
                continue;
            }
            for &v in vars {
                if (v as usize) < n_vars {
                    outs.push(sd.diff(g, r, v));
                    pattern.push((row, v as usize));
                }
            }
        }
        let mut lin_grad = Vec::new();
        let mut lin_hess = Vec::new();
        let mut hess_outs = Vec::new();
        for (row, c) in claims.claims.iter().enumerate() {
            if lin_weight[row] == 0.0 {
                continue;
            }
            for &v in &deps[row] {
                if (v as usize) >= n_vars {
                    continue;
                }
                let d = sd.diff(g, c.residual, v);
                outs.push(d);
                lin_grad.push((row, v as usize));
                // ∂L/∂θ_v depends only on v's neighbours: sparse Hessian.
                let dd = var_deps(g, &[d]).pop().unwrap_or_default();
                for u in dd {
                    if (u as usize) < n_vars {
                        hess_outs.push(sd.diff(g, d, u));
                        lin_hess.push((row, v as usize, u as usize));
                    }
                }
            }
        }
        outs.extend(hess_outs);
        let raw_ids: Vec<ExprId> = claims.claims.iter().map(|c| c.residual).collect();
        Self {
            n_vars,
            rows: rho.len(),
            eval: g.compile_many(&outs),
            raw: g.compile_many(&raw_ids),
            pattern,
            hard: claims.claims.iter().map(|c| c.hard).collect(),
            shift_slot,
            lin_grad,
            lin_hess,
            lin_weight,
        }
    }

    /// Number of Jacobian nonzeros.
    pub fn nnz(&self) -> usize {
        self.pattern.len()
    }

    fn residuals(&self, theta: &[f64], buf: &mut [f64]) {
        (self.eval)(theta, buf);
    }

    /// Merit `Σ (s_i ρ_i)²` from an evaluation buffer.
    fn merit(&self, buf: &[f64], scale: &[f64]) -> f64 {
        (0..self.rows).map(|i| (scale[i] * buf[i]).powi(2)).sum()
    }

    /// Raw claim residuals (exact predicates: claim holds iff ≤ 0).
    fn raw_residuals(&self, x: &[f64]) -> Vec<f64> {
        let mut r = vec![0.0; self.rows];
        (self.raw)(x, &mut r);
        r
    }

    /// Run SCO from `theta0`.
    pub fn solve(&self, theta0: &[f64], opts: &ScoOptions) -> (Vec<f64>, ScoReport) {
        let n = self.n_vars;
        let n_in = n + self.shift_slot.iter().flatten().count();
        // Solver input: θ followed by the AL shifts (start at zero).
        let mut theta = theta0.to_vec();
        theta.resize(n_in, 0.0);
        let mut scale = vec![1.0; self.rows];
        let mut buf =
            vec![0.0; self.rows + self.pattern.len() + self.lin_grad.len() + self.lin_hess.len()];
        let mut trial = buf.clone();
        let mut report = ScoReport::default();
        let mut radius = opts.init_radius;
        let mut lambda = 1e-3;

        for outer in 0..opts.max_outer.max(1) {
            report.outer = outer + 1;
            self.residuals(&theta, &mut buf);
            let mut f = self.merit(&buf, &scale);
            let mut stall = 0;
            while report.iters < opts.max_iters {
                report.iters += 1;
                // Normal equations from the sparse Jacobian.
                let mut h = vec![0.0; n * n];
                let mut grad = vec![0.0; n];
                let jv = &buf[self.rows..];
                // Group nonzeros by row (pattern is row-major).
                let mut k = 0;
                while k < self.pattern.len() {
                    let row = self.pattern[k].0;
                    let mut end = k;
                    while end < self.pattern.len() && self.pattern[end].0 == row {
                        end += 1;
                    }
                    let s = scale[row];
                    let r = s * buf[row];
                    for a in k..end {
                        let (ca, va) = (self.pattern[a].1, s * jv[a]);
                        if va == 0.0 {
                            continue;
                        }
                        grad[ca] += va * r;
                        for (&(_, cb), &jb) in self.pattern[k..end].iter().zip(&jv[k..end]) {
                            let vb = s * jb;
                            h[ca * n + cb] += va * vb;
                        }
                    }
                    k = end;
                }
                // Linear objectives, exact second order (half-scaled to
                // match the GN normal equations: grad = ½∇f, h = ½∇²f).
                let off = self.rows + self.pattern.len();
                for (k, &(row, c)) in self.lin_grad.iter().enumerate() {
                    grad[c] += 0.5 * self.lin_weight[row] * buf[off + k];
                }
                let off = off + self.lin_grad.len();
                for (k, &(row, i, j)) in self.lin_hess.iter().enumerate() {
                    h[i * n + j] += 0.5 * self.lin_weight[row] * buf[off + k];
                }
                let gnorm = grad.iter().map(|g| g * g).sum::<f64>().sqrt();
                if gnorm < 1e-10 {
                    break;
                }
                // Trust region by damping, not clipping: raise λ until the
                // step fits the radius. (Clipping a near-singular GN step
                // keeps its direction — dominated by the flat "slide along
                // the leg" null space — and stalled the long fixture at a
                // 0.03 mm radius.) Clipping remains only as a last resort.
                let neg: Vec<f64> = grad.iter().map(|g| -g).collect();
                let mut solved = None;
                for _ in 0..40 {
                    let mut a = h.clone();
                    for i in 0..n {
                        a[i * n + i] += lambda * (h[i * n + i] + 1.0);
                    }
                    match cholesky_solve(&mut a, &neg, n) {
                        Some(d) => {
                            let dn = d.iter().map(|x| x * x).sum::<f64>().sqrt();
                            solved = Some((d, dn));
                            if dn <= radius {
                                break;
                            }
                            lambda *= 3.0;
                        }
                        None => lambda *= 10.0,
                    }
                }
                let Some((mut delta, dn)) = solved else {
                    break;
                };
                if dn > radius {
                    let c = radius / dn;
                    delta.iter_mut().for_each(|d| *d *= c);
                }
                // Model decrease: f − ‖r + Jδ‖² = −2gᵀδ − δᵀHδ.
                let mut hd = 0.0;
                for i in 0..n {
                    let mut row = 0.0;
                    for j in 0..n {
                        row += h[i * n + j] * delta[j];
                    }
                    hd += delta[i] * row;
                }
                let gd: f64 = grad.iter().zip(&delta).map(|(g, d)| g * d).sum();
                let pred = -2.0 * gd - hd;
                let mut cand = theta.clone();
                for (c, d) in cand.iter_mut().zip(&delta) {
                    *c += d;
                }
                self.residuals(&cand, &mut trial);
                let f_new = self.merit(&trial, &scale);
                let ratio = if pred > 0.0 { (f - f_new) / pred } else { -1.0 };
                let step = dn.min(radius);
                if f_new < f && ratio > 1e-4 {
                    let rel = (f - f_new) / f.max(1e-300);
                    theta = cand;
                    std::mem::swap(&mut buf, &mut trial);
                    f = f_new;
                    if ratio > 0.75 {
                        radius = (2.0 * step).max(radius);
                        lambda = (lambda / 3.0).max(1e-9);
                    } else if ratio < 0.25 {
                        radius *= 0.5;
                        lambda *= 2.0;
                    }
                    stall = if rel < 1e-9 { stall + 1 } else { 0 };
                    if stall >= 3 || step < 1e-9 {
                        break;
                    }
                } else {
                    radius *= 0.25;
                    lambda *= 4.0;
                    if radius < 1e-9 {
                        break;
                    }
                }
            }
            report.merit = f;
            let raw = self.raw_residuals(&theta);
            let bad: Vec<usize> = (0..self.rows)
                .filter(|&i| self.hard[i] && raw[i] > 0.0)
                .collect();
            report.hard_ok = bad.is_empty();
            if bad.is_empty() || report.iters >= opts.max_iters {
                break;
            }
            // Multiplier update σ ← max(0, σ + r + 1e-6) on every hard row
            // (inactive rows relax back toward zero), penalty growth on the
            // violated ones.
            for (slot, r) in self.shift_slot.iter().zip(&raw) {
                if let Some(slot) = *slot {
                    theta[slot] = (theta[slot] + r + 1e-6).max(0.0);
                }
            }
            // Grow the penalty on violated rows, rescaling the shift so the
            // multiplier estimate λ = 2·w·s²·σ is preserved.
            for i in bad {
                scale[i] *= opts.penalty_growth;
                if let Some(slot) = self.shift_slot[i] {
                    theta[slot] /= opts.penalty_growth * opts.penalty_growth;
                }
            }
            // Fresh trust region for the reshaped merit.
            radius = radius.max(opts.init_radius * 0.25);
            lambda = lambda.max(1e-3);
        }
        theta.truncate(n);
        (theta, report)
    }
}

/// In-place Cholesky solve of SPD `a` (n×n row-major); `None` if not SPD.
fn cholesky_solve(a: &mut [f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 || !d.is_finite() {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    let mut y = b.to_vec();
    for i in 0..n {
        for k in 0..i {
            y[i] -= a[i * n + k] * y[k];
        }
        y[i] /= a[i * n + i];
    }
    for i in (0..n).rev() {
        for k in i + 1..n {
            y[i] -= a[k * n + i] * y[k];
        }
        y[i] /= a[i * n + i];
    }
    Some(y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::claims_ir as cir;

    /// LM on a pure least-squares problem with a hard clearance claim: the
    /// unconstrained optimum violates the claim; the outer loop escalates
    /// until the exact predicate holds.
    #[test]
    fn sco_escalates_until_hard_claims_hold() {
        let mut g = ExprGraph::new();
        let p = (g.var(0), g.var(1));
        let mut cs = ClaimSet::new();
        // Soft pull toward (5, 0): a gap claim to a fixed anchor, target 0.
        let anchor = (g.lit(5.0), g.lit(0.0));
        cs.push(cir::gap(&mut g, p, anchor, 0.0, 0.0, 1.0));
        // Hard: stay 0.5 from the segment y = 0.2 (x in 0..10).
        cs.push(cir::clearance(&mut g, p, (0.0, 0.2), (10.0, 0.2), 0.5, 1.0));
        let sys = ResidualSystem::compile(&mut g, &cs, 2);
        let (th, rep) = sys.solve(&[5.0, -2.0], &ScoOptions::default());
        assert!(rep.hard_ok, "{rep:?} at {th:?}");
        assert!(th[1] <= -0.3 + 1e-9 && th[1] > -0.31, "{th:?}");
        assert!((th[0] - 5.0).abs() < 1e-3);
    }

    #[test]
    fn cholesky_solves_spd() {
        let mut a = vec![4.0, 1.0, 1.0, 3.0];
        let x = cholesky_solve(&mut a, &[1.0, 2.0], 2).unwrap();
        assert!((4.0 * x[0] + x[1] - 1.0).abs() < 1e-12);
        assert!((x[0] + 3.0 * x[1] - 2.0).abs() < 1e-12);
    }
}
