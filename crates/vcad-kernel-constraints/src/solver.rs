//! Levenberg-Marquardt solver for constraint systems.
//!
//! The solver minimizes the sum of squared residuals (errors) from all
//! constraints by iteratively adjusting the parameter values.

use crate::constraint::Constraint;
use crate::entity::{EntityId, SketchEntity};
use crate::symbolic::CompiledSystem;
use slotmap::SlotMap;
use tang_la::{DMat, DVec};

/// Configuration for the Levenberg-Marquardt solver.
#[derive(Debug, Clone)]
pub struct SolverConfig {
    /// Maximum number of iterations before stopping.
    pub max_iterations: usize,
    /// Convergence tolerance for the residual norm.
    pub tolerance: f64,
    /// Initial damping factor (λ).
    pub initial_lambda: f64,
    /// Factor to increase λ when a step is rejected.
    pub lambda_increase: f64,
    /// Factor to decrease λ when a step is accepted.
    pub lambda_decrease: f64,
    /// Minimum damping factor.
    pub min_lambda: f64,
    /// Maximum damping factor.
    pub max_lambda: f64,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 1e-10,
            initial_lambda: 1e-3,
            lambda_increase: 10.0,
            lambda_decrease: 0.1,
            min_lambda: 1e-12,
            max_lambda: 1e12,
        }
    }
}

/// Result of the constraint solver.
#[derive(Debug, Clone)]
pub struct SolveResult {
    /// Final parameter values after solving.
    pub parameters: Vec<f64>,
    /// Final residual norm (sum of squared errors).
    pub residual_norm: f64,
    /// Number of iterations performed.
    pub iterations: usize,
    /// Whether the solver converged within tolerance.
    pub converged: bool,
    /// Reason for termination.
    pub status: SolveStatus,
}

/// Status indicating why the solver stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolveStatus {
    /// Converged: residual norm is below tolerance.
    Converged,
    /// Reached maximum iterations without converging.
    MaxIterations,
    /// Lambda became too large (numerical issues).
    LambdaOverflow,
    /// No constraints to solve.
    NoConstraints,
    /// No parameters to optimize.
    NoParameters,
    /// Singular matrix encountered.
    SingularMatrix,
}

/// Run the Levenberg-Marquardt solver.
///
/// # Arguments
///
/// * `constraints` - The constraints to satisfy
/// * `params` - Initial parameter values (modified in place)
/// * `entities` - Entity definitions
/// * `config` - Solver configuration
///
/// # Returns
///
/// The solve result containing final parameters and convergence info.
pub fn solve(
    constraints: &[Constraint],
    params: &mut [f64],
    entities: &SlotMap<EntityId, SketchEntity>,
    config: &SolverConfig,
) -> SolveResult {
    if constraints.is_empty() {
        return SolveResult {
            parameters: params.to_vec(),
            residual_norm: 0.0,
            iterations: 0,
            converged: true,
            status: SolveStatus::NoConstraints,
        };
    }

    if params.is_empty() {
        let system = CompiledSystem::build(constraints, entities, 0);
        return SolveResult {
            parameters: vec![],
            residual_norm: system.residual_norm_squared(params).sqrt(),
            iterations: 0,
            converged: false,
            status: SolveStatus::NoParameters,
        };
    }

    // Build the symbolic system once (depends only on constraint topology),
    // then hand it to the generic Levenberg-Marquardt driver.
    let system = CompiledSystem::build(constraints, entities, params.len());
    levenberg_marquardt(&system, params, config)
}

/// A least-squares system the Levenberg-Marquardt driver can minimize.
///
/// Implementors expose the normal-equation pieces (`J'J`, `J'r`) and the
/// squared residual norm; the driver never touches residuals directly.
/// `project` is the box-bounds hook — the default no-op leaves unconstrained
/// solving (e.g. the sketch constraint solver) byte-for-byte unchanged.
pub trait LeastSquares {
    /// Number of free parameters (the dimension of the step vector).
    fn num_params(&self) -> usize;

