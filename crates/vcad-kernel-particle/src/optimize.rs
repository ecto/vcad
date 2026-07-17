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
        if gnorm < 1e-15 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
