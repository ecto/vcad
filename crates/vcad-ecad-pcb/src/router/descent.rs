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
use super::sco::{ResidualSystem, ScoMethod, ScoOptions, ScoReport};
use tang_expr::{ExprGraph, ExprId};
use vcad_ir::ecad::{Pcb, PcbLayer, Trace};
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
    /// Sequential convex optimization with hard claims as linearized
    /// constraints in each trust-region QP ([`super::sco::ScoMethod::Sqp`],
    /// M7). Opt-in: after M9-M11 it ties LM on CM5 acceptance (A 13 vs
    /// 12 of 20, B 19 vs 20 of 45) at 3.4x LM's wall time on B.
    Sco,
    /// The M2 SCO: hard claims as penalty rows in LM + augmented-Lagrangian
    /// continuation ([`super::sco::ScoMethod::Lm`]). The default.
    #[default]
    ScoLm,
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

/// What the exact oracle said about a candidate geometry.
#[derive(Debug, Clone, Default)]
pub struct OracleVerdict {
    /// No violation at all.
    pub clean: bool,
    /// Obstacles behind the violations, as clearance cuts to mine. May be
    /// empty for a dirty verdict the claim vocabulary cannot express.
    pub cuts: Vec<DescentObstacle>,
    /// Twin-gap violations to mine (M6): `(P segment, N segment)` index
    /// pairs into the two point lists — segment `i` runs `pts[i]→pts[i+1]`.
    pub twin_cuts: Vec<(usize, usize)>,
    /// Every dirty segment, `(leg, segment)` with leg 0 = P, 1 = N. Lets the
    /// caller compare two verdicts violation-for-violation.
    pub dirty: Vec<(u8, usize)>,
}

/// Outcome of [`descend_pair_mined`].
pub struct MinedOutcome {
    /// Final geometry.
    pub result: DescentResult,
    /// Solve rounds run (1 = the oracle was clean after the first solve).
    pub rounds: usize,
    /// Clearance claims added from oracle violations.
    pub cuts: usize,
    /// The oracle accepted the final geometry. When false the caller MUST
    /// reject the descent (fail-closed).
    pub clean: bool,
    /// Dirty segments of the UNMODIFIED copper against the same oracle
    /// (non-zero = the pair was "pre-dirty").
    pub baseline_dirty: usize,
    /// Dirty segments of the final geometry.
    pub final_dirty: usize,
    /// Final geometry is dirty but strictly less so than the baseline, with
    /// no violation the baseline didn't already have ("improved-but-dirty").
    /// Diagnostics only — never committed.
    pub improved: bool,
}

/// Lazy constraint generation ("tang-route" M3): descend with only the
/// cheap claims plus clearance against `seed` obstacles, then run the exact
/// `oracle` on the result. The oracle's verdict carries the obstacles the
/// geometry violates; each becomes new clearance claims (points and
/// segment midpoints near it), and the problem is re-solved warm-started
/// from the previous θ — until the oracle is clean, `max_rounds` is spent,
/// or a round mines nothing new (a violation the claim vocabulary can't
/// express). Twin violations (M6) mine [`cir::twin_gap`] claims between the
/// offending P and N segments when `twin_required` (centre-line separation)
/// is given. Never returns oracle-dirty geometry as clean.
#[allow(clippy::too_many_arguments)]
pub fn descend_pair_mined(
    p_pts: &[Vec2],
    n_pts: &[Vec2],
    extra_p: f64,
    extra_n: f64,
    springs: bool,
    gap_centre: f64,
    twin_required: Option<f64>,
    seed: &[DescentObstacle],
    weights: &DescentWeights,
    solver: DescentSolver,
    iters: usize,
    max_rounds: usize,
    mut oracle: impl FnMut(&[Vec2], &[Vec2]) -> OracleVerdict,
) -> Option<MinedOutcome> {
    let mut prob = PairProblem::build(
        p_pts, n_pts, extra_p, extra_n, springs, gap_centre, seed, weights,
    )?;
    if let Some(req) = twin_required {
        prob.seed_twin(req);
    }
    let mut theta = prob.theta0.clone();
    let mut cuts = 0;
    let mut total_iters = 0;
    // Baseline: is the UNCHANGED geometry oracle-clean? A pair that starts
    // dirty is reported distinctly, and is the reference for
    // "improved-but-dirty".
    let v0 = oracle(p_pts, n_pts);
    log::debug!(
        target: "descent_stats",
        "round=0 baseline_clean={} viol_cuts={} dirty_segs={}",
        v0.clean,
        v0.cuts.len(),
        v0.dirty.len()
    );
    let base: std::collections::BTreeSet<(u8, usize)> = v0.dirty.iter().copied().collect();
    let finish = |result: DescentResult, rounds, cuts, v: &OracleVerdict| {
        let fin: std::collections::BTreeSet<(u8, usize)> = v.dirty.iter().copied().collect();
        MinedOutcome {
            result,
            rounds,
            cuts,
            clean: v.clean,
            baseline_dirty: base.len(),
            final_dirty: fin.len(),
            improved: !v.clean && fin.len() < base.len() && fin.is_subset(&base),
        }
    };
    let mut restarted = false;
    for round in 1..=max_rounds.max(1) {
        let t0 = std::time::Instant::now();
        let (th, it) = prob.solve(&theta, solver, iters);
        let solve_ms = t0.elapsed().as_secs_f64() * 1e3;
        theta = th;
        total_iters += it;
        let r = prob.result(&theta, total_iters);
        let t1 = std::time::Instant::now();
        let verdict = oracle(&r.p_pts, &r.n_pts);
        log::debug!(
            target: "descent_stats",
            "round={round} vars={} claims={} iters={it} solve_ms={solve_ms:.1} oracle_ms={:.1} clean={} viol_cuts={}",
            prob.theta0.len(),
            prob.claims.claims.len(),
            t1.elapsed().as_secs_f64() * 1e3,
            verdict.clean,
            verdict.cuts.len()
        );
        if verdict.clean {
            return Some(finish(r, round, cuts, &verdict));
        }
        let mut added: usize = verdict.cuts.iter().map(|ob| prob.add_cut(ob, &theta)).sum();
        if let Some(req) = twin_required {
            added += verdict
                .twin_cuts
                .iter()
                .map(|&(i, j)| prob.add_twin(i, j, req))
                .sum::<usize>();
        }
        cuts += added;
        if added == 0 {
            // Why nothing new: classify the twin hits the oracle reported
            // (all already claimed by construction). `crossing` = the legs
            // properly intersect; `pinned` = a breakout segment touching a
            // pinned endpoint; `claim_holds` = the claim's own predicate
            // passes although the oracle fails (vocabulary gap).
            let (mut crossing, mut pinned, mut holds) = (0usize, 0usize, 0usize);
            let (pl, nl) = (r.p_pts.len() - 1, r.n_pts.len() - 1);
            for &(i, j) in &verdict.twin_cuts {
                if i + 1 > pl || j + 1 > nl {
                    continue;
                }
                let (a0, a1, b0, b1) = (r.p_pts[i], r.p_pts[i + 1], r.n_pts[j], r.n_pts[j + 1]);
                if seg_cross_f64(a0, a1, b0, b1) {
                    crossing += 1;
                }
                if i == 0 || i + 1 == pl || j == 0 || j + 1 == nl {
                    pinned += 1;
                }
                if let Some(req) = twin_required {
                    if seg_seg_dist_f64(a0, a1, b0, b1) >= req - 0.01 {
                        holds += 1;
                    }
                }
            }
            log::debug!(
                target: "descent_stats",
                "no_new_cuts twin_hits={} crossing={crossing} pinned={pinned} claim_holds={holds} third_cuts={}",
                verdict.twin_cuts.len(),
                verdict.cuts.len()
            );
        }
        if added == 0 && !restarted && round < max_rounds.max(1) {
            // Nothing new to claim, yet the oracle is dirty: the claims
            // exist but the solve ended violating them — typically legs
            // that crossed before their twin claim existed, a basin the
            // signed distance can't climb out of locally. Re-solve ONCE
            // from the original (uncrossed) geometry with every claim
            // mined so far.
            restarted = true;
            theta = prob.theta0.clone();
            log::debug!(target: "descent_stats", "restart=cold round={round}");
            continue;
        }
        if added == 0 || round == max_rounds.max(1) {
            return Some(finish(r, round, cuts, &verdict));
        }
    }
    None
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
    /// Obstacles the claim set knows, and which (sample, obstacle) pairs
    /// already carry a clearance claim — so mined cuts never duplicate.
    known: Vec<DescentObstacle>,
    claimed: std::collections::BTreeSet<(Sample, usize)>,
    /// (P segment, N segment) pairs already carrying a twin-gap claim.
    twin_claimed: std::collections::BTreeSet<(usize, usize)>,
}

