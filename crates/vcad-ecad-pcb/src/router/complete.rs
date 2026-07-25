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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use vcad_ir::ecad::PcbLayer;
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::CopperGeom;

/// Default cap on grid cells per axis; the pitch coarsens to fit. Keeps the
/// state space honest (≤ 48×48×4 ≈ 9.2k nodes).
pub const MAX_AXIS_CELLS: usize = 48;

/// Which copper layers a connection's two terminals may attach on — normally
/// the layers each terminal's pad actually exists on.
///
/// Without this the search takes the first layer whose cell happens to be free,
/// which on a ten-layer board is usually *not* the pad's layer: the emitted stub
/// then floats on an inner layer with nothing joining it to the pad, and the
/// board grows a net island instead of a connection. Empty means unconstrained
/// (the whole searched stack).
#[derive(Debug, Clone, Default)]
pub struct TerminalLayers {
    /// Layers the `from` terminal may attach on.
    pub from: Vec<PcbLayer>,
    /// Layers the `to` terminal may attach on.
    pub to: Vec<PcbLayer>,
}

/// The via the caller will actually commit for a layer change.
///
/// Supplying it makes the router's legality model identical to the caller's
/// commit rule instead of merely similar: the barrel is probed at its real pad
/// size, the drill is checked against the board's hole-to-hole rule (which no
/// layer-scoped copper probe can see), and the grid pitch is floored so two
/// vias of one routing can never land closer than that rule allows. Without it
/// the router falls back to a pad-size heuristic and leaves drills to the
/// caller — which is where a "routed" path can still fail a fail-closed commit.
#[derive(Debug, Clone, Copy)]
pub struct ViaClass {
    /// Via pad (annulus) diameter, mm.
    pub pad_diameter: f64,
    /// Drilled hole diameter, mm.
    pub drill: f64,
}

/// Search resources the caller may raise without weakening the decision
/// procedure: the DFS expansion budget and the grid's cells-per-axis cap.
///
/// The two are coupled on purpose. A wide window at a fixed cell cap coarsens
/// the pitch until unrelated terminals collide in one cell and free channels
/// vanish — the discretization, not the copper, then decides the verdict.
/// Raising [`Self::max_axis_cells`] alongside the window keeps the pitch at its
/// `(width + separation)` floor, so the answer stays about the board.
#[derive(Debug, Clone, Copy)]
pub struct WindowBudget {
    /// Maximum DFS node expansions before the search reports "unknown".
    pub expansions: usize,
    /// Cap on grid cells per axis. The pitch never goes *below* the
    /// `width + separation` floor, so a generous cap simply stops the pitch
    /// from coarsening on a large window.
    pub max_axis_cells: usize,
}

impl WindowBudget {
    /// `expansions` at the default cell cap.
    pub fn new(expansions: usize) -> Self {
        Self {
            expansions,
            max_axis_cells: MAX_AXIS_CELLS,
        }
    }

    /// Same budget with a raised cells-per-axis cap.
    pub fn with_max_axis_cells(self, max_axis_cells: usize) -> Self {
        Self {
            max_axis_cells,
            ..self
        }
    }
}

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

/// The real vias of a [`CompleteOutcome::Routed`] path: one barrel per run of
/// layer changes at a shared point, as `(center, top, bottom)`.
///
/// A path that steps F.Cu → In1 → In2 at one point is ONE barrel spanning
/// F.Cu → In2, not two. Reading the transitions off `windows(2)` without
/// merging them emits coincident vias, which stacks drills at zero spacing and
/// fails hole-to-hole against itself — a "routed" path that cannot commit. The
/// coincidence test also guards the other direction: consecutive segments on
/// different layers whose endpoints do *not* meet are not a via at all.
pub fn path_vias(path: &[(Vec2, Vec2, PcbLayer)]) -> Vec<(Vec2, PcbLayer, PcbLayer)> {
    let mut vias: Vec<(Vec2, PcbLayer, PcbLayer)> = Vec::new();
    for w in path.windows(2) {
        let (_, b0, l0) = w[0];
        let (a1, _, l1) = w[1];
        if l0 == l1 || dist(b0, a1) > 1e-9 {
            continue;
        }
        match vias.last_mut() {
            // Same point, and the previous barrel ends where this one starts:
            // extend it rather than opening a second one.
            Some((p, _, end)) if dist(*p, b0) < 1e-9 && *end == l0 => *end = l1,
            _ => vias.push((b0, l0, l1)),
        }
    }
    vias
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
    route_window_complete_with(
        session,
        window,
        layers,
        conns,
        width,
        WindowBudget::new(budget),
    )
}

