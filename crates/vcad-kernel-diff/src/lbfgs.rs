//! L-BFGS over the reverse-mode seam: many-parameter optimization at one
//! pullback per iterate.
//!
//! [`crate::minimize`] is projected gradient descent driven by forward mode
//! ([`crate::objective_gradient`]): `n` seam passes per iterate, and the
//! zig-zag convergence of steepest descent on an anisotropic objective.
//! `minimize_lbfgs` swaps both halves out: gradients come from
//! [`crate::objective_gradient_reverse`] (one pullback + `n` dot products),
//! and the search direction is the two-loop L-BFGS recursion over a short
//! memory of `(s, y)` curvature pairs, so the optimizer learns the
//! objective's scaling instead of paying for it every step.
//!
//! It keeps the discipline the GD loop established:
//!
//! - **Box projection.** Iterates are clamped into `OptimizeOptions::bounds`;
//!   the Armijo test measures sufficient decrease along the *projected*
//!   displacement `trial − θ`, so a step that projection flattens against a
//!   bound is simply not accepted.
//! - **Frozen errors are subgradient signals.** A trial step whose rebuild
//!   trips a topology change / lost correspondence / failed boundary solve
//!   is a failed step: the line search shrinks, exactly as a non-decreasing
//!   objective would (never silently accepted). Errors at an *accepted*
//!   iterate propagate.
//!
//! ## L-BFGS near the seam's subgradients
//!
//! Quasi-Newton curvature memory assumes a smooth objective; the frozen seam
//! is smooth only inside a topology class, with subgradient walls at the
//! edges. Two guards keep a corrupted pair out of the memory:
//!
//! 1. **Positive curvature only.** A pair `(s, y)` is stored only when
//!    `s·y > εₖ` (with `εₖ` scaled by `‖s‖‖y‖`); a non-positive inner
//!    product means the secant carries no usable convexity and would make
//!    the inverse-Hessian estimate indefinite, so the pair is dropped.
//! 2. **Subgradient-straddling pairs are dropped.** If the line search that
//!    produced an accepted step had to shrink *past a frozen error* to get
//!    there, the accepted `(s, y)` straddles a topology wall and its `y`
//!    mixes two different smooth branches. Such a pair is not stored — the
//!    step is still taken, but the curvature it implies is discarded.
//!
//! If the two-loop direction ever fails to be a descent direction, or the
//! line search cannot find any accepted step, the memory is **restarted**
//! (cleared) and the next iterate falls back to projected steepest descent —
//! the always-correct direction — before rebuilding curvature.

use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TessellationParams;

use crate::optimize::{objective_gradient_reverse, project, MeshObjective};
use crate::{DiffError, IterateRecord, OptimizeOptions, OptimizeResult, ParamSeeding, StopReason};

/// Curvature memory depth (the classic L-BFGS default range is 3–20).
const MEMORY: usize = 8;
/// Armijo sufficient-decrease coefficient.
const ARMIJO_C1: f64 = 1e-4;
/// Backtracking contraction factor per trial.
const BACKTRACK: f64 = 0.5;
/// Relative floor on `s·y` for accepting a curvature pair.
const CURVATURE_EPS: f64 = 1e-12;

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// One stored curvature pair: `s = θ⁺ − θ`, `y = g⁺ − g`, and `1/(s·y)`.
struct Pair {
    s: Vec<f64>,
    y: Vec<f64>,
    rho: f64,
}

/// The two-loop recursion: `d = −H·g`, with `H` the L-BFGS inverse-Hessian
/// estimate from `history` (oldest first) and the scaling `γ = (sₖ·yₖ)/(yₖ·yₖ)`
/// from the most recent pair. With no history this returns `−g` (steepest
/// descent).
fn two_loop_direction(g: &[f64], history: &[Pair]) -> Vec<f64> {
    let mut q = g.to_vec();
    let mut alpha = vec![0.0; history.len()];
    // First loop: newest → oldest.
    for (i, pair) in history.iter().enumerate().rev() {
        let a = pair.rho * dot(&pair.s, &q);
        alpha[i] = a;
        for (qj, yj) in q.iter_mut().zip(&pair.y) {
            *qj -= a * yj;
        }
    }
    // Initial inverse-Hessian scaling from the most recent pair.
    let gamma = match history.last() {
        Some(p) => {
            let yy = dot(&p.y, &p.y);
            if yy > 0.0 {
                dot(&p.s, &p.y) / yy
            } else {
                1.0
            }
        }
        None => 1.0,
    };
    let mut r: Vec<f64> = q.iter().map(|qi| gamma * qi).collect();
    // Second loop: oldest → newest.
    for (i, pair) in history.iter().enumerate() {
        let beta = pair.rho * dot(&pair.y, &r);
        let coeff = alpha[i] - beta;
        for (ri, si) in r.iter_mut().zip(&pair.s) {
            *ri += coeff * si;
        }
    }
    r.iter().map(|ri| -ri).collect()
}