/// Twin-gap seeding broadphase: P/N segment pairs closer than
/// `required + TWIN_SEED_BAND` at the start get a claim up front.
const TWIN_SEED_BAND: f64 = 0.5;

/// A clearance site: a moving point, or the whole segment starting at that
/// index (leg 0 = P, 1 = N).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sample {
    Point(u8, usize),
    /// The whole segment `i → i+1` (mined cuts).
    Seg(u8, usize),
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
            known: Vec::new(),
            claimed: Default::default(),
            twin_claimed: Default::default(),
        };
        // Seed claims are WHOLE-segment (M9), like mined cuts: the oracle
        // judges segments, and a solver riding the boundary exactly puts the
        // unsampled interior between two point claims through the obstacle.
        for ob in obstacles {
            prob.add_seed_clearance(ob);
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
    #[allow(dead_code)]
    pub(crate) fn add_point_clearance(
        &mut self,
        ob: &DescentObstacle,
        at: Option<&[f64]>,
    ) -> usize {
        self.add_clearance(ob, at, false, ob.required + 2.0)
    }

    /// Seed claims: whole-segment clearance at the starting geometry, with
    /// a tighter broadphase (`required + 1 mm`) — a segment that far away
    /// needs several trust-region steps to reach the obstacle, and the
    /// oracle mines it if it ever does.
    pub(crate) fn add_seed_clearance(&mut self, ob: &DescentObstacle) -> usize {
        self.add_clearance(ob, None, true, ob.required + 1.0)
    }

    /// A mined cut: WHOLE-segment clearance claims ([`cir::seg_clearance`])
    /// for every segment near `ob` at the current θ — the oracle judges whole
    /// segments, and a segment can cross an obstacle between two legal
    /// points.
    pub(crate) fn add_cut(&mut self, ob: &DescentObstacle, at: &[f64]) -> usize {
        self.add_clearance(ob, Some(at), true, ob.required + 2.0)
    }

    fn add_clearance(
        &mut self,
        ob: &DescentObstacle,
        at: Option<&[f64]>,
        mids: bool,
        reach: f64,
    ) -> usize {
        let oi =
            match self.known.iter().position(|k| {
                k.a == ob.a && k.b == ob.b && (k.required - ob.required).abs() < 1e-12
            }) {
                Some(i) => i,
                None => {
                    self.known.push(*ob);
                    self.known.len() - 1
                }
            };
        let (pp, nn) = match at {
            Some(th) => self.points(th),
            None => (self.p_pts.clone(), self.n_pts.clone()),
        };
        let mut added = 0;
        for (li, leg, pts) in [(0u8, self.pe.clone(), pp), (1u8, self.ne.clone(), nn)] {
            let last = pts.len() - 1;
            if mids {
                // Mined cut: claim WHOLE segments. Point/midpoint samples
                // leave the stretches between them unconstrained, and a
                // solver that rides the constraint boundary exactly (the M7
                // SQP) puts segment interiors through the obstacle there —
                // the oracle judges segments, so the cut does too.
                for i in 0..last {
                    let (a, b) = (pts[i], pts[i + 1]);
                    if seg_seg_dist_f64(a, b, ob.a, ob.b) > reach
                        || !self.claimed.insert((Sample::Seg(li, i), oi))
                    {
                        continue;
                    }
                    // A pinned endpoint can't move: never demand more than
                    // it already has (the oracle, not the cushion, rules).
                    let mut req = ob.required;
                    for (k, p) in [(i, a), (i + 1, b)] {
                        if k == 0 || k == last {
                            req = req.min(pt_seg_dist_f64(p, ob.a, ob.b) - 1e-6);
                        }
                    }
                    let c = cir::seg_clearance(
                        &mut self.g,
                        (leg[i], leg[i + 1]),
                        (ob.a.x, ob.a.y),
                        (ob.b.x, ob.b.y),
                        req,
                        self.clearance_weight,
                    );
                    self.claims.push(c);
                    added += 1;
                }
                continue;
            }
            for i in 0..=last {
                let moving = i != 0 && i != last;
                let mut sites: Vec<(Sample, PointExpr, Vec2)> = Vec::new();
                if moving {
                    sites.push((Sample::Point(li, i), leg[i], pts[i]));
                }
                for (key, pe, now) in sites {
                    if pt_seg_dist_f64(now, ob.a, ob.b) > reach || !self.claimed.insert((key, oi)) {
                        continue;
                    }
                    let c = cir::clearance(
                        &mut self.g,
                        pe,
                        (ob.a.x, ob.a.y),
                        (ob.b.x, ob.b.y),
                        ob.required,
                        self.clearance_weight,
                    );
                    self.claims.push(c);
                    added += 1;
                }
            }
        }
        added
    }

    /// Twin-gap claim between P segment `i` and N segment `j` (centre-line
    /// separation `required`). Returns 1 if added, 0 if already present.
    pub(crate) fn add_twin(&mut self, i: usize, j: usize, required: f64) -> usize {
        if i + 1 >= self.pe.len() || j + 1 >= self.ne.len() || !self.twin_claimed.insert((i, j)) {
            return 0;
        }
        let c = cir::twin_gap(
            &mut self.g,
            (self.pe[i], self.pe[i + 1]),
            (self.ne[j], self.ne[j + 1]),
            required,
            self.clearance_weight,
        );
        self.claims.push(c);
        1
    }

    /// Seed twin-gap claims for every P/N segment pair that starts within
    /// `required + TWIN_SEED_BAND` of each other but still holds — the ones a descent
    /// step can push through the gap. Pairs already closer than `required`
    /// at the start (breakouts converging on their pads, where the oracle
    /// relaxes the rule) are left to mining: forcing them apart against
    /// pinned endpoints would make the problem infeasible.
    pub(crate) fn seed_twin(&mut self, required: f64) -> usize {
        let mut pairs = Vec::new();
        for i in 0..self.p_pts.len() - 1 {
            for j in 0..self.n_pts.len() - 1 {
                let d = seg_seg_dist_f64(
                    self.p_pts[i],
                    self.p_pts[i + 1],
                    self.n_pts[j],
                    self.n_pts[j + 1],
                );
                if d >= required && d < required + TWIN_SEED_BAND {
                    pairs.push((i, j));
                }
            }
        }
        pairs
            .into_iter()
            .map(|(i, j)| self.add_twin(i, j, required))
            .sum()
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
            DescentSolver::Sco | DescentSolver::ScoLm => {
                let method = if solver == DescentSolver::ScoLm {
                    ScoMethod::Lm
                } else {
                    ScoMethod::Sqp
                };
                let (th, rep) = self.solve_sco(theta0, iters, method);
                log::debug!(
                    target: "descent_stats",
                    "sco method={method:?} iters={} cap_hit={} hard_ok={} mu={:.0}",
                    rep.iters,
                    rep.cap_hit,
                    rep.hard_ok,
                    rep.mu
                );
                (th, rep.iters)
            }
        }
    }

    /// SCO on the claim-compiled residual system.
    pub(crate) fn solve_sco(
        &self,
        theta0: &[f64],
        iters: usize,
        method: ScoMethod,
    ) -> (Vec<f64>, ScoReport) {
        let mut g = self.g.clone();
        let sys = ResidualSystem::compile_with(&mut g, &self.claims, theta0.len(), method);
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
    /// Clearance claims mined from oracle violations (M3 lazy cuts).
    pub cuts: usize,
    /// Attempted pairs whose UNMODIFIED copper already fails the oracle.
    pub pre_dirty: usize,
    /// Rejected pairs whose result was strictly less dirty than the original
    /// against the same oracle, adding no violation (not committed).
    pub improved_but_dirty: usize,
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
    // The pass classifies nets and installs the diff-pair class so the oracle
    // session enforces the pair gap — but that is working state, not an
    // edit: leaving it on the board changes DRC for pairs the descent never
    // touched (an imported board whose USB legs are narrower than the 0.2 mm
    // class default gained MinTraceWidth violations from a no-op run).
    let rules = pcb.rules.clone();
    let report = descend_board_inner(pcb, iters, solver);
    pcb.rules = rules;
    report
}

