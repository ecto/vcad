//! Claim compiler — one source, two compilations ("tang-route" M1).
//!
//! A [`Claim`] is authored ONCE as a `tang-expr` scalar residual `r(θ)` with
//! the fixed semantics **"the claim holds iff r ≤ 0"** (`−r` is the margin).
//! Every downstream consumer is derived from that single expression:
//!
//! * [`ClaimSet::loss`] — the smooth descent objective: each claim
//!   contributes `w · hinge(r)²` (or `w · r` for an [`Penalty::Linear`]
//!   objective such as wirelength), built *on top of* `r` in the same graph;
//! * [`ClaimSet::gn_residuals`] — the same terms in least-squares form
//!   (`√w · hinge(r)`, so `Σ ρ² = loss` exactly) for the Gauss-Newton / LM
//!   solver in [`super::sco`];
//! * [`ClaimSet::verdict`] — the exact predicate: `r` evaluated in plain
//!   `f64` with no smoothing, giving pass/fail plus a signed margin.
//!
//! Because loss and predicate share one `ExprId`, they cannot drift: the
//! hinge is nonzero exactly where the predicate fails (proved by
//! `predicate_flips_where_hinge_turns_on` below). The only approximation in
//! the residual itself is the `1e-9` regulariser under every `sqrt` (a
//! distance error below 3e-5 mm at zero separation, ~1e-9 mm at routing
//! distances), which the predicate inherits identically.
//!
//! The exact predicate is still a *model* of the board — a clearance claim
//! knows only the obstacles it was authored against. The real DRC oracle
//! (`RouteSession::probe`) remains the gate; [`super::descent`] mines its
//! violations back into new claims (M3), and the test
//! `clearance_predicate_agrees_with_session_probe` pins the compiled
//! predicate to the oracle on a fixture.
//!
//! # Next steps (not implemented)
//!
//! * **M4 — duals → PathFinder history costs.** The penalty multipliers the
//!   SCO outer loop escalates on violated clearance claims are (scaled)
//!   Lagrange-multiplier estimates; accumulating them per board region and
//!   feeding them back into the global/maze router's history cost would let
//!   negotiation learn where continuous descent keeps fighting the geometry.
//! * **M5 — soft topology.** Today descent moves geometry but never
//!   topology (layers, which side of an obstacle). Relaxing those discrete
//!   choices into continuous weights (e.g. per-obstacle side variables with
//!   an annealed sharpness) would let descent propose topology changes that
//!   the exact oracle then accepts or rejects.

use tang_expr::{ExprGraph, ExprId, Node};

/// How a claim's residual becomes a loss term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Penalty {
    /// `w · hinge(r)²` — zero while the claim holds, quadratic past it.
    SquaredHinge,
    /// `w · r` — a pure objective (e.g. wirelength with `r = length`).
    /// The residual must stay non-negative for the least-squares form.
    Linear,
}

/// What a claim measures (diagnostics and solver policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    /// Point (or segment sample) vs obstacle clearance.
    Clearance,
    /// Intra-pair length skew.
    Skew,
    /// Deviation of a pair-spring distance from the coupling gap.
    Gap,
    /// Length inside a `[lo, hi]` window.
    LengthWindow,
    /// Wirelength objective.
    Wirelength,
    /// Hard minimum edge-to-edge separation between a P-leg and an N-leg
    /// segment (the intra-pair gap rule the oracle enforces on the twin).
    TwinGap,
}

/// One claim: a residual expression plus how to penalize it.
#[derive(Debug, Clone)]
pub struct Claim {
    /// What it measures.
    pub kind: ClaimKind,
    /// The residual `r(θ)`; the claim holds iff `r ≤ 0`.
    pub residual: ExprId,
    /// Loss weight.
    pub weight: f64,
    /// Smoothing applied for the loss.
    pub penalty: Penalty,
    /// Hard claims are the ones a solver must satisfy (the SCO outer loop
    /// escalates their penalty while they fail); soft claims are traded.
    pub hard: bool,
}

