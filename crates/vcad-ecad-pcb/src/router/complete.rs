//! Complete multi-net window router — an exhaustive, certificate-producing
//! decision procedure for small joint routing problems.
//!
//! Given k connections (k ≤ ~6) inside a rectangular window on ≤ 4 copper
//! layers, this module either **jointly routes** all of them, **proves** that
//! no joint routing exists at the current rules and discretization, or
//! honestly reports that its budget ran out before deciding.
//!
//! # Canonical model
//!
//! The window is discretized to a coarse uniform grid with pitch
//! `width + clearance` (coarsened further if the window would exceed
//! [`MAX_AXIS_CELLS`] cells per axis). Routing happens on the finite graph
//! whose nodes are `(cell, layer)`:
//!
//! - a node is **free** iff a trace centre there clears all fixed copper
//!   (everything in the [`RouteSession`] that is not on one of the k nets),
//!   tested with the exact incremental oracle ([`RouteSession::probe`]) —
//!   the same clearance geometry the DRC pass uses;
//! - in-plane edges connect 4-adjacent cells on the same layer and are legal
//!   iff the swept capsule between the two cell centres probes clean;
//! - via edges connect the same cell on adjacent layers and are legal iff a
//!   via-sized disc probes clean on both layers;
//! - every node has **unit capacity**: at most one net's path may occupy a
//!   `(cell, layer)` node. At pitch = width + clearance, paths through
//!   distinct cells are clearance-compatible by construction, so node
//!   disjointness is the canonical inter-net legality rule.
//!
//! # Completeness argument
//!
//! Over this finite unit-capacity graph, a joint routing is a choice of one
//! path per net such that the paths are pairwise node-disjoint. Any joint
//! routing can be reduced to one using only **simple** paths (deleting a
//! cycle from a path never creates a conflict), so it suffices to enumerate
//! simple paths. The search routes the nets in a fixed order and, for net
//! `i`, performs a depth-first enumeration of *all* simple paths from its
//! source to its target through nodes not occupied by nets `< i`; on each
//! completed path it recurses to net `i+1`, and on failure it backtracks
//! through every predecessor choice of net `i` — i.e. the recursion tree
//! ranges over the entire (finite) set of joint simple-path assignments.
//! With a fixed net order this is already exhaustive: for any joint routing
//! `(p_1, …, p_k)` the DFS branch that picks exactly `p_1` for net 1, `p_2`
//! for net 2, … exists in the tree, so backtracking over net *order* is
//! unnecessary for completeness (it is purely a speed heuristic and is not
//! used here — determinism is worth more).
//!
//! States are deduplicated at net boundaries by `(net index, occupied-node
//! bitset)`: whether the remaining nets `i..k` can be routed depends only on
//! which nodes are occupied, not on which earlier net occupies them or via
//! which path, so a state that exhausted without a solution can be pruned
//! when reached again. Memoized failures are only recorded for subtrees that
//! ran to exhaustion (a budget trip aborts the whole search), so the memo
//! can never convert "unknown" into "infeasible".
//!
//! Hence, when the search returns without finding a routing **and** without
//! tripping the expansion budget, no node-disjoint joint routing exists on
//! the canonical grid — [`CompleteOutcome::ProvedInfeasible`]. When the
//! budget trips first, the result is [`CompleteOutcome::BudgetExhausted`]
//! ("unknown"), never a feigned proof.
//!
//! The proof is relative to the canonical discretization (on-grid paths,
//! vias at cell centres between adjacent layers, node-disjoint nets). A
//! finer, off-grid router could in principle still succeed; the certificate
//! states what was exhausted.
//!
//! # Infeasibility certificate
//!
//! Before the search, a max-flow necessary-condition check runs: k pairwise
//! node-disjoint paths require a flow of k through the free-node graph with
//! unit node capacities (Menger). BFS augmenting-path max flow (Edmonds–
//! Karp on the node-split graph) from a super-source (unit edge per net's
//! source terminal) to a super-sink (unit edge per target terminal) is
//! computed; if it is `f < k`, no assignment of *any* pairing can exist —
//! joint routing is infeasible outright, and the min-cut nodes (free cells
//! whose in-half is residual-reachable but whose out-half is not) name the
//! bottleneck: the certificate reports how many free channels the cut
//! offers and where they sit. This is a strictly weaker condition than the
//! search (it ignores which net must pair with which terminal), so it can
//! only fire on genuinely infeasible instances.

