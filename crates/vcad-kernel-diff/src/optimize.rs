//! The optimizer harness: geometry improved by gradient descent through
//! the seam.
//!
//! Each iterate rebuilds the model at θ, **re-captures** a frozen plan
//! (plans are only valid near their capture point), evaluates the seam once
//! per parameter (forward mode: k passes for k parameters), and takes a
//! projected gradient-descent step with backtracking. Frozen-tessellation
//! errors during a *trial* step — a topology change, lost correspondence, a
//! failed boundary solve — are the subgradient signals of the seam design:
//! the step is treated as failed and shrunk, never silently accepted.
//!
//! The optimizer is deliberately the simplest thing that closes the loop;
//! anything smarter (L-BFGS, trust regions) plugs into
//! [`objective_gradient`] unchanged.

use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

use crate::{evaluate_with_sensitivity, DiffError, ParamSeeding, SeamMesh};

/// Options for [`minimize`].
#[derive(Debug, Clone)]
pub struct OptimizeOptions {
    /// Maximum accepted iterations.
    pub max_iters: usize,
    /// Initial step length for each line search (in θ units).
    pub initial_step: f64,
    /// Line-search steps below this length terminate the run.
    pub min_step: f64,
    /// Gradient ∞-norm below which the run terminates.
    pub grad_tol: f64,
    /// Per-parameter inclusive bounds; iterates are projected into the box.
    /// Empty = unbounded.
    pub bounds: Vec<(f64, f64)>,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            max_iters: 50,
            initial_step: 1.0,
            min_step: 1e-8,
            grad_tol: 1e-8,
            bounds: Vec::new(),
        }
    }
}

/// Why [`minimize`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Gradient ∞-norm fell below `grad_tol`.
    GradientConverged,
    /// The line search could not find a decreasing step above `min_step`
    /// (at a minimum, a bound, or a subgradient edge).
    StepConverged,
    /// Iteration budget exhausted.
    MaxIters,
}

/// One accepted iterate.
#[derive(Debug, Clone)]
pub struct IterateRecord {
    /// Parameter vector.
    pub theta: Vec<f64>,
    /// Objective value.
    pub objective: f64,
    /// Gradient dJ/dθ at this iterate.
    pub gradient: Vec<f64>,
}

/// Result of [`minimize`].
#[derive(Debug, Clone)]
pub struct OptimizeResult {
    /// Final parameters.
    pub theta: Vec<f64>,
    /// Final objective value.
    pub objective: f64,
    /// Every accepted iterate, first to last.
    pub history: Vec<IterateRecord>,
    /// Termination reason.
    pub stop: StopReason,
}

/// Objective value and full gradient at θ: rebuild, capture a fresh frozen
/// plan, and evaluate the seam once per parameter.
///
/// - `build` constructs the model at θ.
/// - `seeding_for(brep, k)` maps parameter `k` to its surface seeding on
///   the freshly built B-rep.
/// - `objective(seam)` returns `(J, dJ/dθ_k)` for a seam whose velocities
///   carry parameter `k` (dual-number objectives like
///   [`crate::volume_with_derivative`] or
///   [`crate::mass_properties_with_derivative`] fit directly).
pub fn objective_gradient(
    build: &impl Fn(&[f64]) -> BRepSolid,
    seeding_for: &impl Fn(&BRepSolid, usize) -> ParamSeeding,
    objective: &impl Fn(&SeamMesh) -> (f64, f64),
    theta: &[f64],
    params: &TessellationParams,
) -> Result<(f64, Vec<f64>), DiffError> {
    let brep = build(theta);
    let plan = capture_plan(&brep, params)?;
    let mut value = None;
    let mut gradient = Vec::with_capacity(theta.len());
    for k in 0..theta.len() {
        let seam = evaluate_with_sensitivity(&brep, &plan, &seeding_for(&brep, k))?;
        let (j, dj) = objective(&seam);
        value.get_or_insert(j);
        gradient.push(dj);
    }
    Ok((value.expect("at least one parameter"), gradient))
}

fn project(theta: &mut [f64], bounds: &[(f64, f64)]) {
    for (k, t) in theta.iter_mut().enumerate() {
        if let Some(&(lo, hi)) = bounds.get(k) {
            *t = t.clamp(lo, hi);
        }
    }
}

/// Minimize `objective` over θ by projected gradient descent with
/// backtracking. See the module docs for the re-capture-per-iterate
/// contract and subgradient handling.
pub fn minimize(
    build: &impl Fn(&[f64]) -> BRepSolid,
    seeding_for: &impl Fn(&BRepSolid, usize) -> ParamSeeding,
    objective: &impl Fn(&SeamMesh) -> (f64, f64),
    theta0: &[f64],
    params: &TessellationParams,
    options: &OptimizeOptions,
) -> Result<OptimizeResult, DiffError> {
    let mut theta = theta0.to_vec();
    project(&mut theta, &options.bounds);

    // Errors at the starting point are real errors; errors during trial
    // steps are treated as failed steps (subgradient edges) and shrunk.
    let (mut value, mut gradient) =
        objective_gradient(build, seeding_for, objective, &theta, params)?;
    let mut history = vec![IterateRecord {
        theta: theta.clone(),
        objective: value,
        gradient: gradient.clone(),
    }];

    let grad_inf = |g: &[f64]| g.iter().fold(0.0_f64, |m, v| m.max(v.abs()));

    // Warm-start each line search near the last accepted step: narrow
    // curved valleys otherwise re-pay the full backtracking cascade every
    // iteration.
    let mut step_hint = options.initial_step;

    for _ in 0..options.max_iters {
        if grad_inf(&gradient) < options.grad_tol {
            return Ok(OptimizeResult {
                objective: value,
                stop: StopReason::GradientConverged,
                theta,
                history,
            });
        }

        let mut step = step_hint;
        let accepted = loop {
            let mut trial: Vec<f64> = theta
                .iter()
                .zip(&gradient)
                .map(|(t, g)| t - step * g)
                .collect();
            project(&mut trial, &options.bounds);
            let moved = trial
                .iter()
                .zip(&theta)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f64, f64::max);
            if moved > 0.0 {
                match objective_gradient(build, seeding_for, objective, &trial, params) {
                    Ok((jt, gt)) if jt < value => {
                        // Grow the hint past the accepted step: flat
                        // valleys with small gradients need step lengths
                        // far above the initial hint, and backtracking
                        // recovers instantly when the growth overshoots.
                        step_hint = step * 2.0;
                        break Some((trial, jt, gt));
                    }
                    // Non-decreasing, or the trial crossed a subgradient
                    // (topology change / lost correspondence / failed
                    // boundary solve): shrink and retry.
                    Ok(_) | Err(_) => {}
                }
            }
            step *= 0.5;
            if step < options.min_step {
                break None;
            }
        };

        match accepted {
            Some((t, j, g)) => {
                theta = t;
                value = j;
                gradient = g;
                history.push(IterateRecord {
                    theta: theta.clone(),
                    objective: value,
                    gradient: gradient.clone(),
                });
            }
            None => {
                return Ok(OptimizeResult {
                    objective: value,
                    stop: StopReason::StepConverged,
                    theta,
                    history,
                });
            }
        }
    }

    Ok(OptimizeResult {
        objective: value,
        stop: StopReason::MaxIters,
        theta,
        history,
    })
}