/// Exact-predicate outcome for one claim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// `r ≤ 0`.
    pub holds: bool,
    /// `−r`: positive margin when holding, negative overshoot otherwise.
    pub margin: f64,
}

/// A 2D point as a pair of expressions.
pub type PointExpr = (ExprId, ExprId);

/// An ordered collection of claims over one graph.
#[derive(Debug, Clone, Default)]
pub struct ClaimSet {
    /// The claims, in authoring order.
    pub claims: Vec<Claim>,
}

impl ClaimSet {
    /// Empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a claim, returning its index.
    pub fn push(&mut self, c: Claim) -> usize {
        self.claims.push(c);
        self.claims.len() - 1
    }

    /// Compilation (a): the smooth loss term of one claim.
    pub fn loss_term(g: &mut ExprGraph, c: &Claim) -> ExprId {
        let w = g.lit(c.weight);
        match c.penalty {
            Penalty::SquaredHinge => {
                let r2 = g.mul(c.residual, c.residual);
                let zero = g.lit(0.0);
                let h = g.select(c.residual, r2, zero);
                g.mul(w, h)
            }
            Penalty::Linear => g.mul(w, c.residual),
        }
    }

    /// The whole smooth objective `Σ loss_term`.
    pub fn loss(&self, g: &mut ExprGraph) -> ExprId {
        let mut total = g.lit(0.0);
        for c in &self.claims {
            let t = Self::loss_term(g, c);
            total = g.add(total, t);
        }
        total
    }

    /// Least-squares form of one claim: `ρ` with `ρ² = loss_term` exactly
    /// (up to the `1e-300` floor keeping `√` differentiable at a zero linear
    /// objective).
    pub fn gn_residual(g: &mut ExprGraph, c: &Claim) -> ExprId {
        match c.penalty {
            Penalty::SquaredHinge => {
                let sw = g.lit(c.weight.sqrt());
                let zero = g.lit(0.0);
                let h = g.select(c.residual, c.residual, zero);
                g.mul(sw, h)
            }
            Penalty::Linear => {
                let w = g.lit(c.weight);
                let wr = g.mul(w, c.residual);
                let tiny = g.lit(1e-300);
                let s = g.add(wr, tiny);
                g.sqrt(s)
            }
        }
    }

    /// [`Self::gn_residual`] with an augmented-Lagrangian shift: a hinge
    /// claim's residual becomes `√w · hinge(r + σ)` where `σ ≥ 0` is an
    /// input (the scaled multiplier estimate). `σ = 0` reproduces the
    /// unshifted form; the predicate is untouched (it is always raw `r`).
    pub fn gn_residual_shifted(g: &mut ExprGraph, c: &Claim, shift: ExprId) -> ExprId {
        match c.penalty {
            Penalty::SquaredHinge => {
                let sw = g.lit(c.weight.sqrt());
                let zero = g.lit(0.0);
                let rs = g.add(c.residual, shift);
                let h = g.select(rs, rs, zero);
                g.mul(sw, h)
            }
            Penalty::Linear => Self::gn_residual(g, c),
        }
    }

    /// Least-squares residual vector, one row per claim.
    pub fn gn_residuals(&self, g: &mut ExprGraph) -> Vec<ExprId> {
        self.claims
            .iter()
            .map(|c| Self::gn_residual(g, c))
            .collect()
    }

    /// Compilation (b): the exact predicate — the SAME residual evaluated in
    /// `f64`, no smoothing.
    pub fn verdict(g: &ExprGraph, c: &Claim, theta: &[f64]) -> Verdict {
        let r: f64 = g.eval(c.residual, theta);
        Verdict {
            holds: r <= 0.0,
            margin: -r,
        }
    }

