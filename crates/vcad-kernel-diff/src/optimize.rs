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

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

use crate::{
    evaluate_with_pullback, evaluate_with_sensitivity, mesh_volume, volume_gradient, DiffError,
    ParamSeeding, SeamMesh,
};

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
    let mut value: Option<f64> = None;
    let mut gradient = Vec::with_capacity(theta.len());
    for k in 0..theta.len() {
        let seam = evaluate_with_sensitivity(&brep, &plan, &seeding_for(&brep, k))?;
        let (j, dj) = objective(&seam);
        match value {
            // J depends only on positions, which are identical across
            // seedings of the same plan — enforce it, since an objective
            // that (incorrectly) reads velocities into its value would
            // otherwise be silently truncated to parameter 0's answer.
            Some(v) => debug_assert!(
                (j - v).abs() <= 1e-9 * v.abs().max(1.0),
                "objective value must not depend on which parameter is seeded ({v} vs {j})"
            ),
            None => value = Some(j),
        }
        gradient.push(dj);
    }
    Ok((value.expect("at least one parameter"), gradient))
}

/// A functional of the seam mesh expressed in **mesh space**: its value and
/// its gradient with respect to every node's position.
///
/// This is the interface reverse mode ([`objective_gradient_reverse`]) prices
/// against. Where the forward [`objective_gradient`] wants a
/// per-seeded-parameter directional derivative `(J, dJ/dθ_k)` (one seam pass
/// per parameter), a `MeshObjective` reports `∂J/∂x_i` once — the mesh
/// gradient a single [`evaluate_with_pullback`] transposes into every
/// parameter's derivative.
///
/// The gradient is evaluated on a **positions-only** seam (velocities are
/// irrelevant and typically zero): the value `J` and each `∂J/∂x_i` depend
/// only on where the nodes are, not on how any one parameter moves them.
pub trait MeshObjective {
    /// `J` and `∂J/∂x_i` (one [`Vec3`] per seam node, indexed like
    /// `seam.positions`). The returned vector must have length
    /// `seam.positions.len()`.
    fn value_and_mesh_gradient(&self, seam: &SeamMesh) -> (f64, Vec<Vec3>);
}

/// The squared relative volume miss `((V − target)/target)²`, with the
/// analytic mesh gradient built on [`volume_gradient`].
///
/// The reference [`MeshObjective`]: `∂J/∂x_i = (2·miss/target)·∂V/∂x_i`, the
/// divergence-theorem per-node volume gradient scaled by the outer
/// derivative. Any other mesh QoI (centroid, inertia, a physics rollout's
/// adjoint) follows the same shape — a value plus a per-node gradient.
#[derive(Debug, Clone, Copy)]
pub struct VolumeMatch {
    /// Target volume (in mm³); must be nonzero.
    pub target: f64,
}

impl MeshObjective for VolumeMatch {
    fn value_and_mesh_gradient(&self, seam: &SeamMesh) -> (f64, Vec<Vec3>) {
        let v = mesh_volume(&seam.positions, &seam.triangles);
        let miss = (v - self.target) / self.target;
        let scale = 2.0 * miss / self.target;
        let dj_dx = volume_gradient(&seam.positions, &seam.triangles)
            .into_iter()
            .map(|g| g * scale)
            .collect();
        (miss * miss, dj_dx)
    }
}

/// Objective value and full gradient at θ **in reverse mode**: rebuild,
/// capture a fresh frozen plan, take **one** positions-only seam pass and
/// **one** [`evaluate_with_pullback`], then contract the resulting
/// cotangents against each parameter's seeding.
///
/// The contract mirrors [`objective_gradient`] — same `build`, same
/// `seeding_for`, same re-capture-per-iterate discipline — but the cost is
/// one pullback plus `n` dot products instead of `n` forward seam passes.
/// The two agree to near machine precision (they share row construction and
/// differ only in linear-algebra order); the `m9_many_parameters` gate pins
/// that at ≤1e-11 relative, per component.
///
/// - `build` constructs the model at θ.
/// - `seeding_for(brep, k)` maps parameter `k` to its surface seeding on the
///   freshly built B-rep — exactly the seeding [`objective_gradient`] uses,
///   so the two paths are drop-in interchangeable.
/// - `objective` supplies `J` and `∂J/∂x` on the positions-only seam.
pub fn objective_gradient_reverse(
    build: &impl Fn(&[f64]) -> BRepSolid,
    seeding_for: &impl Fn(&BRepSolid, usize) -> ParamSeeding,
    objective: &impl MeshObjective,
    theta: &[f64],
    params: &TessellationParams,
) -> Result<(f64, Vec<f64>), DiffError> {
    let brep = build(theta);
    let plan = capture_plan(&brep, params)?;
    // One positions-only forward pass: the empty seeding leaves every
    // velocity zero, so this reads back only node positions — all the
    // mesh objective needs.
    let seam = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new())?;
    let (value, dj_dx) = objective.value_and_mesh_gradient(&seam);
    // One pullback prices every parameter.
    let cots = evaluate_with_pullback(&brep, &plan, &dj_dx)?;
    let gradient = (0..theta.len())
        .map(|k| cots.contract(&seeding_for(&brep, k)))
        .collect();
    Ok((value, gradient))
}

pub(crate) fn project(theta: &mut [f64], bounds: &[(f64, f64)]) {
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