fn descend_board_inner(pcb: &mut Pcb, iters: usize, solver: DescentSolver) -> DescentReport {
    use super::classes::{apply_classes, classify_nets};
    use super::length_match::longest_chain;
    use crate::session::RouteSession;
    use crate::spatial::CopperGeom;

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
            // The run is rewritten at ONE width: take the widest segment ON
            // the run (the coupled leg). Any trace of the net on the layer —
            // the old pick — could be a breakout neck, and rewriting the
            // whole run at neck width is a MinTraceWidth violation the
            // oracle (which probes at the same width) never sees.
            let width = pcb
                .traces
                .iter()
                .filter(|t| on_run(t, net, layer, &pts))
                .map(|t| t.width)
                .fold(None, |m: Option<f64>, w| Some(m.map_or(w, |m| m.max(w))))
                .unwrap_or(0.08);
            (layer, pts, total - len, width)
        })
    };

    let mut report = DescentReport::default();
    for (pn, nn) in &c.pairs {
        let (Some((pl, p_raw, extra_p, p_w)), Some((nlayer, mut n_raw, extra_n, n_w))) =
            (net_run(pcb, pn), net_run(pcb, nn))
        else {
            log::debug!(target: "descent_stats", "pair={pn} skip=no-run");
            continue;
        };
        // Walk both legs the same way: the gap springs pair samples by
        // index, so an N run walked end-to-start couples P's head to N's
        // tail and drags the legs across each other.
        {
            let (p0, p1) = (p_raw[0], p_raw[p_raw.len() - 1]);
            let (n0, n1) = (n_raw[0], n_raw[n_raw.len() - 1]);
            if (p0 - n1).length() + (p1 - n0).length() < (p0 - n0).length() + (p1 - n1).length() {
                n_raw.reverse();
            }
        }
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
            log::debug!(
                target: "descent_stats",
                "pair={pn} skip={} skew={:.3} layers={:?}/{:?}",
                if (plen - nlen).abs() < 0.2 { "matched" } else { "no-freedom" },
                (plen - nlen).abs(),
                pl,
                nlayer
            );
            continue; // already matched, or no interior freedom
        }
        report.attempted += 1;
        let pair_t0 = std::time::Instant::now();
        log::debug!(
            target: "descent_stats",
            "pair={pn} attempt skew={:.3} pts={}/{} same_layer={} extra={:.2}/{:.2}",
            (plen - nlen).abs(),
            p_pts.len(),
            n_pts.len(),
            pl == nlayer,
            extra_p,
            extra_n
        );

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
        // +0.05mm safety: the hinge is a soft penalty traded against skew,
        // so its equilibrium must land OUTSIDE the hard rule the oracle
        // enforces.
        let to_obstacle =
            |geom: &CopperGeom, emin: [f64; 2], emax: [f64; 2], req: f64| -> DescentObstacle {
                let required = req + run_w / 2.0 + 0.05;
                match geom {
                    CopperGeom::Segment { a, b, half_w } => DescentObstacle {
                        a: *a,
                        b: *b,
                        required: required + half_w,
                    },
                    CopperGeom::Disc { center, r } => DescentObstacle {
                        a: *center,
                        b: *center,
                        required: required + r,
                    },
                    _ => {
                        let c = Vec2::new((emin[0] + emax[0]) / 2.0, (emin[1] + emax[1]) / 2.0);
                        let hd = ((emax[0] - emin[0]).powi(2) + (emax[1] - emin[1]).powi(2)).sqrt()
                            / 2.0;
                        DescentObstacle {
                            a: c,
                            b: c,
                            required: required + hd,
                        }
                    }
                }
            };
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
                |geom, emin, emax, req| obstacles.push(to_obstacle(geom, emin, emax, req)),
            );
        }

        // The exact oracle, as a cut generator (M3). Every optimized segment
        // must pass `RouteSession::probe`. Each leg is checked against a
        // board with only ITS OWN old copper removed, so the twin stays an
        // obstacle — the session applies the declared pair gap to it, so a
        // correctly coupled leg passes while one that has drifted into its
        // partner does not. (Dropping both nets would let the descent short
        // the pair to itself unnoticed: the gap spring is a soft term, not a
        // guarantee.) Both legs move together, so the twin must be present
        // at its NEW position: checking P-new against N-old rejects every
        // descent, because the pair as a whole has shifted.
        let with_new = |moved: &[Vec2], moved_net: &str, layer: PcbLayer, w: f64| {
            RouteSession::from_pcb(&oracle_board(
                pcb,
                [(pn.as_str(), pl, &p_raw), (nn.as_str(), nlayer, &n_raw)],
                moved,
                moved_net,
                layer,
                w,
            ))
        };
        // Each violating segment mines the third-party copper around it as
        // new clearance claims. A violation against the twin (the pair gap
        // rule) mines nothing — descend_pair_mined then stops, not clean.
        // The twin's copper OFF its run, per leg: a twin blocker that isn't
        // the moving run is static copper, mined as an ordinary obstacle at
        // the pair-gap requirement.
        let twin_static = |twin: &str, layer: PcbLayer, run: &[Vec2]| -> Vec<(Vec2, Vec2, f64)> {
            pcb.traces
                .iter()
                .filter(|t| t.net == twin && t.layer == layer && !on_run(t, twin, layer, run))
                .map(|t| (t.start, t.end, t.width / 2.0))
                .collect()
        };
        let p_twin_static = twin_static(nn, pl, &n_raw);
        let n_twin_static = twin_static(pn, nlayer, &p_raw);
        let twin_edge = clearance.max(gap_edge);
        let oracle = |pp: &[Vec2], np: &[Vec2]| -> OracleVerdict {
            let p_session = with_new(np, nn, nlayer, n_w);
            let n_session = with_new(pp, pn, pl, p_w);
            let mut bad = Vec::new();
            let mut twin_cuts: std::collections::BTreeSet<(usize, usize)> = Default::default();
            let mut dirty_list = Vec::new();
            let mut dirty = false;
            let (mut dirty_segs, mut twin_only) = (0usize, 0usize);
            let mut blk: std::collections::BTreeMap<&str, usize> = Default::default();
            for (leg, sess, pts, other, net, layer, w, statics) in [
                (
                    0u8,
                    &p_session,
                    pp,
                    np,
                    pn.as_str(),
                    pl,
                    p_w,
                    &p_twin_static,
                ),
                (
                    1u8,
                    &n_session,
                    np,
                    pp,
                    nn.as_str(),
                    nlayer,
                    n_w,
                    &n_twin_static,
                ),
            ] {
                for (si, seg) in pts.windows(2).enumerate() {
                    let geom = CopperGeom::Segment {
                        a: seg[0],
                        b: seg[1],
                        half_w: w / 2.0,
                    };
                    let pr = sess.probe(&geom, layer, net, clearance);
                    if pr.legal {
                        continue;
                    }
                    let mut twin_hit = false;
                    for bl in &pr.blockers {
                        let kind = if bl.net == *pn || bl.net == *nn {
                            twin_hit = true;
                            "twin"
                        } else {
                            "third"
                        };
                        *blk.entry(kind).or_insert(0usize) += 1;
                    }
                    dirty = true;
                    dirty_segs += 1;
                    dirty_list.push((leg, si));
                    if twin_hit {
                        // Which twin copper: the moving run (same layer
                        // only) → twin-gap claims; otherwise static copper.
                        let other_w = if leg == 0 { n_w } else { p_w };
                        let need = twin_edge - 0.005 + (w + other_w) / 2.0;
                        let mut found = false;
                        if pl == nlayer {
                            for (oj, os) in other.windows(2).enumerate() {
                                if seg_seg_dist_f64(seg[0], seg[1], os[0], os[1]) < need {
                                    found = true;
                                    twin_cuts.insert(if leg == 0 { (si, oj) } else { (oj, si) });
                                }
                            }
                        }
                        if !found {
                            for &(a, b, hw) in statics.iter() {
                                if seg_seg_dist_f64(seg[0], seg[1], a, b)
                                    < need - other_w / 2.0 + hw
                                {
                                    bad.push(DescentObstacle {
                                        a,
                                        b,
                                        required: twin_edge + run_w / 2.0 + hw + 0.05,
                                    });
                                }
                            }
                        }
                    }
                    let before_n = bad.len();
                    let m = 1.0;
                    ob_session.for_each_blocking(
                        layer,
                        "",
                        [seg[0].x.min(seg[1].x) - m, seg[0].y.min(seg[1].y) - m],
                        [seg[0].x.max(seg[1].x) + m, seg[0].y.max(seg[1].y) + m],
                        |g, emin, emax, req| {
                            if g.distance_to(&geom) < req + 0.05 {
                                bad.push(to_obstacle(g, emin, emax, req));
                            }
                        },
                    );
                    if bad.len() == before_n {
                        twin_only += 1;
                    }
                }
            }
            log::debug!(
                target: "descent_stats",
                "oracle dirty_segs={dirty_segs} no_third_party={twin_only} twin_cuts={} blockers={blk:?}",
                twin_cuts.len()
            );
            OracleVerdict {
                clean: !dirty,
                cuts: bad,
                twin_cuts: twin_cuts.into_iter().collect(),
                dirty: dirty_list,
            }
        };
        // Centre-line separation the twin-gap claims demand: the oracle's
        // edge rule plus both half-widths plus a 10 um cushion (the claims
        // are hard, so the cushion only has to clear the predicate).
        let twin_required = (pl == nlayer).then_some(twin_edge + (p_w + n_w) / 2.0 + 0.01);

        let Some(mined) = descend_pair_mined(
            &p_pts,
            &n_pts,
            extra_p,
            extra_n,
            pl == nlayer,
            gap_centre,
            twin_required,
            &obstacles,
            &DescentWeights::default(),
            solver,
            iters,
            4,
            &oracle,
        ) else {
            log::debug!(target: "descent_stats", "pair={pn} reject=build-failed");
            continue;
        };
        report.cuts += mined.cuts;
        if mined.baseline_dirty > 0 {
            report.pre_dirty += 1;
        }
        if !mined.clean {
            report.rejected += 1;
            if mined.improved {
                report.improved_but_dirty += 1;
            }
            log::debug!(
                target: "descent_stats",
                "pair={pn} reject=oracle-dirty rounds={} cuts={} ms={:.0} reason={} pre_dirty={} base_dirty={} final_dirty={} improved_but_dirty={}",
                mined.rounds,
                mined.cuts,
                pair_t0.elapsed().as_secs_f64() * 1e3,
                if mined.rounds >= 4 { "round-budget" } else { "no-new-cuts" },
                mined.baseline_dirty > 0,
                mined.baseline_dirty,
                mined.final_dirty,
                mined.improved
            );
            continue;
        }
        let r = mined.result;
        // Whole-net accounting: r.skew is run-only; judge the committed
        // outcome on the full nets and accept only real improvements.
        let before = (plen - nlen).abs();
        let run_p_after: f64 = r.p_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let run_n_after: f64 = r.n_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let after = ((run_p_after + extra_p) - (run_n_after + extra_n)).abs();
        if after >= before {
            report.rejected += 1;
            log::debug!(
                target: "descent_stats",
                "pair={pn} reject=no-improvement before={before:.3} after={after:.3} rounds={} cuts={} ms={:.0}",
                mined.rounds,
                mined.cuts,
                pair_t0.elapsed().as_secs_f64() * 1e3
            );
            continue;
        }
        let mut work2 = pcb.clone();
        let replace_run = |work2: &mut Pcb,
                           net: &str,
                           layer: PcbLayer,
                           old: &[Vec2],
                           new: &[Vec2],
                           width: f64| {
            work2.traces.retain(|t| !on_run(t, net, layer, old));
            work2.traces.extend(new.windows(2).map(|w| Trace {
                start: w[0],
                end: w[1],
                width,
                layer,
                net: net.to_string(),
                source: None,
            }));
        };
        // M11: the descent runs on a 0.4 mm densified chain; committing it
        // verbatim multiplies the segment count (and every per-segment DRC
        // finding). Merge near-collinear stretches back first. A merged
        // chord shifts copper by up to the tolerance, so each candidate is
        // re-judged by the same oracle: 10 um, then 1 um, else the
        // unmerged (already oracle-clean) chain.
        let (cp, cn, merge_tol) = [1e-2, 1e-3]
            .into_iter()
            .map(|tol| {
                (
                    merge_collinear(&r.p_pts, tol),
                    merge_collinear(&r.n_pts, tol),
                    tol,
                )
            })
            .find(|(cp, cn, _)| oracle(cp, cn).clean)
            .unwrap_or_else(|| (r.p_pts.clone(), r.n_pts.clone(), 0.0));
        log::debug!(
            target: "descent_stats",
            "pair={pn} merge tol={merge_tol} segs={}/{} -> {}/{}",
            r.p_pts.len() - 1,
            r.n_pts.len() - 1,
            cp.len() - 1,
            cn.len() - 1
        );
        replace_run(&mut work2, pn, pl, &p_raw, &cp, p_w);
        replace_run(&mut work2, nn, nlayer, &n_raw, &cn, n_w);
        *pcb = work2;
        report.tuned += 1;
        log::info!("descent: {pn} whole-net skew {before:.3} -> {after:.3} mm");
        log::debug!(
            target: "descent_stats",
            "pair={pn} accept before={before:.3} after={after:.3} rounds={} cuts={} ms={:.0}",
            mined.rounds,
            mined.cuts,
            pair_t0.elapsed().as_secs_f64() * 1e3
        );
    }
    report
}

