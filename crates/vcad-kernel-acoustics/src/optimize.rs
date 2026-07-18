//! Finite-difference box-constrained maximization.
//!
//! The `vcad-kernel-particle` optimizer pattern, reused verbatim: central-
//! difference gradient + projected ascent with backtracking, deterministic and
//! dependency-free. In M0 every acoustic design parameter is geometric (it
//! moves the mesh), so finite differences is the whole story — the objective
//! is a field solve, not a closed form, and the FD probe is cheap relative to
//! it. The **scale-invariant** stop is load-bearing: acoustic objectives can
//! be a squared frequency error in the hundreds or a velocity in the
//! milli-units, and an absolute epsilon would mistake small units for
//! convergence.

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
            max_iters: 30,
            initial_step: 0.2,
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
        // Central-difference gradient (scale-free direction).
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
            grad[i] = g * range[i];
            gnorm += grad[i] * grad[i];
        }
        gnorm = gnorm.sqrt();
        // Scale-invariant stop.
        if gnorm <= 1e-12 * value.abs() || gnorm == 0.0 {
            break;
        }

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
        assert!(r.history.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn respects_bounds() {
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

    #[test]
    fn survives_tiny_objective_units() {
        // A milli-unit-scale objective (e.g. a port velocity) must not read as
        // converged just because its magnitude is small.
        let mut f = |x: &[f64]| 1.0e-3 * (4.0 - (x[0] - 2.0).powi(2));
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
        assert!(r.evals > 8, "stopped too early: {} evals", r.evals);
    }
}