/// [`route_window_complete`] with explicit search resources — see
/// [`WindowBudget`].
pub fn route_window_complete_with(
    session: &RouteSession,
    window: (Vec2, Vec2),
    layers: &[PcbLayer],
    conns: &[(String, Vec2, Vec2)],
    width: f64,
    limits: WindowBudget,
) -> CompleteOutcome {
    route_window_complete_pinned(session, window, layers, conns, &[], width, None, limits)
}

/// [`route_window_complete_with`], with each connection's terminals pinned to
/// the layers they may attach on (see [`TerminalLayers`]) and the caller's real
/// via geometry (see [`ViaClass`]). A shorter `terminals` slice than `conns`
/// leaves the remaining connections unconstrained.
#[allow(clippy::too_many_arguments)]
pub fn route_window_complete_pinned(
    session: &RouteSession,
    window: (Vec2, Vec2),
    layers: &[PcbLayer],
    conns: &[(String, Vec2, Vec2)],
    terminals: &[TerminalLayers],
    width: f64,
    via: Option<ViaClass>,
    limits: WindowBudget,
) -> CompleteOutcome {
    let budget = limits.expansions;
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
    // Pair-aware pitch. Node disjointness is only a *sufficient* legality rule
    // when one pitch of separation satisfies every rule that can apply between
    // two paths — and between the two legs of a declared differential pair the
    // probe demands the intra-pair GAP, which routinely exceeds the base
    // clearance. Two twins in one window at clearance pitch route uncoupled and
    // then fail the oracle (the fail-closed probe the caller commits through),
    // which shows up as an honest-but-avoidable unknown. Spacing the grid by the
    // widest gap among the window's twin pairs restores "distinct cells ⇒
    // mutually legal" for pairs, exactly as it already holds for singles.
    let mut separation = max_clearance;
    for (i, (a, _, _)) in conns.iter().enumerate() {
        for (b, _, _) in conns.iter().skip(i + 1) {
            if let Some(gap) = session.pair_gap_between(a, b) {
                separation = separation.max(gap);
            }
        }
    }
    // Second pitch floor: two vias one cell apart must satisfy the board's
    // hole-to-hole rule, or a routing this model calls legal lands drill
    // collisions the moment it is committed.
    let drill_floor = via.map_or(0.0, |v| v.drill + session.hole_to_hole());
    let grid = WinGrid::new(
        window,
        width,
        separation,
        drill_floor,
        limits.max_axis_cells,
    );
    let plane = grid.nx * grid.ny;
    let nl = layers.len();
    let n_nodes = plane * nl;
    let half_w = width / 2.0;
    // Via legality is probed as a disc covering the real committed pad on every
    // spanned layer. Probing smaller than the committed pad let verdict copper
    // pass here and fail board DRC by the difference.
    let via_r = (width * 1.5)
        .max(0.11)
        .max(via.map_or(0.0, |v| v.pad_diameter / 2.0));
    let via_drill = via.map(|v| v.drill);
    // Every layer change the router emits must also clear existing *holes*.
    let barrel_ok =
        |center: Vec2| -> bool { via_drill.is_none_or(|d| session.probe_drill(center, d).legal) };

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
    /// Per-connection escape layers: `Some(pad layer)` when that terminal
    /// reaches the grid through a stub-plus-via dog-bone rather than landing on
    /// the pad's own layer.
    type Escape = (Option<PcbLayer>, Option<PcbLayer>);
    let mut escapes: Vec<Escape> = Vec::with_capacity(conns.len());
    for (ci, (net, from, to)) in conns.iter().enumerate() {
        let clr = clearances[ci];
        let pinned = terminals.get(ci);
        // Attachment candidates: cells in rings around the snapped one, nearest
        // first. A pad on a fine-pitch part often has its own nearest cell
        // centre buried in a neighbour's clearance while a cell one step out is
        // free; since the pad-to-cell connector is probed exactly either way,
        // reaching for that cell is not a cheat — and it is what lets the stub
        // stay on the pad's own layer instead of surfacing on an inner one with
        // nothing joining it to the pad.
        // Reach measured in millimetres, not cells: a dog-bone escape lands
        // within about a millimetre of its pad whatever the pitch happens to be.
        let attach_ring = ((1.0 / grid.pitch).ceil() as i64).clamp(2, 4);
        let attach = |p: Vec2, allowed: &[PcbLayer]| -> Option<(usize, Option<PcbLayer>)> {
            let base = grid.snap(p);
            let (bx, by) = ((base % grid.nx) as i64, (base / grid.nx) as i64);
            let mut cands: Vec<(i64, usize)> = Vec::new();
            for dy in -attach_ring..=attach_ring {
                for dx in -attach_ring..=attach_ring {
                    let (x, y) = (bx + dx, by + dy);
                    if x < 0 || y < 0 || x >= grid.nx as i64 || y >= grid.ny as i64 {
                        continue;
                    }
                    cands.push((dx * dx + dy * dy, y as usize * grid.nx + x as usize));
                }
            }
            cands.sort_unstable();
            let stub_ok = |p: Vec2, c: Vec2, layer: PcbLayer| {
                session
                    .probe(&CopperGeom::Segment { a: p, b: c, half_w }, layer, net, clr)
                    .legal
            };
            // First choice: land directly on a layer the pad is on. Nothing to
            // join, nothing to drill.
            let direct = cands.iter().find_map(|&(_, cell)| {
                let c = grid.world(cell);
                (0..nl)
                    .filter(|&li| allowed.is_empty() || allowed.contains(&layers[li]))
                    .find_map(|li| {
                        (free[li * plane + cell] && stub_ok(p, c, layers[li]))
                            .then_some((li * plane + cell, None))
                    })
            });
            if direct.is_some() || allowed.is_empty() {
                return direct;
            }
            // Otherwise escape like a real router does: a stub on the pad's own
            // layer to a nearby cell, then a via down to the layer the search
            // wants. The whole barrel is probed — pad copper on every layer it
            // spans plus the hole-to-hole rule — so the escape is legal by the
            // same oracle that will judge the committed board, not by assumption.
            cands.iter().find_map(|&(_, cell)| {
                let c = grid.world(cell);
                if dist(p, c) < 1e-6 {
                    // Pad centre on the cell centre leaves no stub to carry the
                    // layer change; take another cell.
                    return None;
                }
                let pad_li = (0..nl).find(|&li| allowed.contains(&layers[li]))?;
                if !stub_ok(p, c, layers[pad_li]) || !barrel_ok(c) {
                    return None;
                }
                (0..nl).find_map(|li| {
                    if li == pad_li || !free[li * plane + cell] {
                        return None;
                    }
                    let span = if li < pad_li {
                        li..=pad_li
                    } else {
                        pad_li..=li
                    };
                    let barrel = CopperGeom::Disc {
                        center: c,
                        r: via_r,
                    };
                    span.clone()
                        .all(|s| session.probe(&barrel, layers[s], net, clr).legal)
                        .then_some((li * plane + cell, Some(layers[pad_li])))
                })
            })
        };
        let (from_pins, to_pins) = pinned
            .map(|t| (t.from.as_slice(), t.to.as_slice()))
            .unwrap_or((&[], &[]));
        match (attach(*from, from_pins), attach(*to, to_pins)) {
            (Some((s, se)), Some((t, te))) => {
                terms.push((s, t));
                escapes.push((se, te));
            }
            (s, _) => {
                // Name the copper that walls the pad in: the cut here is the
                // ring of blockers around the terminal itself.
                let stuck = if s.is_none() { *from } else { *to };
                let pins = if s.is_none() { from_pins } else { to_pins };
                let on = if pins.is_empty() {
                    format!("all {nl} copper layers")
                } else {
                    format!("its {} pad layer(s) {pins:?}", pins.len())
                };
                let census = blocking_nets(
                    session,
                    &CopperGeom::Segment {
                        a: stuck,
                        b: grid.world(grid.snap(stuck)),
                        half_w,
                    },
                    layers,
                    net,
                    clr,
                );
                return CompleteOutcome::ProvedInfeasible {
                    reason: format!(
                        "terminal of net {net} at ({:.2}, {:.2})/({:.2}, {:.2}) has no \
                         clearance-legal grid attachment — the pad at ({:.2}, {:.2}) is \
                         walled in on {on} by {} at the current rules and grid pitch \
                         {:.3} mm",
                        from.x,
                        from.y,
                        to.x,
                        to.y,
                        stuck.x,
                        stuck.y,
                        name_census(&census),
                        grid.pitch
                    ),
                };
            }
        }
    }
    // Two DIFFERENT-net connections snapping onto the same node can never be
    // node-disjoint — a genuine infeasibility at this pitch. Two connections
    // of the SAME net sharing a cell is not: same-net copper may legally
    // share space, but this model's per-connection node-disjointness can't
    // express that, so the honest answer is unknown, never a proof. A single
    // connection whose OWN two terminals snap into one cell is neither: it is
    // simply shorter than the pitch, and its path is that one node.
    {
        let mut seen: HashMap<usize, (&str, usize)> = HashMap::new();
        for (ci, &(s, t)) in terms.iter().enumerate() {
            for node in [s, t] {
                match seen.get(&node) {
                    None => {
                        seen.insert(node, (conns[ci].0.as_str(), ci));
                    }
                    Some(&(_, cj)) if cj == ci => {}
                    Some(&(other, _)) if other != conns[ci].0 => {
                        return CompleteOutcome::ProvedInfeasible {
                            reason: format!(
                                "terminals of nets {} and {other} collide in the same \
                                 {:.3} mm grid cell — the window cannot host \
                                 node-disjoint paths",
                                conns[ci].0, grid.pitch
                            ),
                        };
                    }
                    Some(_) => return CompleteOutcome::BudgetExhausted,
                }
            }
        }
    }
    // An escape barrel occupies its cell on every layer it spans, so those
    // nodes are no longer free for anyone. Terminal nodes are exempt: they are
    // already reserved one-per-connection by node disjointness, and blanking
    // one here would make its own connection unroutable.
    {
        let terminal: HashSet<usize> = terms.iter().flat_map(|&(s, t)| [s, t]).collect();
        for (ci, &(se, te)) in escapes.iter().enumerate() {
            for (escape, node) in [(se, terms[ci].0), (te, terms[ci].1)] {
                let Some(pad_layer) = escape else { continue };
                let (cell, attach_li) = (node % plane, node / plane);
                let pad_li = layers.iter().position(|l| *l == pad_layer).unwrap_or(0);
                let span = if attach_li < pad_li {
                    attach_li..=pad_li
                } else {
                    pad_li..=attach_li
                };
                for li in span {
                    let barrel_node = li * plane + cell;
                    if !terminal.contains(&barrel_node) {
                        free[barrel_node] = false;
                    }
                }
            }
        }
    }
    let free = free;

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
            barrel_ok(grid.world(ca))
                && conns.iter().zip(&clearances).all(|((net, _, _), &clr)| {
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

    // --- One connection: reachability decides it exactly ------------------
    // With k = 1 there is nothing to be node-disjoint *from*, so "a joint
    // routing exists" collapses to s–t reachability over the free graph. BFS
    // settles that in O(E): either a shortest path (fewest cells, hence least
    // copper) or an exhausted reachable component, which *is* the proof. No
    // budget can trip, so a lone connection never comes back unknown — the
    // exhaustive DFS is only ever needed to decide genuinely joint instances.
    if conns.len() == 1 {
        let (s, t) = terms[0];
        let empty = BitSet::new(n_nodes);
        return match bfs_path(s, t, &free, &empty, &neighbors, &mut edge_ok) {
            Ok(path) => CompleteOutcome::Routed(vec![emit_segments(
                &grid, plane, layers, conns[0].1, conns[0].2, escapes[0], &path,
            )]),
            Err(reached_from) => {
                // Report the tighter pocket. The graph is undirected, so either
                // terminal's reachable component certifies the severance; the
                // one enclosed by fewer nodes names a small, checkable cut
                // instead of "everything but this corner".
                let reached_to = bfs_path(t, usize::MAX, &free, &empty, &neighbors, &mut edge_ok)
                    .err()
                    .unwrap_or_default();
                let live = |r: &Vec<bool>| r.iter().filter(|v| **v).count();
                let (reached, from_side) =
                    if live(&reached_to) > 0 && live(&reached_to) < live(&reached_from) {
                        (reached_to, false)
                    } else {
                        (reached_from, true)
                    };
                CompleteOutcome::ProvedInfeasible {
                    reason: severed_reason(
                        session,
                        &grid,
                        &Topology {
                            plane,
                            layers,
                            via_r,
                            half_w,
                        },
                        &conns[0],
                        clearances[0],
                        &free,
                        &reached,
                        &neighbors,
                        terms[0],
                        from_side,
                    ),
                }
            }
        };
    }

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
        } else {
            // Flow 0 with an empty min *vertex* cut means the terminals are
            // outright severed: nothing is saturated, so the cut is the ring of
            // blocked nodes enclosing the sources. Name it — a certificate that
            // does not say where the wall is, or whose copper it is, is not
            // much of a certificate.
            let mut reached = vec![false; n_nodes];
            for &(s, _) in &terms {
                if !free[s] || reached[s] {
                    continue;
                }
                if let Err(seen) = bfs_path(
                    s,
                    usize::MAX,
                    &free,
                    &BitSet::new(n_nodes),
                    &neighbors,
                    &mut edge_ok,
                ) {
                    for (v, r) in seen.iter().enumerate() {
                        reached[v] |= *r;
                    }
                }
            }
            reason.push_str(&format!(
                " ({})",
                frontier_census(
                    session,
                    &grid,
                    &Topology {
                        plane,
                        layers,
                        via_r,
                        half_w,
                    },
                    &conns[0].0,
                    clearances[0],
                    &free,
                    &reached,
                    &neighbors,
                )
            ));
        }
        return CompleteOutcome::ProvedInfeasible { reason };
    }

    // --- Cheap witness attempt: sequential shortest paths ------------------
    // The DFS's first descent is a greedy *walk*, not a shortest path, so on a
    // wide window it can wander for millions of expansions before its first
    // completion — the dominant source of honest-but-avoidable unknowns. A few
    // sequential BFS assignments (each net taking a shortest path through what
    // the earlier nets left free) find the easy joint routings in milliseconds.
    // Success is a witness: the paths are node-disjoint by construction, so it
    // needs no proof. Failure proves nothing and falls through to the
    // exhaustive search, so completeness is untouched.
    for order in attempt_orders(&terms, grid.nx, plane) {
        if let Some(paths) =
            sequential_bfs(&order, &terms, &free, n_nodes, &neighbors, &mut edge_ok)
        {
            return CompleteOutcome::Routed(
                paths
                    .iter()
                    .enumerate()
                    .map(|(ci, path)| {
                        emit_segments(
                            &grid,
                            plane,
                            layers,
                            conns[ci].1,
                            conns[ci].2,
                            escapes[ci],
                            path,
                        )
                    })
                    .collect(),
            );
        }
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
                    escapes[ci],
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
    escape: (Option<PcbLayer>, Option<PcbLayer>),
    path: &[usize],
) -> Vec<(Vec2, Vec2, PcbLayer)> {
    let mut segs: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();
    if path.is_empty() {
        return segs;
    }
    // A dog-bone terminal contributes its stub on the pad's own layer; the
    // layer change at the stub's far end is a via the caller materializes from
    // the layer discontinuity, exactly as for a mid-path layer change.
    let first_cell = grid.world(path[0] % plane);
    let mut run: Vec<Vec2> = match escape.0 {
        Some(pad_layer) => {
            segs.push((from, first_cell, pad_layer));
            vec![first_cell]
        }
        None => vec![from],
    };
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
    match escape.1 {
        Some(pad_layer) => {
            let last_cell = grid.world(path[path.len() - 1] % plane);
            flush(&mut run, layers[run_layer], &mut segs);
            segs.push((last_cell, to, pad_layer));
        }
        None => {
            if run.last().map(|p| dist(*p, to) > 1e-9).unwrap_or(true) {
                run.push(to);
            }
            flush(&mut run, layers[run_layer], &mut segs);
        }
    }
    segs
}

/// The window discretization facts the path and certificate helpers share.
struct Topology<'a> {
    plane: usize,
    layers: &'a [PcbLayer],
    via_r: f64,
    half_w: f64,
}

