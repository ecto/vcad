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

use super::claims_ir::{self as cir, ClaimSet, PointExpr};
use super::sco::{ResidualSystem, ScoOptions, ScoReport};
use tang_expr::{ExprGraph, ExprId};
use vcad_ir::ecad::Pcb;
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

/// Which optimizer drives the descent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DescentSolver {
    /// Sequential convex optimization (compiled LM + trust region +
    /// penalty continuation, [`super::sco`]). The default.
    #[default]
    Sco,
    /// The original Adam loop on the interpreted gradient graph — kept for
    /// comparison.
    Adam,
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
    let prob = PairProblem::build(
        p_pts, n_pts, extra_p, extra_n, springs, gap_centre, obstacles, weights,
    )?;
    let (theta, it) = prob.solve(&prob.theta0, DescentSolver::default(), iters);
    Some(prob.result(&theta, it))
}

/// One pair's descent problem: the graph, its variables, and the claims the
/// objective is compiled from (see [`super::claims_ir`]).
pub(crate) struct PairProblem {
    pub(crate) g: ExprGraph,
    pub(crate) theta0: Vec<f64>,
    pe: Vec<PointExpr>,
    ne: Vec<PointExpr>,
    p_pts: Vec<Vec2>,
    n_pts: Vec<Vec2>,
    pub(crate) claims: ClaimSet,
    clearance_weight: f64,
}

