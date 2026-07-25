//! Symbolic Jacobian computation via tang-expr.
//!
//! Builds an expression graph from the constraint system, differentiates
//! symbolically, and compiles to fast closures. The compiled Jacobian is
//! cached and reused across solver iterations — only rebuilt when the
//! constraint topology changes.
//!
//! ## Sparsity-aware compilation
//!
//! Most constraints only touch a handful of parameters (e.g. a horizontal
//! constraint depends on 2 out of potentially hundreds of parameters). The
//! [`CompiledSystem`] uses tang-expr's `jacobian_sparsity()` to identify
//! which Jacobian entries are structurally non-zero **before** differentiating,
//! then only compiles closures for the non-zero entries. For a sketch with
//! N points and M constraints, this typically reduces compiled expressions
//! from M*2N to roughly M*6 — a significant speedup for large sketches.

use crate::constraint::{Constraint, EntityRef};
use crate::entity::{EntityId, SketchEntity};
use slotmap::SlotMap;
use tang::Scalar;
use tang_expr::{trace, ExprId};
use tang_la::DMat;

/// Compiled multi-output closure: maps input params to output values.
type CompiledFn = Box<dyn Fn(&[f64], &mut [f64])>;

/// Sparse Jacobian entry: (residual_row, param_col).
#[derive(Debug, Clone, Copy)]
pub struct SparseEntry {
    /// Residual index (row in the Jacobian).
    pub row: usize,
    /// Parameter index (column in the Jacobian).
    pub col: usize,
}

/// Compiled Jacobian: closures that evaluate residuals and Jacobian entries.
///
/// The Jacobian is stored in a sparse format — only structurally non-zero
/// entries are compiled and evaluated.
pub struct CompiledSystem {
    /// Total number of residual equations.
    pub num_residuals: usize,
    /// Total number of parameters.
    pub num_params: usize,
    /// Evaluate all residuals given parameter values.
    residual_fn: CompiledFn,
    /// Evaluate non-zero Jacobian entries given parameter values.
    /// Output length equals `sparse_entries.len()`.
    jacobian_fn: CompiledFn,
    /// Sparsity pattern: which (row, col) positions are non-zero.
    /// Ordered row-major (by residual index, then by param index).
    sparse_entries: Vec<SparseEntry>,
    /// Number of non-zero Jacobian entries that were compiled.
    pub num_nonzero: usize,
    /// Total dense Jacobian size (for comparison / statistics).
    pub dense_size: usize,
}