use std::collections::{HashMap, HashSet, VecDeque};

use vcad_ir::ecad::PcbLayer;
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::CopperGeom;

/// Hard cap on grid cells per axis; the pitch coarsens to fit. Keeps the
/// state space honest (≤ 48×48×4 ≈ 9.2k nodes).
const MAX_AXIS_CELLS: usize = 48;

/// Outcome of the complete window router.
#[derive(Debug, Clone)]
pub enum CompleteOutcome {
    /// A joint routing was found: per-connection segment lists, in the same
    /// order as the input `conns`. Layer transitions within a connection are
    /// vias at the shared segment endpoint (adjacent-layer span).
    Routed(Vec<Vec<(Vec2, Vec2, PcbLayer)>>),
    /// No joint routing exists at the current rules on the canonical grid —
    /// the search space was fully enumerated (or a max-flow cut proves it
    /// outright). `reason` is a human-readable certificate.
    ProvedInfeasible {
        /// Human-readable explanation of why the window is unroutable.
        reason: String,
    },
    /// The expansion budget tripped before the space was exhausted: the
    /// honest "unknown". Never claimed as infeasibility.
    BudgetExhausted,
}

/// Decide joint routability of `conns` inside `window`.
///
/// * `window` — `(lo, hi)` corners of the search rectangle (board mm). Must
///   lie inside the board outline; the router does not consult the outline.
/// * `layers` — copper stack to route on, front → back (≤ 4 recommended).
/// * `conns` — the k connections as `(net, from, to)`. Copper on any other
///   net in `session` is a fixed obstacle.
/// * `width` — trace width for all k connections (pitch = width + max
///   clearance across the k nets).
/// * `budget` — maximum DFS node expansions. A result of
///   [`CompleteOutcome::ProvedInfeasible`] is only ever returned when the
///   full space was enumerated within the budget (or the flow cut fired).
pub fn route_window_complete(
    session: &RouteSession,
    window: (Vec2, Vec2),
    layers: &[PcbLayer],
    conns: &[(String, Vec2, Vec2)],
    width: f64,
    budget: usize,
) -> CompleteOutcome {
    if conns.is_empty() {
        return CompleteOutcome::Routed(Vec::new());
    }
    if layers.is_empty() {
        return CompleteOutcome::ProvedInfeasible {
            reason: "no copper layers to route on".into(),
        };
    }

    let clearances: Vec<f64> = conns
        .iter()
        .map(|(n, _, _)| session.clearance_for(n))
        .collect();
    let max_clearance = clearances.iter().cloned().fold(0.0_f64, f64::max);
    let grid = WinGrid::new(window, width, max_clearance);
    let plane = grid.nx * grid.ny;
    let nl = layers.len();
    let n_nodes = plane * nl;
    let half_w = width / 2.0;
    // Via legality is probed as a disc of radius `width` (a conservative
    // stand-in for the via pad) on both spanned layers.
    let via_r = width;

    // --- Free-node raster (fixed copper only) ----------------------------
    // A node is free iff a zero-length capsule (trace centre) at the cell
    // clears all copper not on this net-set. Copper *on one of the k nets*
    // is not an obstacle to any of them here: the k nets are being decided
    // jointly, and same-net copper never blocks. To keep the model simple
    // and sound we probe against the union net exclusion: a cell is free
    // for the window iff it is legal for EVERY conn net (the most
    // conservative choice keeps node-disjointness sufficient).
    let mut free = vec![true; n_nodes];
    for (li, &layer) in layers.iter().enumerate() {
        for cell in 0..plane {
            let p = grid.world(cell);
            let ok = conns.iter().zip(&clearances).all(|((net, _, _), &clr)| {
                session
                    .probe(&CopperGeom::Segment { a: p, b: p, half_w }, layer, net, clr)
                    .legal
            });
            if !ok {
                free[li * plane + cell] = false;
            }
        }
    }

    // --- Terminals -------------------------------------------------------
    // Each terminal snaps to its nearest cell; the endpoint may attach on
    // any layer whose node is free (pad-side via permitted, as in the 3D
    // maze router). For the joint model we pick the first free layer for
    // determinism; the connector from the exact endpoint to the cell centre
    // must itself probe legal.
    let mut terms: Vec<(usize, usize)> = Vec::with_capacity(conns.len()); // (src node, dst node)
    for (ci, (net, from, to)) in conns.iter().enumerate() {
        let clr = clearances[ci];
        let attach = |p: Vec2| -> Option<usize> {
            let cell = grid.snap(p);
            let c = grid.world(cell);
            (0..nl).find_map(|li| {
                let node = li * plane + cell;
                let conn_ok = session
                    .probe(
                        &CopperGeom::Segment { a: p, b: c, half_w },
                        layers[li],
                        net,
                        clr,
                    )
                    .legal;
                (free[node] && conn_ok).then_some(node)
            })
        };
        match (attach(*from), attach(*to)) {
            (Some(s), Some(t)) => terms.push((s, t)),
            _ => {
                return CompleteOutcome::ProvedInfeasible {
                    reason: format!(
                        "terminal of net {net} at ({:.2}, {:.2})/({:.2}, {:.2}) has no \
                         clearance-legal grid attachment on any layer — the pad is walled \
                         in at the current rules and grid pitch {:.3} mm",
                        from.x, from.y, to.x, to.y, grid.pitch
                    ),
                }
            }
        }
    }
    // Two connections snapping onto the same node can never be node-disjoint.
    {
        let mut seen: HashSet<usize> = HashSet::new();
        for (ci, &(s, t)) in terms.iter().enumerate() {
            for node in [s, t] {
                if !seen.insert(node) {
                    return CompleteOutcome::ProvedInfeasible {
                        reason: format!(
                            "terminals of net {} collide with another connection's terminal \
                             in the same {:.3} mm grid cell — the window cannot host \
                             node-disjoint paths",
                            conns[ci].0, grid.pitch
                        ),
                    };
                }
            }
        }
    }

    // --- Edge legality (lazy, memoized) ----------------------------------
    let mut edge_memo: HashMap<u64, bool> = HashMap::new();
    let mut edge_ok = |a: usize, b: usize| -> bool {
        let key = ((a.min(b) as u64) << 32) | a.max(b) as u64;
        if let Some(&v) = edge_memo.get(&key) {
            return v;
        }
        let (la, ca) = (a / plane, a % plane);
        let (lb, cb) = (b / plane, b % plane);
        let ok = if la == lb {
            let pa = grid.world(ca);
            let pb = grid.world(cb);
            conns.iter().zip(&clearances).all(|((net, _, _), &clr)| {
                session
                    .probe(
                        &CopperGeom::Segment {
                            a: pa,
                            b: pb,
                            half_w,
                        },
                        layers[la],
                        net,
                        clr,
                    )
                    .legal
            })
        } else {
            // Via edge: disc on both spanned (adjacent) layers.
            debug_assert_eq!(ca, cb);
            let disc = CopperGeom::Disc {
                center: grid.world(ca),
                r: via_r,
            };
            conns.iter().zip(&clearances).all(|((net, _, _), &clr)| {
                session.probe(&disc, layers[la], net, clr).legal
                    && session.probe(&disc, layers[lb], net, clr).legal
            })
        };
        edge_memo.insert(key, ok);
        ok
    };

    // Neighbor generator: 4-adjacent in-plane + adjacent-layer via.
    let neighbors = |node: usize| -> Vec<usize> {
        let (li, cell) = (node / plane, node % plane);
        let (ix, iy) = (cell % grid.nx, cell / grid.nx);
        let mut out = Vec::with_capacity(6);
        if ix > 0 {
            out.push(li * plane + cell - 1);
        }
        if ix + 1 < grid.nx {
            out.push(li * plane + cell + 1);
        }
        if iy > 0 {
            out.push(li * plane + cell - grid.nx);
        }
        if iy + 1 < grid.ny {
            out.push(li * plane + cell + grid.nx);
        }
        if li > 0 {
            out.push((li - 1) * plane + cell);
        }
        if li + 1 < nl {
            out.push((li + 1) * plane + cell);
        }
        out
    };

    // --- Max-flow necessary-condition pre-pass ---------------------------
    let (flow, cut_cells) = max_node_disjoint_flow(&free, plane, grid.nx, nl, &terms, &neighbors);
    if flow < conns.len() {
        let names: Vec<&str> = conns.iter().map(|(n, _, _)| n.as_str()).collect();
        let mut reason = format!(
            "nets {} require {} node-disjoint paths, but the free-cell graph admits at \
             most {} — only {} free channel{} cross the bottleneck",
            names.join(", "),
            conns.len(),
            flow,
            flow,
            if flow == 1 { "" } else { "s" },
        );
        if !cut_cells.is_empty() {
            let (mut lo, mut hi) = (
                Vec2::new(f64::INFINITY, f64::INFINITY),
                Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
            );
            for &node in &cut_cells {
                let p = grid.world(node % plane);
                lo.x = lo.x.min(p.x);
                lo.y = lo.y.min(p.y);
                hi.x = hi.x.max(p.x);
                hi.y = hi.y.max(p.y);
            }
            reason.push_str(&format!(
                " (min cut of {} cell{} near x={:.1}..{:.1}, y={:.1}..{:.1})",
                cut_cells.len(),
                if cut_cells.len() == 1 { "" } else { "s" },
                lo.x - grid.pitch / 2.0,
                hi.x + grid.pitch / 2.0,
                lo.y - grid.pitch / 2.0,
                hi.y + grid.pitch / 2.0,
            ));
        }
        return CompleteOutcome::ProvedInfeasible { reason };
    }

    // --- Exhaustive backtracking DFS -------------------------------------
    let mut search = Search {
        free: &free,
        terms: &terms,
        plane,
        nx: grid.nx,
        occ: BitSet::new(n_nodes),
        paths: vec![Vec::new(); conns.len()],
        expansions: 0,
        budget,
        memo: HashSet::new(),
        edge_ok: &mut edge_ok,
        neighbors: &neighbors,
    };
    match search.solve(0) {
        Step::Found => {
            let paths = std::mem::take(&mut search.paths);
            let mut out = Vec::with_capacity(conns.len());
            for (ci, path) in paths.iter().enumerate() {
                out.push(emit_segments(
                    &grid,
                    plane,
                    layers,
                    conns[ci].1,
                    conns[ci].2,
                    path,
                ));
            }
            CompleteOutcome::Routed(out)
        }
        Step::Tripped => CompleteOutcome::BudgetExhausted,
        Step::Exhausted => {
            let names: Vec<&str> = conns.iter().map(|(n, _, _)| n.as_str()).collect();
            CompleteOutcome::ProvedInfeasible {
                reason: format!(
                    "exhaustive enumeration of all joint simple-path assignments for nets \
                     {} over the {}x{}x{} window grid (pitch {:.3} mm, {} expansions) found \
                     no node-disjoint routing — no joint routing exists at the current \
                     rules and discretization",
                    names.join(", "),
                    grid.nx,
                    grid.ny,
                    nl,
                    grid.pitch,
                    search.expansions,
                ),
            }
        }
    }
}

