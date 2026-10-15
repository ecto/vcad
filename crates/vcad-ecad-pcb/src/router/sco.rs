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
//!
//! # M7: hard claims as linearized constraints (SQP)
//!
//! [`ScoMethod::Sqp`] (the default) stops treating hard claims as penalty
//! rows. Each step linearizes every hard residual at the current iterate,
//! `r_i + a_iᵀδ ≤ 0`, and solves the TrajOpt-style trust-region subproblem
//!
//! ```text
//! min_δ  ½δᵀ(2A)δ + 2gᵀδ + μ Σ_i max(0, r_i + a_iᵀδ)      A = JᵀJ + λ·D
//! ```
//!
//! — soft claims stay Gauss-Newton least squares, hard ones enter as an
//! exact (ℓ1) penalty, which is the elastic form of the linear inequality
//! constraints (feasible whenever the linearization is, for μ above the
//! multipliers). The QP is solved in its dual, a box-constrained
//! `0 ≤ y ≤ μ` problem, by Hildreth's coordinate ascent over a working set
//! grown from the linearized constraints the step would violate — only the
//! working-set rows ever get an `A⁻¹a_i` solve, so a few thousand mostly
//! inactive clearance rows cost nothing. Steps are accepted on the TRUE
//! merit `Σ ρ_soft² + μ Σ max(0, r_hard)` against the model's predicted
//! decrease; μ grows ×10 when a dual sits on its bound with the linearized
//! constraint still violated. The penalty/AL loop above stays available as
//! [`ScoMethod::Lm`] for comparison.

use tang_expr::compile::CompiledMany;
use tang_expr::{ExprGraph, ExprId};

use super::claims_ir::{var_deps, ClaimSet, Penalty, SparseDiff};

/// How hard claims are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoMethod {
    /// Linearized inequality constraints in each trust-region subproblem,
    /// exact-penalty merit (M7). The default.
    #[default]
    Sqp,
    /// Penalty rows inside the LM least squares + augmented-Lagrangian
    /// outer loop (M2).
    Lm,
}

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
    /// Inner-iteration cap per outer round: chattering on a hinge kink
    /// should hand over to the multiplier update, not burn the budget.
    pub max_inner: usize,
    /// Initial exact-penalty weight μ for [`ScoMethod::Sqp`].
    pub init_mu: f64,
    /// SQP convergence: stop (feasible) once a step's model-predicted merit
    /// decrease falls below `min_improve · (1 + merit)`.
    pub min_improve: f64,
}