impl CompiledSystem {
    /// Build a compiled constraint system from constraints and entities.
    ///
    /// This traces the residual computation symbolically, uses sparsity
    /// analysis to identify non-zero Jacobian entries, differentiates only
    /// those entries, simplifies, and compiles everything to closures.
    pub fn build(
        constraints: &[Constraint],
        entities: &SlotMap<EntityId, SketchEntity>,
        num_params: usize,
    ) -> Self {
        let num_residuals: usize = constraints.iter().map(|c| c.num_residuals()).sum();
        let dense_size = num_residuals * num_params;

        if num_residuals == 0 || num_params == 0 {
            return Self {
                num_residuals,
                num_params,
                residual_fn: Box::new(|_, _| {}),
                jacobian_fn: Box::new(|_, _| {}),
                sparse_entries: Vec::new(),
                num_nonzero: 0,
                dense_size,
            };
        }

        let (mut graph, residual_exprs) = trace(|| build_residual_exprs(constraints, entities));

        // Use sparsity analysis to find which Jacobian entries are non-zero.
        // Each bitmask has bit j set if residual[i] depends on Var(j).
        // Falls back to dense if num_params > 64 (bitmask limit).
        let use_sparsity = num_params <= 64;
        let sparsity_masks: Vec<u64> = if use_sparsity {
            graph.jacobian_sparsity(&residual_exprs, num_params)
        } else {
            // All bits set — treat everything as non-zero
            vec![u64::MAX; num_residuals]
        };

        // Build the sparse entry list and differentiate only non-zero entries
        let mut sparse_entries = Vec::new();
        let mut jac_exprs = Vec::new();

        for (i, r) in residual_exprs.iter().enumerate() {
            let mask = sparsity_masks[i];
            for j in 0..num_params {
                if !use_sparsity || mask & (1u64 << j) != 0 {
                    sparse_entries.push(SparseEntry { row: i, col: j });
                    let d = graph.diff(*r, j as u16);
                    let d = graph.simplify(d);
                    jac_exprs.push(d);
                }
            }
        }

        let num_nonzero = sparse_entries.len();

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
            sparse_entries,
            num_nonzero,
            dense_size,
        }
    }

    /// Evaluate all residuals.
    pub fn eval_residuals(&self, params: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.num_residuals];
        (self.residual_fn)(params, &mut out);
        out
    }

    /// Evaluate non-zero Jacobian entries (sparse).
    ///
    /// Returns a vector of values corresponding to `self.sparse_entries`.
    /// Use [`sparse_entries()`](Self::sparse_entries) to get the (row, col) positions.
    pub fn eval_jacobian_sparse(&self, params: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.num_nonzero];
        (self.jacobian_fn)(params, &mut out);
        out
    }

    /// Get the sparsity pattern of the Jacobian.
    pub fn sparse_entries(&self) -> &[SparseEntry] {
        &self.sparse_entries
    }

    /// Evaluate the Jacobian matrix (dense).
    ///
    /// Reconstructs a full dense matrix from the sparse evaluation.
    pub fn eval_jacobian(&self, params: &[f64]) -> DMat<f64> {
        let nr = self.num_residuals;
        let np = self.num_params;
        let mut j = DMat::zeros(nr, np);

        let values = self.eval_jacobian_sparse(params);
        for (idx, entry) in self.sparse_entries.iter().enumerate() {
            j[(entry.row, entry.col)] = values[idx];
        }

        j
    }

    /// Compute J'J and J'r using sparse Jacobian evaluation.
    ///
    /// Instead of forming the full J matrix and computing J'J via dense
    /// matrix multiplication, this accumulates only non-zero contributions:
    ///
    /// ```text
    /// (J'J)[p, q] = sum_i J[i,p] * J[i,q]
    /// ```
    ///
    /// For each residual row, we only iterate over its non-zero columns.
    pub fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>) {
        let np = self.num_params;
        let values = self.eval_jacobian_sparse(params);
        let residuals = self.eval_residuals(params);

        let mut jtj = DMat::zeros(np, np);
        let mut jtr = vec![0.0; np];

        // Group sparse entries by row for efficient accumulation.
        // Since entries are stored row-major, we can iterate once.
        let mut idx = 0;
        let total = self.sparse_entries.len();

        while idx < total {
            let row = self.sparse_entries[idx].row;
            let row_start = idx;

            // Find extent of this row's entries
            while idx < total && self.sparse_entries[idx].row == row {
                idx += 1;
            }
            let row_end = idx;

            let r_i = residuals[row];

            // Accumulate J'r: jtr[p] += J[i,p] * r[i]
            let row_entries = &self.sparse_entries[row_start..row_end];
            let row_values = &values[row_start..row_end];

            for (entry, &j_ip) in row_entries.iter().zip(row_values.iter()) {
                jtr[entry.col] += j_ip * r_i;
            }

            // Accumulate J'J: jtj[p,q] += J[i,p] * J[i,q]
            // Only need to iterate non-zero cols for this row
            for (e1, &j_ip) in row_entries.iter().zip(row_values.iter()) {
                for (e2, &j_iq) in row_entries.iter().zip(row_values.iter()) {
                    jtj[(e1.col, e2.col)] += j_ip * j_iq;
                }
            }
        }

        (jtj, jtr)
    }

    /// Evaluate residual squared norm.
    pub fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        let r = self.eval_residuals(params);
        r.iter().map(|v| v * v).sum()
    }

    /// Sparsity ratio: fraction of Jacobian entries that are non-zero.
    ///
    /// Returns a value between 0.0 and 1.0. Lower means sparser.
    pub fn sparsity_ratio(&self) -> f64 {
        if self.dense_size == 0 {
            return 0.0;
        }
        self.num_nonzero as f64 / self.dense_size as f64
    }
}

/// Bridge to the generic Levenberg-Marquardt driver. Sketch constraints are
/// unbounded, so `project` keeps its default no-op — making `solve` behave
/// exactly as before the driver was extracted. Inherent methods are called by
/// explicit path to avoid resolving back into the trait method (recursion).
impl crate::solver::LeastSquares for CompiledSystem {
    fn num_params(&self) -> usize {
        self.num_params
    }
    fn eval_jtj_jtr(&self, params: &[f64]) -> (DMat<f64>, Vec<f64>) {
        CompiledSystem::eval_jtj_jtr(self, params)
    }
    fn residual_norm_squared(&self, params: &[f64]) -> f64 {
        CompiledSystem::residual_norm_squared(self, params)
    }
}