/// Minimize a [`MeshObjective`] over θ by projected L-BFGS driven by
/// reverse-mode gradients. See the module docs for the box-projection,
/// subgradient, and curvature-memory policies.
///
/// The `build` / `seeding_for` / `params` triple is identical to
/// [`crate::minimize`]'s, so a problem posed for GD ports to L-BFGS by
/// swapping the objective for its [`MeshObjective`] form. `OptimizeOptions`,
/// [`StopReason`], and [`IterateRecord`] are reused unchanged;
/// `initial_step` seeds only the first (steepest-descent) line search —
/// once curvature exists the natural quasi-Newton trial length is 1.
pub fn minimize_lbfgs(
    build: &impl Fn(&[f64]) -> BRepSolid,
    seeding_for: &impl Fn(&BRepSolid, usize) -> ParamSeeding,
    objective: &impl MeshObjective,
    theta0: &[f64],
    params: &TessellationParams,
    options: &OptimizeOptions,
) -> Result<OptimizeResult, DiffError> {
    let mut theta = theta0.to_vec();
    project(&mut theta, &options.bounds);

    // Errors at the starting point are real errors; errors during trial
    // steps are treated as failed steps (subgradient edges) and shrunk.
    let (mut value, mut gradient) =
        objective_gradient_reverse(build, seeding_for, objective, &theta, params)?;
    let mut history: Vec<Pair> = Vec::with_capacity(MEMORY);
    let mut records = vec![IterateRecord {
        theta: theta.clone(),
        objective: value,
        gradient: gradient.clone(),
    }];

    let grad_inf = |g: &[f64]| g.iter().fold(0.0_f64, |m, v| m.max(v.abs()));

    for _ in 0..options.max_iters {
        if grad_inf(&gradient) < options.grad_tol {
            return Ok(OptimizeResult {
                objective: value,
                stop: StopReason::GradientConverged,
                theta,
                history: records,
            });
        }

        // Search direction from the curvature memory; fall back to steepest
        // descent if it is not a descent direction (a corrupted/indefinite
        // memory), restarting the memory so it is rebuilt cleanly.
        let mut direction = two_loop_direction(&gradient, &history);
        if dot(&gradient, &direction) >= 0.0 {
            history.clear();
            direction = gradient.iter().map(|g| -g).collect();
        }

        // Quasi-Newton natural trial length is 1 once curvature exists; the
        // first (steepest-descent) step uses the caller's hint.
        let mut step = if history.is_empty() {
            options.initial_step
        } else {
            1.0
        };

        let mut hit_subgradient = false;
        let accepted = loop {
            let mut trial: Vec<f64> = theta
                .iter()
                .zip(&direction)
                .map(|(t, d)| t + step * d)
                .collect();
            project(&mut trial, &options.bounds);
            // Sufficient decrease along the actual (projected) displacement:
            // g·(trial − θ) is the directional derivative projection, so a
            // step flattened against a bound is measured honestly.
            let displacement: Vec<f64> = trial.iter().zip(&theta).map(|(a, b)| a - b).collect();
            let moved = displacement.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            if moved > 0.0 {
                let expected = ARMIJO_C1 * dot(&gradient, &displacement);
                match objective_gradient_reverse(build, seeding_for, objective, &trial, params) {
                    Ok((jt, gt)) if jt <= value + expected => {
                        break Some((trial, jt, gt, displacement));
                    }
                    // Non-decreasing: shrink and retry.
                    Ok(_) => {}
                    // Trial crossed a subgradient (topology change / lost
                    // correspondence / failed boundary solve): treat as a
                    // failed step, and remember that the eventual accepted
                    // step straddled a topology wall.
                    Err(_) => hit_subgradient = true,
                }
            }
            step *= BACKTRACK;
            if step < options.min_step {
                break None;
            }
        };

        match accepted {
            Some((t, j, g, s)) => {
                let y: Vec<f64> = g.iter().zip(&gradient).map(|(a, b)| a - b).collect();
                let sy = dot(&s, &y);
                let ss = dot(&s, &s).sqrt();
                let yy = dot(&y, &y).sqrt();
                // Store the pair only with positive curvature and only when
                // the line search did not have to cross a subgradient wall
                // to reach this step (see module docs).
                if !hit_subgradient && sy > CURVATURE_EPS * ss * yy {
                    if history.len() == MEMORY {
                        history.remove(0);
                    }
                    history.push(Pair {
                        rho: 1.0 / sy,
                        s,
                        y,
                    });
                }
                theta = t;
                value = j;
                gradient = g;
                records.push(IterateRecord {
                    theta: theta.clone(),
                    objective: value,
                    gradient: gradient.clone(),
                });
            }
            None => {
                // No accepted step. If curvature memory might be steering us
                // wrong, restart and let the next iterate try steepest
                // descent; if we already have none, we are at a minimum, a
                // bound, or a subgradient edge.
                if history.is_empty() {
                    return Ok(OptimizeResult {
                        objective: value,
                        stop: StopReason::StepConverged,
                        theta,
                        history: records,
                    });
                }
                history.clear();
            }
        }
    }

    Ok(OptimizeResult {
        objective: value,
        stop: StopReason::MaxIters,
        theta,
        history: records,
    })
}