impl PairProblem {
    /// Author the pair's claims: wirelength, skew, gap springs, and point
    /// clearance against every obstacle near a moving point's start.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        p_pts: &[Vec2],
        n_pts: &[Vec2],
        extra_p: f64,
        extra_n: f64,
        springs: bool,
        gap_centre: f64,
        obstacles: &[DescentObstacle],
        weights: &DescentWeights,
    ) -> Option<Self> {
        if p_pts.len() < 3 || n_pts.len() < 3 {
            return None;
        }
        // Var ids are u16: bail before building rather than overflow.
        if 2 * (p_pts.len() + n_pts.len()) > u16::MAX as usize {
            return None;
        }
        let mut g = ExprGraph::new();
        // Variables: interior points of both legs. Var index = position in θ.
        let mut theta0: Vec<f64> = Vec::new();
        let mut build_leg = |g: &mut ExprGraph, pts: &[Vec2]| -> Vec<PointExpr> {
            pts.iter()
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 || i + 1 == pts.len() {
                        (g.lit(p.x), g.lit(p.y))
                    } else {
                        let x = g.var(theta0.len() as u16);
                        theta0.push(p.x);
                        let y = g.var(theta0.len() as u16);
                        theta0.push(p.y);
                        (x, y)
                    }
                })
                .collect()
        };
        let pe = build_leg(&mut g, p_pts);
        let ne = build_leg(&mut g, n_pts);
        if theta0.is_empty() {
            return None;
        }

        let mut claims = ClaimSet::new();
        let run_p = cir::polyline_len(&mut g, &pe);
        let run_n = cir::polyline_len(&mut g, &ne);
        let extra_p_lit = g.lit(extra_p);
        let extra_n_lit = g.lit(extra_n);
        let len_p = g.add(run_p, extra_p_lit);
        let len_n = g.add(run_n, extra_n_lit);
        let len_sum = g.add(len_p, len_n);
        claims.push(cir::wirelength(len_sum, weights.len));
        // Skew with a zero bound: the descent drives skew to zero (the
        // receipt's 1.1 mm bound is judged on the committed board).
        claims.push(cir::skew(&mut g, len_p, len_n, 0.0, weights.skew));

        // Gap springs between arclength-matched samples (index-proportional
        // pairing keeps the term smooth and cheap).
        let n_springs = if springs { pe.len().min(ne.len()) } else { 0 };
        for k in 1..n_springs.saturating_sub(1) {
            let pi = pe[k * (pe.len() - 1) / (n_springs - 1)];
            let ni = ne[k * (ne.len() - 1) / (n_springs - 1)];
            claims.push(cir::gap(&mut g, pi, ni, gap_centre, 0.0, weights.gap));
        }

        let mut prob = Self {
            g,
            theta0,
            pe,
            ne,
            p_pts: p_pts.to_vec(),
            n_pts: n_pts.to_vec(),
            claims,
            clearance_weight: weights.clearance,
        };
        for ob in obstacles {
            prob.add_point_clearance(ob, None);
        }
        Some(prob)
    }

    /// Point positions at `theta` (pinned endpoints included).
    fn points(&self, theta: &[f64]) -> (Vec<Vec2>, Vec<Vec2>) {
        let mut idx = 0usize;
        let rebuild = |pts: &[Vec2], idx: &mut usize| -> Vec<Vec2> {
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
        let p = rebuild(&self.p_pts, &mut idx);
        let n = rebuild(&self.n_pts, &mut idx);
        (p, n)
    }

    /// Clearance claims for every MOVING point near `ob`. Proximity is
    /// judged at `at` (current θ) or, when `None`, at the starting geometry:
    /// obstacles far from a point can't be reached within a few descent
    /// steps, so skipping them keeps the graph small (radius required + 2mm).
    /// Returns the number of claims added.
    pub(crate) fn add_point_clearance(
        &mut self,
        ob: &DescentObstacle,
        at: Option<&[f64]>,
    ) -> usize {
        let (pp, nn) = match at {
            Some(th) => self.points(th),
            None => (self.p_pts.clone(), self.n_pts.clone()),
        };
        let mut added = 0;
        for (leg, pts) in [(self.pe.clone(), pp), (self.ne.clone(), nn)] {
            for (i, &pt) in leg.iter().enumerate() {
                if i == 0 || i + 1 == pts.len() {
                    continue;
                }
                if pt_seg_dist_f64(pts[i], ob.a, ob.b) > ob.required + 2.0 {
                    continue;
                }
                let c = cir::clearance(
                    &mut self.g,
                    pt,
                    (ob.a.x, ob.a.y),
                    (ob.b.x, ob.b.y),
                    ob.required,
                    self.clearance_weight,
                );
                self.claims.push(c);
                added += 1;
            }
        }
        added
    }

    /// Solve from `theta0` with `solver`; `iters` is Adam's step budget, or
    /// the SCO inner-iteration cap (SCO stops far earlier on convergence).
    pub(crate) fn solve(
        &self,
        theta0: &[f64],
        solver: DescentSolver,
        iters: usize,
    ) -> (Vec<f64>, usize) {
        match solver {
            DescentSolver::Adam => self.solve_adam(theta0, iters),
            DescentSolver::Sco => {
                let (th, rep) = self.solve_sco(theta0, iters);
                (th, rep.iters)
            }
        }
    }

    /// SCO on the claim-compiled residual system.
    pub(crate) fn solve_sco(&self, theta0: &[f64], iters: usize) -> (Vec<f64>, ScoReport) {
        let mut g = self.g.clone();
        let sys = ResidualSystem::compile(&mut g, &self.claims, theta0.len());
        let opts = ScoOptions {
            max_iters: iters.clamp(1, 400),
            ..ScoOptions::default()
        };
        sys.solve(theta0, &opts)
    }

    /// Adam on the claim-compiled loss (the original M5 optimizer).
    pub(crate) fn solve_adam(&self, theta0: &[f64], iters: usize) -> (Vec<f64>, usize) {
        let mut g = self.g.clone();
        let loss = self.claims.loss(&mut g);
        let n_vars = theta0.len();
        let grads: Vec<ExprId> = (0..n_vars as u16).map(|v| g.diff(loss, v)).collect();
        let mut theta = theta0.to_vec();
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
        (theta, it)
    }

    /// Smooth loss at `theta`.
    pub(crate) fn loss_at(&self, theta: &[f64]) -> f64 {
        let mut g = self.g.clone();
        let loss = self.claims.loss(&mut g);
        g.eval(loss, theta)
    }

    /// Package `theta` as a [`DescentResult`].
    pub(crate) fn result(&self, theta: &[f64], iters: usize) -> DescentResult {
        let (p_out, n_out) = self.points(theta);
        let plen: f64 = p_out.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let nlen: f64 = n_out.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        DescentResult {
            p_pts: p_out,
            n_pts: n_out,
            loss: self.loss_at(theta),
            skew: (plen - nlen).abs(),
            iters,
        }
    }
}

/// What [`descend_board`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DescentReport {
    /// Pairs whose geometry was replaced with a lower-skew descent.
    pub tuned: usize,
    /// Pairs that had enough interior freedom to attempt.
    pub attempted: usize,
    /// Attempts thrown away because the exact oracle rejected the geometry
    /// or the whole-net skew did not actually improve.
    pub rejected: usize,
}

