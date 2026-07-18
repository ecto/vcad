//! Finite-difference minimization over prescription parameters.
//!
//! The M0 stand-in for the adjoint through the Snell chain (every
//! operation in the exact trace is smooth — intersection root, refraction,
//! transfer — so the adjoint is a later milestone, not a rewrite; see
//! `docs/optics-m0.md`). Mirrors the particle crate's optimizer contract:
//! central-difference gradient, projected descent with backtracking,
//! **scale-invariant stopping** (an absolute gradient epsilon silently
//! kills objectives living at extreme scales — the 1e-32 lesson,
//! regression-tested here too), plus a non-finite guard: an infinite
//! objective (a bundle with TIR/missed rays) is never accepted.

/// Options for [`minimize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdOptions {
    /// Central-difference step, as a fraction of each parameter's range.
    pub rel_step: f64,
    /// Maximum descent iterations.
    pub max_iters: usize,
    /// Initial step length, as a fraction of the parameter range.
    pub initial_step: f64,
    /// Stop when the step length falls below this fraction of the range.
    pub min_step: f64,
}

impl Default for FdOptions {
    fn default() -> Self {
        Self {
            rel_step: 1e-3,
            max_iters: 60,
            initial_step: 0.1,
            min_step: 1e-4,
        }
    }
}

/// Result of [`minimize`] / [`minimize_multi_start`].
#[derive(Debug, Clone, PartialEq)]
pub struct FdResult {
    /// Best parameters found.
    pub x: Vec<f64>,
    /// Objective at `x`.
    pub value: f64,
    /// Total objective evaluations.
    pub evals: usize,
    /// Objective after each accepted step (starts with f(x₀)).
    pub history: Vec<f64>,
}

/// Minimize `f` over the box `[lo, hi]` starting at `x0`.
///
/// The objective may return `f64::INFINITY` for infeasible designs
/// (e.g. a bundle with TIR); such points are never accepted, and a start
/// point that is itself non-finite returns immediately.
pub fn minimize(
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
    if !value.is_finite() {
        return FdResult {
            x,
            value,
            evals,
            history,
        };
    }
    let mut step = opts.initial_step;

    for _ in 0..opts.max_iters {
        let mut grad = vec![0.0; dim];
        let mut gnorm = 0.0;
        let mut finite = true;
        for i in 0..dim {
            let h = opts.rel_step * range[i];
            let mut xp = x.clone();
            xp[i] += h;
            clamp(&mut xp);
            let mut xm = x.clone();
            xm[i] -= h;
            clamp(&mut xm);
            let denom = xp[i] - xm[i];
            let (fp, fm) = (f(&xp), f(&xm));
            evals += 2;
            let g = if denom.abs() < 1e-15 || !fp.is_finite() || !fm.is_finite() {
                finite = finite && fp.is_finite() && fm.is_finite();
                0.0
            } else {
                (fp - fm) / denom
            };
            grad[i] = g * range[i]; // scale-free direction
            gnorm += grad[i] * grad[i];
        }
        gnorm = gnorm.sqrt();
        // Scale-invariant stop (the particle crate's 1e-32 lesson): the
        // gradient is negligible relative to the objective's own scale.
        // Skipped when a probe was infeasible — the boundary is real,
        // keep line-searching along whatever gradient survives.
        if finite && (gnorm <= 1e-12 * value.abs() || gnorm == 0.0) {
            break;
        }
        if gnorm == 0.0 {
            break;
        }

        let mut improved = false;
        let mut s = step;
        for _ in 0..10 {
            let mut xt = x.clone();
            for i in 0..dim {
                xt[i] -= s * range[i] * grad[i] / gnorm;
            }
            clamp(&mut xt);
            let vt = f(&xt);
            evals += 1;
            if vt.is_finite() && vt < value {
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

/// Run [`minimize`] from every start and keep the best finite result —
/// multimodal landscapes (the particle crate's recirculation-vs-energy
/// hills; here the sign basins of a doublet's inner curvature) require
/// multi-start.
pub fn minimize_multi_start(
    f: &mut dyn FnMut(&[f64]) -> f64,
    starts: &[Vec<f64>],
    lo: &[f64],
    hi: &[f64],
    opts: &FdOptions,
) -> Option<FdResult> {
    let mut best: Option<FdResult> = None;
    for s in starts {
        let r = minimize(f, s, lo, hi, opts);
        if !r.value.is_finite() {
            continue;
        }
        if best.as_ref().is_none_or(|b| r.value < b.value) {
            best = Some(r);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descends_a_smooth_bowl() {
        let mut f = |x: &[f64]| (x[0] - 2.0).powi(2) + 3.0 * (x[1] + 1.0).powi(2);
        let r = minimize(
            &mut f,
            &[-4.0, 4.0],
            &[-5.0, -5.0],
            &[5.0, 5.0],
            &FdOptions::default(),
        );
        assert!((r.x[0] - 2.0).abs() < 0.05, "x = {:?}", r.x);
        assert!((r.x[1] + 1.0).abs() < 0.05, "x = {:?}", r.x);
        assert!(r.history.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn survives_astronomically_small_objectives() {
        // The 1e-32 lesson: tiny units must not read as converged.
        let mut f = |x: &[f64]| 1.0e-30 * (x[0] - 2.0).powi(2);
        let r = minimize(&mut f, &[-3.0], &[-5.0], &[5.0], &FdOptions::default());
        assert!((r.x[0] - 2.0).abs() < 0.05, "x = {:?}", r.x);
        assert!(r.evals > 10, "stopped suspiciously early: {}", r.evals);
    }

    #[test]
    fn infeasible_regions_are_never_accepted() {
        // Objective is +∞ left of x = 0; the minimum inside the feasible
        // region is at the boundary of the bowl.
        let mut f = |x: &[f64]| {
            if x[0] < 0.0 {
                f64::INFINITY
            } else {
                (x[0] - 1.0).powi(2)
            }
        };
        let r = minimize(&mut f, &[3.0], &[-5.0], &[5.0], &FdOptions::default());
        assert!(r.value.is_finite());
        assert!((r.x[0] - 1.0).abs() < 0.05, "x = {:?}", r.x);
    }

    #[test]
    fn multi_start_escapes_a_bad_basin() {
        // Two wells; the deeper one at x = 3.
        let mut f = |x: &[f64]| {
            let a = (x[0] + 2.0).powi(2) + 1.0;
            let b = (x[0] - 3.0).powi(2);
            a.min(b)
        };
        let r = minimize_multi_start(
            &mut f,
            &[vec![-3.0], vec![2.0]],
            &[-6.0],
            &[6.0],
            &FdOptions::default(),
        )
        .unwrap();
        assert!((r.x[0] - 3.0).abs() < 0.1, "x = {:?}", r.x);
        assert!(r.value < 0.01);
    }
}
