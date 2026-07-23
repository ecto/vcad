//! Differentiable copper (GPU-router charter M5).
//!
//! A routed differential pair's geometry becomes an optimization variable:
//! interior polyline points of both legs are `tang-expr` graph variables,
//! and the board objective is built symbolically from exactly the terms the
//! SI receipt judges —
//!
//! ```text
//! L(θ) = w_len   · (len_P + len_N)                    (wirelength)
//!      + w_skew  · (len_P − len_N)²                   (intra-pair skew claim)
//!      + w_gap   · Σ (‖P_i − N_i‖ − gap)²             (coupled-fraction claim)
//!      + w_clr   · Σ hinge(req − dist(seg, obstacle))² (clearance / DRC)
//! ```
//!
//! Gradients come from `tang-expr`'s symbolic `diff` (memoized, shared
//! subexpressions computed once) and an Adam loop descends the whole pair at
//! once. Endpoints stay pinned to their pads (they are literals, not
//! variables) and layers never change — geometry flows, topology holds.
//!
//! The optimizer's output is a PROPOSAL like any other: the caller runs the
//! exact oracle (`session.probe`) over the final geometry and rejects the
//! descent wholesale if anything is illegal — fail-closed, per the charter.
//! The same expression graph evaluates on `f64` today and `GpuTensor`
//! tomorrow (tang's `Scalar` genericity); the objective, not the backend, is
//! the product.

use tang_expr::{ExprGraph, ExprId};
use vcad_ir::Vec2;

/// Weights for the objective's terms.
#[derive(Debug, Clone)]
pub struct DescentWeights {
    /// Wirelength.
    pub len: f64,
    /// Intra-pair length skew (the receipt bound is 1.1mm; the square pulls
    /// hard well before that).
    pub skew: f64,
    /// Gap spring toward the class gap (coupling).
    pub gap: f64,
    /// Clearance hinge (quadratic beyond the requirement).
    pub clearance: f64,
}

impl Default for DescentWeights {
    fn default() -> Self {
        Self {
            len: 0.05,
            skew: 4.0,
            gap: 2.0,
            clearance: 50.0,
        }
    }
}

/// A static obstacle for the clearance term: segment with a required
/// edge-to-edge distance (its half-width + clearance already folded in).
#[derive(Debug, Clone, Copy)]
pub struct DescentObstacle {
    /// Segment start.
    pub a: Vec2,
    /// Segment end.
    pub b: Vec2,
    /// Required centre-to-segment distance (obstacle half-width + own
    /// half-width + clearance).
    pub required: f64,
}

/// Result of a pair descent.
pub struct DescentResult {
    /// Optimized P-leg points (endpoints unchanged).
    pub p_pts: Vec<Vec2>,
    /// Optimized N-leg points (endpoints unchanged).
    pub n_pts: Vec<Vec2>,
    /// Final objective value.
    pub loss: f64,
    /// |len_P − len_N| after descent (mm).
    pub skew: f64,
    /// Iterations run.
    pub iters: usize,
}

/// Descend one differential pair. `p_pts`/`n_pts` are single-layer polylines
/// (same point count per leg is NOT required); `gap` is the class edge gap
/// plus trace width (centre-to-centre target).
pub fn descend_pair(
    p_pts: &[Vec2],
    n_pts: &[Vec2],
    gap_centre: f64,
    obstacles: &[DescentObstacle],
    weights: &DescentWeights,
    iters: usize,
) -> Option<DescentResult> {
    descend_pair_runs(
        p_pts, n_pts, 0.0, 0.0, true, gap_centre, obstacles, weights, iters,
    )
}

