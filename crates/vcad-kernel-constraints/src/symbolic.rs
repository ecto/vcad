//! Symbolic Jacobian computation via tang-expr.
//!
//! Builds an expression graph from the constraint system, differentiates
//! symbolically, and compiles to fast closures. The compiled Jacobian is
//! cached and reused across solver iterations — only rebuilt when the
//! constraint topology changes.

use crate::constraint::{Constraint, EntityRef};
use crate::entity::{EntityId, SketchEntity};
use slotmap::SlotMap;
use tang::Scalar;
use tang_expr::{trace, ExprId};
use tang_la::DMat;

/// Compiled multi-output closure: maps input params to output values.
type CompiledFn = Box<dyn Fn(&[f64], &mut [f64])>;

/// Compiled Jacobian: closures that evaluate residuals and Jacobian entries.
pub struct CompiledSystem {
    /// Total number of residual equations.
    pub num_residuals: usize,
    /// Total number of parameters.
    pub num_params: usize,
    /// Evaluate all residuals given parameter values.
    residual_fn: CompiledFn,
    /// Evaluate the full Jacobian given parameter values (row-major).
    jacobian_fn: CompiledFn,
}

impl CompiledSystem {
    /// Build a compiled constraint system from constraints and entities.
    ///
    /// This traces the residual computation symbolically, differentiates
    /// each residual w.r.t. each parameter, simplifies, and compiles
    /// everything to closures.
    pub fn build(
        constraints: &[Constraint],
        entities: &SlotMap<EntityId, SketchEntity>,
        num_params: usize,
    ) -> Self {
        let num_residuals: usize = constraints.iter().map(|c| c.num_residuals()).sum();

        if num_residuals == 0 || num_params == 0 {
            return Self {
                num_residuals,
                num_params,
                residual_fn: Box::new(|_, _| {}),
                jacobian_fn: Box::new(|_, _| {}),
            };
        }

        let (mut graph, residual_exprs) = trace(|| build_residual_exprs(constraints, entities));

        // Differentiate each residual w.r.t. each parameter
        let mut jac_exprs = Vec::with_capacity(num_residuals * num_params);
        for r in &residual_exprs {
            for j in 0..num_params {
                let d = graph.diff(*r, j as u16);
                let d = graph.simplify(d);
                jac_exprs.push(d);
            }
        }

        // Simplify residuals too
        let residual_exprs: Vec<ExprId> = residual_exprs
            .into_iter()
            .map(|r| graph.simplify(r))
            .collect();

        // Compile
        let residual_fn = graph.compile_many(&residual_exprs);
        let jacobian_fn = graph.compile_many(&jac_exprs);

        Self {
            num_residuals,
            num_params,
            residual_fn,
            jacobian_fn,
        }
    }

    /// Evaluate all residuals.
    pub fn eval_residuals(&self, params: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.num_residuals];
        (self.residual_fn)(params, &mut out);
        out
    }

    /// Evaluate the Jacobian matrix.
    pub fn eval_jacobian(&self, params: &[f64]) -> DMat<f64> {
        let nr = self.num_residuals;
        let np = self.num_params;
        let mut flat = vec![0.0; nr * np];
        (self.jacobian_fn)(params, &mut flat);
        // flat is row-major: [dr0/dp0, dr0/dp1, ..., dr1/dp0, ...]
        let mut j = DMat::zeros(nr, np);
        for i in 0..nr {
            for k in 0..np {
                j[(i, k)] = flat[i * np + k];
            }
        }
        j
    }

    /// Evaluate residual squared norm.
    pub fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        let r = self.eval_residuals(params);
        r.iter().map(|v| v * v).sum()
    }
}

