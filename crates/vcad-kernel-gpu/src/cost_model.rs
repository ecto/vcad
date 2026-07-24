//! Expression-defined routing cost model (GPU-router charter M3).
//!
//! The router's step costs are authored as a `tang-expr` graph — editable,
//! inspectable, and (for charter M5) differentiable — and compiled each
//! round into a dense `(layer, move)` table the batch kernel reads. The
//! expression is the single source of truth: the table is its evaluation
//! over the discrete move alphabet, and the CPU search can evaluate the same
//! graph directly, so CPU and GPU can never disagree about what a move
//! costs.
//!
//! Move alphabet per layer (10 entries):
//! `[E, W, N, S, NE, NW, SE, SW, via_up, via_down]`.

use tang_expr::{ExprGraph, ExprId};

/// Variables the cost expression may read (graph var indices).
pub mod vars {
    /// 1.0 when the move is diagonal, else 0.0.
    pub const IS_DIAG: u16 = 0;
    /// 1.0 when the move is a via, else 0.0.
    pub const IS_VIA: u16 = 1;
    /// 1.0 when the move runs cross-grain for its layer, else 0.0.
    pub const CROSS_GRAIN: u16 = 2;
    /// 1.0 on outer layers (no grain), else 0.0.
    pub const IS_OUTER: u16 = 3;
    /// Base step pitch cost (scaled units).
    pub const STEP: u16 = 4;
}

/// A cost model: the expression graph plus its output.
pub struct CostModel {
    graph: ExprGraph,
    cost: ExprId,
}

impl CostModel {
    /// The router's default cost model, as an expression:
    /// `step * (1 + 0.414*diag + 3*via + (1-outer)*(0.6*cross + 0.25*diag))`
    /// — the same arithmetic the CPU maze uses (step/diag/via plus the
    /// human-look grain discipline).
    pub fn default_model() -> Self {
        let mut g = ExprGraph::new();
        let is_diag = g.var(vars::IS_DIAG);
        let is_via = g.var(vars::IS_VIA);
        let cross = g.var(vars::CROSS_GRAIN);
        let outer = g.var(vars::IS_OUTER);
        let step = g.var(vars::STEP);

        let c0414 = g.lit(0.414_213_56);
        let c3 = g.lit(3.0);
        let c06 = g.lit(0.6);
        let c025 = g.lit(0.25);
        let one = g.lit(1.0);

        let diag_term = g.mul(c0414, is_diag);
        let via_term = g.mul(c3, is_via);
        let grain_cross = g.mul(c06, cross);
        let grain_diag = g.mul(c025, is_diag);
        let grain_sum = g.add(grain_cross, grain_diag);
        let neg_outer = g.neg(outer);
        let inner = g.add(one, neg_outer);
        let grain_term = g.mul(inner, grain_sum);

        let s1 = g.add(one, diag_term);
        let s2 = g.add(s1, via_term);
        let s3 = g.add(s2, grain_term);
        let cost = g.mul(step, s3);
        Self { graph: g, cost }
    }

    /// Evaluate one move.
    pub fn eval(
        &self,
        is_diag: f64,
        is_via: f64,
        cross_grain: f64,
        is_outer: f64,
        step: f64,
    ) -> f64 {
        self.graph
            .eval(self.cost, &[is_diag, is_via, cross_grain, is_outer, step])
    }

    /// Compile the model into a `(layers x 10)` u32 table for the batch
    /// kernel. `step_scaled` is the base pitch cost in integer units.
    /// Layer grain: inner layers alternate (odd = horizontal-preferred),
    /// outers are free — mirroring the CPU maze's discipline.
    pub fn to_table(&self, layers: usize, step_scaled: f64) -> Vec<u32> {
        let mut out = Vec::with_capacity(layers * 10);
        for li in 0..layers {
            let outer = li == 0 || li + 1 == layers;
            let horizontal_layer = li % 2 == 1;
            // moves: E, W, N, S, NE, NW, SE, SW, via_up, via_down
            for mv in 0..10 {
                let is_via = mv >= 8;
                let is_diag = (4..8).contains(&mv);
                let cross = if is_via || outer {
                    false
                } else {
                    let moving_h = mv < 2;
                    let moving_v = mv == 2 || mv == 3;
                    (horizontal_layer && moving_v) || (!horizontal_layer && moving_h)
                };
                let c = self.eval(
                    f64::from(is_diag),
                    f64::from(is_via),
                    f64::from(cross),
                    f64::from(outer),
                    step_scaled,
                );
                out.push(c.round().max(1.0) as u32);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_matches_hand_arithmetic() {
        let m = CostModel::default_model();
        // Orthogonal, inner, with grain violation:
        // step * (1 + 0.6) = 1600.
        assert_eq!(m.eval(0.0, 0.0, 1.0, 0.0, 1000.0).round(), 1600.0);
        // Diagonal on outer: step * 1.414.
        assert_eq!(m.eval(1.0, 0.0, 0.0, 1.0, 1000.0).round(), 1414.0);
        // Via: step * 4.
        assert_eq!(m.eval(0.0, 1.0, 0.0, 0.0, 1000.0).round(), 4000.0);
    }

    #[test]
    fn table_layout_and_grain_discipline() {
        let m = CostModel::default_model();
        let t = m.to_table(4, 1000.0);
        assert_eq!(t.len(), 40);
        // Outer layer 0: E == N (no grain).
        assert_eq!(t[0], t[2]);
        // Inner layer 1 (horizontal-preferred): N (cross) > E (with-grain).
        assert!(t[10 + 2] > t[10]);
        // Inner layer 2 (vertical-preferred): E (cross) > N.
        assert!(t[20] > t[20 + 2]);
        // Vias cost most.
        assert!(t[8] > t[4]);
    }
}