/// Whether `t` is one of the segments of `run` (a chain on `net`/`layer`).
fn on_run(t: &Trace, net: &str, layer: PcbLayer, run: &[Vec2]) -> bool {
    t.net == net
        && t.layer == layer
        && run.windows(2).any(|w| {
            (t.start - w[0]).length() < 1e-6 && (t.end - w[1]).length() < 1e-6
                || (t.start - w[1]).length() < 1e-6 && (t.end - w[0]).length() < 1e-6
        })
}

/// The board the oracle probes one leg against: both legs' OLD runs
/// removed, `moved` (the twin's new run) added, and every other trace —
/// including the twin's copper OFF its run — kept. Removing the twin's whole
/// net let a run on one layer descend straight through the twin's copper on
/// that layer (its run lived on another layer): the oracle passed, the DRC
/// then reported a pair shorted to itself.
fn oracle_board(
    pcb: &Pcb,
    old_runs: [(&str, PcbLayer, &[Vec2]); 2],
    moved: &[Vec2],
    moved_net: &str,
    layer: PcbLayer,
    w: f64,
) -> Pcb {
    let mut b = pcb.clone();
    b.traces
        .retain(|t| !old_runs.iter().any(|(n, l, run)| on_run(t, n, *l, run)));
    b.traces.extend(moved.windows(2).map(|s| Trace {
        start: s[0],
        end: s[1],
        width: w,
        layer,
        net: moved_net.to_string(),
        source: None,
    }));
    b
}