    /// Exact verdicts for every claim (one shared evaluation pass).
    pub fn verdicts(&self, g: &ExprGraph, theta: &[f64]) -> Vec<Verdict> {
        let ids: Vec<ExprId> = self.claims.iter().map(|c| c.residual).collect();
        g.eval_many(&ids, theta)
            .into_iter()
            .map(|r: f64| Verdict {
                holds: r <= 0.0,
                margin: -r,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Claim builders. Each returns the residual with "holds iff r ≤ 0".
// ---------------------------------------------------------------------------

/// `|x|` as an expression.
pub fn abs(g: &mut ExprGraph, x: ExprId) -> ExprId {
    let nx = g.neg(x);
    g.select(x, x, nx)
}

/// `max(a, b)` as an expression.
pub fn max(g: &mut ExprGraph, a: ExprId, b: ExprId) -> ExprId {
    let nb = g.neg(b);
    let d = g.add(a, nb);
    g.select(d, a, b)
}

/// Euclidean distance (with the `1e-9` regulariser).
pub fn dist(g: &mut ExprGraph, a: PointExpr, b: PointExpr) -> ExprId {
    let nax = g.neg(a.0);
    let nay = g.neg(a.1);
    let dx = g.add(b.0, nax);
    let dy = g.add(b.1, nay);
    let dx2 = g.mul(dx, dx);
    let dy2 = g.mul(dy, dy);
    let d2 = g.add(dx2, dy2);
    let eps = g.lit(1e-9);
    let d2e = g.add(d2, eps);
    g.sqrt(d2e)
}

/// Polyline length.
pub fn polyline_len(g: &mut ExprGraph, pts: &[PointExpr]) -> ExprId {
    let mut total = g.lit(0.0);
    for w in pts.windows(2) {
        let d = dist(g, w[0], w[1]);
        total = g.add(total, d);
    }
    total
}

/// Midpoint of two point expressions.
pub fn midpoint(g: &mut ExprGraph, a: PointExpr, b: PointExpr) -> PointExpr {
    let half = g.lit(0.5);
    let sx = g.add(a.0, b.0);
    let sy = g.add(a.1, b.1);
    (g.mul(sx, half), g.mul(sy, half))
}

/// Differentiable point-to-segment distance: parameter t clamped to [0, 1]
/// via two selects, then Euclidean distance to the clamped foot point.
pub fn pt_seg_dist(g: &mut ExprGraph, p: PointExpr, a: PointExpr, b: PointExpr) -> ExprId {
    let eps = g.lit(1e-9);
    let nax = g.neg(a.0);
    let nay = g.neg(a.1);
    let abx = g.add(b.0, nax);
    let aby = g.add(b.1, nay);
    let apx = g.add(p.0, nax);
    let apy = g.add(p.1, nay);
    let abx2 = g.mul(abx, abx);
    let aby2 = g.mul(aby, aby);
    let l2 = g.add(abx2, aby2);
    let l2e = g.add(l2, eps);
    let dx = g.mul(apx, abx);
    let dy = g.mul(apy, aby);
    let dot = g.add(dx, dy);
    let inv = g.recip(l2e);
    let t_raw = g.mul(dot, inv);
    // select(c, x, y) = x when c > 0 else y.
    let one = g.lit(1.0);
    let none = g.neg(one);
    let tm1 = g.add(t_raw, none);
    let hi = g.select(tm1, one, t_raw);
    let zero_t = g.lit(0.0);
    let t = g.select(t_raw, hi, zero_t);
    let fx_off = g.mul(abx, t);
    let fy_off = g.mul(aby, t);
    let fx = g.add(a.0, fx_off);
    let fy = g.add(a.1, fy_off);
    dist(g, p, (fx, fy))
}

/// Clearance: `required − dist(p, seg[a,b])`.
pub fn clearance(
    g: &mut ExprGraph,
    p: PointExpr,
    a: (f64, f64),
    b: (f64, f64),
    required: f64,
    weight: f64,
) -> Claim {
    let a = (g.lit(a.0), g.lit(a.1));
    let b = (g.lit(b.0), g.lit(b.1));
    let d = pt_seg_dist(g, p, a, b);
    let req = g.lit(required);
    let nd = g.neg(d);
    Claim {
        kind: ClaimKind::Clearance,
        residual: g.add(req, nd),
        weight,
        penalty: Penalty::SquaredHinge,
        hard: true,
    }
}

/// `min(a, b)` as an expression.
pub fn min(g: &mut ExprGraph, a: ExprId, b: ExprId) -> ExprId {
    let nb = g.neg(b);
    let d = g.add(a, nb);
    g.select(d, b, a)
}

/// Segment-to-segment distance as the minimum of the four endpoint-to-
/// segment distances. Exact for non-crossing segments; a CROSSING pair reads
/// its endpoint distance instead of 0 — the exact oracle still catches it.
pub fn seg_seg_dist(
    g: &mut ExprGraph,
    a0: PointExpr,
    a1: PointExpr,
    b0: PointExpr,
    b1: PointExpr,
) -> ExprId {
    let d0 = pt_seg_dist(g, a0, b0, b1);
    let d1 = pt_seg_dist(g, a1, b0, b1);
    let d2 = pt_seg_dist(g, b0, a0, a1);
    let d3 = pt_seg_dist(g, b1, a0, a1);
    let m01 = min(g, d0, d1);
    let m23 = min(g, d2, d3);
    min(g, m01, m23)
}

/// Twin gap (hard): `required − seg_seg_dist(P seg, N seg)`, with
/// `required` the centre-line separation (edge gap + both half-widths).
pub fn twin_gap(
    g: &mut ExprGraph,
    p_seg: (PointExpr, PointExpr),
    n_seg: (PointExpr, PointExpr),
    required: f64,
    weight: f64,
) -> Claim {
    let d = seg_seg_dist(g, p_seg.0, p_seg.1, n_seg.0, n_seg.1);
    let req = g.lit(required);
    let nd = g.neg(d);
    Claim {
        kind: ClaimKind::TwinGap,
        residual: g.add(req, nd),
        weight,
        penalty: Penalty::SquaredHinge,
        hard: true,
    }
}

/// Skew: `|len_p − len_n| − bound`.
pub fn skew(g: &mut ExprGraph, len_p: ExprId, len_n: ExprId, bound: f64, weight: f64) -> Claim {
    let nln = g.neg(len_n);
    let s = g.add(len_p, nln);
    let a = abs(g, s);
    let nb = g.lit(-bound);
    Claim {
        kind: ClaimKind::Skew,
        residual: g.add(a, nb),
        weight,
        penalty: Penalty::SquaredHinge,
        hard: false,
    }
}

/// Gap: `|dist(p, q) − target| − tol`.
pub fn gap(
    g: &mut ExprGraph,
    p: PointExpr,
    q: PointExpr,
    target: f64,
    tol: f64,
    weight: f64,
) -> Claim {
    let d = dist(g, p, q);
    let nt = g.lit(-target);
    let dev = g.add(d, nt);
    let a = abs(g, dev);
    let ntol = g.lit(-tol);
    Claim {
        kind: ClaimKind::Gap,
        residual: g.add(a, ntol),
        weight,
        penalty: Penalty::SquaredHinge,
        hard: false,
    }
}

/// Length window: `max(lo − len, len − hi)`.
pub fn length_window(g: &mut ExprGraph, len: ExprId, lo: f64, hi: f64, weight: f64) -> Claim {
    let lo_l = g.lit(lo);
    let nlen = g.neg(len);
    let below = g.add(lo_l, nlen);
    let nhi = g.lit(-hi);
    let above = g.add(len, nhi);
    Claim {
        kind: ClaimKind::LengthWindow,
        residual: max(g, below, above),
        weight,
        penalty: Penalty::SquaredHinge,
        hard: false,
    }
}

/// Wirelength objective: `r = len` with a linear penalty (predicate "holds"
/// only at zero length — wirelength is an objective, not a bound).
pub fn wirelength(len: ExprId, weight: f64) -> Claim {
    Claim {
        kind: ClaimKind::Wirelength,
        residual: len,
        weight,
        penalty: Penalty::Linear,
        hard: false,
    }
}

/// Variables each root depends on (sorted, deduplicated). `tang-expr`'s own
/// `deps`/`jacobian_sparsity` are u64 bitmasks capped at 64 variables; a
/// densified pair run has hundreds, so the walk lives here.
pub fn var_deps(g: &ExprGraph, roots: &[ExprId]) -> Vec<Vec<u16>> {
    let nodes = g.nodes_slice();
    let mut mark = vec![u32::MAX; nodes.len()];
    let mut out = Vec::with_capacity(roots.len());
    let mut stack = Vec::new();
    for (k, r) in roots.iter().enumerate() {
        let tag = k as u32;
        let mut vars = Vec::new();
        stack.push(r.index() as usize);
        while let Some(i) = stack.pop() {
            if mark[i] == tag {
                continue;
            }
            mark[i] = tag;
            match nodes[i] {
                Node::Var(v) => vars.push(v),
                Node::Lit(_) => {}
                Node::Add(a, b) | Node::Mul(a, b) | Node::Atan2(a, b) => {
                    stack.push(a.index() as usize);
                    stack.push(b.index() as usize);
                }
                Node::Neg(a)
                | Node::Recip(a)
                | Node::Sqrt(a)
                | Node::Sin(a)
                | Node::Exp2(a)
                | Node::Log2(a) => stack.push(a.index() as usize),
                Node::Select(c, a, b) => {
                    stack.push(c.index() as usize);
                    stack.push(a.index() as usize);
                    stack.push(b.index() as usize);
                }
            }
        }
        vars.sort_unstable();
        vars.dedup();
        out.push(vars);
    }
    out
}

/// Dependency-aware symbolic differentiation.
///
/// `ExprGraph::diff` recurses into every subtree and emits `0 + 0`,
/// `0 · b` nodes for the parts that don't depend on the variable, so the
/// derivative of a polyline length (a chain of n adds) costs O(n) NEW nodes
/// per variable — O(n²) for a full Jacobian row; the long descent fixture
/// grew to 3.1M nodes and 95 s of compile. This differentiator tracks a
/// conservative `[min, max]` variable-index range per node, returns `0`
/// for any subtree whose range excludes the variable, and folds `0`/`1`
/// through add/mul/neg/select — a derivative costs O(1) new nodes for local
/// terms. (Upstream note: the same short-circuit would belong in
/// `tang-expr`'s own `diff`.)
#[derive(Debug, Default)]
pub struct SparseDiff {
    lo: Vec<u16>,
    hi: Vec<u16>,
}

impl SparseDiff {
    /// Empty; ranges are computed lazily as the graph grows.
    pub fn new() -> Self {
        Self::default()
    }

    fn sync(&mut self, g: &ExprGraph) {
        let nodes = g.nodes_slice();
        let start = self.lo.len();
        for node in &nodes[start..] {
            let (lo, hi) = match *node {
                Node::Var(v) => (v, v),
                Node::Lit(_) => (u16::MAX, 0),
                Node::Add(a, b) | Node::Mul(a, b) | Node::Atan2(a, b) => {
                    let (a, b) = (a.index() as usize, b.index() as usize);
                    (self.lo[a].min(self.lo[b]), self.hi[a].max(self.hi[b]))
                }
                Node::Neg(a)
                | Node::Recip(a)
                | Node::Sqrt(a)
                | Node::Sin(a)
                | Node::Exp2(a)
                | Node::Log2(a) => {
                    let a = a.index() as usize;
                    (self.lo[a], self.hi[a])
                }
                Node::Select(c, a, b) => {
                    let (c, a, b) = (c.index() as usize, a.index() as usize, b.index() as usize);
                    (
                        self.lo[c].min(self.lo[a]).min(self.lo[b]),
                        self.hi[c].max(self.hi[a]).max(self.hi[b]),
                    )
                }
            };
            self.lo.push(lo);
            self.hi.push(hi);
        }
    }

    /// `∂expr/∂Var(var)`, appended to `g`.
    pub fn diff(&mut self, g: &mut ExprGraph, expr: ExprId, var: u16) -> ExprId {
        self.sync(g);
        let mut memo = std::collections::HashMap::new();
        let d = self.inner(g, expr, var, &mut memo);
        self.sync(g);
        d
    }

    fn inner(
        &self,
        g: &mut ExprGraph,
        e: ExprId,
        var: u16,
        memo: &mut std::collections::HashMap<ExprId, ExprId>,
    ) -> ExprId {
        const Z: ExprId = ExprId::ZERO;
        const O: ExprId = ExprId::ONE;
        let i = e.index() as usize;
        // Nodes created during this walk are derivative nodes of in-range
        // subtrees; they are never revisited as inputs, so `i` is in range.
        if var < self.lo[i] || var > self.hi[i] {
            return Z;
        }
        if let Some(&d) = memo.get(&e) {
            return d;
        }
        let add = |g: &mut ExprGraph, a: ExprId, b: ExprId| {
            if a == Z {
                b
            } else if b == Z {
                a
            } else {
                g.add(a, b)
            }
        };
        let mul = |g: &mut ExprGraph, a: ExprId, b: ExprId| {
            if a == Z || b == Z {
                Z
            } else if a == O {
                b
            } else if b == O {
                a
            } else {
                g.mul(a, b)
            }
        };
        let d = match g.node(e) {
            Node::Var(v) => {
                if v == var {
                    O
                } else {
                    Z
                }
            }
            Node::Lit(_) => Z,
            Node::Add(a, b) => {
                let da = self.inner(g, a, var, memo);
                let db = self.inner(g, b, var, memo);
                add(g, da, db)
            }
            Node::Mul(a, b) => {
                let da = self.inner(g, a, var, memo);
                let db = self.inner(g, b, var, memo);
                let t1 = mul(g, da, b);
                let t2 = mul(g, a, db);
                add(g, t1, t2)
            }
            Node::Neg(a) => {
                let da = self.inner(g, a, var, memo);
                if da == Z {
                    Z
                } else {
                    g.neg(da)
                }
            }
            Node::Recip(a) => {
                let da = self.inner(g, a, var, memo);
                if da == Z {
                    Z
                } else {
                    let a2 = g.mul(a, a);
                    let r = g.recip(a2);
                    let t = mul(g, da, r);
                    g.neg(t)
                }
            }
            Node::Sqrt(a) => {
                let da = self.inner(g, a, var, memo);
                if da == Z {
                    Z
                } else {
                    let sq = g.sqrt(a);
                    let two_sq = g.mul(ExprId::TWO, sq);
                    let r = g.recip(two_sq);
                    mul(g, da, r)
                }
            }
            Node::Select(c, a, b) => {
                let da = self.inner(g, a, var, memo);
                let db = self.inner(g, b, var, memo);
                if da == db {
                    da
                } else {
                    g.select(c, da, db)
                }
            }
            // Ops the claim builders never emit: defer to tang's rules.
            Node::Sin(_) | Node::Atan2(_, _) | Node::Exp2(_) | Node::Log2(_) => g.diff(e, var),
        };
        memo.insert(e, d);
        d
    }
}

/// Minimal one-layer board for router tests (100 mm square, 0.15 mm
/// default clearance, no footprints).
#[cfg(test)]
pub(crate) fn fixture_pcb(traces: Vec<vcad_ir::ecad::Trace>) -> vcad_ir::ecad::Pcb {
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(-50.0, -50.0),
                Vec2::new(100.0, -50.0),
                Vec2::new(100.0, 100.0),
                Vec2::new(-50.0, 100.0),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: vec![StackupLayer {
                layer: PcbLayer::FCu,
                copper_thickness: Some(0.035),
                dielectric_thickness: None,
                dielectric_er: None,
                material: None,
            }],
        },
        nets: vec![],
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.2,
                clearance: 0.15,
                via_diameter: 0.8,
                via_drill: 0.4,
                diff_pair_gap: None,
                diff_pair_width: None,
                target_impedance: None,
                target_diff_impedance: None,
            },
            class_rules: vec![],
            net_class_assignments: std::collections::HashMap::new(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.2,
        },
        footprints: vec![],
        traces,
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loss and predicate come from ONE expression: sweeping a point across
    /// an obstacle's requirement, the hinge loss is nonzero exactly where the
    /// exact predicate fails, and the GN residual squares to the loss.
    #[test]
    fn predicate_flips_where_hinge_turns_on() {
        let mut g = ExprGraph::new();
        let p = (g.var(0), g.var(1));
        let c = clearance(&mut g, p, (0.0, 0.0), (10.0, 0.0), 0.5, 7.0);
        let loss = ClaimSet::loss_term(&mut g, &c);
        let rho = ClaimSet::gn_residual(&mut g, &c);
        for k in 0..=200 {
            let y = 0.3 + 0.4 * k as f64 / 200.0; // crosses 0.5
            let th = [5.0, y];
            let v = ClaimSet::verdict(&g, &c, &th);
            let l: f64 = g.eval(loss, &th);
            let r: f64 = g.eval(rho, &th);
            assert_eq!(v.holds, l == 0.0, "y={y}: holds={} loss={l}", v.holds);
            assert!((r * r - l).abs() <= 1e-12 * (1.0 + l));
            assert!((v.margin - (y - 0.5)).abs() < 1e-6, "margin {}", v.margin);
        }
        // Exactly at the boundary (to the regulariser) the claim holds.
        assert!(ClaimSet::verdict(&g, &c, &[5.0, 0.500_001]).holds);
        assert!(!ClaimSet::verdict(&g, &c, &[5.0, 0.499_999]).holds);
    }

    #[test]
    fn skew_gap_window_semantics() {
        let mut g = ExprGraph::new();
        let lp = g.var(0);
        let ln = g.var(1);
        let s = skew(&mut g, lp, ln, 1.1, 1.0);
        assert!(ClaimSet::verdict(&g, &s, &[10.0, 11.0]).holds);
        assert!(!ClaimSet::verdict(&g, &s, &[10.0, 11.2]).holds);
        assert!(!ClaimSet::verdict(&g, &s, &[11.2, 10.0]).holds);
        let w = length_window(&mut g, lp, 9.0, 10.5, 1.0);
        assert!(ClaimSet::verdict(&g, &w, &[10.0, 0.0]).holds);
        assert!(!ClaimSet::verdict(&g, &w, &[8.0, 0.0]).holds);
        let vw = ClaimSet::verdict(&g, &w, &[11.0, 0.0]);
        assert!(!vw.holds && (vw.margin + 0.5).abs() < 1e-12);
        let p = (g.var(0), g.var(1));
        let q = (g.lit(0.0), g.lit(0.0));
        let gp = gap(&mut g, p, q, 0.5, 0.01, 1.0);
        assert!(ClaimSet::verdict(&g, &gp, &[0.3, 0.4]).holds);
        assert!(!ClaimSet::verdict(&g, &gp, &[0.3, 0.5]).holds);
        // Zero-tolerance skew loss is exactly the old w·(lp−ln)² spring.
        let s0 = skew(&mut g, lp, ln, 0.0, 4.0);
        let l0 = ClaimSet::loss_term(&mut g, &s0);
        let v: f64 = g.eval(l0, &[10.0, 11.5]);
        assert!((v - 4.0 * 2.25).abs() < 1e-12);
    }

    #[test]
    fn var_deps_walks_past_64_vars() {
        let mut g = ExprGraph::new();
        let a = g.var(3);
        let b = g.var(300);
        let s = g.add(a, b);
        let c = g.var(70);
        let d = g.sqrt(c);
        assert_eq!(var_deps(&g, &[s, d]), vec![vec![3, 300], vec![70]]);
    }

    /// The compiled predicate agrees with the real routing oracle: a point
    /// claim against a trace (required = clearance + both half-widths) and
    /// `RouteSession::probe` of a zero-length capsule at the same point give
    /// the same pass/fail everywhere outside a 1 µm band at the boundary.
    #[test]
    fn clearance_predicate_agrees_with_session_probe() {
        use crate::session::RouteSession;
        use crate::spatial::CopperGeom;
        use vcad_ir::ecad::{Pcb, PcbLayer, Trace};
        use vcad_ir::Vec2;

        let pcb = fixture_pcb(vec![Trace {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(10.0, 2.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "B".into(),
            source: None,
        }]);
        let _: &Pcb = &pcb;
        let session = RouteSession::from_pcb(&pcb);
        let clr = session.clearance_for("A");
        let own_w = 0.2;
        let required = clr + 0.125 + own_w / 2.0;

        let mut g = ExprGraph::new();
        let p = (g.var(0), g.var(1));
        let c = clearance(&mut g, p, (0.0, 0.0), (10.0, 2.0), required, 1.0);
        let mut checked = 0;
        for i in 0..60 {
            for j in 0..40 {
                let pt = Vec2::new(-1.0 + i as f64 * 0.2, -1.0 + j as f64 * 0.1);
                let v = ClaimSet::verdict(&g, &c, &[pt.x, pt.y]);
                if v.margin.abs() < 1e-6 {
                    continue;
                }
                let probe = session.probe(
                    &CopperGeom::Segment {
                        a: pt,
                        b: pt,
                        half_w: own_w / 2.0,
                    },
                    PcbLayer::FCu,
                    "A",
                    clr,
                );
                assert_eq!(
                    v.holds, probe.legal,
                    "disagree at ({:.2},{:.2}): margin {:.4}",
                    pt.x, pt.y, v.margin
                );
                checked += 1;
            }
        }
        assert!(checked > 2000);
    }

    /// Twin gap: holds iff the two segments' centre lines are `required`
    /// apart, judged on whole segments (an interior approach counts).
    #[test]
    fn twin_gap_is_segment_to_segment() {
        let mut g = ExprGraph::new();
        let a0 = (g.lit(0.0), g.lit(0.0));
        let a1 = (g.lit(10.0), g.lit(0.0));
        let b0 = (g.var(0), g.var(1));
        let b1 = (g.var(2), g.var(3));
        let c = twin_gap(&mut g, (a0, a1), (b0, b1), 0.4, 1.0);
        // Parallel at 0.5: holds; at 0.3: fails.
        assert!(ClaimSet::verdict(&g, &c, &[0.0, 0.5, 10.0, 0.5]).holds);
        assert!(!ClaimSet::verdict(&g, &c, &[0.0, 0.3, 10.0, 0.3]).holds);
        // A V whose apex dips to 0.2 above the middle of A: endpoints far,
        // segment b's endpoint near A's interior.
        let v = ClaimSet::verdict(&g, &c, &[5.0, 0.2, 5.0, 3.0]);
        assert!(!v.holds && (v.margin + 0.2).abs() < 1e-6, "{v:?}");
        // A's endpoint near b's interior (b far off to the side otherwise).
        let v = ClaimSet::verdict(&g, &c, &[10.3, -5.0, 10.3, 5.0]);
        assert!(!v.holds && (v.margin + 0.1).abs() < 1e-6, "{v:?}");
    }

    /// The folded differentiator agrees with tang's `diff` numerically and
    /// stays O(1) nodes per variable on a long sum.
    #[test]
    fn sparse_diff_matches_and_stays_small() {
        let mut g = ExprGraph::new();
        let pts: Vec<PointExpr> = (0..200).map(|i| (g.var(2 * i), g.var(2 * i + 1))).collect();
        let len = polyline_len(&mut g, &pts);
        let theta: Vec<f64> = (0..400)
            .map(|i| (i as f64 * 0.37).sin() + i as f64 * 0.1)
            .collect();
        let mut sd = SparseDiff::new();
        let before = g.len();
        let ds: Vec<ExprId> = (0..400u16).map(|v| sd.diff(&mut g, len, v)).collect();
        let grown = g.len() - before;
        assert!(grown < 400 * 40, "folded diff grew {grown} nodes");
        for v in [0u16, 7, 199, 398, 399] {
            let a: f64 = g.eval(ds[v as usize], &theta);
            let r = g.diff(len, v);
            let b: f64 = g.eval(r, &theta);
            assert!((a - b).abs() < 1e-12, "var {v}: {a} vs {b}");
        }
    }
}