/// DFS step result. `Tripped` aborts the whole search (no memoization of the
/// interrupted subtree, so a trip can never masquerade as a proof).
#[derive(PartialEq)]
enum Step {
    Found,
    Exhausted,
    Tripped,
}

struct Search<'a> {
    free: &'a [bool],
    terms: &'a [(usize, usize)],
    plane: usize,
    nx: usize,
    /// Nodes occupied by committed paths of earlier nets AND the current
    /// net's partial path — together the full search state.
    occ: BitSet,
    paths: Vec<Vec<usize>>,
    expansions: usize,
    budget: usize,
    /// Failed states `(net index, occupied bitset)`, recorded only when the
    /// subtree exhausted.
    memo: HashSet<(usize, Box<[u64]>)>,
    edge_ok: &'a mut dyn FnMut(usize, usize) -> bool,
    neighbors: &'a dyn Fn(usize) -> Vec<usize>,
}

impl Search<'_> {
    /// Route nets `i..` given the occupancy of nets `< i`.
    fn solve(&mut self, i: usize) -> Step {
        if i == self.terms.len() {
            return Step::Found;
        }
        let key = (i, self.occ.words.clone().into_boxed_slice());
        if self.memo.contains(&key) {
            return Step::Exhausted;
        }
        let (s, t) = self.terms[i];
        if self.occ.get(s) || self.occ.get(t) || !self.free[s] || !self.free[t] {
            self.memo.insert(key);
            return Step::Exhausted;
        }
        self.occ.set(s);
        self.paths[i].push(s);
        let r = self.dfs_path(i, s, t);
        if r == Step::Found {
            // Keep the committed path/occupancy intact for reconstruction.
            return r;
        }
        self.paths[i].pop();
        self.occ.clear(s);
        if r == Step::Exhausted {
            self.memo.insert(key);
        }
        r
    }

    /// Enumerate all simple extensions of the current path of net `i` from
    /// `cur` to `t`, recursing into `solve(i + 1)` at every completion.
    fn dfs_path(&mut self, i: usize, cur: usize, t: usize) -> Step {
        self.expansions += 1;
        if self.expansions > self.budget {
            return Step::Tripped;
        }
        if cur == t {
            return self.solve(i + 1);
        }
        // Order neighbors by goal distance so the first completed path is
        // near-shortest (shortest-first flavor without a separate k-shortest
        // machinery; the enumeration is exhaustive either way).
        let (tx, ty, tl) = self.coords(t);
        let mut nbs = (self.neighbors)(cur);
        nbs.sort_by(|&a, &b| {
            let da = self.dist2(a, tx, ty, tl);
            let db = self.dist2(b, tx, ty, tl);
            da.cmp(&db)
        });
        for nb in nbs {
            if !self.free[nb] || self.occ.get(nb) || !(self.edge_ok)(cur, nb) {
                continue;
            }
            self.occ.set(nb);
            self.paths[i].push(nb);
            let r = self.dfs_path(i, nb, t);
            if r == Step::Found {
                return r;
            }
            self.paths[i].pop();
            self.occ.clear(nb);
            if r == Step::Tripped {
                return r;
            }
        }
        Step::Exhausted
    }

    fn coords(&self, node: usize) -> (i64, i64, i64) {
        let cell = node % self.plane;
        (
            (cell % self.nx) as i64,
            (cell / self.nx) as i64,
            (node / self.plane) as i64,
        )
    }

    fn dist2(&self, node: usize, tx: i64, ty: i64, tl: i64) -> i64 {
        let (x, y, l) = self.coords(node);
        (x - tx) * (x - tx) + (y - ty) * (y - ty) + (l - tl) * (l - tl)
    }
}