impl Default for ScoOptions {
    fn default() -> Self {
        Self {
            max_iters: 300,
            max_outer: 10,
            penalty_growth: 2.0,
            init_radius: 0.5,
            max_inner: 60,
            init_mu: 100.0,
            min_improve: 1e-5,
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
    /// The solve stopped on the iteration budget, not on convergence.
    pub cap_hit: bool,
    /// Final exact-penalty weight (SQP only).
    pub mu: f64,
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
    /// SQP mode: hard rows output their RAW residual `r` (and `∂r`), not a
    /// least-squares row.
    sqp: bool,
}

impl ResidualSystem {
    /// Compile `claims` (over `n_vars` variables) into a residual system for
    /// the default method ([`ScoMethod::Sqp`]). Appends derivative nodes to
    /// `g`.
    pub fn compile(g: &mut ExprGraph, claims: &ClaimSet, n_vars: usize) -> Self {
        Self::compile_with(g, claims, n_vars, ScoMethod::default())
    }

    /// [`Self::compile`] for an explicit method.
    pub fn compile_with(
        g: &mut ExprGraph,
        claims: &ClaimSet,
        n_vars: usize,
        method: ScoMethod,
    ) -> Self {
        let sqp = method == ScoMethod::Sqp;
        // LM: hard claims get an AL shift input appended after θ — when the
        // var id space (u16) has room; otherwise they fall back to pure
        // penalty. SQP: hard rows are the raw residual.
        let n_hard = claims.claims.iter().filter(|c| c.hard).count();
        let use_shift = !sqp && n_vars + n_hard <= u16::MAX as usize;
        let mut shift_slot = Vec::with_capacity(claims.claims.len());
        let mut next = n_vars;
        let rho: Vec<ExprId> = claims
            .claims
            .iter()
            .map(|c| {
                if c.hard && sqp {
                    shift_slot.push(None);
                    c.residual
                } else if c.hard && use_shift {
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
            sqp,
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

    /// Run SCO from `theta0` with the method the system was compiled for.
    pub fn solve(&self, theta0: &[f64], opts: &ScoOptions) -> (Vec<f64>, ScoReport) {
        if self.sqp {
            self.solve_sqp(theta0, opts)
        } else {
            self.solve_lm(theta0, opts)
        }
    }

    fn buf_len(&self) -> usize {
        self.rows + self.pattern.len() + self.lin_grad.len() + self.lin_hess.len()
    }

    /// `[start, end)` of each row's Jacobian nonzeros (pattern is row-major).
    fn row_ranges(&self) -> Vec<(usize, usize)> {
        let mut out = vec![(0, 0); self.rows];
        let mut k = 0;
        while k < self.pattern.len() {
            let row = self.pattern[k].0;
            let mut end = k;
            while end < self.pattern.len() && self.pattern[end].0 == row {
                end += 1;
            }
            out[row] = (k, end);
            k = end;
        }
        out
    }

    /// Half gradient `Jᵀρ` and Gauss-Newton half Hessian `JᵀJ` (+ exact
    /// linear-objective second order) of `Σ (s_i ρ_i)²` over the rows
    /// `include` admits.
    fn normal_eqs(
        &self,
        buf: &[f64],
        scale: &[f64],
        include: impl Fn(usize) -> bool,
    ) -> (Vec<f64>, Vec<f64>) {
        let n = self.n_vars;
        let mut h = vec![0.0; n * n];
        let mut grad = vec![0.0; n];
        let jv = &buf[self.rows..];
        let mut k = 0;
        while k < self.pattern.len() {
            let row = self.pattern[k].0;
            let mut end = k;
            while end < self.pattern.len() && self.pattern[end].0 == row {
                end += 1;
            }
            if include(row) {
                let s = scale[row];
                let r = s * buf[row];
                for a in k..end {
                    let (ca, va) = (self.pattern[a].1, s * jv[a]);
                    if va == 0.0 {
                        continue;
                    }
                    grad[ca] += va * r;
                    for (&(_, cb), &jb) in self.pattern[k..end].iter().zip(&jv[k..end]) {
                        h[ca * n + cb] += va * s * jb;
                    }
                }
            }
            k = end;
        }
        let off = self.rows + self.pattern.len();
        for (k, &(row, c)) in self.lin_grad.iter().enumerate() {
            grad[c] += 0.5 * self.lin_weight[row] * buf[off + k];
        }
        let off = off + self.lin_grad.len();
        for (k, &(row, i, j)) in self.lin_hess.iter().enumerate() {
            h[i * n + j] += 0.5 * self.lin_weight[row] * buf[off + k];
        }
        (grad, h)
    }

    /// SQP merit: soft least squares + `μ Σ max(0, r_hard)`.
    fn merit_sqp(&self, buf: &[f64], mu: f64) -> f64 {
        (0..self.rows)
            .map(|i| {
                if self.hard[i] {
                    mu * buf[i].max(0.0)
                } else {
                    buf[i] * buf[i]
                }
            })
            .sum()
    }

    /// M7: sequential QP with linearized hard constraints (module docs).
    fn solve_sqp(&self, theta0: &[f64], opts: &ScoOptions) -> (Vec<f64>, ScoReport) {
        const EPS: f64 = 1e-6; // linearized target: r + aᵀδ ≤ −EPS
        let n = self.n_vars;
        let ranges = self.row_ranges();
        let hard_rows: Vec<usize> = (0..self.rows).filter(|&i| self.hard[i]).collect();
        let ones = vec![1.0; self.rows];
        let mut theta = theta0.to_vec();
        let mut buf = vec![0.0; self.buf_len()];
        let mut trial = buf.clone();
        let mut report = ScoReport::default();
        let mut radius = opts.init_radius;
        let mut lambda = 1e-3;
        let mut mu = opts.init_mu;
        self.residuals(&theta, &mut buf);
        let mut phi = self.merit_sqp(&buf, mu);
        let mut stall = 0;
        let hard_ok = |buf: &[f64]| hard_rows.iter().all(|&i| buf[i] <= 0.0);
        let mut converged = false;
        while report.iters < opts.max_iters {
            report.iters += 1;
            let (grad, h) = self.normal_eqs(&buf, &ones, |r| !self.hard[r]);
            let jv = &buf[self.rows..];
            // Linearized hard rows: (r, [(col, ∂r)]).
            let lin: Vec<(f64, Vec<(usize, f64)>)> = hard_rows
                .iter()
                .map(|&i| {
                    let (a, b) = ranges[i];
                    (buf[i], (a..b).map(|k| (self.pattern[k].1, jv[k])).collect())
                })
                .collect();
            let gnorm = grad.iter().map(|g| g * g).sum::<f64>().sqrt();
            if gnorm < 1e-10 && hard_ok(&buf) {
                converged = true;
                break;
            }
            // Rows that could turn active within the trust radius.
            let cand: Vec<usize> = (0..lin.len())
                .filter(|&k| {
                    let (r, a) = &lin[k];
                    let an: f64 = a.iter().map(|(_, v)| v.abs()).sum();
                    r + an * radius > -EPS
                })
                .collect();
            // Trust region by damping (as in LM), the QP re-solved per λ —
            // a few times at most (each re-solve redoes the working set's
            // `A⁻¹a` solves), then the step is clipped: by convexity a
            // scaled QP step still decreases the model and keeps every
            // linearization that held at δ = 0.
            let mut solved = None;
            for _ in 0..6 {
                let mut a = h.clone();
                for i in 0..n {
                    a[i * n + i] += lambda * (h[i * n + i] + 1.0);
                }
                if !cholesky_factor(&mut a, n) {
                    lambda *= 10.0;
                    continue;
                }
                let (d, at_bound) = qp_l1(&a, n, &grad, &lin, &cand, mu, EPS);
                let dn = d.iter().map(|x| x * x).sum::<f64>().sqrt();
                solved = Some((d, dn, at_bound));
                if dn <= radius {
                    break;
                }
                lambda *= 3.0;
            }
            let Some((mut delta, dn, at_bound)) = solved else {
                break;
            };
            if dn > radius {
                let c = radius / dn;
                delta.iter_mut().for_each(|d| *d *= c);
            }
            // Model decrease: soft GN model + exact linearized penalty.
            let mut hd = 0.0;
            for i in 0..n {
                let mut row = 0.0;
                for j in 0..n {
                    row += h[i * n + j] * delta[j];
                }
                hd += delta[i] * row;
            }
            let gd: f64 = grad.iter().zip(&delta).map(|(g, d)| g * d).sum();
            let mut pred = -2.0 * gd - hd;
            for (r, a) in &lin {
                let ad: f64 = a.iter().map(|&(c, v)| v * delta[c]).sum();
                pred += mu * (r.max(0.0) - (r + ad).max(0.0));
            }
            // The model sees (almost) no progress left — TrajOpt's
            // "min approx improve": past it the l1 merit's kinks make the
            // trust region crawl for hundreds of steps of sub-µm sliding.
            if pred <= opts.min_improve * (1.0 + phi) && hard_ok(&buf) {
                converged = true;
                break;
            }
            let mut cand_th = theta.clone();
            for (c, d) in cand_th.iter_mut().zip(&delta) {
                *c += d;
            }
            self.residuals(&cand_th, &mut trial);
            let phi_new = self.merit_sqp(&trial, mu);
            let ratio = if pred > 0.0 {
                (phi - phi_new) / pred
            } else {
                -1.0
            };
            log::trace!(
                target: "sco_trace",
                "it={} phi={phi:.6e} new={phi_new:.6e} pred={pred:.3e} ratio={ratio:.3} dn={dn:.3e} radius={radius:.3e} lambda={lambda:.1e} mu={mu:.0e} cand={} at_bound={at_bound} hard_ok={}",
                report.iters,
                cand.len(),
                hard_ok(&buf)
            );
            let step = dn.min(radius);
            if phi_new < phi && ratio > 1e-4 {
                let rel = (phi - phi_new) / phi.max(1e-300);
                theta = cand_th;
                std::mem::swap(&mut buf, &mut trial);
                phi = phi_new;
                if ratio > 0.75 {
                    // Grow the radius itself, not just to 2·step: the
                    // λ-damped step lands well inside the radius, and
                    // `max(2·step, radius)` then never grows — the solve
                    // crawled at ratio ≈ 1 with a frozen 1.6 µm radius.
                    radius = (2.0 * radius).min(10.0);
                    lambda = (lambda / 3.0).max(1e-9);
                } else if ratio < 0.25 {
                    radius *= 0.5;
                    lambda *= 2.0;
                }
                stall = if rel < 1e-8 { stall + 1 } else { 0 };
                if (stall >= 3 || step < 1e-9) && hard_ok(&buf) {
                    converged = true;
                    break;
                }
            } else {
                radius *= 0.25;
                lambda *= 4.0;
                if radius < 1e-7 {
                    converged = hard_ok(&buf);
                    break;
                }
            }
            // A dual on its bound with the linearization still violated
            // while the iterate itself is infeasible: μ is below the
            // multiplier — the penalty is not yet exact. (At a feasible
            // iterate a bound-hit only means the trust region cut the
            // step; raising μ there just stiffens the merit.)
            if at_bound && !hard_ok(&buf) && mu < 1e6 {
                mu *= 10.0;
                report.outer += 1;
                phi = self.merit_sqp(&buf, mu);
                radius = radius.max(opts.init_radius * 0.25);
            }
        }
        report.merit = phi;
        report.hard_ok = hard_ok(&buf);
        report.cap_hit = !converged && report.iters >= opts.max_iters;
        report.mu = mu;
        (theta, report)
    }

    /// M2: LM on penalty rows + augmented-Lagrangian outer loop.
    fn solve_lm(&self, theta0: &[f64], opts: &ScoOptions) -> (Vec<f64>, ScoReport) {
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
            let round_end = (report.iters + opts.max_inner).min(opts.max_iters);
            while report.iters < round_end {
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
                    stall = if rel < 1e-8 { stall + 1 } else { 0 };
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
                report.cap_hit = !bad.is_empty();
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

/// The ℓ1-penalized trust-region QP of one SQP step, given the Cholesky
/// factor `l` of `A` (so `Q = 2A`):
///
/// `min ½δᵀQδ + 2gᵀδ + μ Σ_{k∈cand} max(0, r_k + a_kᵀδ + eps)`
///
/// solved in the dual `0 ≤ y ≤ μ` over a working set (rows the current δ
/// violates are added until none is): a few Hildreth coordinate-ascent
/// sweeps warm-start a projected-Newton solve of the box-constrained dual
/// (`min ½yᵀMy − cᵀy`, `M_kj = a_k·z_j`) — Hildreth alone stalls on the
/// near-singular `A` of a nearly converged LM step, leaving linearized
/// constraints violated. The primal is `δ = −A⁻¹g − Σ y_k z_k`,
/// `z_k = Q⁻¹a_k`. Returns δ and whether some dual sits on its bound with
/// its linearization still violated (μ too small to be exact).
fn qp_l1(
    l: &[f64],
    n: usize,
    g: &[f64],
    lin: &[(f64, Vec<(usize, f64)>)],
    cand: &[usize],
    mu: f64,
    eps: f64,
) -> (Vec<f64>, bool) {
    let neg: Vec<f64> = g.iter().map(|x| -x).collect();
    let delta0 = cholesky_apply(l, &neg, n);
    let mut delta = delta0.clone();
    let dot = |a: &[(usize, f64)], v: &[f64]| a.iter().map(|&(c, x)| x * v[c]).sum::<f64>();
    // Working set: (lin index, z, a·z, y).
    let mut ws: Vec<(usize, Vec<f64>, f64, f64)> = Vec::new();
    let mut in_ws = vec![false; lin.len()];
    for _ in 0..50 {
        let mut grew = false;
        for &k in cand {
            if in_ws[k] {
                continue;
            }
            let (r, a) = &lin[k];
            if r + dot(a, &delta) + eps <= 1e-12 {
                continue;
            }
            let mut rhs = vec![0.0; n];
            for &(c, v) in a {
                rhs[c] += 0.5 * v;
            }
            let z = cholesky_apply(l, &rhs, n);
            let az = dot(a, &z);
            if az > 1e-300 {
                in_ws[k] = true;
                ws.push((k, z, az, 0.0));
                grew = true;
            }
        }
        if !grew {
            break;
        }
        for _ in 0..30 {
            let mut change = 0.0f64;
            for (k, z, az, y) in ws.iter_mut() {
                let (r, a) = &lin[*k];
                let v = r + dot(a, &delta) + eps;
                let ny = (*y + v / *az).clamp(0.0, mu);
                let d = ny - *y;
                if d != 0.0 {
                    for (x, zi) in delta.iter_mut().zip(z.iter()) {
                        *x -= d * zi;
                    }
                    *y = ny;
                    change = change.max(d.abs() * *az);
                }
            }
            if change < 1e-12 {
                break;
            }
        }
        // Exact dual on the working set (dense m×m: past a few hundred rows
        // the projected Newton costs more than it buys — more sweeps).
        let m = ws.len();
        if m > 400 {
            for _ in 0..300 {
                let mut change = 0.0f64;
                for (k, z, az, y) in ws.iter_mut() {
                    let (r, a) = &lin[*k];
                    let v = r + dot(a, &delta) + eps;
                    let ny = (*y + v / *az).clamp(0.0, mu);
                    let d = ny - *y;
                    if d != 0.0 {
                        for (x, zi) in delta.iter_mut().zip(z.iter()) {
                            *x -= d * zi;
                        }
                        *y = ny;
                        change = change.max(d.abs() * *az);
                    }
                }
                if change < 1e-12 {
                    break;
                }
            }
            continue;
        }
        let mut mm = vec![0.0; m * m];
        for (i, (ki, _, _, _)) in ws.iter().enumerate() {
            for (j, (_, zj, _, _)) in ws.iter().enumerate() {
                mm[i * m + j] = dot(&lin[*ki].1, zj);
            }
        }
        let c: Vec<f64> = ws
            .iter()
            .map(|(k, _, _, _)| lin[*k].0 + eps + dot(&lin[*k].1, &delta0))
            .collect();
        let mut y: Vec<f64> = ws.iter().map(|w| w.3).collect();
        box_qp_newton(&mm, &c, &mut y, mu);
        delta.copy_from_slice(&delta0);
        for ((_, z, _, yk), &ny) in ws.iter_mut().zip(&y) {
            *yk = ny;
            if ny != 0.0 {
                for (x, zi) in delta.iter_mut().zip(z.iter()) {
                    *x -= ny * zi;
                }
            }
        }
    }
    let at_bound = ws.iter().any(|(k, _, _, y)| {
        let (r, a) = &lin[*k];
        *y >= mu * (1.0 - 1e-9) && r + dot(a, &delta) > 1e-9
    });
    (delta, at_bound)
}

/// Projected Newton for `min ½yᵀMy − cᵀy` s.t. `0 ≤ y ≤ mu` (M PSD,
/// m×m row-major), refining `y` in place: Newton on the free variables
/// (those not held at a bound by their gradient), projected back onto the
/// box with a backtracking line search.
fn box_qp_newton(mm: &[f64], c: &[f64], y: &mut [f64], mu: f64) {
    let m = y.len();
    let obj = |y: &[f64]| -> f64 {
        let mut v = 0.0;
        for i in 0..m {
            let mut row = 0.0;
            for j in 0..m {
                row += mm[i * m + j] * y[j];
            }
            v += y[i] * (0.5 * row - c[i]);
        }
        v
    };
    let ridge = 1e-12 * (1.0 + (0..m).map(|i| mm[i * m + i]).fold(0.0, f64::max));
    for _ in 0..60 {
        let grad: Vec<f64> = (0..m)
            .map(|i| (0..m).map(|j| mm[i * m + j] * y[j]).sum::<f64>() - c[i])
            .collect();
        let free: Vec<usize> = (0..m)
            .filter(|&i| !((y[i] <= 0.0 && grad[i] >= 0.0) || (y[i] >= mu && grad[i] <= 0.0)))
            .collect();
        if free.is_empty() {
            break;
        }
        let f = free.len();
        let mut a = vec![0.0; f * f];
        let mut rhs = vec![0.0; f];
        for (p, &i) in free.iter().enumerate() {
            for (q, &j) in free.iter().enumerate() {
                a[p * f + q] = mm[i * m + j];
            }
            a[p * f + p] += ridge;
            // Newton step on the free block: M_FF Δ = −grad_F.
            rhs[p] = -grad[i];
        }
        let Some(step) = cholesky_solve(&mut a, &rhs, f) else {
            break;
        };
        let f0 = obj(y);
        let mut alpha = 1.0;
        let mut improved = false;
        for _ in 0..30 {
            let mut t = y.to_vec();
            for (p, &i) in free.iter().enumerate() {
                t[i] = (y[i] + alpha * step[p]).clamp(0.0, mu);
            }
            if obj(&t) < f0 {
                let moved = t
                    .iter()
                    .zip(y.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max);
                y.copy_from_slice(&t);
                improved = moved > 1e-15 * (1.0 + mu);
                break;
            }
            alpha *= 0.5;
        }
        if !improved {
            break;
        }
    }
}

/// In-place Cholesky factorization of SPD `a` (n×n row-major, lower factor
/// left in the lower triangle); false if not SPD.
fn cholesky_factor(a: &mut [f64], n: usize) -> bool {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 || !d.is_finite() {
            return false;
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
    true
}

/// Solve `L Lᵀ x = b` with a factor from [`cholesky_factor`].
fn cholesky_apply(l: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    let mut y = b.to_vec();
    for i in 0..n {
        for k in 0..i {
            y[i] -= l[i * n + k] * y[k];
        }
        y[i] /= l[i * n + i];
    }
    for i in (0..n).rev() {
        for k in i + 1..n {
            y[i] -= l[k * n + i] * y[k];
        }
        y[i] /= l[i * n + i];
    }
    y
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
        for method in [ScoMethod::Lm, ScoMethod::Sqp] {
            let mut g = g.clone();
            let sys = ResidualSystem::compile_with(&mut g, &cs, 2, method);
            let (th, rep) = sys.solve(&[5.0, -2.0], &ScoOptions::default());
            assert!(rep.hard_ok, "{method:?}: {rep:?} at {th:?}");
            assert!(th[1] <= -0.3 + 1e-9 && th[1] > -0.31, "{method:?}: {th:?}");
            assert!((th[0] - 5.0).abs() < 1e-3, "{method:?}: {th:?}");
        }
    }

    /// SQP: the unconstrained optimum sits deep inside two hard claims at
    /// once; the linearized-constraint step lands on their intersection
    /// (the constrained optimum) without escalating any penalty past what
    /// the multipliers need, and starting INSIDE the violation is fine.
    #[test]
    fn sqp_lands_on_constrained_optimum() {
        let mut g = ExprGraph::new();
        let p = (g.var(0), g.var(1));
        let mut cs = ClaimSet::new();
        let anchor = (g.lit(0.0), g.lit(0.0));
        cs.push(cir::gap(&mut g, p, anchor, 0.0, 0.0, 1.0));
        // Hard: stay 1.0 from the point (0.5, 0) and from the point (-0.5, 0).
        cs.push(cir::clearance(&mut g, p, (0.5, 0.0), (0.5, 0.0), 1.0, 1.0));
        cs.push(cir::clearance(
            &mut g,
            p,
            (-0.5, 0.0),
            (-0.5, 0.0),
            1.0,
            1.0,
        ));
        let sys = ResidualSystem::compile_with(&mut g, &cs, 2, ScoMethod::Sqp);
        let _ = env_logger::builder().is_test(true).try_init();
        let (th, rep) = sys.solve(&[0.05, 0.2], &ScoOptions::default());
        assert!(rep.hard_ok && !rep.cap_hit, "{rep:?} at {th:?}");
        // Constrained optimum: (0, √0.75).
        assert!(th[0].abs() < 1e-4, "{th:?}");
        assert!((th[1] - 0.75f64.sqrt()).abs() < 1e-4, "{th:?}");
    }

    #[test]
    fn cholesky_solves_spd() {
        let mut a = vec![4.0, 1.0, 1.0, 3.0];
        let x = cholesky_solve(&mut a, &[1.0, 2.0], 2).unwrap();
        assert!((4.0 * x[0] + x[1] - 1.0).abs() < 1e-12);
        assert!((x[0] + 3.0 * x[1] - 2.0).abs() < 1e-12);
    }
}
