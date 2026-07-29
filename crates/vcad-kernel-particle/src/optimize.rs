//! Finite-difference maximization over electrode parameters.
//!
//! M0 stand-in for the discrete adjoint (see the milestone ladder in
//! `docs/particle-optics-m0.md`): central-difference gradient + projected
//! gradient ascent with backtracking. Deterministic, dependency-free, and
//! good enough to close the design loop on a handful of parameters. The
//! adjoint replaces the gradient estimate later without changing callers.

/// Options for [`maximize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdOptions {
    /// Central-difference step, as a fraction of each parameter's range.
    pub rel_step: f64,
    /// Maximum ascent iterations.
    pub max_iters: usize,
    /// Initial step length, as a fraction of the parameter range.
    pub initial_step: f64,
    /// Stop when the step length falls below this fraction of the range.
    pub min_step: f64,
}

impl Default for FdOptions {
    fn default() -> Self {
        Self {
            rel_step: 1e-2,
            max_iters: 20,
            initial_step: 0.1,
            min_step: 1e-3,
        }
    }
}

/// Result of [`maximize`].
#[derive(Debug, Clone, PartialEq)]
pub struct FdResult {
    /// Best parameters found.
    pub x: Vec<f64>,
    /// Objective at `x`.
    pub value: f64,
    /// Total objective evaluations.
    pub evals: usize,
    /// Objective value after each accepted step (starts with f(x₀)).
    pub history: Vec<f64>,
}

/// Maximize `f` over the box `[lo, hi]` starting at `x0`.
///
/// The objective may be noisy-deterministic (e.g. a traced ensemble with
/// fixed seeds); backtracking only accepts strict improvements.
pub fn maximize(
    f: &mut dyn FnMut(&[f64]) -> f64,
    x0: &[f64],
    lo: &[f64],
    hi: &[f64],
    opts: &FdOptions,
) -> FdResult {
    assert_eq!(x0.len(), lo.len());
    assert_eq!(x0.len(), hi.len());
    let dim = x0.len();
    let range: Vec<f64> = lo
        .iter()
        .zip(hi)
        .map(|(a, b)| (b - a).abs().max(1e-12))
        .collect();
    let clamp = |x: &mut [f64]| {
        for i in 0..dim {
            x[i] = x[i].clamp(lo[i].min(hi[i]), hi[i].max(lo[i]));
        }
    };

    let mut x: Vec<f64> = x0.to_vec();
    clamp(&mut x);
    let mut value = f(&x);
    let mut evals = 1;
    let mut history = vec![value];
    let mut step = opts.initial_step;

    for _ in 0..opts.max_iters {
        // Central-difference gradient.
        let mut grad = vec![0.0; dim];
        let mut gnorm = 0.0;
        for i in 0..dim {
            let h = opts.rel_step * range[i];
            let mut xp = x.clone();
            xp[i] += h;
            clamp(&mut xp);
            let mut xm = x.clone();
            xm[i] -= h;
            clamp(&mut xm);
            let denom = xp[i] - xm[i];
            let g = if denom.abs() < 1e-15 {
                0.0
            } else {
                (f(&xp) - f(&xm)) / denom
            };
            evals += 2;
            grad[i] = g * range[i]; // scale-free direction
            gnorm += grad[i] * grad[i];
        }
        gnorm = gnorm.sqrt();
        // Scale-invariant stop: the gradient (per unit of scaled parameter)
        // is negligible relative to the objective's own magnitude. An
        // absolute epsilon here silently kills objectives that live at
        // 1e-32 (e.g. cross-section integrals in SI units).
        if gnorm <= 1e-12 * value.abs() || gnorm == 0.0 {
            break;
        }

        // Backtracking line search along the normalized gradient.
        let mut improved = false;
        let mut s = step;
        for _ in 0..8 {
            let mut xt = x.clone();
            for i in 0..dim {
                xt[i] += s * range[i] * grad[i] / gnorm;
            }
            clamp(&mut xt);
            let vt = f(&xt);
            evals += 1;
            if vt > value {
                x = xt;
                value = vt;
                history.push(value);
                step = s * 1.5;
                improved = true;
                break;
            }
            s *= 0.5;
        }
        if !improved {
            step *= 0.25;
            if step < opts.min_step {
                break;
            }
        }
    }

    FdResult {
        x,
        value,
        evals,
        history,
    }
}

/// A multi-start FD-ascent plan over the box `[lo, hi]`.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiStart {
    /// Lower bounds per variable.
    pub lo: Vec<f64>,
    /// Upper bounds per variable.
    pub hi: Vec<f64>,
    /// Explicit first start point; when present it replaces the first
    /// seed fraction's point.
    pub explicit_start: Option<Vec<f64>>,
    /// Seed fractions of the box, one start each (e.g. `[0.2, 0.5, 0.8]`).
    pub seed_fractions: Vec<f64>,
    /// Ascent options shared by every start.
    pub opts: FdOptions,
}