/// Shortest path (fewest nodes) from `start` to `goal` over free, unoccupied
/// nodes joined by legal edges.
///
/// On failure the reachable set is returned — the exhausted component that *is*
/// the infeasibility proof for a single connection. Passing `usize::MAX` as
/// `goal` asks for that set outright.
fn bfs_path(
    start: usize,
    goal: usize,
    free: &[bool],
    occ: &BitSet,
    neighbors: &dyn Fn(usize) -> Vec<usize>,
    edge_ok: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Vec<usize>, Vec<bool>> {
    let mut seen = vec![false; free.len()];
    if !free[start] || occ.get(start) {
        return Err(seen);
    }
    let mut prev = vec![usize::MAX; free.len()];
    seen[start] = true;
    let mut q = VecDeque::from([start]);
    while let Some(u) = q.pop_front() {
        if u == goal {
            let mut path = vec![u];
            let mut cur = u;
            while cur != start {
                cur = prev[cur];
                path.push(cur);
            }
            path.reverse();
            return Ok(path);
        }
        for nb in neighbors(u) {
            if seen[nb] || !free[nb] || occ.get(nb) || !edge_ok(u, nb) {
                continue;
            }
            seen[nb] = true;
            prev[nb] = u;
            q.push_back(nb);
        }
    }
    Err(seen)
}

/// Route the connections in `order`, each taking a shortest path through the
/// nodes its predecessors left free. Returns per-connection paths (indexed by
/// connection, not by position in `order`) when every one succeeds — a
/// node-disjoint joint routing by construction. `None` means only "this order
/// did not work"; it is never evidence of infeasibility.
fn sequential_bfs(
    order: &[usize],
    terms: &[(usize, usize)],
    free: &[bool],
    n_nodes: usize,
    neighbors: &dyn Fn(usize) -> Vec<usize>,
    edge_ok: &mut dyn FnMut(usize, usize) -> bool,
) -> Option<Vec<Vec<usize>>> {
    let mut occ = BitSet::new(n_nodes);
    let mut paths = vec![Vec::new(); terms.len()];
    for &i in order {
        let (s, t) = terms[i];
        if occ.get(t) {
            return None;
        }
        let path = bfs_path(s, t, free, &occ, neighbors, edge_ok).ok()?;
        for &node in &path {
            occ.set(node);
        }
        paths[i] = path;
    }
    Some(paths)
}

/// Deterministic connection orders for the witness attempt: as given, longest
/// terminal span first (the constrained nets pick their corridor before the
/// short ones fill it), then shortest first.
fn attempt_orders(terms: &[(usize, usize)], nx: usize, plane: usize) -> Vec<Vec<usize>> {
    let span = |&(s, t): &(usize, usize)| {
        let coords = |n: usize| ((n % plane) % nx, (n % plane) / nx, n / plane);
        let (sx, sy, sl) = coords(s);
        let (tx, ty, tl) = coords(t);
        sx.abs_diff(tx) + sy.abs_diff(ty) + sl.abs_diff(tl)
    };
    let identity: Vec<usize> = (0..terms.len()).collect();
    let mut longest = identity.clone();
    longest.sort_by_key(|&i| (std::cmp::Reverse(span(&terms[i])), i));
    let mut shortest = identity.clone();
    shortest.sort_by_key(|&i| (span(&terms[i]), i));
    let mut orders = vec![identity];
    for order in [longest, shortest] {
        if !orders.contains(&order) {
            orders.push(order);
        }
    }
    orders
}

/// Tally, per net, how many blockers stand between `geom` and legality on any
/// of `layers`, for any of the candidate `nets` (net name, clearance).
fn census_add(
    census: &mut BTreeMap<String, usize>,
    session: &RouteSession,
    geom: &CopperGeom,
    layers: &[PcbLayer],
    nets: &[(&str, f64)],
) {
    for &layer in layers {
        for &(net, clearance) in nets {
            for blocker in session.probe(geom, layer, net, clearance).blockers {
                *census.entry(blocker.net).or_default() += 1;
            }
        }
    }
}

/// Nets whose copper blocks `geom`, as a census.
fn blocking_nets(
    session: &RouteSession,
    geom: &CopperGeom,
    layers: &[PcbLayer],
    net: &str,
    clearance: f64,
) -> BTreeMap<String, usize> {
    let mut census = BTreeMap::new();
    census_add(&mut census, session, geom, layers, &[(net, clearance)]);
    census
}

/// The heaviest few blockers, named: `"GND (61), /VDD_5V (4)"`.
fn name_census(census: &BTreeMap<String, usize>) -> String {
    if census.is_empty() {
        return "copper the window's union-legality raster rejects for a sibling net".into();
    }
    let mut ranked: Vec<(&String, &usize)> = census.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    ranked
        .iter()
        .take(3)
        .map(|(net, n)| {
            let named = if net.is_empty() { "<no net>" } else { net };
            format!("{named} ({n})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Describe the ring of blocked nodes that encloses `reached` — the vertex cut
/// the search ran into — by size, extent, and the copper that forms it.
#[allow(clippy::too_many_arguments)]
fn frontier_census(
    session: &RouteSession,
    grid: &WinGrid,
    topo: &Topology,
    net: &str,
    clearance: f64,
    free: &[bool],
    reached: &[bool],
    neighbors: &dyn Fn(usize) -> Vec<usize>,
) -> String {
    let mut cut: Vec<usize> = Vec::new();
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lo, mut hi) = (
        Vec2::new(f64::INFINITY, f64::INFINITY),
        Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for (v, _) in reached.iter().enumerate().filter(|(_, r)| **r) {
        for nb in neighbors(v) {
            if reached[nb] {
                continue;
            }
            cut.push(nb);
            let p = grid.world(nb % topo.plane);
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
            // The copper that forbids this step: the swept trace for an
            // in-plane move, the via pad for a layer change.
            let (la, ca) = (v / topo.plane, v % topo.plane);
            let (lb, cb) = (nb / topo.plane, nb % topo.plane);
            if la == lb {
                let geom = CopperGeom::Segment {
                    a: grid.world(ca),
                    b: grid.world(cb),
                    half_w: topo.half_w,
                };
                census_add(
                    &mut census,
                    session,
                    &geom,
                    &[topo.layers[la]],
                    &[(net, clearance)],
                );
            } else {
                let geom = CopperGeom::Disc {
                    center: grid.world(ca),
                    r: topo.via_r,
                };
                census_add(
                    &mut census,
                    session,
                    &geom,
                    &[topo.layers[la], topo.layers[lb]],
                    &[(net, clearance)],
                );
            }
        }
    }
    cut.sort_unstable();
    cut.dedup();
    if cut.is_empty() {
        return format!(
            "the {} free node{} it can reach have no blocked neighbour at all: the window \
             itself is too small to leave the terminal's pocket",
            free.iter().filter(|f| **f).count(),
            if free.len() == 1 { "" } else { "s" }
        );
    }
    format!(
        "the enclosing cut is {} blocked node{} spanning x={:.2}..{:.2}, y={:.2}..{:.2}, held \
         by {}",
        cut.len(),
        if cut.len() == 1 { "" } else { "s" },
        lo.x - grid.pitch / 2.0,
        hi.x + grid.pitch / 2.0,
        lo.y - grid.pitch / 2.0,
        hi.y + grid.pitch / 2.0,
        name_census(&census),
    )
}

/// Certificate for a lone connection whose terminals are severed on the
/// canonical grid: what was exhausted, and which copper closed the door.
#[allow(clippy::too_many_arguments)]
fn severed_reason(
    session: &RouteSession,
    grid: &WinGrid,
    topo: &Topology,
    conn: &(String, Vec2, Vec2),
    clearance: f64,
    free: &[bool],
    reached: &[bool],
    neighbors: &dyn Fn(usize) -> Vec<usize>,
    terms: (usize, usize),
    from_side: bool,
) -> String {
    let (net, from, to) = conn;
    let n_free = free.iter().filter(|f| **f).count();
    let n_reached = reached.iter().filter(|r| **r).count();
    let (searched, other, other_node) = if from_side {
        (from, to, terms.1)
    } else {
        (to, from, terms.0)
    };
    format!(
        "net {net} is severed inside the window: breadth-first search from its \
         {} terminal at ({:.2}, {:.2}) exhausted every reachable node — {n_reached} of the \
         {n_free} clearance-free (cell, layer) nodes on the {}-layer stack — without touching \
         the other terminal at ({:.2}, {:.2}) (layer {:?}); {}. With k = 1 reachability is \
         exact, so no path exists on the canonical grid at pitch {:.3} mm",
        if from_side { "from" } else { "to" },
        searched.x,
        searched.y,
        topo.layers.len(),
        other.x,
        other.y,
        topo.layers[other_node / topo.plane],
        frontier_census(session, grid, topo, net, clearance, free, reached, neighbors),
        grid.pitch,
    )
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
    fn new(
        window: (Vec2, Vec2),
        width: f64,
        separation: f64,
        drill_floor: f64,
        max_axis_cells: usize,
    ) -> Self {
        let (lo, hi) = window;
        let span_x = (hi.x - lo.x).max(1e-3);
        let span_y = (hi.y - lo.y).max(1e-3);
        // 6% over the exact width+separation floor: node-disjoint paths in
        // adjacent columns sit at pitch - width — exactly the separation at
        // the floor, which board DRC then fails on floating-point margins.
        let mut pitch = ((width + separation) * 1.06)
            .max(0.02)
            .max(drill_floor * 1.02);
        let cap = max_axis_cells.max(2) as f64;
        let need = (span_x / pitch).max(span_y / pitch);
        if need > cap {
            pitch = (span_x / cap).max(span_y / cap);
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
                    target_impedance: None,
                    target_diff_impedance: None,
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
            // Channels sized so each free-node band (gap minus wall
            // clearance) is narrower than the grid pitch — at most one
            // node-disjoint path per channel, the premise of the
            // 3-through-2 infeasibility certificate.
            trace("GND", Vec2::new(20.0, 0.0), Vec2::new(20.0, 14.75)),
            trace("GND", Vec2::new(20.0, 15.75), Vec2::new(20.0, 24.75)),
            trace("GND", Vec2::new(20.0, 25.75), Vec2::new(20.0, 40.0)),
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

    /// A lone connection is decided by reachability, which no budget can
    /// interrupt: budget 1 must still answer, and answer definitively.
    #[test]
    fn one_connection_is_never_unknown() {
        let pcb = board(vec![], &[PcbLayer::FCu, PcbLayer::BCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![(
            "A".to_string(),
            Vec2::new(12.0, 12.0),
            Vec2::new(28.0, 28.0),
        )];
        let r = route_window_complete(&session, WINDOW, &[PcbLayer::FCu], &conns, 0.25, 1);
        let CompleteOutcome::Routed(routed) = r else {
            panic!("a reachable lone connection must route at any budget, got {r:?}");
        };
        assert_connected(&conns, &routed);
        assert_probe_legal(&session, &conns, &routed, 0.25);
    }

    /// A lone connection walled off by foreign copper is *proved* infeasible —
    /// never unknown — and the certificate names the cut it ran into.
    #[test]
    fn one_severed_connection_is_proved_with_a_named_cut() {
        // A closed box of foreign copper around the source terminal.
        let boxed = vec![
            trace("GND", Vec2::new(11.0, 11.0), Vec2::new(13.0, 11.0)),
            trace("GND", Vec2::new(13.0, 11.0), Vec2::new(13.0, 13.0)),
            trace("GND", Vec2::new(13.0, 13.0), Vec2::new(11.0, 13.0)),
            trace("GND", Vec2::new(11.0, 13.0), Vec2::new(11.0, 11.0)),
        ];
        let pcb = board(boxed, &[PcbLayer::FCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![(
            "A".to_string(),
            Vec2::new(12.0, 12.0),
            Vec2::new(28.0, 28.0),
        )];
        let r = route_window_complete(&session, WINDOW, &[PcbLayer::FCu], &conns, 0.25, 2_000_000);
        let CompleteOutcome::ProvedInfeasible { reason } = r else {
            panic!("a boxed-in terminal must be proved infeasible, got {r:?}");
        };
        assert!(
            reason.contains("GND"),
            "certificate must name the copper forming the cut: {reason}"
        );
    }

    /// Terminals pinned to a layer must attach there — a path that surfaces on
    /// another layer would be electrically dangling.
    #[test]
    fn pinned_terminals_attach_on_their_own_layer() {
        let pcb = board(vec![], &[PcbLayer::FCu, PcbLayer::BCu]);
        let session = RouteSession::from_pcb(&pcb);
        let conns = vec![(
            "A".to_string(),
            Vec2::new(12.0, 12.0),
            Vec2::new(28.0, 28.0),
        )];
        let pins = vec![TerminalLayers {
            from: vec![PcbLayer::BCu],
            to: vec![PcbLayer::BCu],
        }];
        let r = route_window_complete_pinned(
            &session,
            WINDOW,
            &[PcbLayer::FCu, PcbLayer::BCu],
            &conns,
            &pins,
            0.25,
            None,
            WindowBudget::new(2_000_000),
        );
        let CompleteOutcome::Routed(routed) = r else {
            panic!("an empty board must route a pinned connection, got {r:?}");
        };
        assert_connected(&conns, &routed);
        assert_eq!(
            routed[0].first().map(|s| s.2),
            Some(PcbLayer::BCu),
            "the first segment must leave the pad on the pinned layer"
        );
        assert_eq!(
            routed[0].last().map(|s| s.2),
            Some(PcbLayer::BCu),
            "the last segment must reach the pad on the pinned layer"
        );
    }

    #[test]
    fn tiny_budget_never_fakes_infeasibility() {
        // Same (feasible) instance as the crossing test, with budget=1. The
        // budget bounds the exhaustive DFS only: the sequential-BFS witness
        // pass runs first and settles this instance, so the outcome here is a
        // routing. What must never happen — at any budget — is a claim of
        // infeasibility for an instance whose space was not exhausted.
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
            !matches!(r, CompleteOutcome::ProvedInfeasible { .. }),
            "budget=1 must never yield an infeasibility proof, got {r:?}"
        );
        let CompleteOutcome::Routed(routed) = r else {
            panic!("the witness pass settles this feasible instance, got {r:?}");
        };
        assert_connected(&conns, &routed);
        assert_probe_legal(&session, &conns, &routed, 0.25);
    }

    #[test]
    fn a_run_of_layer_changes_at_one_point_is_one_barrel() {
        // F.Cu -> In1 -> In2 without moving: one barrel spanning F.Cu -> In2.
        // Reading the transitions off `windows(2)` unmerged yields two
        // coincident vias, which stacks drills at zero spacing and fails
        // hole-to-hole against itself — a "routed" path that cannot commit.
        let p = Vec2::new(5.0, 5.0);
        let path = vec![
            (Vec2::new(0.0, 5.0), p, PcbLayer::FCu),
            (p, p, PcbLayer::In1Cu),
            (p, Vec2::new(10.0, 5.0), PcbLayer::In2Cu),
        ];
        assert_eq!(
            path_vias(&path),
            vec![(p, PcbLayer::FCu, PcbLayer::In2Cu)],
            "a run of transitions at one point must merge into a single barrel"
        );
    }

    #[test]
    fn separate_layer_changes_stay_separate_barrels() {
        // Two transitions at *different* points are two real vias, and a layer
        // change across a gap (endpoints not coincident) is no via at all.
        let (p, q) = (Vec2::new(5.0, 5.0), Vec2::new(9.0, 5.0));
        let path = vec![
            (Vec2::new(0.0, 5.0), p, PcbLayer::FCu),
            (p, q, PcbLayer::In1Cu),
            (q, Vec2::new(14.0, 5.0), PcbLayer::FCu),
        ];
        assert_eq!(
            path_vias(&path),
            vec![
                (p, PcbLayer::FCu, PcbLayer::In1Cu),
                (q, PcbLayer::In1Cu, PcbLayer::FCu),
            ]
        );
        let disjoint = vec![
            (Vec2::new(0.0, 5.0), p, PcbLayer::FCu),
            (Vec2::new(20.0, 5.0), Vec2::new(30.0, 5.0), PcbLayer::In1Cu),
        ];
        assert!(
            path_vias(&disjoint).is_empty(),
            "a layer change whose endpoints do not meet is not a via"
        );
    }
}