/// Multi-run variant (charter M6): optimize one single-layer RUN per leg
/// while the rest of each net's length rides along as a constant, so the
/// skew term still measures the WHOLE net. `extra_p`/`extra_n` are the
/// untouched copper lengths; `springs` couples the runs toward the gap only
/// when they share a layer.
#[allow(clippy::too_many_arguments)]
pub fn descend_pair_runs(
    p_pts: &[Vec2],
    n_pts: &[Vec2],
    extra_p: f64,
    extra_n: f64,
    springs: bool,
    gap_centre: f64,
    obstacles: &[DescentObstacle],
    weights: &DescentWeights,
    iters: usize,
) -> Option<DescentResult> {
    if p_pts.len() < 3 || n_pts.len() < 3 {
        return None;
    }
    let mut g = ExprGraph::new();

    // Variables: interior points of both legs. Var index = position in θ.
    let mut theta0: Vec<f64> = Vec::new();
    let var = |g: &mut ExprGraph, v: f64, theta0: &mut Vec<f64>| -> ExprId {
        let id = g.var(theta0.len() as u16);
        theta0.push(v);
        id
    };
    let build_leg =
        |g: &mut ExprGraph, pts: &[Vec2], theta0: &mut Vec<f64>| -> Vec<(ExprId, ExprId)> {
            let mut out = Vec::with_capacity(pts.len());
            for (i, p) in pts.iter().enumerate() {
                if i == 0 || i + 1 == pts.len() {
                    let x = g.lit(p.x);
                    let y = g.lit(p.y);
                    out.push((x, y));
                } else {
                    let x = var(g, p.x, theta0);
                    let y = var(g, p.y, theta0);
                    out.push((x, y));
                }
            }
            out
        };
    let pe = build_leg(&mut g, p_pts, &mut theta0);
    let ne = build_leg(&mut g, n_pts, &mut theta0);
    let n_vars = theta0.len();
    if n_vars == 0 || n_vars > u16::MAX as usize {
        return None;
    }

    let eps = g.lit(1e-9);
    let dist2 = |g: &mut ExprGraph, a: (ExprId, ExprId), b: (ExprId, ExprId)| -> ExprId {
        let nax = g.neg(a.0);
        let nay = g.neg(a.1);
        let dx = g.add(b.0, nax);
        let dy = g.add(b.1, nay);
        let dx2 = g.mul(dx, dx);
        let dy2 = g.mul(dy, dy);
        g.add(dx2, dy2)
    };
    let dist =
        |g: &mut ExprGraph, a: (ExprId, ExprId), b: (ExprId, ExprId), eps: ExprId| -> ExprId {
            let d2 = dist2(g, a, b);
            let d2e = g.add(d2, eps);
            g.sqrt(d2e)
        };
    let leg_len = |g: &mut ExprGraph, leg: &[(ExprId, ExprId)], eps: ExprId| -> ExprId {
        let mut total = g.lit(0.0);
        for w in leg.windows(2) {
            let d = dist(g, w[0], w[1], eps);
            total = g.add(total, d);
        }
        total
    };

    let run_p = leg_len(&mut g, &pe, eps);
    let run_n = leg_len(&mut g, &ne, eps);
    let extra_p_lit = g.lit(extra_p);
    let extra_n_lit = g.lit(extra_n);
    let len_p = g.add(run_p, extra_p_lit);
    let len_n = g.add(run_n, extra_n_lit);

    // Wirelength.
    let w_len = g.lit(weights.len);
    let len_sum = g.add(len_p, len_n);
    let mut loss = g.mul(w_len, len_sum);

    // Skew: (len_P − len_N)².
    let w_skew = g.lit(weights.skew);
    let nln = g.neg(len_n);
    let skew = g.add(len_p, nln);
    let skew2 = g.mul(skew, skew);
    let skew_term = g.mul(w_skew, skew2);
    loss = g.add(loss, skew_term);

    // Gap springs between arclength-matched samples (index-proportional
    // pairing keeps the term smooth and cheap).
    let w_gap = g.lit(weights.gap);
    let gap_lit = g.lit(gap_centre);
    let n_springs = if springs { pe.len().min(ne.len()) } else { 0 };
    for k in 1..n_springs.saturating_sub(1) {
        let pi = pe[k * (pe.len() - 1) / (n_springs - 1)];
        let ni = ne[k * (ne.len() - 1) / (n_springs - 1)];
        let d = dist(&mut g, pi, ni, eps);
        let ng = g.neg(gap_lit);
        let dev = g.add(d, ng);
        let dev2 = g.mul(dev, dev);
        let spring = g.mul(w_gap, dev2);
        loss = g.add(loss, spring);
    }

    // Clearance hinge: for each moving point vs each obstacle,
    // hinge(required − dist_pt_seg)² with a select-based smooth hinge.
    let w_clr = g.lit(weights.clearance);
    let zero = g.lit(0.0);
    for (leg, pts) in [(&pe, p_pts), (&ne, n_pts)] {
        for (i, &pt) in leg.iter().enumerate() {
            if i == 0 || i + 1 == pts.len() {
                continue;
            }
            for ob in obstacles {
                // Prefilter: obstacles far from the ORIGINAL point can't be
                // reached within a few descent steps; skip to keep the graph
                // small. (Conservative radius: required + 2mm.)
                let p0 = pts[i];
                let seg_d = pt_seg_dist_f64(p0, ob.a, ob.b);
                if seg_d > ob.required + 2.0 {
                    continue;
                }
                let (ax, ay) = (g.lit(ob.a.x), g.lit(ob.a.y));
                let (bx, by) = (g.lit(ob.b.x), g.lit(ob.b.y));
                let d = pt_seg_dist_expr(&mut g, pt, (ax, ay), (bx, by), eps);
                let req = g.lit(ob.required);
                let ndist = g.neg(d);
                let short = g.add(req, ndist); // > 0 when violating
                let short2 = g.mul(short, short);
                let hinge = g.select(short, short2, zero); // select(cond>0, a, b)
                let term = g.mul(w_clr, hinge);
                loss = g.add(loss, term);
            }
        }
    }

    // Symbolic gradient for every variable.
    let grads: Vec<ExprId> = (0..n_vars as u16).map(|v| g.diff(loss, v)).collect();

    // Adam.
    let mut theta = theta0.clone();
    let (mut m, mut v_) = (vec![0.0; n_vars], vec![0.0; n_vars]);
    let (b1, b2, lr, eps_a) = (0.9, 0.999, 0.02, 1e-8);
    let mut last_loss = f64::INFINITY;
    let mut it = 0;
    while it < iters {
        it += 1;
        let gvals = g.eval_many(&grads, &theta);
        for i in 0..n_vars {
            m[i] = b1 * m[i] + (1.0 - b1) * gvals[i];
            v_[i] = b2 * v_[i] + (1.0 - b2) * gvals[i] * gvals[i];
            let mh = m[i] / (1.0 - b1.powi(it as i32));
            let vh = v_[i] / (1.0 - b2.powi(it as i32));
            theta[i] -= lr * mh / (vh.sqrt() + eps_a);
        }
        if it % 50 == 0 {
            let l = g.eval(loss, &theta);
            if (last_loss - l).abs() < 1e-9 {
                break;
            }
            last_loss = l;
        }
    }
    let final_loss = g.eval(loss, &theta);

    // Rebuild point lists.
    let mut idx = 0usize;
    let rebuild = |pts: &[Vec2], idx: &mut usize, theta: &[f64]| -> Vec<Vec2> {
        pts.iter()
            .enumerate()
            .map(|(i, p)| {
                if i == 0 || i + 1 == pts.len() {
                    *p
                } else {
                    let out = Vec2::new(theta[*idx], theta[*idx + 1]);
                    *idx += 2;
                    out
                }
            })
            .collect()
    };
    let p_out = rebuild(p_pts, &mut idx, &theta);
    let n_out = rebuild(n_pts, &mut idx, &theta);
    let plen: f64 = p_out.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let nlen: f64 = n_out.windows(2).map(|w| (w[1] - w[0]).length()).sum();

    Some(DescentResult {
        p_pts: p_out,
        n_pts: n_out,
        loss: final_loss,
        skew: (plen - nlen).abs(),
        iters: it,
    })
}