impl MultiStart {
    /// The start point for start index `k`.
    fn x0(&self, k: usize) -> Vec<f64> {
        if k == 0 {
            if let Some(x0) = &self.explicit_start {
                return x0.clone();
            }
        }
        let f = self.seed_fractions[k];
        self.lo
            .iter()
            .zip(&self.hi)
            .map(|(a, b)| a + f * (b - a))
            .collect()
    }
}

/// Stepwise driver for a multi-start [`maximize`] search: each
/// [`MultiStartRun::advance_one`] runs exactly one complete start, so a
/// host can stream per-start progress. The one-shot
/// [`multi_start_maximize`] is implemented on top, making the two paths
/// identical by construction.
#[derive(Debug, Clone)]
pub struct MultiStartRun {
    plan: MultiStart,
    /// Per-start results, in start order.
    starts: Vec<FdResult>,
    best: Option<usize>,
}

impl MultiStartRun {
    /// Set up the run. Panics if the plan has no seed fractions.
    pub fn new(plan: MultiStart) -> Self {
        assert!(
            !plan.seed_fractions.is_empty(),
            "multi-start needs at least one seed fraction"
        );
        MultiStartRun {
            plan,
            starts: Vec::new(),
            best: None,
        }
    }

    /// Starts completed so far.
    pub fn starts_done(&self) -> usize {
        self.starts.len()
    }

    /// Total starts this run will perform.
    pub fn total_starts(&self) -> usize {
        self.plan.seed_fractions.len()
    }

    /// Run the next start to completion against `f`. Returns the just-
    /// finished start's result, or `None` when every start already ran.
    pub fn advance_one(&mut self, f: &mut dyn FnMut(&[f64]) -> f64) -> Option<&FdResult> {
        let k = self.starts.len();
        if k >= self.total_starts() {
            return None;
        }
        let x0 = self.plan.x0(k);
        let r = maximize(f, &x0, &self.plan.lo, &self.plan.hi, &self.plan.opts);
        let better = self
            .best
            .map(|b| r.value > self.starts[b].value)
            .unwrap_or(true);
        self.starts.push(r);
        if better {
            self.best = Some(k);
        }
        self.starts.last()
    }

    /// Per-start results so far, in start order.
    pub fn starts(&self) -> &[FdResult] {
        &self.starts
    }

    /// The best start so far (`None` before the first `advance_one`).
    pub fn best(&self) -> Option<&FdResult> {
        self.best.map(|b| &self.starts[b])
    }
}

/// One-shot multi-start maximize: every start in [`MultiStart`] run to
/// completion via [`MultiStartRun`]. Returns `(per-start results, best
/// index)`.
pub fn multi_start_maximize(
    f: &mut dyn FnMut(&[f64]) -> f64,
    plan: MultiStart,
) -> (Vec<FdResult>, usize) {
    let mut run = MultiStartRun::new(plan);
    while run.advance_one(f).is_some() {}
    let best = run.best.expect("at least one start ran");
    (run.starts, best)
}

/// An objective that returns its value and gradient together.
pub type ValueAndGradFn = dyn FnMut(&[f64]) -> (f64, Vec<f64>);