    /// Compute `(J'J, J'r)` at `params`: `J'J` is `n×n`, `J'r` is length `n`.
    fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>);

    /// Sum of squared residuals at `params`.
    fn residual_norm_squared(&self, params: &[f64]) -> f64;

    /// Project a candidate parameter vector back into the feasible set (e.g.
    /// box bounds). Default is identity (unconstrained).
    fn project(&self, params: &mut [f64]) {
        let _ = params;
    }
}

/// Run Levenberg-Marquardt on any [`LeastSquares`] system, mutating `params`
/// in place. This is the generic core extracted from [`solve`]; the constraint
/// solver is now a thin wrapper around it, and differentiable design reuses it.
pub fn levenberg_marquardt<Sys: LeastSquares>(
    system: &Sys,
    params: &mut [f64],
    config: &SolverConfig,
) -> SolveResult {
    if params.is_empty() {
        return SolveResult {
            parameters: vec![],
            residual_norm: system.residual_norm_squared(params).sqrt(),
            iterations: 0,
            converged: false,
            status: SolveStatus::NoParameters,
        };
    }

    let mut lambda = config.initial_lambda;
    let mut current_norm_sq = system.residual_norm_squared(params);

    for iteration in 0..config.max_iterations {
        // Check convergence
        if current_norm_sq.sqrt() < config.tolerance {
            return SolveResult {
                parameters: params.to_vec(),
                residual_norm: current_norm_sq.sqrt(),
                iterations: iteration,
                converged: true,
                status: SolveStatus::Converged,
            };
        }

        // Evaluate J'J and J'r using the sparse Jacobian — skips known-zero entries.
        let (jtj, jtr_vec) = system.eval_jtj_jtr(params);
        let jtr = DVec::from_vec(jtr_vec);

        match try_step_generic(system, params, &jtj, &jtr, lambda) {
            StepResult::Accepted {
                new_params,
                new_norm_sq,
            } => {
                params.copy_from_slice(&new_params);
                current_norm_sq = new_norm_sq;
                // Decrease lambda (trust the linear approximation more).
                lambda = (lambda * config.lambda_decrease).max(config.min_lambda);
            }
            StepResult::Rejected => {
                // Increase lambda (be more conservative).
                lambda *= config.lambda_increase;
                if lambda > config.max_lambda {
                    return SolveResult {
                        parameters: params.to_vec(),
                        residual_norm: current_norm_sq.sqrt(),
                        iterations: iteration,
                        converged: false,
                        status: SolveStatus::LambdaOverflow,
                    };
                }
            }
            StepResult::SingularMatrix => {
                lambda *= config.lambda_increase;
                if lambda > config.max_lambda {
                    return SolveResult {
                        parameters: params.to_vec(),
                        residual_norm: current_norm_sq.sqrt(),
                        iterations: iteration,
                        converged: false,
                        status: SolveStatus::SingularMatrix,
                    };
                }
            }
        }
    }

    SolveResult {
        parameters: params.to_vec(),
        residual_norm: current_norm_sq.sqrt(),
        iterations: config.max_iterations,
        converged: false,
        status: SolveStatus::MaxIterations,
    }
}

enum StepResult {
    Accepted {
        new_params: Vec<f64>,
        new_norm_sq: f64,
    },
    Rejected,
    SingularMatrix,
}