/// Convert a node path into world segments per layer, with the exact
/// endpoints attached and collinear runs collapsed.
fn emit_segments(
    grid: &WinGrid,
    plane: usize,
    layers: &[PcbLayer],
    from: Vec2,
    to: Vec2,
    path: &[usize],
) -> Vec<(Vec2, Vec2, PcbLayer)> {
    let mut segs: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();
    if path.is_empty() {
        return segs;
    }
    let mut run: Vec<Vec2> = vec![from];
    let mut run_layer = path[0] / plane;
    let flush = |run: &mut Vec<Vec2>, layer: PcbLayer, segs: &mut Vec<(Vec2, Vec2, PcbLayer)>| {
        let pts = simplify(run);
        segs.extend(
            pts.windows(2)
                .filter(|w| dist(w[0], w[1]) > 1e-9)
                .map(|w| (w[0], w[1], layer)),
        );
        run.clear();
    };
    for &node in path {
        let (li, cell) = (node / plane, node % plane);
        let w = grid.world(cell);
        if li != run_layer {
            // Via at the shared cell centre: end the old-layer run there and
            // begin the new-layer run there.
            if run.last().map(|p| dist(*p, w) > 1e-9).unwrap_or(true) {
                run.push(w);
            }
            flush(&mut run, layers[run_layer], &mut segs);
            run.push(w);
            run_layer = li;
        }
        if run.last().map(|p| dist(*p, w) > 1e-9).unwrap_or(true) {
            run.push(w);
        }
    }
    if run.last().map(|p| dist(*p, to) > 1e-9).unwrap_or(true) {
        run.push(to);
    }
    flush(&mut run, layers[run_layer], &mut segs);
    segs
}