/// Drop interior points of `pts` that lie within `tol` of the chord
/// between the last kept point and the next candidate, so collinear
/// stretches (the densified samples a descent left straight) collapse to
/// one segment. Greedy: the chord is extended while every skipped point
/// stays within `tol`; endpoints always survive.
fn merge_collinear(pts: &[Vec2], tol: f64) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut anchor = 0usize;
    let mut k = 2usize;
    while k < pts.len() {
        let fits = (anchor + 1..k).all(|m| pt_seg_dist_f64(pts[m], pts[anchor], pts[k]) <= tol);
        if !fits {
            anchor = k - 1;
            out.push(pts[anchor]);
        }
        k += 1;
    }
    out.push(pts[pts.len() - 1]);
    out
}

/// The segments properly cross (endpoints strictly on opposite sides).
fn seg_cross_f64(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    cross(a0, a1, b0) * cross(a0, a1, b1) < 0.0 && cross(b0, b1, a0) * cross(b0, b1, a1) < 0.0
}

/// Segment-segment distance (0 when they cross).
fn seg_seg_dist_f64(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> f64 {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    let (d1, d2) = (cross(a0, a1, b0), cross(a0, a1, b1));
    let (d3, d4) = (cross(b0, b1, a0), cross(b0, b1, a1));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    pt_seg_dist_f64(a0, b0, b1)
        .min(pt_seg_dist_f64(a1, b0, b1))
        .min(pt_seg_dist_f64(b0, a0, a1))
        .min(pt_seg_dist_f64(b1, a0, a1))
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

    /// M11: collinear runs collapse, bends survive, nothing moves > tol.
    #[test]
    fn merge_collinear_keeps_bends_and_tolerance() {
        let mut pts: Vec<Vec2> = (0..=10).map(|i| Vec2::new(0.4 * i as f64, 0.0)).collect();
        pts.extend((1..=5).map(|i| Vec2::new(4.0 + 0.4 * i as f64, 0.4 * i as f64)));
        // A 0.5 um wiggle on the first leg merges; a 5 um one would not.
        pts[3].y = 0.0005;
        let m = merge_collinear(&pts, 1e-3);
        assert_eq!(m.len(), 3, "{m:?}");
        assert!((m[1] - Vec2::new(4.0, 0.0)).length() < 1e-12);
        for p in &pts {
            let d = m
                .windows(2)
                .map(|w| pt_seg_dist_f64(*p, w[0], w[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(d <= 1e-3 + 1e-12);
        }
        pts[3].y = 0.005;
        assert!(merge_collinear(&pts, 1e-3).len() > 3);
    }

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
        // Seed clearance is whole-segment (M9): every segment whose START
        // geometry is within required + 1 of the obstacle, pinned ends
        // relaxed to what they already have.
        for (leg, start) in [(&pp, &p), (&nn, &n)] {
            let last = leg.len() - 1;
            for i in 0..last {
                let (s0, s1) = (start[i], start[i + 1]);
                if seg_seg_dist_f64(s0, s1, obs[0].a, obs[0].b) > obs[0].required + 1.0 {
                    continue;
                }
                let mut req = obs[0].required;
                for k in [i, i + 1] {
                    if k == 0 || k == last {
                        req = req.min(pt_seg_dist_f64(start[k], obs[0].a, obs[0].b) - 1e-6);
                    }
                }
                let short = req - seg_seg_dist_f64(leg[i], leg[i + 1], obs[0].a, obs[0].b);
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
            let (tl, il) = prob.solve(&prob.theta0, DescentSolver::ScoLm, 3000);
            let lm_ms = t.elapsed().as_secs_f64() * 1e3;
            let rl = prob.result(&tl, il);
            eprintln!(
                "{name}: sco-lm loss={:.5} skew={:.4} margin={:+.4} iters={} {:.1}ms",
                rl.loss,
                rl.skew,
                min_margin(&rl, &obs),
                rl.iters,
                lm_ms
            );
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

    /// M3: lazy constraint generation. The seed obstacle set deliberately
    /// omits one obstacle that the straightened N leg would cut through. One
    /// solve against the incomplete set is oracle-dirty (fail-closed: not
    /// clean); with mining, the violation becomes clearance cuts, the warm
    /// re-solve routes around it, and the result is oracle-clean.
    #[test]
    fn mined_cuts_make_incomplete_obstacle_set_oracle_clean() {
        use crate::spatial::CopperGeom;
        let (p, n, seed) = long_fixture();
        // Hidden: a short trace just above where N straightens to (y=0.45),
        // clear of N's starting bump (y≈1.05 there). Claims carry a 0.05
        // cushion over the oracle's rule, as descend_board's do.
        let hidden = DescentObstacle {
            a: Vec2::new(13.0, 0.62),
            b: Vec2::new(13.6, 0.62),
            required: 0.3,
        };
        let all: Vec<DescentObstacle> = seed.iter().copied().chain([hidden]).collect();
        let oracle = |pp: &[Vec2], np: &[Vec2]| -> OracleVerdict {
            let mut v = OracleVerdict {
                clean: true,
                ..Default::default()
            };
            for (leg, pts) in [pp, np].into_iter().enumerate() {
                for (si, seg) in pts.windows(2).enumerate() {
                    let s = CopperGeom::Segment {
                        a: seg[0],
                        b: seg[1],
                        half_w: 0.0,
                    };
                    for o in &all {
                        let og = CopperGeom::Segment {
                            a: o.a,
                            b: o.b,
                            half_w: 0.0,
                        };
                        if s.distance_to(&og) < o.required - 0.05 {
                            v.clean = false;
                            v.dirty.push((leg as u8, si));
                            if !v.cuts.iter().any(|c| c.a == o.a && c.b == o.b) {
                                v.cuts.push(*o);
                            }
                        }
                    }
                }
            }
            v
        };
        // The starting geometry is legal against everything.
        assert!(oracle(&p, &n).clean, "fixture must start legal");
        let w = DescentWeights::default();
        let once = descend_pair_mined(
            &p,
            &n,
            0.0,
            0.0,
            true,
            0.45,
            None,
            &seed,
            &w,
            DescentSolver::Sco,
            400,
            1,
            oracle,
        )
        .unwrap();
        assert!(
            !once.clean,
            "the incomplete seed set must let descent cut the hidden obstacle"
        );
        let mined = descend_pair_mined(
            &p,
            &n,
            0.0,
            0.0,
            true,
            0.45,
            None,
            &seed,
            &w,
            DescentSolver::Sco,
            400,
            8,
            oracle,
        )
        .unwrap();
        eprintln!(
            "mined: rounds={} cuts={} clean={} skew={:.4} iters={}",
            mined.rounds, mined.cuts, mined.clean, mined.result.skew, mined.result.iters
        );
        assert!(mined.clean, "mining must reach an oracle-clean result");
        assert!(mined.rounds >= 2 && mined.cuts > 0);
        assert!(oracle(&mined.result.p_pts, &mined.result.n_pts).clean);
        assert_eq!(mined.result.n_pts[0], n[0]);
    }

    /// M6: a violation against the TWIN is expressible. The gap springs
    /// here target a centre separation (0.30) tighter than the oracle's
    /// twin rule (0.40), so descent pulls the legs into each other (N starts
    /// 0.75 off, outside the seeding band, so nothing is pre-claimed). Without
    /// twin claims the oracle verdict mines nothing and the result stays
    /// dirty; with them the twin hits become twin-gap claims and the
    /// re-solve is oracle-clean.
    #[test]
    fn twin_cuts_make_pair_gap_violation_expressible() {
        let p: Vec<Vec2> = (0..=50).map(|i| Vec2::new(0.4 * i as f64, 0.0)).collect();
        let n: Vec<Vec2> = (0..=50)
            .map(|i| {
                let x = 0.4 * i as f64;
                let bump = if (4.0..16.0).contains(&x) {
                    0.5 * (std::f64::consts::PI * (x - 4.0) / 12.0).sin()
                } else {
                    0.0
                };
                Vec2::new(x, 0.75 + bump)
            })
            .collect();
        let oracle = |pp: &[Vec2], np: &[Vec2]| -> OracleVerdict {
            let mut v = OracleVerdict {
                clean: true,
                ..Default::default()
            };
            for (i, a) in pp.windows(2).enumerate() {
                for (j, b) in np.windows(2).enumerate() {
                    if seg_seg_dist_f64(a[0], a[1], b[0], b[1]) < 0.40 {
                        v.clean = false;
                        v.dirty.push((0, i));
                        v.twin_cuts.push((i, j));
                    }
                }
            }
            v.dirty.sort();
            v.dirty.dedup();
            v
        };
        assert!(oracle(&p, &n).clean, "fixture must start legal");
        let w = DescentWeights::default();
        let run = |twin: Option<f64>| {
            descend_pair_mined(
                &p,
                &n,
                0.0,
                0.0,
                true,
                0.30,
                twin,
                &[],
                &w,
                DescentSolver::Sco,
                400,
                6,
                oracle,
            )
            .unwrap()
        };
        let without = run(None);
        assert!(
            !without.clean,
            "springs at 0.30 must breach a 0.40 twin rule"
        );
        let with = run(Some(0.41));
        eprintln!(
            "twin: rounds={} cuts={} clean={} skew={:.4}",
            with.rounds, with.cuts, with.clean, with.result.skew
        );
        assert!(
            with.clean,
            "twin-gap claims must make the pair oracle-clean"
        );
        assert!(with.result.skew < 0.6, "skew {}", with.result.skew);
    }

    /// M8: the board pass is bit-for-bit deterministic. Every hashed
    /// container on the path gets fresh random keys per instance, so
    /// repeating the pass on clones of one board exercises the orders that
    /// used to vary (chain walk direction, which flipped the gap-spring
    /// pairing).
    #[test]
    fn descend_board_is_deterministic() {
        use vcad_ir::ecad::*;
        let pad = |num: &str, x: f64, y: f64, net: &str| Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 0.2,
                height: 0.2,
            },
            position: Vec2::new(x, y),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        };
        let tr = |net: &str, a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.1,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        };
        let mut pcb: Pcb = serde_json::from_str(
            r#"{"outline":{"vertices":[{"x":0,"y":0},{"x":30,"y":0},{"x":30,"y":30},{"x":0,"y":30}],"cutouts":[],"thickness":1.6},
                "stackup":{"layers":[]},
                "rules":{"defaultRules":{"name":"Default","traceWidth":0.1,"clearance":0.1,
                         "viaDiameter":0.4,"viaDrill":0.2},
                         "edgeClearance":0.3,"holeToHole":0.25,"minAnnularRing":0.1,"minDrill":0.2}}"#,
        )
        .expect("minimal board");
        pcb.footprints = vec![Footprint {
            reference: "J1".into(),
            value: "usb".into(),
            footprint_name: "test".into(),
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            front: true,
            pads: vec![
                pad("1", 2.0, 10.0, "USB_DP"),
                pad("2", 2.0, 10.6, "USB_DM"),
                pad("3", 22.0, 10.0, "USB_DP"),
                pad("4", 22.0, 10.6, "USB_DM"),
            ],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }];
        pcb.traces = vec![
            tr("USB_DP", (2.0, 10.0), (12.0, 10.0)),
            tr("USB_DP", (12.0, 10.0), (22.0, 10.0)),
            tr("USB_DM", (2.0, 10.6), (7.0, 11.6)),
            tr("USB_DM", (7.0, 11.6), (12.0, 10.6)),
            tr("USB_DM", (12.0, 10.6), (17.0, 11.6)),
            tr("USB_DM", (17.0, 11.6), (22.0, 10.6)),
            tr("OTHER", (2.0, 13.0), (22.0, 13.0)),
        ];
        let run = || {
            let mut b = pcb.clone();
            let r = descend_board(&mut b, 400);
            (r, serde_json::to_string(&b.traces).unwrap())
        };
        let (r0, t0) = run();
        eprintln!("determinism fixture: {r0:?}");
        assert_eq!(r0.attempted, 1, "fixture pair must be attempted");
        for _ in 0..5 {
            let (r, t) = run();
            assert_eq!(r, r0);
            assert!(t == t0, "descent output differs between identical runs");
        }
    }

    /// A run that commits nothing must not change the board: classifying
    /// nets installs a diff-pair class (0.2 mm legs) that the pass needs
    /// internally, and leaking it onto an imported board turned its
    /// narrower pair traces into MinTraceWidth violations.
    #[test]
    fn descend_board_leaves_rules_untouched() {
        use vcad_ir::ecad::*;
        let pad = |num: &str, x: f64, net: &str| Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 0.5,
                height: 0.5,
            },
            position: Vec2::new(x, 0.0),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        };
        let mut pcb = Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(30.0, 0.0),
                    Vec2::new(30.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup { layers: vec![] },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.1,
                    clearance: 0.1,
                    via_diameter: 0.4,
                    via_drill: 0.2,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.3,
                hole_to_hole: 0.25,
                min_annular_ring: 0.1,
                min_drill: 0.2,
            },
            footprints: vec![Footprint {
                reference: "J1".into(),
                value: "usb".into(),
                footprint_name: "test".into(),
                position: Vec2::new(15.0, 15.0),
                rotation: 0.0,
                front: true,
                pads: vec![pad("1", -0.5, "USB_DP"), pad("2", 0.5, "USB_DM")],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            }],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        };
        let before = pcb.rules.clone();
        let r = descend_board(&mut pcb, 10);
        assert_eq!(r.tuned, 0);
        assert_eq!(
            pcb.rules, before,
            "descent leaked its diff-pair class onto the board"
        );
    }

    /// The oracle board must keep the twin's copper OFF its run: a pair
    /// whose legs' longest runs sit on different layers otherwise descends
    /// one leg straight through the twin's copper on its layer, and the
    /// committed board shorts the pair to itself.
    #[test]
    fn oracle_board_keeps_twin_copper_off_its_run() {
        let tr = |net: &str, layer: PcbLayer, a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.1,
            layer,
            net: net.into(),
            source: None,
        };
        let p_run = [
            Vec2::new(0.0, 0.0),
            Vec2::new(5.0, 0.0),
            Vec2::new(10.0, 0.0),
        ];
        let n_run = [Vec2::new(0.0, 1.0), Vec2::new(10.0, 1.0)];
        let mut pcb: Pcb = serde_json::from_str(
            r#"{"outline":{"vertices":[],"cutouts":[],"thickness":1.6},
                "stackup":{"layers":[]},
                "rules":{"defaultRules":{"name":"Default","traceWidth":0.1,"clearance":0.1,
                         "viaDiameter":0.4,"viaDrill":0.2},
                         "edgeClearance":0.3,"holeToHole":0.25,"minAnnularRing":0.1,"minDrill":0.2}}"#,
        )
        .expect("minimal board");
        pcb.traces = vec![
            tr("P", PcbLayer::FCu, (0.0, 0.0), (5.0, 0.0)),
            tr("P", PcbLayer::FCu, (5.0, 0.0), (10.0, 0.0)),
            tr("N", PcbLayer::BCu, (0.0, 1.0), (10.0, 1.0)),
            // The twin's breakout on P's layer — not part of its run.
            tr("N", PcbLayer::FCu, (5.0, 0.5), (5.0, 2.0)),
            tr("OTHER", PcbLayer::FCu, (0.0, 3.0), (10.0, 3.0)),
        ];
        let moved = [
            Vec2::new(0.0, 1.0),
            Vec2::new(5.0, 1.2),
            Vec2::new(10.0, 1.0),
        ];
        let b = oracle_board(
            &pcb,
            [("P", PcbLayer::FCu, &p_run), ("N", PcbLayer::BCu, &n_run)],
            &moved,
            "N",
            PcbLayer::BCu,
            0.1,
        );
        let has = |net: &str, layer: PcbLayer| {
            b.traces
                .iter()
                .filter(|t| t.net == net && t.layer == layer)
                .count()
        };
        assert_eq!(has("P", PcbLayer::FCu), 0, "P's old run must be gone");
        assert_eq!(
            has("N", PcbLayer::BCu),
            2,
            "N's old run replaced by the 2 moved segments"
        );
        assert_eq!(
            has("N", PcbLayer::FCu),
            1,
            "the twin's off-run copper must stay an obstacle"
        );
        assert_eq!(has("OTHER", PcbLayer::FCu), 1);
    }
}