/// Run the differentiable pair polish over every classified pair on a routed
/// board, committing only geometry the exact oracle accepts.
///
/// This is the library form of what `examples/si_descent.rs` used to do
/// inline, so the router's finishing stage and the example share one
/// implementation. Per pair: take the longest single-layer run of each leg,
/// densify it, build the obstacle set from every other net's copper near the
/// corridor, descend, then accept only if (a) every optimized segment probes
/// legal against a session with both nets' old copper removed and (b) the
/// WHOLE-net skew actually fell. Anything else leaves the pair untouched.
pub fn descend_board(pcb: &mut Pcb, iters: usize) -> DescentReport {
    descend_board_with(pcb, iters, DescentSolver::default())
}

/// [`descend_board`] with an explicit optimizer (Adam kept for comparison).
pub fn descend_board_with(pcb: &mut Pcb, iters: usize, solver: DescentSolver) -> DescentReport {
    use super::classes::{apply_classes, classify_nets};
    use super::length_match::longest_chain;
    use crate::session::RouteSession;
    use crate::spatial::CopperGeom;
    use vcad_ir::ecad::{PcbLayer, Trace};

    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
        for f in &pcb.footprints {
            for pad in &f.pads {
                if let Some(n) = &pad.net {
                    if !n.is_empty() {
                        v.insert(n.clone());
                    }
                }
            }
        }
        v.into_iter().collect()
    };
    let c = classify_nets(&nets);
    apply_classes(pcb, &c);
    let dp = pcb
        .rules
        .class_rules
        .iter()
        .find(|r| r.name == super::classes::DIFF_PAIR_CLASS);
    let gap_edge = dp.and_then(|r| r.diff_pair_gap).unwrap_or(0.25);
    let leg_w = dp.and_then(|r| r.diff_pair_width).unwrap_or(0.2);
    // +20um cushion, the same reasoning the clearance hinge uses: the gap
    // spring is a SOFT term traded against skew, so its equilibrium has to
    // land outside the hard rule the oracle enforces (pair gap - 5um).
    // Targeting the gap exactly puts every descended pair on the threshold
    // and the oracle then rejects all of them.
    let gap_centre = gap_edge + leg_w + 0.02;

    // Longest single-layer run of a net, plus the length of everything else.
    let net_run = |pcb: &Pcb, net: &str| -> Option<(PcbLayer, Vec<Vec2>, f64, f64)> {
        let mut layers: Vec<PcbLayer> = pcb
            .traces
            .iter()
            .filter(|t| t.net == net)
            .map(|t| t.layer)
            .collect();
        layers.sort();
        layers.dedup();
        let total: f64 = pcb
            .traces
            .iter()
            .filter(|t| t.net == net)
            .map(|t| (t.end - t.start).length())
            .sum();
        let mut best: Option<(f64, Vec<Vec2>, PcbLayer)> = None;
        for layer in layers {
            let segs: Vec<&Trace> = pcb
                .traces
                .iter()
                .filter(|t| t.net == net && t.layer == layer)
                .collect();
            if let Some(points) = longest_chain(&segs) {
                let len: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
                if best.as_ref().map(|(l, _, _)| len > *l).unwrap_or(true) {
                    best = Some((len, points, layer));
                }
            }
        }
        best.map(|(len, pts, layer)| {
            let width = pcb
                .traces
                .iter()
                .find(|t| t.net == net && t.layer == layer)
                .map(|t| t.width)
                .unwrap_or(0.08);
            (layer, pts, total - len, width)
        })
    };

    let mut report = DescentReport::default();
    for (pn, nn) in &c.pairs {
        let (Some((pl, p_raw, extra_p, p_w)), Some((nlayer, n_raw, extra_n, n_w))) =
            (net_run(pcb, pn), net_run(pcb, nn))
        else {
            continue;
        };
        // Point hinges only constrain POINTS — a long segment between two
        // legal points sweeps across the board unconstrained. Resampling at
        // ~0.4mm makes point coverage approximate segment coverage.
        let densify = |pts: &[Vec2]| -> Vec<Vec2> {
            let mut out = vec![pts[0]];
            for w in pts.windows(2) {
                let len = (w[1] - w[0]).length();
                let n = (len / 0.4).ceil().max(1.0) as usize;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    out.push(w[0] + (w[1] - w[0]).scale(t));
                }
            }
            out
        };
        let p_pts = densify(&p_raw);
        let n_pts = densify(&n_raw);
        let run_w = p_w.max(n_w);
        let plen: f64 = p_pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum::<f64>()
            + extra_p;
        let nlen: f64 = n_pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum::<f64>()
            + extra_n;
        if (plen - nlen).abs() < 0.2 || p_pts.len() < 3 || n_pts.len() < 3 {
            continue; // already matched, or no interior freedom
        }
        report.attempted += 1;

        let session = RouteSession::from_pcb(pcb);
        let clearance = session.clearance_for(pn);
        let (mut lo, mut hi) = (
            Vec2::new(f64::INFINITY, f64::INFINITY),
            Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for p in p_pts.iter().chain(n_pts.iter()) {
            lo.x = lo.x.min(p.x - 3.0);
            lo.y = lo.y.min(p.y - 3.0);
            hi.x = hi.x.max(p.x + 3.0);
            hi.y = hi.y.max(p.y + 3.0);
        }
        // Obstacles: EVERY copper element near the corridor on either run
        // layer, from a session with both nets removed — pads and vias
        // included, since a descended run can swing across a pad field a
        // trace-only obstacle set cannot see.
        let mut ob_board = pcb.clone();
        ob_board.traces.retain(|t| t.net != *pn && t.net != *nn);
        ob_board.vias.retain(|v| v.net != *pn && v.net != *nn);
        let ob_session = RouteSession::from_pcb(&ob_board);
        let mut obstacles = Vec::new();
        let mut ob_layers = vec![pl];
        if nlayer != pl {
            ob_layers.push(nlayer);
        }
        for &layer in &ob_layers {
            ob_session.for_each_blocking(
                layer,
                "",
                [lo.x, lo.y],
                [hi.x, hi.y],
                |geom, emin, emax, req| {
                    // +0.05mm safety: the hinge is a soft penalty traded
                    // against skew, so its equilibrium must land OUTSIDE the
                    // hard rule the oracle enforces.
                    let required = req + run_w / 2.0 + 0.05;
                    match geom {
                        CopperGeom::Segment { a, b, half_w } => obstacles.push(DescentObstacle {
                            a: *a,
                            b: *b,
                            required: required + half_w,
                        }),
                        CopperGeom::Disc { center, r } => obstacles.push(DescentObstacle {
                            a: *center,
                            b: *center,
                            required: required + r,
                        }),
                        _ => {
                            let c = Vec2::new((emin[0] + emax[0]) / 2.0, (emin[1] + emax[1]) / 2.0);
                            let hd = ((emax[0] - emin[0]).powi(2) + (emax[1] - emin[1]).powi(2))
                                .sqrt()
                                / 2.0;
                            obstacles.push(DescentObstacle {
                                a: c,
                                b: c,
                                required: required + hd,
                            });
                        }
                    }
                },
            );
        }

        let Some(prob) = PairProblem::build(
            &p_pts,
            &n_pts,
            extra_p,
            extra_n,
            pl == nlayer,
            gap_centre,
            &obstacles,
            &DescentWeights::default(),
        ) else {
            continue;
        };
        let (theta, it) = prob.solve(&prob.theta0, solver, iters);
        let r = prob.result(&theta, it);

        // Fail-closed: every optimized segment passes the exact oracle. Each
        // leg is checked against a board with only ITS OWN old copper removed,
        // so the twin stays an obstacle — the session applies the declared
        // pair gap to it, so a correctly coupled leg passes while one that has
        // drifted into its partner does not. (Dropping both nets would let the
        // descent short the pair to itself unnoticed: the gap spring is a soft
        // term, not a guarantee.)
        // Both legs move together, so the twin must be present at its NEW
        // position: checking P-new against N-old rejects every descent,
        // because the pair as a whole has shifted.
        let with_new = |keep: &str, moved: &[Vec2], moved_net: &str, layer: PcbLayer, w: f64| {
            let mut b = pcb.clone();
            b.traces.retain(|t| t.net != *pn && t.net != *nn);
            b.traces.extend(moved.windows(2).map(|s| Trace {
                start: s[0],
                end: s[1],
                width: w,
                layer,
                net: moved_net.to_string(),
                source: None,
            }));
            let _ = keep;
            RouteSession::from_pcb(&b)
        };
        // P is judged against a board holding N's descended copper, and
        // vice versa.
        let p_session = with_new(pn, &r.n_pts, nn, nlayer, n_w);
        let n_session = with_new(nn, &r.p_pts, pn, pl, p_w);
        let legal = |sess: &RouteSession, pts: &[Vec2], net: &str, layer: PcbLayer, w: f64| {
            pts.windows(2).all(|seg| {
                sess.probe(
                    &CopperGeom::Segment {
                        a: seg[0],
                        b: seg[1],
                        half_w: w / 2.0,
                    },
                    layer,
                    net,
                    clearance,
                )
                .legal
            })
        };
        if !legal(&p_session, &r.p_pts, pn, pl, p_w)
            || !legal(&n_session, &r.n_pts, nn, nlayer, n_w)
        {
            report.rejected += 1;
            continue;
        }
        // Whole-net accounting: r.skew is run-only; judge the committed
        // outcome on the full nets and accept only real improvements.
        let before = (plen - nlen).abs();
        let run_p_after: f64 = r.p_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let run_n_after: f64 = r.n_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let after = ((run_p_after + extra_p) - (run_n_after + extra_n)).abs();
        if after >= before {
            report.rejected += 1;
            continue;
        }
        let mut work2 = pcb.clone();
        let replace_run = |work2: &mut Pcb,
                           net: &str,
                           layer: PcbLayer,
                           old: &[Vec2],
                           new: &[Vec2],
                           width: f64| {
            let on_old = |t: &Trace| {
                t.net == net
                    && t.layer == layer
                    && old.windows(2).any(|w| {
                        (t.start - w[0]).length() < 1e-6 && (t.end - w[1]).length() < 1e-6
                            || (t.start - w[1]).length() < 1e-6 && (t.end - w[0]).length() < 1e-6
                    })
            };
            work2.traces.retain(|t| !on_old(t));
            work2.traces.extend(new.windows(2).map(|w| Trace {
                start: w[0],
                end: w[1],
                width,
                layer,
                net: net.to_string(),
                source: None,
            }));
        };
        replace_run(&mut work2, pn, pl, &p_raw, &r.p_pts, p_w);
        replace_run(&mut work2, nn, nlayer, &n_raw, &r.n_pts, n_w);
        *pcb = work2;
        report.tuned += 1;
        log::info!("descent: {pn} whole-net skew {before:.3} -> {after:.3} mm");
    }
    report
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

    /// M1 equivalence: the claim-compiled objective equals the original
    /// hand-written M5 formula (len + skew² + gap springs + clearance
    /// hinge²) evaluated directly in f64 at a perturbed θ.
    #[test]
    fn claim_objective_matches_handwritten_formula() {
        let p: Vec<Vec2> = (0..=6).map(|i| Vec2::new(2.0 * i as f64, 0.0)).collect();
        let n: Vec<Vec2> = (0..=6)
            .map(|i| Vec2::new(2.0 * i as f64, 0.5 + 0.3 * (i % 2) as f64))
            .collect();
        let obs = vec![DescentObstacle {
            a: Vec2::new(5.0, -0.5),
            b: Vec2::new(7.0, -0.5),
            required: 0.6,
        }];
        let w = DescentWeights::default();
        let prob = PairProblem::build(&p, &n, 0.3, 0.0, true, 0.45, &obs, &w).unwrap();
        let theta: Vec<f64> = prob
            .theta0
            .iter()
            .enumerate()
            .map(|(i, v)| v + 0.07 * ((i * 7 % 5) as f64 - 2.0))
            .collect();
        let (pp, nn) = prob.points(&theta);
        let len = |v: &[Vec2]| -> f64 {
            v.windows(2)
                .map(|w| ((w[1] - w[0]).length().powi(2) + 1e-9).sqrt())
                .sum()
        };
        let (lp, ln) = (len(&pp) + 0.3, len(&nn));
        let mut want = w.len * (lp + ln) + w.skew * (lp - ln).powi(2);
        let ns = pp.len().min(nn.len());
        for k in 1..ns - 1 {
            let a = pp[k * (pp.len() - 1) / (ns - 1)];
            let b = nn[k * (nn.len() - 1) / (ns - 1)];
            let d = ((b - a).length().powi(2) + 1e-9).sqrt();
            want += w.gap * (d - 0.45).powi(2);
        }
        for leg in [&pp, &nn] {
            for (i, &pt) in leg.iter().enumerate() {
                if i == 0 || i + 1 == leg.len() {
                    continue;
                }
                let start = if std::ptr::eq(leg, &pp) { p[i] } else { n[i] };
                if pt_seg_dist_f64(start, obs[0].a, obs[0].b) > obs[0].required + 2.0 {
                    continue;
                }
                let short = obs[0].required - pt_seg_dist_f64(pt, obs[0].a, obs[0].b);
                if short > 0.0 {
                    want += w.clearance * short * short;
                }
            }
        }
        let got = prob.loss_at(&theta);
        assert!(
            (got - want).abs() < 1e-6 * (1.0 + want),
            "claims {got} vs formula {want}"
        );
    }

    /// Fixture B: a 40 mm pair densified at 0.4 mm (≈400 variables), N
    /// carrying a 3-bump detour (≈2 mm skew), obstacles flanking both legs.
    fn long_fixture() -> (Vec<Vec2>, Vec<Vec2>, Vec<DescentObstacle>) {
        let p: Vec<Vec2> = (0..=100).map(|i| Vec2::new(0.4 * i as f64, 0.0)).collect();
        let n: Vec<Vec2> = (0..=100)
            .map(|i| {
                let x = 0.4 * i as f64;
                let bump = if (10.0..30.0).contains(&x) {
                    0.6 * (std::f64::consts::PI * (x - 10.0) / 20.0 * 3.0).sin().abs()
                } else {
                    0.0
                };
                Vec2::new(x, 0.45 + bump)
            })
            .collect();
        let mut obs = Vec::new();
        for k in 0..6 {
            let x0 = 3.0 + 6.0 * k as f64;
            obs.push(DescentObstacle {
                a: Vec2::new(x0, -0.55),
                b: Vec2::new(x0 + 2.0, -0.55),
                required: 0.4,
            });
            obs.push(DescentObstacle {
                a: Vec2::new(x0 + 3.0, 1.6),
                b: Vec2::new(x0 + 5.0, 1.6),
                required: 0.4,
            });
        }
        (p, n, obs)
    }

    fn min_margin(r: &DescentResult, obs: &[DescentObstacle]) -> f64 {
        let mut m = f64::INFINITY;
        for pt in r.p_pts.iter().chain(r.n_pts.iter()) {
            for o in obs {
                m = m.min(pt_seg_dist_f64(*pt, o.a, o.b) - o.required);
            }
        }
        m
    }

    /// M2 benchmark: SCO vs Adam on the descent fixtures — same claims, same
    /// objective. Reports final loss, iterations and wall time (run with
    /// `--nocapture` to see the table); asserts SCO reaches an objective no
    /// worse than Adam's and holds every clearance claim exactly.
    #[test]
    fn sco_vs_adam_on_descent_fixtures() {
        let short_p: Vec<Vec2> = (0..=10).map(|i| Vec2::new(2.0 * i as f64, 0.0)).collect();
        let short_n: Vec<Vec2> = (0..=10)
            .map(|i| Vec2::new(2.0 * i as f64, 0.45 + if i % 2 == 0 { 0.0 } else { 0.8 }))
            .collect();
        let short_obs = vec![DescentObstacle {
            a: Vec2::new(9.0, -0.6),
            b: Vec2::new(11.0, -0.6),
            required: 0.4,
        }];
        let (lp, ln, lobs) = long_fixture();
        let w = DescentWeights::default();
        for (name, p, n, obs) in [
            ("A short", short_p, short_n, short_obs),
            ("B long", lp, ln, lobs),
        ] {
            let prob = PairProblem::build(&p, &n, 0.0, 0.0, true, 0.45, &obs, &w).unwrap();
            let t = std::time::Instant::now();
            let (ta, ia) = prob.solve(&prob.theta0, DescentSolver::Adam, 3000);
            let adam_ms = t.elapsed().as_secs_f64() * 1e3;
            let ra = prob.result(&ta, ia);
            let t = std::time::Instant::now();
            let (ts, is) = prob.solve(&prob.theta0, DescentSolver::Sco, 3000);
            let sco_ms = t.elapsed().as_secs_f64() * 1e3;
            let rs = prob.result(&ts, is);
            eprintln!(
                "{name}: vars={} claims={} | adam loss={:.5} skew={:.4} margin={:+.4} iters={} {:.1}ms | sco loss={:.5} skew={:.4} margin={:+.4} iters={} {:.1}ms",
                prob.theta0.len(),
                prob.claims.claims.len(),
                ra.loss, ra.skew, min_margin(&ra, &obs), ra.iters, adam_ms,
                rs.loss, rs.skew, min_margin(&rs, &obs), rs.iters, sco_ms,
            );
            assert!(
                rs.loss <= ra.loss * 1.01 + 1e-6,
                "{name}: sco {} vs adam {}",
                rs.loss,
                ra.loss
            );
            assert!(
                min_margin(&rs, &obs) >= -1e-9,
                "{name}: sco broke clearance"
            );
            assert!(rs.skew < 0.15, "{name}: sco skew {}", rs.skew);
        }
    }
}