/// Max number of pairwise node-disjoint source→target paths through the free
/// nodes (unit node capacities via node splitting), together with the cells
/// of the min vertex cut when the flow is short. Pairing-agnostic: any source
/// may serve any target, so the value is an upper bound on (i.e. necessary
/// for) the paired joint routing.
fn max_node_disjoint_flow(
    free: &[bool],
    plane: usize,
    _nx: usize,
    nl: usize,
    terms: &[(usize, usize)],
    neighbors: &dyn Fn(usize) -> Vec<usize>,
) -> (usize, Vec<usize>) {
    let n = plane * nl;
    // Node split: in(v) = 2v, out(v) = 2v+1; S = 2n, T = 2n+1.
    let s = 2 * n;
    let t = 2 * n + 1;
    let mut g = FlowGraph::new(2 * n + 2);
    for (v, &f) in free.iter().enumerate().take(n) {
        if f {
            g.add_edge(2 * v, 2 * v + 1, 1);
        }
    }
    for (v, &f) in free.iter().enumerate().take(n) {
        if !f {
            continue;
        }
        for nb in neighbors(v) {
            if free[nb] {
                g.add_edge(2 * v + 1, 2 * nb, 1);
            }
        }
    }
    for &(src, dst) in terms {
        g.add_edge(s, 2 * src, 1);
        g.add_edge(2 * dst + 1, t, 1);
    }
    let flow = g.max_flow(s, t);
    if flow >= terms.len() {
        return (flow, Vec::new());
    }
    // Min cut: free nodes whose in-half is residual-reachable but whose
    // out-half is not — the saturated bottleneck cells.
    let reach = g.residual_reachable(s);
    let cut: Vec<usize> = (0..n)
        .filter(|&v| free[v] && reach[2 * v] && !reach[2 * v + 1])
        .collect();
    (flow, cut)
}