/// Build symbolic residual expressions for all constraints.
///
/// Each parameter maps to `ExprId::var(param_index)`. Entity lookups
/// are resolved at graph-build time (structural, not mathematical).
fn build_residual_exprs(
    constraints: &[Constraint],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> Vec<ExprId> {
    let mut residuals = Vec::new();

    for constraint in constraints {
        match constraint {
            Constraint::Coincident { point_a, point_b } => {
                let (ax, ay) = sym_point(*point_a, entities);
                let (bx, by) = sym_point(*point_b, entities);
                residuals.push(ax - bx);
                residuals.push(ay - by);
            }

            Constraint::PointOnLine { point, line } => {
                let (px, py) = sym_point(*point, entities);
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let dx = ex - sx;
                let dy = ey - sy;
                let len = (dx * dx + dy * dy).sqrt();
                // Cross product / length = signed distance
                let dist = ((px - sx) * dy - (py - sy) * dx) / len;
                residuals.push(dist);
            }

            Constraint::Parallel { line_a, line_b } => {
                let (s1x, s1y, e1x, e1y) = sym_line(*line_a, entities);
                let (s2x, s2y, e2x, e2y) = sym_line(*line_b, entities);
                let d1x = e1x - s1x;
                let d1y = e1y - s1y;
                let d2x = e2x - s2x;
                let d2y = e2y - s2y;
                let len1 = (d1x * d1x + d1y * d1y).sqrt();
                let len2 = (d2x * d2x + d2y * d2y).sqrt();
                let cross = (d1x * d2y - d1y * d2x) / (len1 * len2);
                residuals.push(cross);
            }

            Constraint::Perpendicular { line_a, line_b } => {
                let (s1x, s1y, e1x, e1y) = sym_line(*line_a, entities);
                let (s2x, s2y, e2x, e2y) = sym_line(*line_b, entities);
                let d1x = e1x - s1x;
                let d1y = e1y - s1y;
                let d2x = e2x - s2x;
                let d2y = e2y - s2y;
                let len1 = (d1x * d1x + d1y * d1y).sqrt();
                let len2 = (d2x * d2x + d2y * d2y).sqrt();
                let dot = (d1x * d2x + d1y * d2y) / (len1 * len2);
                residuals.push(dot);
            }

            Constraint::Horizontal { line } => {
                let (_sx, sy, _ex, ey) = sym_line(*line, entities);
                residuals.push(ey - sy);
            }

            Constraint::Vertical { line } => {
                let (sx, _sy, ex, _ey) = sym_line(*line, entities);
                residuals.push(ex - sx);
            }

            Constraint::Tangent {
                line,
                curve,
                at_point,
            } => {
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let (cx, cy) = sym_circle_center(*curve, entities);
                let (px, py) = sym_point(*at_point, entities);
                let ldx = ex - sx;
                let ldy = ey - sy;
                let rdx = px - cx;
                let rdy = py - cy;
                let line_len = (ldx * ldx + ldy * ldy).sqrt();
                let rad_len = (rdx * rdx + rdy * rdy).sqrt();
                let dot = (ldx * rdx + ldy * rdy) / (line_len * rad_len);
                residuals.push(dot);
            }

            Constraint::EqualLength { line_a, line_b } => {
                let (s1x, s1y, e1x, e1y) = sym_line(*line_a, entities);
                let (s2x, s2y, e2x, e2y) = sym_line(*line_b, entities);
                let d1x = e1x - s1x;
                let d1y = e1y - s1y;
                let d2x = e2x - s2x;
                let d2y = e2y - s2y;
                let len1 = (d1x * d1x + d1y * d1y).sqrt();
                let len2 = (d2x * d2x + d2y * d2y).sqrt();
                residuals.push(len1 - len2);
            }

            Constraint::EqualRadius { circle_a, circle_b } => {
                let r1 = sym_radius(*circle_a, entities);
                let r2 = sym_radius(*circle_b, entities);
                residuals.push(r1 - r2);
            }

            Constraint::Concentric { circle_a, circle_b } => {
                let (c1x, c1y) = sym_circle_center(*circle_a, entities);
                let (c2x, c2y) = sym_circle_center(*circle_b, entities);
                residuals.push(c1x - c2x);
                residuals.push(c1y - c2y);
            }

            Constraint::Fixed { point, x, y } => {
                let (px, py) = sym_point(*point, entities);
                let tx = ExprId::from_f64(*x);
                let ty = ExprId::from_f64(*y);
                residuals.push(px - tx);
                residuals.push(py - ty);
            }

            Constraint::PointOnCircle { point, circle } => {
                let (px, py) = sym_point(*point, entities);
                let (cx, cy) = sym_circle_center(*circle, entities);
                let radius = sym_radius(*circle, entities);
                let dx = px - cx;
                let dy = py - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                residuals.push(dist - radius);
            }

            Constraint::LineThroughCenter { line, circle } => {
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let (cx, cy) = sym_circle_center(*circle, entities);
                let dx = ex - sx;
                let dy = ey - sy;
                let len = (dx * dx + dy * dy).sqrt();
                let dist = ((cx - sx) * dy - (cy - sy) * dx) / len;
                residuals.push(dist);
            }

            Constraint::Midpoint { point, line } => {
                let (px, py) = sym_point(*point, entities);
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let half = ExprId::from_f64(0.5);
                let mx = (sx + ex) * half;
                let my = (sy + ey) * half;
                residuals.push(px - mx);
                residuals.push(py - my);
            }

            Constraint::Symmetric {
                point_a,
                point_b,
                axis,
            } => {
                let (ax, ay) = sym_point(*point_a, entities);
                let (bx, by) = sym_point(*point_b, entities);
                let (sx, sy, ex, ey) = sym_line(*axis, entities);
                let half = ExprId::from_f64(0.5);
                let mx = (ax + bx) * half;
                let my = (ay + by) * half;
                let dx = ex - sx;
                let dy = ey - sy;
                let len = (dx * dx + dy * dy).sqrt();
                let dist_to_axis = ((mx - sx) * dy - (my - sy) * dx) / len;
                let abx = bx - ax;
                let aby = by - ay;
                let ab_len = (abx * abx + aby * aby).sqrt();
                let perp = (abx * dx + aby * dy) / (ab_len * len);
                residuals.push(dist_to_axis);
                residuals.push(perp);
            }

            Constraint::Distance {
                point_a,
                point_b,
                distance,
            } => {
                let (ax, ay) = sym_point(*point_a, entities);
                let (bx, by) = sym_point(*point_b, entities);
                let dx = bx - ax;
                let dy = by - ay;
                let dist = (dx * dx + dy * dy).sqrt();
                let target = ExprId::from_f64(*distance);
                residuals.push(dist - target);
            }

            Constraint::PointLineDistance {
                point,
                line,
                distance,
            } => {
                let (px, py) = sym_point(*point, entities);
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let dx = ex - sx;
                let dy = ey - sy;
                let len = (dx * dx + dy * dy).sqrt();
                let signed_dist = ((px - sx) * dy - (py - sy) * dx) / len;
                // abs(signed_dist) via sqrt(x^2)
                let abs_dist = (signed_dist * signed_dist).sqrt();
                let target = ExprId::from_f64(*distance);
                residuals.push(abs_dist - target);
            }

            Constraint::Angle {
                line_a,
                line_b,
                angle_rad,
            } => {
                let (s1x, s1y, e1x, e1y) = sym_line(*line_a, entities);
                let (s2x, s2y, e2x, e2y) = sym_line(*line_b, entities);
                let d1x = e1x - s1x;
                let d1y = e1y - s1y;
                let d2x = e2x - s2x;
                let d2y = e2y - s2y;
                let len1 = (d1x * d1x + d1y * d1y).sqrt();
                let len2 = (d2x * d2x + d2y * d2y).sqrt();
                let cos_a = (d1x * d2x + d1y * d2y) / (len1 * len2);
                let sin_a = (d1x * d2y - d1y * d2x) / (len1 * len2);
                let actual_angle = sin_a.atan2(cos_a);
                let target = ExprId::from_f64(*angle_rad);
                // Simple difference (no modular wrapping in symbolic form —
                // the solver handles small perturbations where wrapping isn't needed)
                residuals.push(actual_angle - target);
            }

            Constraint::Radius { circle, radius } => {
                let r = sym_radius(*circle, entities);
                let target = ExprId::from_f64(*radius);
                residuals.push(r - target);
            }

            Constraint::Length { line, length } => {
                let (sx, sy, ex, ey) = sym_line(*line, entities);
                let dx = ex - sx;
                let dy = ey - sy;
                let len = (dx * dx + dy * dy).sqrt();
                let target = ExprId::from_f64(*length);
                residuals.push(len - target);
            }

            Constraint::HorizontalDistance { point, x } => {
                let (px, _py) = sym_point(*point, entities);
                let target = ExprId::from_f64(*x);
                residuals.push(px - target);
            }

            Constraint::VerticalDistance { point, y } => {
                let (_px, py) = sym_point(*point, entities);
                let target = ExprId::from_f64(*y);
                residuals.push(py - target);
            }

            Constraint::Diameter { circle, diameter } => {
                let r = sym_radius(*circle, entities);
                let two = ExprId::from_f64(2.0);
                let target = ExprId::from_f64(*diameter);
                residuals.push(two * r - target);
            }
        }
    }

    residuals
}

// --- Entity → ExprId helpers ---
// These resolve entity references to symbolic variables at graph-build time.

fn sym_point(point_ref: EntityRef, entities: &SlotMap<EntityId, SketchEntity>) -> (ExprId, ExprId) {
    match point_ref {
        EntityRef::Point(id) => {
            if let Some(SketchEntity::Point(p)) = entities.get(id) {
                (ExprId::var(p.param_x as u16), ExprId::var(p.param_y as u16))
            } else {
                (ExprId::ZERO, ExprId::ZERO)
            }
        }
        EntityRef::LineStart(id) => {
            if let Some(SketchEntity::Line(l)) = entities.get(id) {
                sym_point(EntityRef::Point(l.start), entities)
            } else {
                (ExprId::ZERO, ExprId::ZERO)
            }
        }
        EntityRef::LineEnd(id) => {
            if let Some(SketchEntity::Line(l)) = entities.get(id) {
                sym_point(EntityRef::Point(l.end), entities)
            } else {
                (ExprId::ZERO, ExprId::ZERO)
            }
        }
        EntityRef::Center(id) => sym_circle_center(id, entities),
        EntityRef::ArcStart(id) => {
            if let Some(SketchEntity::Arc(a)) = entities.get(id) {
                sym_point(EntityRef::Point(a.start), entities)
            } else {
                (ExprId::ZERO, ExprId::ZERO)
            }
        }
        EntityRef::ArcEnd(id) => {
            if let Some(SketchEntity::Arc(a)) = entities.get(id) {
                sym_point(EntityRef::Point(a.end), entities)
            } else {
                (ExprId::ZERO, ExprId::ZERO)
            }
        }
    }
}

fn sym_line(
    line_id: EntityId,
    entities: &SlotMap<EntityId, SketchEntity>,
) -> (ExprId, ExprId, ExprId, ExprId) {
    if let Some(SketchEntity::Line(l)) = entities.get(line_id) {
        let (sx, sy) = sym_point(EntityRef::Point(l.start), entities);
        let (ex, ey) = sym_point(EntityRef::Point(l.end), entities);
        (sx, sy, ex, ey)
    } else {
        (ExprId::ZERO, ExprId::ZERO, ExprId::ZERO, ExprId::ZERO)
    }
}

fn sym_circle_center(id: EntityId, entities: &SlotMap<EntityId, SketchEntity>) -> (ExprId, ExprId) {
    match entities.get(id) {
        Some(SketchEntity::Circle(c)) => sym_point(EntityRef::Point(c.center), entities),
        Some(SketchEntity::Arc(a)) => sym_point(EntityRef::Point(a.center), entities),
        _ => (ExprId::ZERO, ExprId::ZERO),
    }
}

fn sym_radius(id: EntityId, entities: &SlotMap<EntityId, SketchEntity>) -> ExprId {
    match entities.get(id) {
        Some(SketchEntity::Circle(c)) => ExprId::var(c.param_radius as u16),
        Some(SketchEntity::Arc(a)) => {
            let (cx, cy) = sym_point(EntityRef::Point(a.center), entities);
            let (sx, sy) = sym_point(EntityRef::Point(a.start), entities);
            let dx = sx - cx;
            let dy = sy - cy;
            (dx * dx + dy * dy).sqrt()
        }
        _ => ExprId::ZERO,
    }
}

/// Compute Jacobian using the symbolic/compiled system.
///
/// Drop-in replacement for `compute_jacobian` in `jacobian.rs`.
pub fn compute_jacobian_symbolic(
    constraints: &[Constraint],
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> DMat<f64> {
    let system = CompiledSystem::build(constraints, entities, params.len());
    system.eval_jacobian(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::EntityRef;
    use crate::entity::{SketchLine, SketchPoint};
    use crate::jacobian::compute_jacobian;

    /// Helper: compare two Jacobians element-wise.
    fn assert_jacobians_match(j_fd: &DMat<f64>, j_sym: &DMat<f64>, tol: f64) {
        assert_eq!(j_fd.nrows(), j_sym.nrows(), "row count mismatch");
        assert_eq!(j_fd.ncols(), j_sym.ncols(), "col count mismatch");
        for i in 0..j_fd.nrows() {
            for j in 0..j_fd.ncols() {
                let fd = j_fd[(i, j)];
                let sym = j_sym[(i, j)];
                assert!(
                    (fd - sym).abs() < tol,
                    "Jacobian mismatch at ({i}, {j}): FD={fd}, symbolic={sym}, diff={}",
                    (fd - sym).abs()
                );
            }
        }
    }

    #[test]
    fn symbolic_vs_fd_horizontal() {
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

        let params = vec![0.0, 0.0, 10.0, 5.0];
        let constraints = vec![Constraint::Horizontal { line }];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        assert_jacobians_match(&j_fd, &j_sym, 1e-6);
    }

    #[test]
    fn symbolic_vs_fd_distance() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        let params = vec![0.0, 0.0, 3.0, 4.0];
        let constraints = vec![Constraint::Distance {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
            distance: 5.0,
        }];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        assert_jacobians_match(&j_fd, &j_sym, 1e-6);
    }

    #[test]
    fn symbolic_vs_fd_coincident() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        let params = vec![1.0, 2.0, 5.0, 7.0];
        let constraints = vec![Constraint::Coincident {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
        }];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        assert_jacobians_match(&j_fd, &j_sym, 1e-6);
    }

    #[test]
    fn symbolic_vs_fd_perpendicular() {
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

        let params = vec![0.0, 0.0, 10.0, 2.0, 5.0, 0.0, 8.0, 10.0];
        let constraints = vec![Constraint::Perpendicular {
            line_a: line1,
            line_b: line2,
        }];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        assert_jacobians_match(&j_fd, &j_sym, 1e-5);
    }

    #[test]
    fn symbolic_vs_fd_fixed() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));

        let params = vec![3.0, 7.0];
        let constraints = vec![Constraint::Fixed {
            point: EntityRef::Point(p1),
            x: 5.0,
            y: 10.0,
        }];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        assert_jacobians_match(&j_fd, &j_sym, 1e-6);
    }

    #[test]
    fn symbolic_vs_fd_mixed() {
        // Multiple constraints at once
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
        let line = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));

        let params = vec![0.0, 0.0, 10.0, 3.0, 5.0, 1.5];
        let constraints = vec![
            Constraint::Horizontal { line },
            Constraint::Distance {
                point_a: EntityRef::Point(p1),
                point_b: EntityRef::Point(p2),
                distance: 10.0,
            },
            Constraint::Fixed {
                point: EntityRef::Point(p3),
                x: 5.0,
                y: 1.5,
            },
        ];

        let j_fd = compute_jacobian(&constraints, &params, &entities);
        let j_sym = compute_jacobian_symbolic(&constraints, &params, &entities);

        // 1 + 1 + 2 = 4 residuals, 6 params
        assert_eq!(j_sym.nrows(), 4);
        assert_eq!(j_sym.ncols(), 6);
        assert_jacobians_match(&j_fd, &j_sym, 1e-5);
    }

    #[test]
    fn symbolic_residuals_match() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));

        let params = vec![0.0, 0.0, 3.0, 4.0];
        let constraints = vec![Constraint::Distance {
            point_a: EntityRef::Point(p1),
            point_b: EntityRef::Point(p2),
            distance: 5.0,
        }];

        let system = CompiledSystem::build(&constraints, &entities, params.len());
        let residuals = system.eval_residuals(&params);

        // distance = sqrt(9 + 16) = 5, error = 5 - 5 = 0
        assert_eq!(residuals.len(), 1);
        assert!(residuals[0].abs() < 1e-10);
    }

    #[test]
    fn compiled_system_reuse() {
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

        let constraints = vec![Constraint::Horizontal { line }];
        let system = CompiledSystem::build(&constraints, &entities, 4);

        // Evaluate at different parameter values — same compiled system
        let j1 = system.eval_jacobian(&[0.0, 0.0, 10.0, 5.0]);
        let j2 = system.eval_jacobian(&[1.0, 2.0, 8.0, 3.0]);

        // Horizontal: error = ey - sy, so d/d(sy) = -1, d/d(ey) = 1, others = 0
        // This is constant regardless of parameter values
        assert!((j1[(0, 1)] - (-1.0)).abs() < 1e-10);
        assert!((j1[(0, 3)] - 1.0).abs() < 1e-10);
        assert!((j2[(0, 1)] - (-1.0)).abs() < 1e-10);
        assert!((j2[(0, 3)] - 1.0).abs() < 1e-10);
    }
}