/// Maximize using a caller-supplied gradient (e.g. the discrete adjoint
/// from [`crate::adjoint::yield_gradient`]): projected ascent with the
/// same backtracking line search as [`maximize`], but no FD probes —
/// one gradient evaluation replaces `2·dim` objective calls per step.
pub fn maximize_with_gradient(
    fg: &mut ValueAndGradFn,
    x0: &[f64],
    lo: &[f64],
    hi: &[f64],
    opts: &FdOptions,
) -> FdResult {
    assert_eq!(x0.len(), lo.len());
    assert_eq!(x0.len(), hi.len());
    let dim = x0.len();
    let range: Vec<f64> = lo
        .iter()
        .zip(hi)
        .map(|(a, b)| (b - a).abs().max(1e-12))
        .collect();
    let clamp = |x: &mut [f64]| {
        for i in 0..dim {
            x[i] = x[i].clamp(lo[i].min(hi[i]), hi[i].max(lo[i]));
        }
    };

    let mut x: Vec<f64> = x0.to_vec();
    clamp(&mut x);
    let (mut value, mut grad) = fg(&x);
    let mut evals = 1;
    let mut history = vec![value];
    let mut step = opts.initial_step;

    for _ in 0..opts.max_iters {
        let scaled: Vec<f64> = grad.iter().zip(&range).map(|(g, r)| g * r).collect();
        let gnorm = scaled.iter().map(|g| g * g).sum::<f64>().sqrt();
        if gnorm <= 1e-12 * value.abs() || gnorm == 0.0 {
            break;
        }
        let mut improved = false;
        let mut s = step;
        for _ in 0..8 {
            let mut xt = x.clone();
            for i in 0..dim {
                xt[i] += s * range[i] * scaled[i] / gnorm;
            }
            clamp(&mut xt);
            let (vt, gt) = fg(&xt);
            evals += 1;
            if vt > value {
                x = xt;
                value = vt;
                grad = gt;
                history.push(value);
                step = s * 1.5;
                improved = true;
                break;
            }
            s *= 0.5;
        }
        if !improved {
            step *= 0.25;
            if step < opts.min_step {
                break;
            }
        }
    }

    FdResult {
        x,
        value,
        evals,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepwise_multi_start_matches_the_one_shot() {
        // Multimodal 1D objective; three seeds land in different basins.
        let obj = |x: &[f64]| (3.0 * x[0]).sin() + 0.5 * x[0];
        let plan = MultiStart {
            lo: vec![0.0],
            hi: vec![6.0],
            explicit_start: None,
            seed_fractions: vec![0.2, 0.5, 0.8],
            opts: FdOptions::default(),
        };
        let (one_shot, best_idx) = multi_start_maximize(&mut { obj }, plan.clone());
        let mut run = MultiStartRun::new(plan);
        let mut f = obj;
        let mut n = 0;
        while run.advance_one(&mut f).is_some() {
            n += 1;
            assert_eq!(run.starts_done(), n);
        }
        assert_eq!(n, 3);
        // Bit-identical per start and same winner.
        assert_eq!(run.starts(), &one_shot[..]);
        assert_eq!(
            run.best().unwrap().value.to_bits(),
            one_shot[best_idx].value.to_bits()
        );
    }

    #[test]
    fn gradient_driven_ascent_climbs() {
        // Same bowl as the FD test, but with supplied gradients.
        let mut fg = |x: &[f64]| {
            let v = -(x[0] - 2.0).powi(2) - 3.0 * (x[1] + 1.0).powi(2);
            let g = vec![-2.0 * (x[0] - 2.0), -6.0 * (x[1] + 1.0)];
            (v, g)
        };
        let r = maximize_with_gradient(
            &mut fg,
            &[-4.0, 4.0],
            &[-5.0, -5.0],
            &[5.0, 5.0],
            &FdOptions {
                max_iters: 60,
                ..FdOptions::default()
            },
        );
        assert!((r.x[0] - 2.0).abs() < 0.05, "x = {:?}", r.x);
        assert!((r.x[1] + 1.0).abs() < 0.05, "x = {:?}", r.x);
        // Far fewer evals than the FD path needs for the same bowl
        // (FD at 2 params spends ~5 evals per accepted step; this spends
        // 1 + line-search retries).
        assert!(r.evals < 200, "evals = {}", r.evals);
    }

    #[test]
    fn climbs_a_smooth_bowl() {
        // f(x, y) = -(x-2)² - 3(y+1)², max at (2, -1) inside the box.
        let mut f = |x: &[f64]| -(x[0] - 2.0).powi(2) - 3.0 * (x[1] + 1.0).powi(2);
        let r = maximize(
            &mut f,
            &[-4.0, 4.0],
            &[-5.0, -5.0],
            &[5.0, 5.0],
            &FdOptions {
                max_iters: 60,
                ..FdOptions::default()
            },
        );
        assert!((r.x[0] - 2.0).abs() < 0.05, "x = {:?}", r.x);
        assert!((r.x[1] + 1.0).abs() < 0.05, "x = {:?}", r.x);
        assert!(r.value > -0.01);
        assert!(r.history.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn survives_astronomically_small_objectives() {
        // Cross-section-integral scale: values around 1e-30. The optimizer
        // must not mistake "tiny units" for "converged".
        let mut f = |x: &[f64]| 1.0e-30 * (4.0 - (x[0] - 2.0).powi(2));
        let r = maximize(
            &mut f,
            &[-3.0],
            &[-5.0],
            &[5.0],
            &FdOptions {
                max_iters: 60,
                ..FdOptions::default()
            },
        );
        assert!((r.x[0] - 2.0).abs() < 0.05, "x = {:?}", r.x);
        assert!(
            r.evals > 10,
            "stopped suspiciously early: {} evals",
            r.evals
        );
    }

    #[test]
    fn respects_bounds() {
        // Max of x+y is at the box corner.
        let mut f = |x: &[f64]| x[0] + x[1];
        let r = maximize(
            &mut f,
            &[0.0, 0.0],
            &[-1.0, -1.0],
            &[1.0, 2.0],
            &FdOptions::default(),
        );
        assert!(r.x[0] <= 1.0 + 1e-12 && r.x[1] <= 2.0 + 1e-12);
        assert!(r.value > 2.5, "should approach the corner: {}", r.value);
    }
}
