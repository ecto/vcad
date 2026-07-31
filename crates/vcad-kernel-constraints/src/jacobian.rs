//! Jacobian matrix computation using finite differences.
//!
//! The Jacobian matrix J has dimensions (num_residuals × num_parameters),
//! where J\[i,j\] = ∂r_i/∂p_j is the partial derivative of the i-th residual
//! with respect to the j-th parameter.
//!
//! Note: the solver now uses symbolic Jacobians via [`crate::symbolic::CompiledSystem`].
//! These functions are retained as a reference implementation for tests.

#![allow(dead_code)]

use crate::constraint::Constraint;
use crate::entity::{EntityId, SketchEntity};
use crate::residual::compute_constraint_residuals;
use slotmap::SlotMap;
use tang_la::DMat;

/// Step size for finite difference computation.
const EPSILON: f64 = 1e-8;

/// Compute the Jacobian matrix for all constraints using central finite differences.
///
/// Central differences give better accuracy than forward differences:
/// J\[i,j\] ≈ (r_i(p + h·e_j) - r_i(p - h·e_j)) / (2h)
///
/// # Arguments
///
/// * `constraints` - The list of constraints
/// * `params` - Current parameter values
/// * `entities` - Entity definitions
///
/// # Returns
///
/// Jacobian matrix with shape (total_residuals × num_parameters)
pub fn compute_jacobian(
    constraints: &[Constraint],
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> DMat<f64> {
    // Count total residuals
    let num_residuals: usize = constraints.iter().map(|c| c.num_residuals()).sum();
    let num_params = params.len();

    if num_residuals == 0 || num_params == 0 {
        return DMat::zeros(num_residuals.max(1), num_params.max(1));
    }

    let mut jacobian = DMat::zeros(num_residuals, num_params);

    // For each parameter, compute partial derivatives using central differences
    for j in 0..num_params {
        // Compute r(p + h·e_j)
        let mut params_plus = params.to_vec();
        params_plus[j] += EPSILON;
        let residuals_plus = compute_all_residuals(constraints, &params_plus, entities);

        // Compute r(p - h·e_j)
        let mut params_minus = params.to_vec();
        params_minus[j] -= EPSILON;
        let residuals_minus = compute_all_residuals(constraints, &params_minus, entities);

        // Central difference for each residual
        for i in 0..num_residuals {
            jacobian[(i, j)] = (residuals_plus[i] - residuals_minus[i]) / (2.0 * EPSILON);
        }
    }

    jacobian
}

/// Compute all residuals for all constraints, flattened into a single vector.
pub fn compute_all_residuals(
    constraints: &[Constraint],
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> Vec<f64> {
    constraints
        .iter()
        .flat_map(|c| compute_constraint_residuals(c, params, entities))
        .collect()
}

/// Compute the squared norm of all residuals.
pub fn residual_norm_squared(
    constraints: &[Constraint],
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> f64 {
    compute_all_residuals(constraints, params, entities)
        .iter()
        .map(|r| r * r)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::EntityRef;
    use crate::entity::{SketchLine, SketchPoint};

    #[test]
    fn test_jacobian_shape() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        let params = vec![0.0, 0.0, 10.0, 0.0]; // 4 parameters

        let constraints = vec![
            Constraint::Coincident {
                point_a: EntityRef::Point(p1),
                point_b: EntityRef::Point(p2),
            }, // 2 residuals
            Constraint::Distance {
                point_a: EntityRef::Point(p1),
                point_b: EntityRef::Point(p2),
                distance: 10.0,
            }, // 1 residual
        ];

        let j = compute_jacobian(&constraints, &params, &entities);
        assert_eq!(j.nrows(), 3); // 2 + 1 residuals
        assert_eq!(j.ncols(), 4); // 4 parameters
    }

    #[test]
    fn test_jacobian_horizontal_constraint() {
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

        // p1 at (0, 0), p2 at (10, 5)
        let params = vec![0.0, 0.0, 10.0, 5.0];

        let constraints = vec![Constraint::Horizontal { line }];
        let j = compute_jacobian(&constraints, &params, &entities);

        // Horizontal constraint: error = end.y - start.y = params[3] - params[1]
        // ∂/∂params[0] = 0
        // ∂/∂params[1] = -1
        // ∂/∂params[2] = 0
        // ∂/∂params[3] = 1
        assert!((j[(0, 0)] - 0.0).abs() < 1e-6);
        assert!((j[(0, 1)] - (-1.0)).abs() < 1e-6);
        assert!((j[(0, 2)] - 0.0).abs() < 1e-6);
        assert!((j[(0, 3)] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_jacobian_distance_constraint() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        // p1 at (0, 0), p2 at (3, 4) - distance = 5
        let params = vec![0.0, 0.0, 3.0, 4.0];

        let constraints = vec![Constraint::Distance {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
            distance: 5.0,
        }];

        let j = compute_jacobian(&constraints, &params, &entities);

        // Distance error = sqrt((x2-x1)^2 + (y2-y1)^2) - d
        // Let dx = x2-x1, dy = y2-y1, r = sqrt(dx^2 + dy^2)
        // ∂r/∂x1 = -dx/r, ∂r/∂y1 = -dy/r, ∂r/∂x2 = dx/r, ∂r/∂y2 = dy/r
        // With dx=3, dy=4, r=5:
        assert!((j[(0, 0)] - (-0.6)).abs() < 1e-6);
        assert!((j[(0, 1)] - (-0.8)).abs() < 1e-6);
        assert!((j[(0, 2)] - 0.6).abs() < 1e-6);
        assert!((j[(0, 3)] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_residual_norm_squared() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        // p1 at (0, 0), p2 at (10, 0) - distance = 10
        let params = vec![0.0, 0.0, 10.0, 0.0];

        let constraints = vec![Constraint::Distance {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
            distance: 8.0, // error = 10 - 8 = 2
        }];

        let norm_sq = residual_norm_squared(&constraints, &params, &entities);
        assert!((norm_sq - 4.0).abs() < 1e-12); // 2^2 = 4
    }
}