/// Minimal adjacency-list max-flow (Edmonds–Karp: BFS augmenting paths).
/// Unit capacities keep each augmentation O(E); the graphs here are ≤ ~20k
/// nodes, so this is instantaneous.
struct FlowGraph {
    /// Per-node list of edge indices into `to`/`cap`.
    adj: Vec<Vec<usize>>,
    to: Vec<usize>,
    cap: Vec<i64>,
}

impl FlowGraph {
    fn new(n: usize) -> Self {
        Self {
            adj: vec![Vec::new(); n],
            to: Vec::new(),
            cap: Vec::new(),
        }
    }

    fn add_edge(&mut self, u: usize, v: usize, c: i64) {
        let e = self.to.len();
        self.to.push(v);
        self.cap.push(c);
        self.adj[u].push(e);
        self.to.push(u);
        self.cap.push(0);
        self.adj[v].push(e + 1);
    }

    fn max_flow(&mut self, s: usize, t: usize) -> usize {
        let mut flow = 0usize;
        loop {
            // BFS for a shortest augmenting path; record the edge used to
            // reach each node.
            let mut prev_edge = vec![usize::MAX; self.adj.len()];
            let mut seen = vec![false; self.adj.len()];
            seen[s] = true;
            let mut q = VecDeque::from([s]);
            'bfs: while let Some(u) = q.pop_front() {
                for &e in &self.adj[u] {
                    let v = self.to[e];
                    if self.cap[e] > 0 && !seen[v] {
                        seen[v] = true;
                        prev_edge[v] = e;
                        if v == t {
                            break 'bfs;
                        }
                        q.push_back(v);
                    }
                }
            }
            if !seen[t] {
                return flow;
            }
            // Augment by 1 (all capacities are unit).
            let mut v = t;
            while v != s {
                let e = prev_edge[v];
                self.cap[e] -= 1;
                self.cap[e ^ 1] += 1;
                v = self.to[e ^ 1];
            }
            flow += 1;
        }
    }

    /// Nodes reachable from `s` in the residual graph (call after
    /// [`Self::max_flow`]).
    fn residual_reachable(&self, s: usize) -> Vec<bool> {
        let mut seen = vec![false; self.adj.len()];
        seen[s] = true;
        let mut q = VecDeque::from([s]);
        while let Some(u) = q.pop_front() {
            for &e in &self.adj[u] {
                let v = self.to[e];
                if self.cap[e] > 0 && !seen[v] {
                    seen[v] = true;
                    q.push_back(v);
                }
            }
        }
        seen
    }
}

/// Fixed-size bitset over search nodes.
#[derive(Clone)]
struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    fn new(n: usize) -> Self {
        Self {
            words: vec![0; n.div_ceil(64)],
        }
    }
    #[inline]
    fn get(&self, i: usize) -> bool {
        self.words[i / 64] >> (i % 64) & 1 != 0
    }
    #[inline]
    fn set(&mut self, i: usize) {
        self.words[i / 64] |= 1 << (i % 64);
    }
    #[inline]
    fn clear(&mut self, i: usize) {
        self.words[i / 64] &= !(1 << (i % 64));
    }
}

/// The window's coarse uniform grid.
struct WinGrid {
    origin: Vec2,
    pitch: f64,
    nx: usize,
    ny: usize,
}