/// Try a damped step `(J'J + λI) δ = -J'r`, projecting the candidate back into
/// the feasible set before measuring its cost.
fn try_step_generic<Sys: LeastSquares>(
    system: &Sys,
    params: &[f64],
    jtj: &DMat<f64>,
    jtr: &DVec<f64>,
    lambda: f64,
) -> StepResult {
    let n = jtj.nrows();

    // Form damped normal equations: (J'J + λI) δ = -J'r
    let mut a = jtj.clone();
    for i in 0..n {
        a[(i, i)] += lambda;
    }

    // Solve for step direction δ via LU decomposition.
    let delta = match a.clone().lu().solve(&(-jtr)) {
        Some(d) => d,
        None => return StepResult::SingularMatrix,
    };

    // Candidate parameters, projected back into the feasible set (box bounds).
    let mut new_params: Vec<f64> = params
        .iter()
        .enumerate()
        .map(|(i, &p)| p + delta[i])
        .collect();
    system.project(&mut new_params);

    let new_norm_sq = system.residual_norm_squared(&new_params);
    let old_norm_sq = system.residual_norm_squared(params);

    // Accept if the new norm is smaller.
    if new_norm_sq < old_norm_sq {
        StepResult::Accepted {
            new_params,
            new_norm_sq,
        }
    } else {
        StepResult::Rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::EntityRef;
    use crate::entity::{SketchLine, SketchPoint};

    #[test]
    fn test_solve_coincident() {
        // Two points that should become coincident
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        // Start with p1 at (0, 0), p2 at (10, 5)
        let mut params = vec![0.0, 0.0, 10.0, 5.0];

        let constraints = vec![Constraint::Coincident {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
        }];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged, "Solver should converge");
        // Points should be at the same location
        let dx = params[2] - params[0];
        let dy = params[3] - params[1];
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(
            dist < 1e-6,
            "Points should be coincident, distance = {dist}"
        );
    }

    #[test]
    fn test_solve_horizontal() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let line = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));

        // Start with diagonal line from (0, 0) to (10, 5)
        let mut params = vec![0.0, 0.0, 10.0, 5.0];

        let constraints = vec![Constraint::Horizontal { line }];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged, "Solver should converge");
        // Line should be horizontal: start.y == end.y
        let dy = (params[3] - params[1]).abs();
        assert!(dy < 1e-6, "Line should be horizontal, dy = {dy}");
    }

    #[test]
    fn test_solve_distance() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        // Start with p1 at (0, 0), p2 at (10, 0)
        let mut params = vec![0.0, 0.0, 10.0, 0.0];

        let constraints = vec![Constraint::Distance {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
            distance: 5.0,
        }];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged, "Solver should converge");
        // Distance should be 5
        let dx = params[2] - params[0];
        let dy = params[3] - params[1];
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(
            (dist - 5.0).abs() < 1e-6,
            "Distance should be 5, got {dist}"
        );
    }

    #[test]
    fn test_solve_fixed_point() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));

        // Start at (10, 20)
        let mut params = vec![10.0, 20.0];

        let constraints = vec![Constraint::Fixed {
            point: EntityRef::Point(p1),
            x: 5.0,
            y: 3.0,
        }];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged, "Solver should converge");
        assert!((params[0] - 5.0).abs() < 1e-6, "X should be 5");
        assert!((params[1] - 3.0).abs() < 1e-6, "Y should be 3");
    }

    #[test]
    fn test_solve_perpendicular() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let p3 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 4,
            param_y: 5,
        }));
        let p4 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 6,
            param_y: 7,
        }));
        let line1 = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));
        let line2 = entities.insert(SketchEntity::Line(SketchLine { start: p3, end: p4 }));

        // Line1: (0,0) to (10,0) - horizontal
        // Line2: (5,0) to (5,10) - vertical (already perpendicular)
        // Start with non-perpendicular lines
        let mut params = vec![0.0, 0.0, 10.0, 2.0, 5.0, 0.0, 8.0, 10.0];

        let constraints = vec![Constraint::Perpendicular {
            line_a: line1,
            line_b: line2,
        }];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged, "Solver should converge");

        // Check perpendicularity: dot product of direction vectors should be 0
        let d1x = params[2] - params[0];
        let d1y = params[3] - params[1];
        let d2x = params[6] - params[4];
        let d2y = params[7] - params[5];
        let len1 = (d1x * d1x + d1y * d1y).sqrt();
        let len2 = (d2x * d2x + d2y * d2y).sqrt();
        let dot = (d1x * d2x + d1y * d2y) / (len1 * len2);
        assert!(
            dot.abs() < 1e-6,
            "Lines should be perpendicular, dot = {dot}"
        );
    }

    #[test]
    fn test_no_constraints() {
        let entities: SlotMap<EntityId, SketchEntity> = SlotMap::with_key();
        let mut params = vec![1.0, 2.0];
        let constraints: Vec<Constraint> = vec![];

        let result = solve(
            &constraints,
            &mut params,
            &entities,
            &SolverConfig::default(),
        );

        assert!(result.converged);
        assert_eq!(result.status, SolveStatus::NoConstraints);
    }
}