fn pt_seg_dist_f64(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-18 {
        return (p - a).length();
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    (p - (a + ab.scale(t))).length()
}

/// Differentiable point-to-segment distance: parameter t clamped to [0, 1]
/// via two selects, then Euclidean distance to the clamped foot point.
fn pt_seg_dist_expr(
    g: &mut ExprGraph,
    p: (ExprId, ExprId),
    a: (ExprId, ExprId),
    b: (ExprId, ExprId),
    eps: ExprId,
) -> ExprId {
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
    // clamp: t = select(t_raw, select(t_raw - 1, 1, t_raw), 0)  — i.e.
    // t_raw <= 0 → 0; t_raw >= 1 → 1; else t_raw. select(c, x, y) = x when
    // c > 0 else y.
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
    let nfx = g.neg(fx);
    let nfy = g.neg(fy);
    let ex = g.add(p.0, nfx);
    let ey = g.add(p.1, nfy);
    let ex2 = g.mul(ex, ex);
    let ey2 = g.mul(ey, ey);
    let e2 = g.add(ex2, ey2);
    let e2e = g.add(e2, eps);
    g.sqrt(e2e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The M5 contract in miniature: a pair with deliberate skew (N zigzags)
    /// and an obstacle near P. Descent must (1) drive intra-pair skew to
    /// near zero, (2) hold the clearance hinge (no point ends inside the
    /// obstacle's requirement), (3) keep endpoints pinned.
    #[test]
    fn descent_kills_skew_and_respects_clearance() {
        // P: straight 20mm. N: zigzag ≈ 21.6mm — skew 1.6mm.
        let p: Vec<Vec2> = (0..=10).map(|i| Vec2::new(2.0 * i as f64, 0.0)).collect();
        let n: Vec<Vec2> = (0..=10)
            .map(|i| Vec2::new(2.0 * i as f64, 0.45 + if i % 2 == 0 { 0.0 } else { 0.8 }))
            .collect();
        let skew0: f64 = {
            let pl: f64 = p.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            let nl: f64 = n.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            (pl - nl).abs()
        };
        assert!(skew0 > 1.0, "test needs real initial skew, got {skew0:.2}");

        let obstacles = vec![DescentObstacle {
            a: Vec2::new(9.0, -0.6),
            b: Vec2::new(11.0, -0.6),
            required: 0.4,
        }];
        let r = descend_pair(&p, &n, 0.45, &obstacles, &DescentWeights::default(), 3000)
            .expect("descent runs");

        assert!(
            r.skew < 0.15,
            "skew must collapse: {:.3} -> {:.3}",
            skew0,
            r.skew
        );
        // Endpoints pinned.
        assert_eq!(r.p_pts[0], p[0]);
        assert_eq!(*r.p_pts.last().unwrap(), *p.last().unwrap());
        // Clearance hinge held.
        for pt in r.p_pts.iter().chain(r.n_pts.iter()) {
            let d = pt_seg_dist_f64(*pt, obstacles[0].a, obstacles[0].b);
            assert!(
                d >= obstacles[0].required - 0.02,
                "point ({:.2},{:.2}) violates the obstacle: {d:.3}",
                pt.x,
                pt.y
            );
        }
    }
}