impl WinGrid {
    fn new(window: (Vec2, Vec2), width: f64, clearance: f64) -> Self {
        let (lo, hi) = window;
        let span_x = (hi.x - lo.x).max(1e-3);
        let span_y = (hi.y - lo.y).max(1e-3);
        let mut pitch = (width + clearance).max(0.02);
        let need = (span_x / pitch).max(span_y / pitch);
        if need > MAX_AXIS_CELLS as f64 {
            pitch = (span_x / MAX_AXIS_CELLS as f64).max(span_y / MAX_AXIS_CELLS as f64);
        }
        let nx = (span_x / pitch).ceil() as usize + 1;
        let ny = (span_y / pitch).ceil() as usize + 1;
        Self {
            origin: lo,
            pitch,
            nx,
            ny,
        }
    }

    fn world(&self, cell: usize) -> Vec2 {
        let (ix, iy) = (cell % self.nx, cell / self.nx);
        Vec2::new(
            self.origin.x + ix as f64 * self.pitch,
            self.origin.y + iy as f64 * self.pitch,
        )
    }

    fn snap(&self, p: Vec2) -> usize {
        let ix = (((p.x - self.origin.x) / self.pitch).round() as i64).clamp(0, self.nx as i64 - 1);
        let iy = (((p.y - self.origin.y) / self.pitch).round() as i64).clamp(0, self.ny as i64 - 1);
        iy as usize * self.nx + ix as usize
    }
}