/// Build symbolic residual expressions for all constraints.
///
/// Each parameter maps to `ExprId::var(param_index)`. Entity lookups
/// are resolved at graph-build time (structural, not mathematical).
pub(crate) fn build_residual_exprs(
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

            Constraint::OffsetCoincident {
                point_a,
                point_b,
                dx,
                dy,
            } => {
                let (ax, ay) = sym_point(*point_a, entities);
                let (bx, by) = sym_point(*point_b, entities);
                residuals.push(bx - ax - ExprId::from_f64(*dx));
                residuals.push(by - ay - ExprId::from_f64(*dy));
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

    // =========================================================================
    // Sparsity tests
    // =========================================================================

    #[test]
    fn sparsity_horizontal_only_touches_y_coords() {
        // Horizontal constraint: error = ey - sy
        // Only depends on params 1 (sy) and 3 (ey), not 0 (sx) or 2 (ex).
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

        // Dense would be 1*4 = 4, sparse should be 2 (only params 1 and 3)
        assert_eq!(system.dense_size, 4);
        assert_eq!(system.num_nonzero, 2);

        // Verify the non-zero entries are at the correct positions
        let cols: Vec<usize> = system.sparse_entries.iter().map(|e| e.col).collect();
        assert!(cols.contains(&1), "Should depend on param 1 (sy)");
        assert!(cols.contains(&3), "Should depend on param 3 (ey)");
    }

    #[test]
    fn sparsity_mixed_constraints() {
        // 3 points, 6 params. Constraints only touch subsets.
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

        let constraints = vec![
            Constraint::Horizontal { line }, // touches params 1, 3
            Constraint::Distance {
                // touches params 0, 1, 2, 3
                point_a: EntityRef::Point(p1),
                point_b: EntityRef::Point(p2),
                distance: 10.0,
            },
            Constraint::Fixed {
                // touches params 4, 5
                point: EntityRef::Point(p3),
                x: 5.0,
                y: 1.5,
            },
        ];

        let system = CompiledSystem::build(&constraints, &entities, 6);

        // Dense: 4 residuals * 6 params = 24
        // Sparse: horizontal=2 + distance=4 + fixed(2 residuals, each touches 1)=2 = 8
        assert_eq!(system.dense_size, 24);
        assert_eq!(system.num_nonzero, 8);
        assert!(system.sparsity_ratio() < 0.5);
    }

    #[test]
    fn sparsity_eval_jtj_jtr_matches_dense() {
        // Verify that eval_jtj_jtr produces the same result as dense J'J and J'r.
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

        let system = CompiledSystem::build(&constraints, &entities, params.len());

        // Dense path: J'J and J'r via full matrix
        let j = system.eval_jacobian(&params);
        let r = tang_la::DVec::from_vec(system.eval_residuals(&params));
        let jt = j.transpose();
        let jtj_dense = &jt * &j;
        let jtr_dense = &jt * &r;

        // Sparse path
        let (jtj_sparse, jtr_sparse) = system.eval_jtj_jtr(&params);

        // Compare
        let np = params.len();
        for i in 0..np {
            assert!(
                (jtr_dense[i] - jtr_sparse[i]).abs() < 1e-10,
                "J'r mismatch at {i}: dense={}, sparse={}",
                jtr_dense[i],
                jtr_sparse[i]
            );
            for k in 0..np {
                assert!(
                    (jtj_dense[(i, k)] - jtj_sparse[(i, k)]).abs() < 1e-10,
                    "J'J mismatch at ({i},{k}): dense={}, sparse={}",
                    jtj_dense[(i, k)],
                    jtj_sparse[(i, k)]
                );
            }
        }
    }

    #[test]
    fn sparsity_large_sketch_reduction() {
        // Build a "grid" sketch with 25 points (50 params) and independent
        // constraints. Verify significant sparsity reduction.
        let mut entities = SlotMap::with_key();
        let mut points = Vec::new();
        let mut param_idx = 0;
        for i in 0..25 {
            let _x = (i % 5) as f64 * 10.0;
            let _y = (i / 5) as f64 * 10.0;
            let p = entities.insert(SketchEntity::Point(SketchPoint {
                param_x: param_idx,
                param_y: param_idx + 1,
            }));
            points.push(p);
            param_idx += 2;
        }

        // Create horizontal lines between adjacent pairs in each row
        let mut lines = Vec::new();
        for row in 0..5 {
            for col in 0..4 {
                let start = points[row * 5 + col];
                let end = points[row * 5 + col + 1];
                let line = entities.insert(SketchEntity::Line(SketchLine { start, end }));
                lines.push(line);
            }
        }

        // Constraints: horizontal on each line, fixed on first point
        let mut constraints = vec![Constraint::Fixed {
            point: EntityRef::Point(points[0]),
            x: 0.0,
            y: 0.0,
        }];
        for &line in &lines {
            constraints.push(Constraint::Horizontal { line });
        }

        let num_params = param_idx; // 50
        let system = CompiledSystem::build(&constraints, &entities, num_params);

        // Fixed: 2 residuals, each touches 1 param = 2 nonzero
        // 20 horizontal constraints: each touches 2 params = 40 nonzero
        // Total nonzero: 42
        // Dense: (2 + 20) * 50 = 1100
        let expected_nonzero = 42;
        assert_eq!(system.num_nonzero, expected_nonzero);
        assert_eq!(system.dense_size, 22 * 50);

        let ratio = system.sparsity_ratio();
        // 42 / 1100 = ~3.8% — very sparse!
        assert!(
            ratio < 0.05,
            "Expected sparsity ratio < 5%, got {:.1}%",
            ratio * 100.0
        );
    }

    #[test]
    fn sparse_solver_produces_same_result() {
        // Solve a constrained rectangle and verify the sparse solver
        // produces the same result as before.
        use crate::sketch::Sketch2D;

        let mut sketch = Sketch2D::new();
        let p0 = sketch.add_point(0.0, 0.0);
        let p1 = sketch.add_point(12.0, 1.0);
        let p2 = sketch.add_point(11.0, 8.0);
        let p3 = sketch.add_point(1.0, 7.0);

        let l0 = sketch.add_line(p0, p1);
        let l1 = sketch.add_line(p1, p2);
        let l2 = sketch.add_line(p2, p3);
        let l3 = sketch.add_line(p3, p0);

        sketch.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
        sketch.constrain_horizontal(l0);
        sketch.constrain_horizontal(l2);
        sketch.constrain_vertical(l1);
        sketch.constrain_vertical(l3);
        sketch.constrain_length(l0, 10.0);
        sketch.constrain_length(l1, 5.0);

        let result = sketch.solve_default();
        assert!(result.converged, "Solver should converge");

        // Verify corners
        let (x0, y0) = sketch.get_point(p0).unwrap();
        let (x1, y1) = sketch.get_point(p1).unwrap();
        let (x2, y2) = sketch.get_point(p2).unwrap();
        let (x3, y3) = sketch.get_point(p3).unwrap();

        assert!((x0 - 0.0).abs() < 1e-6);
        assert!((y0 - 0.0).abs() < 1e-6);
        assert!((x1 - 10.0).abs() < 1e-6);
        assert!((y1 - 0.0).abs() < 1e-6);
        assert!((x2 - 10.0).abs() < 1e-6);
        assert!((y2 - 5.0).abs() < 1e-6);
        assert!((x3 - 0.0).abs() < 1e-6);
        assert!((y3 - 5.0).abs() < 1e-6);
    }

    #[test]
    fn sparse_solver_large_sketch_converges() {
        // 15 independent lines (30 points = 60 params, under 64-bit limit)
        // with horizontal constraints + a few distance constraints.
        // Verify convergence and sparsity reduction.
        use crate::sketch::Sketch2D;

        let mut sketch = Sketch2D::new();
        let mut points = Vec::new();
        let mut lines = Vec::new();

        // Create 15 line segments (30 points = 60 params)
        for i in 0..15 {
            let x = i as f64 * 5.0;
            let y = (i as f64 * 0.3).sin() * 2.0; // Slightly off-horizontal
            let p0 = sketch.add_point(x, y);
            let p1 = sketch.add_point(x + 4.0, y + 1.0);
            let line = sketch.add_line(p0, p1);
            points.push((p0, p1));
            lines.push(line);
        }

        // Fix origin
        sketch.constrain_fixed(EntityRef::Point(points[0].0), 0.0, 0.0);

        // All lines horizontal
        for &line in &lines {
            sketch.constrain_horizontal(line);
        }

        // A few length constraints
        sketch.constrain_length(lines[0], 4.0);
        sketch.constrain_length(lines[5], 4.0);
        sketch.constrain_length(lines[10], 4.0);
        sketch.constrain_length(lines[14], 4.0);

        // 60 params, within 64-bit bitmask limit
        assert!(
            sketch.parameters.len() <= 64,
            "Need <= 64 params for bitmask sparsity, got {}",
            sketch.parameters.len()
        );

        // Check sparsity
        let system = CompiledSystem::build(
            &sketch.constraints,
            &sketch.entities,
            sketch.parameters.len(),
        );
        let dense = system.dense_size;
        assert!(
            system.num_nonzero < dense / 5,
            "Expected >5x reduction: {} nonzero vs {} dense",
            system.num_nonzero,
            dense
        );

        let result = sketch.solve_default();
        assert!(
            result.converged,
            "Large sketch should converge, status = {:?}",
            result.status
        );
    }
}