/// Drop interior points collinear with their neighbors.
fn simplify(pts: &[Vec2]) -> Vec<Vec2> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let a = *out.last().unwrap();
        let b = pts[i];
        let c = pts[i + 1];
        let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if cross.abs() > 1e-9 {
            out.push(b);
        }
    }
    out.push(*pts.last().unwrap());
    out
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RouteSession;
    use vcad_ir::ecad::*;

    fn board(traces: Vec<Trace>, layers: &[PcbLayer]) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: layers
                    .iter()
                    .map(|&layer| StackupLayer {
                        layer,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(0.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".into()),
                    })
                    .collect(),
            },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
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

    fn trace(net: &str, a: Vec2, b: Vec2) -> Trace {
        Trace {
            start: a,
            end: b,
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        }
    }

    const WINDOW: (Vec2, Vec2) = (Vec2 { x: 10.0, y: 10.0 }, Vec2 { x: 30.0, y: 30.0 });

    /// A wall on FCu at x=20 with two ~1.4 mm gaps (y≈14.3..15.7 and
    /// y≈24.3..25.7): exactly two crossing channels at the coarse pitch.
    fn two_channel_wall() -> Vec<Trace> {
        vec![
            trace("GND", Vec2::new(20.0, 0.0), Vec2::new(20.0, 14.3)),
            trace("GND", Vec2::new(20.0, 15.7), Vec2::new(20.0, 24.3)),
            trace("GND", Vec2::new(20.0, 25.7), Vec2::new(20.0, 40.0)),
        ]
    }

    fn assert_probe_legal(
        session: &RouteSession,
        conns: &[(String, Vec2, Vec2)],
        routed: &[Vec<(Vec2, Vec2, PcbLayer)>],
        width: f64,
    ) {
        for ((net, _, _), segs) in conns.iter().zip(routed) {
            let clr = session.clearance_for(net);
            for (a, b, l) in segs {
                assert!(
                    session
                        .probe(
                            &CopperGeom::Segment {
                                a: *a,
                                b: *b,
                                half_w: width / 2.0
                            },
                            *l,
                            net,
                            clr,
                        )
                        .legal,
                    "segment ({:.2},{:.2})->({:.2},{:.2}) on {l:?} of net {net} must be legal",
                    a.x,
                    a.y,
                    b.x,
                    b.y,
                );
            }
        }
    }

    /// Endpoints must be reached: first segment starts at `from`, last ends at `to`.
    fn assert_connected(conns: &[(String, Vec2, Vec2)], routed: &[Vec<(Vec2, Vec2, PcbLayer)>]) {
        for ((net, from, to), segs) in conns.iter().zip(routed) {
            assert!(!segs.is_empty(), "net {net} must have copper");
            assert!(
                dist(segs[0].0, *from) < 1e-9,
                "net {net} must start at its from-terminal"
            );
            assert!(
                dist(segs.last().unwrap().1, *to) < 1e-9,
                "net {net} must end at its to-terminal"
            );
            // Contiguity: each segment starts where the previous one ended.
            for w in segs.windows(2) {
                assert!(
                    dist(w[0].1, w[1].0) < 1e-9,
                    "net {net} path must be contiguous"
                );
            }
        }
    }

    #[test]
    fn three_crossing_nets_route_on_two_layers() {
        let pcb = board(vec![], &[PcbLayer::FCu, PcbLayer::BCu]);
        let session = RouteSession::from_pcb(&pcb);
        // A and B cross; C runs between them. Jointly routable with vias.
        let conns = vec![
            (
                "A".to_string(),
                Vec2::new(12.0, 12.0),
                Vec2::new(28.0, 28.0),
            ),
            (
                "B".to_string(),
                Vec2::new(12.0, 28.0),
                Vec2::new(28.0, 12.0),
            ),
            (
                "C".to_string(),
                Vec2::new(12.0, 20.0),
                Vec2::new(28.0, 20.0),
            ),
        ];
        let r = route_window_complete(
            &session,
            WINDOW,
            &[PcbLayer::FCu, PcbLayer::BCu],
            &conns,
            0.25,
            2_000_000,
        );
        let CompleteOutcome::Routed(routed) = r else {
            panic!("three crossing nets on two layers must be jointly routable, got {r:?}");
        };
        assert_eq!(routed.len(), 3);
        assert_connected(&conns, &routed);
        assert_probe_legal(&session, &conns, &routed, 0.25);
    }

    #[test]
    fn three_nets_through_two_channels_proved_infeasible() {
        let pcb = board(two_channel_wall(), &[PcbLayer::FCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![
            (
                "N1".to_string(),
                Vec2::new(12.0, 14.0),
                Vec2::new(28.0, 14.0),
            ),
            (
                "N2".to_string(),
                Vec2::new(12.0, 20.0),
                Vec2::new(28.0, 20.0),
            ),
            (
                "N3".to_string(),
                Vec2::new(12.0, 26.0),
                Vec2::new(28.0, 26.0),
            ),
        ];
        let r = route_window_complete(&session, WINDOW, &[PcbLayer::FCu], &conns, 0.25, 2_000_000);
        let CompleteOutcome::ProvedInfeasible { reason } = r else {
            panic!("3 nets through a 2-channel wall must be proved infeasible, got {r:?}");
        };
        assert!(
            reason.contains("channel"),
            "certificate must name the bottleneck, got: {reason}"
        );
        assert!(
            reason.contains('2'),
            "certificate must count the free channels: {reason}"
        );
    }

    #[test]
    fn two_nets_through_two_channels_route() {
        let pcb = board(two_channel_wall(), &[PcbLayer::FCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![
            (
                "N1".to_string(),
                Vec2::new(12.0, 14.0),
                Vec2::new(28.0, 14.0),
            ),
            (
                "N2".to_string(),
                Vec2::new(12.0, 26.0),
                Vec2::new(28.0, 26.0),
            ),
        ];
        let r = route_window_complete(&session, WINDOW, &[PcbLayer::FCu], &conns, 0.25, 2_000_000);
        let CompleteOutcome::Routed(routed) = r else {
            panic!("2 nets through 2 channels must route, got {r:?}");
        };
        assert_connected(&conns, &routed);
        assert_probe_legal(&session, &conns, &routed, 0.25);
    }

    #[test]
    fn tiny_budget_reports_unknown_never_infeasible() {
        // Same (feasible) instance as the crossing test: with budget=1 the
        // flow pre-pass passes, the DFS trips immediately, and the honest
        // answer is BudgetExhausted — never a fake infeasibility proof.
        let pcb = board(vec![], &[PcbLayer::FCu, PcbLayer::BCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![
            (
                "A".to_string(),
                Vec2::new(12.0, 12.0),
                Vec2::new(28.0, 28.0),
            ),
            (
                "B".to_string(),
                Vec2::new(12.0, 28.0),
                Vec2::new(28.0, 12.0),
            ),
            (
                "C".to_string(),
                Vec2::new(12.0, 20.0),
                Vec2::new(28.0, 20.0),
            ),
        ];
        let r = route_window_complete(
            &session,
            WINDOW,
            &[PcbLayer::FCu, PcbLayer::BCu],
            &conns,
            0.25,
            1,
        );
        assert!(
            matches!(r, CompleteOutcome::BudgetExhausted),
            "budget=1 must yield BudgetExhausted, got {r:?}"
        );
    }
}
