//! Whole-board auto-routing over the incremental oracle.
//!
//! [`route_all`] is the router orchestration done where the legality oracle
//! lives. It computes the MST ratsnest, then routes each connection against a
//! single [`RouteSession`] that it grows as it goes — so every net avoids the
//! ones already placed. When a connection can't be routed on the front copper
//! it is retried on the back layer with transition vias, and crucially *the
//! vias are probed against the session on both layers before being committed*.
//! A connection that can't be routed legally on any layer is left unrouted
//! rather than shipping copper that shorts — there is no path here that emits
//! an un-probed segment or via.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet, RatsnestLine};
use crate::session::{RouteSession, SpanId};
use crate::spatial::{point_in_polygon, CopperElement, CopperGeom};

use super::congestion::Congestion;
use super::maze::route_net_maze_cong;
use super::push_shove::{Obstacle, PushShoveRouter};
use super::route_net_maze;

/// A trace produced by the auto-router.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoutedTrace {
    /// Segment start (mm).
    pub start: Vec2,
    /// Segment end (mm).
    pub end: Vec2,
    /// Trace width (mm).
    pub width: f64,
    /// Copper layer.
    pub layer: PcbLayer,
    /// Net.
    pub net: String,
}

/// A through via (FCu..BCu) produced by the auto-router.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoutedVia {
    /// Via center (mm).
    pub position: Vec2,
    /// Net.
    pub net: String,
}

/// Why a connection could not be routed, and where — so an agent or human can
/// act on it (change a layer, free the region, add a via) instead of staring at
/// a bare "unrouted" list.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnroutedDiagnostic {
    /// The net that could not be fully routed.
    pub net: String,
    /// Start of the unrouted connection (mm).
    pub from: Vec2,
    /// End of the unrouted connection (mm).
    pub to: Vec2,
    /// Other nets whose copper blocks the corridor between the endpoints,
    /// most-blocking first.
    pub blocking_nets: Vec<String>,
    /// Min corner of the congested region (the blocked corridor's bbox, mm).
    pub region_min: Vec2,
    /// Max corner of the congested region (mm).
    pub region_max: Vec2,
    /// A copper layer the connection has the best chance on (fewest blockers),
    /// if any layer is less congested than where it was tried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_layer: Option<PcbLayer>,
    /// A point where dropping a via to `suggested_layer` would likely help
    /// (the corridor midpoint), when a clearer layer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_via: Option<Vec2>,
    /// Human-readable explanation of the obstruction.
    pub reason: String,
}

/// Result of routing a whole board.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteAllResult {
    /// Routed trace segments (each carries its layer).
    pub traces: Vec<RoutedTrace>,
    /// Transition vias placed for back-layer routes.
    pub vias: Vec<RoutedVia>,
    /// Nets with at least one routed connection.
    pub routed_nets: Vec<String>,
    /// Nets with at least one connection that could not be routed legally.
    pub unrouted_nets: Vec<String>,
    /// Per-unrouted-connection diagnostics: blocking nets, congested region, and
    /// a suggested layer/via. Empty when the board routed completely.
    pub diagnostics: Vec<UnroutedDiagnostic>,
    /// Overall routability in `[0, 1]`: the fraction of attempted connections
    /// that were routed. `1.0` means a fully-routed board.
    pub routability: f64,
}

/// Knobs for [`route_all_with_opts`].
#[derive(Debug, Clone)]
pub struct RouteOptions {
    /// PathFinder negotiation rounds. Each round re-routes the whole board from
    /// scratch with accumulating per-cell history cost on contested corridors,
    /// so flexible nets are nudged off resources a stuck net needs. `1` disables
    /// negotiation (pure greedy + rip-up — the historical behavior); the default
    /// is [`DEFAULT_NEGOTIATION_ROUNDS`]. The loop stops early the moment a round
    /// routes everything, so an easy board never pays for the extra rounds.
    pub negotiation_rounds: usize,
    /// Try the push-and-shove visibility router as a per-layer fallback when the
    /// maze A* can't find a path. Its output is re-probed against the oracle, so
    /// it never relaxes the DRC-clean invariant. It is engaged only from
    /// negotiation round 1 onward (round 0 is the pure historical baseline), so
    /// the result can never regress below greedy + rip-up — only close more nets.
    pub use_push_shove: bool,
}

/// Default PathFinder negotiation rounds for [`route_all`]. Round 0 is the
/// baseline; the win typically lands within a couple of negotiation rounds, and
/// a board that routes fully stops after round 0 — so this only bounds the work
/// a genuinely congested board does.
pub const DEFAULT_NEGOTIATION_ROUNDS: usize = 4;

impl Default for RouteOptions {
    fn default() -> Self {
        Self {
            negotiation_rounds: DEFAULT_NEGOTIATION_ROUNDS,
            use_push_shove: true,
        }
    }
}

/// Copper layers tried per connection, in order (front first, then back).
const LAYERS: [PcbLayer; 2] = [PcbLayer::FCu, PcbLayer::BCu];

/// Maximum rip-up-and-reroute rounds before accepting the still-unrouted set.
/// The loop also stops the instant a round places nothing new, so this only
/// bounds worst-case work on a genuinely over-constrained board.
const MAX_RIPUP_ROUNDS: usize = 8;

/// History cost (mm-equivalent) deposited on a contested corridor per
/// negotiation round. A gentle, accumulating bias: enough that a flexible net
/// with another way around bows out of a repeatedly-contested band, but small
/// enough not to force destabilising detours that collide elsewhere.
const HISTORY_STEP: f64 = 1.0;

/// Half-width (mm) of the corridor band a still-unrouted connection marks as
/// contested each negotiation round.
const CONTEST_HALF_WIDTH: f64 = 1.0;

/// One connection to (re)route: `(net, from, to)`.
type Conn = (String, Vec2, Vec2);

/// The product of one routing pass: the grown session, the placed connections,
/// and the connections still unrouted after rip-up.
type Pass = (RouteSession, Vec<Placed>, Vec<Conn>);

/// A connection that has been routed, plus the session spans it occupies —
/// enough to rip it back out and re-route it.
struct Placed {
    net: String,
    from: Vec2,
    to: Vec2,
    /// Layer the maze `segments` run on.
    layer: PcbLayer,
    width: f64,
    segments: Vec<(Vec2, Vec2)>,
    /// Fan-out / dog-bone stubs that escape a fine-pitch pad to its via, each on
    /// its own copper layer (the pad's layer). Emitted as traces, not on `layer`.
    stubs: Vec<(Vec2, Vec2, PcbLayer)>,
    via_pts: Vec<Vec2>,
    spans: Vec<SpanId>,
}

/// Route every unrouted net on `pcb` (optionally restricted to `nets_filter`).
///
/// Routes greedily (longest connection first) against one growing
/// [`RouteSession`], with bounded rip-up plus PathFinder-style negotiated
/// congestion layered on top (see [`route_all_with_opts`]). `width` is the trace
/// width; via geometry comes from the board's default rules. Returns the new
/// copper to add — all of it clearance-legal against the board and against the
/// copper the router places.
pub fn route_all(pcb: &Pcb, width: f64, nets_filter: &[String]) -> RouteAllResult {
    route_all_with_opts(pcb, width, nets_filter, &RouteOptions::default())
}

/// [`route_all`] with explicit routing options.
///
/// The negotiation loop is the addition over plain greedy + rip-up: when a round
/// leaves connections unrouted, the corridors they wanted accrue history cost,
/// the whole board is re-routed from scratch with those costs folded into the
/// maze A*, and the best pass seen is kept. History only adds *cost*, never
/// relaxes clearance, so every intermediate pass — and the final result — stays
/// DRC-clean. With `negotiation_rounds == 1` this reduces exactly to the
/// historical greedy + rip-up router.
pub fn route_all_with_opts(
    pcb: &Pcb,
    width: f64,
    nets_filter: &[String],
    opts: &RouteOptions,
) -> RouteAllResult {
    let netlist = netlist_from_pads(pcb);
    let mut rats = compute_ratsnest(pcb, &netlist);

    // Nets that own a copper plane (a filled zone) are connected through that
    // plane, not by pad-to-pad traces: drop their ratsnest air-wires here and
    // stitch each of their pads down to the plane with a via below. This keeps a
    // power/ground net from consuming a signal layer with a star of traces and
    // is the only way an SMD power pad on FCu/BCu ever reaches an inner plane.
    let planes = plane_layers(pcb);
    if !planes.is_empty() {
        rats.retain(|l| !planes.contains_key(&l.net));
    }

    // Route the longest connections first. They span the most board and have
    // the least routing freedom, so giving them the emptier board up front
    // leaves fewer dead-ends for the short connections that fill in around them.
    rats.sort_by(|a, b| {
        dist(b.from, b.to)
            .partial_cmp(&dist(a.from, a.to))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Connections this call attempts (after the net filter) — the denominator
    // for the routability score.
    let total_conns = rats
        .iter()
        .filter(|l| nets_filter.is_empty() || nets_filter.iter().any(|n| n == &l.net))
        .count();

    // Negotiated-congestion loop. Round 0 routes longest-first with a flat
    // history field — exactly the historical greedy + rip-up router, so the
    // result can never regress below it. Each later round (a) re-orders the nets
    // so the ones that stayed unrouted get *first pick* of the contested
    // resource (PathFinder's core idea, realised as priority for a hard-legality
    // router where whoever routes first keeps the channel) and (b) prices up the
    // copper that blocked them, so flexible nets bow out of the contested band.
    // We keep the best pass (most connections placed) ever seen.
    let mut cong = Congestion::new(&pcb.outline.vertices);
    let rounds = opts.negotiation_rounds.max(1);
    let mut best: Option<Pass> = None;
    // How many rounds each net has gone unrouted — its negotiation priority.
    let mut fail_count: BTreeMap<String, usize> = BTreeMap::new();

    for round in 0..rounds {
        // Order: most-failed nets first (round 0 has none, so it stays
        // longest-first), breaking ties by length so long runs still lead.
        let mut ordered = rats.clone();
        ordered.sort_by(|a, b| {
            let fa = fail_count.get(&a.net).copied().unwrap_or(0);
            let fb = fail_count.get(&b.net).copied().unwrap_or(0);
            fb.cmp(&fa).then_with(|| {
                dist(b.from, b.to)
                    .partial_cmp(&dist(a.from, a.to))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Round 0 is the pure baseline (no push-shove); the fallback and the
        // history field are enhancement layers applied from round 1 onward.
        let use_push_shove = opts.use_push_shove && round > 0;
        let (session, placed, pending) =
            route_pass(pcb, width, nets_filter, &ordered, &cong, use_push_shove);

        let last_round = round + 1 == rounds;
        if !last_round && !pending.is_empty() {
            // Raise each still-unrouted net's priority for next round.
            for (net, _, _) in &pending {
                *fail_count.entry(net.clone()).or_default() += 1;
            }
            // Price up the contested band so flexible nets relocate next round.
            deposit_congestion(&mut cong, &session, &pending, width, HISTORY_STEP);
        }

        // Keep the pass that placed the most connections.
        let is_better = best
            .as_ref()
            .map(|(_, bp, _)| placed.len() > bp.len())
            .unwrap_or(true);
        if is_better {
            best = Some((session, placed, pending));
        }

        let fully_routed = best.as_ref().map(|(_, _, p)| p.is_empty()).unwrap_or(false);
        if fully_routed || last_round {
            break;
        }
    }

    let (mut session, mut placed, pending) =
        best.expect("negotiation always runs at least one pass");

    // Fan-out rescue (issue #289 part 1): connections rip-up still couldn't
    // place — most often a net into a fine-pitch pad that can't be escaped on
    // its own layer — get a dog-bone escape via to the back copper. This only
    // *adds* copper for already-unrouted nets, so it can't disturb the routes
    // rip-up settled; a connection it still can't route stays unrouted.
    let mut still_unrouted = Vec::new();
    for (net, from, to) in pending {
        match try_route_fanout(&mut session, pcb, width, &net, from, to, &placed) {
            Some(p) => placed.push(p),
            None => still_unrouted.push((net, from, to)),
        }
    }

    // Stitch the planed nets' pads down to their planes (issue #289 part 2).
    // Runs after signal routing so each stitching via avoids the routed copper
    // and is probed on every copper layer it spans before being committed.
    let stitch = stitch_planes(&mut session, pcb, &planes, nets_filter, &placed, width);

    // Flatten the placed connections into the result.
    let mut traces = Vec::new();
    let mut vias = Vec::new();
    let mut routed: BTreeSet<String> = BTreeSet::new();
    for p in &placed {
        routed.insert(p.net.clone());
        for (a, b) in &p.segments {
            traces.push(RoutedTrace {
                start: *a,
                end: *b,
                width: p.width,
                layer: p.layer,
                net: p.net.clone(),
            });
        }
        for (a, b, l) in &p.stubs {
            traces.push(RoutedTrace {
                start: *a,
                end: *b,
                width: p.width,
                layer: *l,
                net: p.net.clone(),
            });
        }
        for &pt in &p.via_pts {
            vias.push(RoutedVia {
                position: pt,
                net: p.net.clone(),
            });
        }
    }
    // Fold in the plane stitching: dog-bone stubs (each on its pad's layer) and
    // the stitching vias themselves.
    for (a, b, net, l) in &stitch.stubs {
        traces.push(RoutedTrace {
            start: *a,
            end: *b,
            width: session.width_for(net, width),
            layer: *l,
            net: net.clone(),
        });
    }
    for (pt, net) in &stitch.vias {
        vias.push(RoutedVia {
            position: *pt,
            net: net.clone(),
        });
    }
    for n in &stitch.nets {
        routed.insert(n.clone());
    }

    // Diagnose every signal connection the router could not close: which nets
    // block it, where, and which layer/via might help. Computed against the
    // final settled session so the blocker set reflects the real obstruction.
    let copper = copper_layers(pcb);
    let signal_unrouted = still_unrouted.len();
    let mut diagnostics: Vec<UnroutedDiagnostic> = still_unrouted
        .iter()
        .map(|(net, from, to)| diagnose_unrouted(&session, net, *from, *to, width, &copper))
        .collect();

    let mut unrouted: BTreeSet<String> = still_unrouted.into_iter().map(|(n, _, _)| n).collect();
    // A planed net with a pad that could not be stitched legally is reported
    // unrouted (some power pad never reached the plane), not silently dropped.
    for (net, pad_pt) in &stitch.failed_pads {
        unrouted.insert(net.clone());
        diagnostics.push(diagnose_stitch_failure(net, *pad_pt));
    }
    // A net both routed and (partially) failed is still incompletely connected —
    // keep it flagged so the caller knows to inspect it.
    routed.retain(|n| !unrouted.contains(n));

    // Routability: the fraction of attempted signal connections that closed.
    // (Plane-stitch failures are surfaced via unrouted_nets/diagnostics; the
    // score reflects the trace-routing problem, which is what "routable" means.)
    let routability = if total_conns == 0 {
        1.0
    } else {
        (total_conns.saturating_sub(signal_unrouted) as f64 / total_conns as f64).clamp(0.0, 1.0)
    };

    RouteAllResult {
        traces,
        vias,
        routed_nets: routed.into_iter().collect(),
        unrouted_nets: unrouted.into_iter().collect(),
        diagnostics,
        routability,
    }
}

/// Build a diagnostic for a signal connection the router could not close.
fn diagnose_unrouted(
    session: &RouteSession,
    net: &str,
    from: Vec2,
    to: Vec2,
    width: f64,
    copper: &[PcbLayer],
) -> UnroutedDiagnostic {
    let w = session.width_for(net, width);
    let clearance = session.clearance_for(net);
    // A corridor a few tracks wide around the direct path — the band a detour
    // would actually thread, so the blockers reported are the real obstruction.
    let chw = w / 2.0 + clearance + w * 3.0;
    let seg = CopperGeom::Segment {
        a: from,
        b: to,
        half_w: chw,
    };

    // Per-layer blocker census: net set + count, so we can pick the clearest layer.
    let mut per_layer: Vec<(PcbLayer, usize, BTreeSet<String>)> = Vec::new();
    let layers = if copper.is_empty() {
        LAYERS.to_vec()
    } else {
        copper.to_vec()
    };
    for &layer in &layers {
        let pr = session.probe(&seg, layer, net, clearance);
        let mut nets = BTreeSet::new();
        for b in &pr.blockers {
            nets.insert(b.net.clone());
        }
        per_layer.push((layer, pr.blockers.len(), nets));
    }

    // Distinct blocking nets across all layers, ordered by how many layers they
    // block on (most-blocking first), then by name for stability.
    let mut block_freq: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, nets) in &per_layer {
        for n in nets {
            *block_freq.entry(n.clone()).or_default() += 1;
        }
    }
    let mut blocking_nets: Vec<String> = block_freq.keys().cloned().collect();
    blocking_nets.sort_by(|a, b| block_freq[b].cmp(&block_freq[a]).then_with(|| a.cmp(b)));

    // The clearest layer (fewest blockers). When it has zero blockers the
    // connection should route there outright — suggest switching to it via a via.
    let clearest = per_layer.iter().min_by_key(|(_, c, _)| *c);
    let (suggested_layer, suggested_via, reason) = match clearest {
        Some((layer, 0, _)) => (
            Some(*layer),
            Some(Vec2::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0)),
            format!(
                "blocked on the tried layer(s) by {}; layer {:?} is clear — drop a via and route there",
                fmt_nets(&blocking_nets),
                layer
            ),
        ),
        Some((layer, count, _)) => (
            Some(*layer),
            None,
            format!(
                "congested on every layer (blocked by {}); least-congested is {:?} with {} blocker(s) — free space there, widen the channel, or add a routing layer",
                fmt_nets(&blocking_nets),
                layer,
                count
            ),
        ),
        None => (None, None, "no copper layers available to route on".to_string()),
    };

    UnroutedDiagnostic {
        net: net.to_string(),
        from,
        to,
        blocking_nets,
        region_min: Vec2::new(from.x.min(to.x) - chw, from.y.min(to.y) - chw),
        region_max: Vec2::new(from.x.max(to.x) + chw, from.y.max(to.y) + chw),
        suggested_layer,
        suggested_via,
        reason,
    }
}

/// Diagnostic for a power/ground pad that could not be stitched to its plane.
fn diagnose_stitch_failure(net: &str, pad_pt: Vec2) -> UnroutedDiagnostic {
    UnroutedDiagnostic {
        net: net.to_string(),
        from: pad_pt,
        to: pad_pt,
        blocking_nets: Vec::new(),
        region_min: Vec2::new(pad_pt.x - 1.0, pad_pt.y - 1.0),
        region_max: Vec2::new(pad_pt.x + 1.0, pad_pt.y + 1.0),
        suggested_layer: None,
        suggested_via: Some(pad_pt),
        reason: format!(
            "plane stitch failed: no clearance-legal via location at or near the {net} pad to reach its plane — move neighbouring copper or fan the pad out"
        ),
    }
}

/// Format a blocker-net list for a human-readable reason string.
fn fmt_nets(nets: &[String]) -> String {
    if nets.is_empty() {
        "board/edge constraints".to_string()
    } else {
        nets.join(", ")
    }
}

/// PathFinder negotiation feedback: every connection this round left unrouted
/// raises the history cost along the *corridor it wanted but could not get* — a
/// persistently-contested region accrues cost round over round. Next round
/// re-routes from scratch with these costs, so flexible nets (those with another
/// way around) bow out of the expensive band, while the stuck net — given first
/// pick by the priority ordering — claims it. History only adds cost, never
/// relaxes legality, so the board stays DRC-clean throughout.
fn deposit_congestion(
    cong: &mut Congestion,
    session: &RouteSession,
    pending: &[Conn],
    width: f64,
    step: f64,
) {
    for (net, from, to) in pending {
        let w = session.width_for(net, width);
        cong.add_corridor(*from, *to, w / 2.0 + CONTEST_HALF_WIDTH, step);
    }
}

/// One full routing pass over `rats`: greedy (longest-first) placement against a
/// fresh session, then bounded rip-up to convergence — all biased by the
/// congestion field `cong`. Returns the session, the placed connections, and the
/// connections still unrouted after rip-up (the negotiation loop's feedback and
/// the fan-out rescue's input). A flat `cong` makes this identical to the
/// historical greedy + rip-up router.
fn route_pass(
    pcb: &Pcb,
    width: f64,
    nets_filter: &[String],
    rats: &[RatsnestLine],
    cong: &Congestion,
    use_push_shove: bool,
) -> Pass {
    let mut session = RouteSession::from_pcb(pcb);
    let mut placed: Vec<Placed> = Vec::new();
    let mut unrouted_conns: Vec<Conn> = Vec::new();

    // Greedy pass: route each connection against the growing session.
    for line in rats {
        if !nets_filter.is_empty() && !nets_filter.iter().any(|n| n == &line.net) {
            continue;
        }
        match try_route(
            &mut session,
            pcb,
            width,
            &line.net,
            line.from,
            line.to,
            &placed,
            cong,
            use_push_shove,
        ) {
            Some(p) => placed.push(p),
            None => unrouted_conns.push((line.net.clone(), line.from, line.to)),
        }
    }

    // Rip-up passes: iterate to convergence (bounded). One pass rips the copper
    // blocking a failed connection, routes it, then re-routes the victims;
    // repeating lets short nets reclaim space from long ones and lets a victim
    // that failed in one round find a path once the board settles. Stop the
    // instant a round places nothing new, so a stuck board exits immediately.
    let mut pending = unrouted_conns;
    for _ in 0..MAX_RIPUP_ROUNDS {
        if pending.is_empty() {
            break;
        }
        let placed_before = placed.len();
        pending = ripup_pass(
            &mut session,
            pcb,
            width,
            &mut placed,
            pending,
            cong,
            use_push_shove,
        );
        if placed.len() <= placed_before {
            break;
        }
    }

    (session, placed, pending)
}

/// Try to route one connection on FCu then BCu against `session`. On success,
/// commits the copper (traces, plus transition vias for a back-layer route) to
/// `session` and returns the [`Placed`] record; otherwise returns `None`
/// without mutating the session. Every committed segment and via is probed —
/// there is no path here that commits illegal copper.
///
/// This is the negotiated-congestion workhorse: it lands a transition via *on*
/// each pad and abandons the back-layer route if either via won't clear, so the
/// greedy + rip-up passes stay byte-stable. Fine-pitch pads that can't take an
/// at-pad via are handled separately by [`try_route_fanout`], a monotonic
/// rescue that only runs on connections this leaves unrouted.
#[allow(clippy::too_many_arguments)]
fn try_route(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &[Placed],
    cong: &Congestion,
    use_push_shove: bool,
) -> Option<Placed> {
    // Net-class width if the net has one (wider power/ground), else the caller's
    // default. The same width drives the maze search, the committed copper, and
    // the reported trace.
    let w = session.width_for(net, width);
    let hw = w / 2.0;
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let clearance = session.clearance_for(net);
    let copper = copper_layers(pcb);

    for (li, &layer) in LAYERS.iter().enumerate() {
        // Maze A* first, biased away from contested regions by the history field.
        let r = route_net_maze_cong(
            session,
            &pcb.outline.vertices,
            layer,
            net,
            from,
            to,
            w,
            Some(cong),
        );
        let segments = if r.success && !r.segments.is_empty() {
            r.segments
        } else if use_push_shove {
            // Fallback: the continuous-space push-and-shove router can find a
            // taut diagonal the grid maze quantized away. Its route is re-probed
            // against the oracle (and clipped to the board) before we trust it,
            // so it never relaxes the DRC-clean invariant.
            match try_push_shove(session, pcb, layer, net, from, to, w) {
                Some(segs) => segs,
                None => continue,
            }
        } else {
            continue;
        };

        // A back-layer route needs a transition via at each endpoint. Probe
        // each on every copper layer it spans before committing; reuse a
        // same-net via already dropped at a shared pad rather than stacking a
        // coincident drill.
        let needs_via = li > 0;
        let mut new_vias: Vec<Vec2> = Vec::new();
        if needs_via {
            let mut ok = true;
            for &p in &[from, to] {
                let reused = placed
                    .iter()
                    .filter(|pl| pl.net == net)
                    .flat_map(|pl| pl.via_pts.iter())
                    .any(|&vp| dist(vp, p) < 0.05);
                if reused || new_vias.iter().any(|&q| dist(q, p) < 0.05) {
                    continue;
                }
                let disc = CopperGeom::Disc {
                    center: p,
                    r: via_r,
                };
                let legal = copper
                    .iter()
                    .all(|&l| session.probe(&disc, l, net, clearance).legal);
                if !legal {
                    ok = false;
                    break;
                }
                new_vias.push(p);
            }
            if !ok {
                continue;
            }
        }

        let mut spans = Vec::new();
        for (a, b) in &segments {
            spans.push(commit_seg(session, net, *a, *b, hw, layer));
        }
        for &p in &new_vias {
            commit_via(session, net, p, via_r, &copper, &mut spans);
        }
        return Some(Placed {
            net: net.to_string(),
            from,
            to,
            layer,
            width: w,
            segments,
            stubs: Vec::new(),
            via_pts: new_vias,
            spans,
        });
    }
    None
}

/// Push-and-shove fallback: route `net` on `layer` with the visibility-graph
/// router over the session's other-net copper (as coarse AABB obstacles), then
/// validate every produced segment against the exact oracle and the board
/// outline. Returns the segments only if the whole route is clearance-legal and
/// in-board; otherwise `None` (committing nothing). Because the result is
/// re-probed, this can only ever close *more* nets — never ship illegal copper.
fn try_push_shove(
    session: &RouteSession,
    pcb: &Pcb,
    layer: PcbLayer,
    net: &str,
    from: Vec2,
    to: Vec2,
    w: f64,
) -> Option<Vec<(Vec2, Vec2)>> {
    let clearance = session.clearance_for(net);
    let hw = w / 2.0;

    // Pull obstacles from a margin around the connection so the visibility graph
    // stays small and local (it is O(corners²)).
    const MARGIN: f64 = 12.0;
    // Skip push-shove past this many local obstacles — the visibility graph is
    // O(corners²), and the maze already handles dense regions; this fallback is
    // for sparse pockets where a taut diagonal beats the grid.
    const MAX_OBSTACLES: usize = 60;
    let lo = [from.x.min(to.x) - MARGIN, from.y.min(to.y) - MARGIN];
    let hi = [from.x.max(to.x) + MARGIN, from.y.max(to.y) + MARGIN];
    let obstacles = session.obstacles_in(layer, net, lo, hi);
    if obstacles.len() > MAX_OBSTACLES {
        return None;
    }

    let mut router = PushShoveRouter::new(w, clearance);
    for (min, max) in obstacles {
        router.add_obstacle(Obstacle::new(min, max));
    }
    let r = router.route_net(net, from, to);
    if !r.success || r.segments.is_empty() {
        return None;
    }

    // Validate: every segment must clear all other-net copper (push-shove used
    // coarse inflated boxes, not the exact geometry) and stay inside the board.
    let bounded = pcb.outline.vertices.len() >= 3;
    for (a, b) in &r.segments {
        if bounded
            && (!point_in_polygon(*a, &pcb.outline.vertices)
                || !point_in_polygon(*b, &pcb.outline.vertices))
        {
            return None;
        }
        let legal = session
            .probe(
                &CopperGeom::Segment {
                    a: *a,
                    b: *b,
                    half_w: hw,
                },
                layer,
                net,
                clearance,
            )
            .legal;
        if !legal {
            return None;
        }
    }
    Some(r.segments)
}

/// Fan-out rescue for a connection [`try_route`] could not place — usually
/// because an endpoint is a fine-pitch pad that can't be escaped on its own
/// layer (issue #289 part 1).
///
/// Escapes each endpoint down to BCu: drops a transition via on the pad when it
/// clears, or fans a short dog-bone stub + via radially out of the pad when an
/// at-pad via would short a neighbour, then routes the whole connection on BCu
/// between the two escape vias. Escapes are committed up front (so the second
/// escape and the maze both avoid the first) and rolled back wholesale if the
/// maze can't connect them — so it either commits a fully clearance-legal route
/// or mutates nothing. Because it only ever *adds* copper for an
/// already-unrouted net, running it never disturbs the routes rip-up settled.
fn try_route_fanout(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &[Placed],
) -> Option<Placed> {
    let w = session.width_for(net, width);
    let hw = w / 2.0;
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let clearance = session.clearance_for(net);
    let copper = copper_layers(pcb);

    let mut spans: Vec<SpanId> = Vec::new();
    let mut via_pts: Vec<Vec2> = Vec::new();
    let mut stubs: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();

    let ea = match escape_endpoint(
        session,
        pcb,
        net,
        from,
        PcbLayer::FCu,
        w,
        clearance,
        via_r,
        &copper,
        placed,
        &via_pts,
        &mut spans,
        false,
    ) {
        Some(e) => e,
        None => {
            rollback(session, &spans);
            return None;
        }
    };
    if let Some(v) = ea.via {
        via_pts.push(v);
    }
    if let Some((a, b)) = ea.stub {
        stubs.push((a, b, PcbLayer::FCu));
    }

    let eb = match escape_endpoint(
        session,
        pcb,
        net,
        to,
        PcbLayer::FCu,
        w,
        clearance,
        via_r,
        &copper,
        placed,
        &via_pts,
        &mut spans,
        false,
    ) {
        Some(e) => e,
        None => {
            rollback(session, &spans);
            return None;
        }
    };
    if let Some(v) = eb.via {
        via_pts.push(v);
    }
    if let Some((a, b)) = eb.stub {
        stubs.push((a, b, PcbLayer::FCu));
    }

    let rb = route_net_maze(
        session,
        &pcb.outline.vertices,
        PcbLayer::BCu,
        net,
        ea.terminal,
        eb.terminal,
        w,
    );
    if !rb.success || rb.segments.is_empty() {
        rollback(session, &spans);
        return None;
    }
    for (a, b) in &rb.segments {
        spans.push(commit_seg(session, net, *a, *b, hw, PcbLayer::BCu));
    }
    Some(Placed {
        net: net.to_string(),
        from,
        to,
        layer: PcbLayer::BCu,
        width: w,
        segments: rb.segments,
        stubs,
        via_pts,
        spans,
    })
}

/// An escape (transition / fan-out) via for one pad endpoint.
struct Escape {
    /// Routing terminal on the escape layer (the via center).
    terminal: Vec2,
    /// Dog-bone stub (pad → via) on the pad's layer, if the via was fanned out.
    stub: Option<(Vec2, Vec2)>,
    /// The newly placed via center (`None` when a coincident same-net via was
    /// reused instead of stacking a drill).
    via: Option<Vec2>,
}

/// Place a transition/escape via for `pad_pt` and connect it to the pad.
///
/// Tries the via *at* the pad first — the normal, non-fine-pitch case where the
/// transition via simply lands on the pad. If an at-pad via would short a
/// neighbour (the fine-pitch case: at 0.5 mm pitch a via barely fits in a pad
/// and never clears the one beside it), it fans a short stub + via radially out
/// of the pad until a legal dog-bone is found — the standard QFP/QFN escape.
///
/// The via is probed on **every** copper layer in the stackup (not just the two
/// outer ones) so a stitch/escape via that passes through an inner signal layer
/// is still clearance-legal there. Commits the stub and via to `session`,
/// recording their spans in `spans`; returns `None` (committing nothing) when
/// the pad cannot be escaped.
///
/// `force_fanout` skips the at-pad via entirely and goes straight to the
/// dog-bone ring search. Set it for sub-0.65 mm-pitch pads: a 0.6 mm via can't
/// physically land between 0.5 mm-pitch QFP/QFN pins, so the stitch must escape
/// into the fan-out even when an at-pad via *would* probe clearance-legal (e.g.
/// a fine-pitch pad whose neighbours share its net).
#[allow(clippy::too_many_arguments)]
fn escape_endpoint(
    session: &mut RouteSession,
    pcb: &Pcb,
    net: &str,
    pad_pt: Vec2,
    stub_layer: PcbLayer,
    w: f64,
    clearance: f64,
    via_r: f64,
    copper: &[PcbLayer],
    placed: &[Placed],
    extra_vias: &[Vec2],
    spans: &mut Vec<SpanId>,
    force_fanout: bool,
) -> Option<Escape> {
    let hw = w / 2.0;

    let via_legal = |session: &RouteSession, p: Vec2| -> bool {
        let disc = CopperGeom::Disc {
            center: p,
            r: via_r,
        };
        copper
            .iter()
            .all(|&l| session.probe(&disc, l, net, clearance).legal)
    };
    // A coincident same-net via already on the board — reuse it rather than
    // stacking a second drill at the same spot.
    let reused = |p: Vec2| -> bool {
        placed
            .iter()
            .filter(|pl| pl.net == net)
            .flat_map(|pl| pl.via_pts.iter())
            .chain(extra_vias.iter())
            .any(|&vp| dist(vp, p) < 0.05)
    };

    // 1) Via straight on the pad — unless the caller forces a fan-out because
    //    the pad is too fine-pitch to take an at-pad drill.
    if !force_fanout {
        if reused(pad_pt) {
            return Some(Escape {
                terminal: pad_pt,
                stub: None,
                via: None,
            });
        }
        if via_legal(session, pad_pt) {
            commit_via(session, net, pad_pt, via_r, copper, spans);
            return Some(Escape {
                terminal: pad_pt,
                stub: None,
                via: Some(pad_pt),
            });
        }
    }

    // 2) Dog-bone: fan a stub + via radially out of the pad.
    let bounded = pcb.outline.vertices.len() >= 3;
    let stub_legal = |session: &RouteSession, a: Vec2, b: Vec2| -> bool {
        session
            .probe(
                &CopperGeom::Segment { a, b, half_w: hw },
                stub_layer,
                net,
                clearance,
            )
            .legal
    };
    let d0 = via_r + clearance + hw;
    let step = via_r.max(0.2);
    for ring in 0..ESCAPE_RINGS {
        let d = d0 + ring as f64 * step;
        for k in 0..ESCAPE_DIRS {
            let ang = std::f64::consts::TAU * k as f64 / ESCAPE_DIRS as f64;
            let vp = Vec2::new(pad_pt.x + d * ang.cos(), pad_pt.y + d * ang.sin());
            if bounded && !point_in_polygon(vp, &pcb.outline.vertices) {
                continue;
            }
            if !stub_legal(session, pad_pt, vp) {
                continue;
            }
            if reused(vp) {
                let s = commit_stub(session, net, pad_pt, vp, hw, stub_layer);
                spans.push(s);
                return Some(Escape {
                    terminal: vp,
                    stub: Some((pad_pt, vp)),
                    via: None,
                });
            }
            if via_legal(session, vp) {
                let s = commit_stub(session, net, pad_pt, vp, hw, stub_layer);
                spans.push(s);
                commit_via(session, net, vp, via_r, copper, spans);
                return Some(Escape {
                    terminal: vp,
                    stub: Some((pad_pt, vp)),
                    via: Some(vp),
                });
            }
        }
    }
    None
}

/// Escape-via fan: directions tried per ring (~22.5° apart).
const ESCAPE_DIRS: usize = 16;
/// Escape-via fan: how many increasing-radius rings to search before giving up.
const ESCAPE_RINGS: usize = 10;
/// Pads at or below this center-to-center pitch (mm) are too fine to take an
/// at-pad stitching/escape via — the via must fan out into a dog-bone instead.
/// 0.65 mm sits just above the 0.5 mm-pitch QFP/QFN floor and below 0.8 mm BGA.
const FINE_PITCH_MM: f64 = 0.65;

/// Commit a trace segment span on `layer`, returning its [`SpanId`].
fn commit_seg(
    session: &mut RouteSession,
    net: &str,
    a: Vec2,
    b: Vec2,
    hw: f64,
    layer: PcbLayer,
) -> SpanId {
    session.commit(CopperElement {
        min: [a.x.min(b.x) - hw, a.y.min(b.y) - hw],
        max: [a.x.max(b.x) + hw, a.y.max(b.y) + hw],
        net: net.to_string(),
        layer,
        geom: CopperGeom::Segment { a, b, half_w: hw },
    })
}

/// Commit a dog-bone stub span (a trace on the pad's layer).
fn commit_stub(
    session: &mut RouteSession,
    net: &str,
    a: Vec2,
    b: Vec2,
    hw: f64,
    layer: PcbLayer,
) -> SpanId {
    commit_seg(session, net, a, b, hw, layer)
}

/// Commit a via as a disc on every copper layer it spans (a through via).
fn commit_via(
    session: &mut RouteSession,
    net: &str,
    p: Vec2,
    via_r: f64,
    copper: &[PcbLayer],
    spans: &mut Vec<SpanId>,
) {
    for &l in copper {
        spans.push(session.commit(CopperElement {
            min: [p.x - via_r, p.y - via_r],
            max: [p.x + via_r, p.y + via_r],
            net: net.to_string(),
            layer: l,
            geom: CopperGeom::Disc {
                center: p,
                r: via_r,
            },
        }));
    }
}

/// Rip a set of just-committed spans back out (escape rollback on a failed maze).
fn rollback(session: &mut RouteSession, spans: &[SpanId]) {
    for &s in spans {
        session.remove(s);
    }
}

/// Copper layers present in the stackup, top → bottom (FCu/BCu fallback).
fn copper_layers(pcb: &Pcb) -> Vec<PcbLayer> {
    let v: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    if v.is_empty() {
        vec![PcbLayer::FCu, PcbLayer::BCu]
    } else {
        v
    }
}

/// Net → the copper layers it owns a (filled) plane/zone on.
///
/// `PcbLayer` isn't `Ord`, so the per-net layers are a deduped `Vec` rather than
/// a set — fine, since a net rarely owns more than a couple of planes.
fn plane_layers(pcb: &Pcb) -> BTreeMap<String, Vec<PcbLayer>> {
    let mut m: BTreeMap<String, Vec<PcbLayer>> = BTreeMap::new();
    for z in &pcb.zones {
        if z.net.is_empty() || z.outline.len() < 3 {
            continue;
        }
        let entry = m.entry(z.net.clone()).or_default();
        if !entry.contains(&z.layer) {
            entry.push(z.layer);
        }
    }
    m
}

/// Plane stitching output: dog-bone stubs, stitching vias, and net bookkeeping.
struct Stitch {
    /// Stub segments (pad → via) with their net and pad layer.
    stubs: Vec<(Vec2, Vec2, String, PcbLayer)>,
    /// Stitching via centers with their net.
    vias: Vec<(Vec2, String)>,
    /// Nets that got at least one pad onto their plane (via or direct flood).
    nets: BTreeSet<String>,
    /// Each (net, pad position) that could not be stitched legally — both the net
    /// (for `unrouted_nets`) and where (for the diagnostic).
    failed_pads: Vec<(String, Vec2)>,
}

/// Stitch every pad of a planed net down to that net's plane(s) (issue #289
/// part 2).
///
/// For each pad whose net owns a zone on a layer the pad is *not* already on,
/// drop a through via (FCu..BCu) at — or dog-boned just outside — the pad: a
/// same-net via floods into the net's plane (connecting the pad), and the
/// copper pour voids around it on every other-net plane it passes through, so
/// the result is clearance-legal by construction. A pad already sitting on one
/// of its planes floods directly and needs no via.
fn stitch_planes(
    session: &mut RouteSession,
    pcb: &Pcb,
    planes: &BTreeMap<String, Vec<PcbLayer>>,
    nets_filter: &[String],
    placed: &[Placed],
    width: f64,
) -> Stitch {
    let mut out = Stitch {
        stubs: Vec::new(),
        vias: Vec::new(),
        nets: BTreeSet::new(),
        failed_pads: Vec::new(),
    };
    if planes.is_empty() {
        return out;
    }
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let copper = copper_layers(pcb);

    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let Some(net) = pad.net.as_ref().filter(|n| !n.is_empty()) else {
                continue;
            };
            let Some(layers) = planes.get(net) else {
                continue;
            };
            if !nets_filter.is_empty() && !nets_filter.iter().any(|n| n == net) {
                continue;
            }
            // A pad already on one of its planes floods straight into it.
            if pad.layers.iter().any(|l| layers.contains(l)) {
                out.nets.insert(net.clone());
                continue;
            }
            let Some(pad_layer) = pad.layers.iter().copied().find(|l| l.is_copper()) else {
                continue;
            };
            let pad_pt = crate::geometry::pad_world_position(fp, pad);
            // A fine-pitch pad can't take an at-pad drill — force the dog-bone.
            // Pitch is the nearest neighbour's spacing in the footprint's own
            // frame (rotation/translation preserve relative distances).
            let pitch = fp
                .pads
                .iter()
                .filter(|o| !std::ptr::eq(*o, pad))
                .map(|o| dist(pad.position, o.position))
                .fold(f64::INFINITY, f64::min);
            let fine_pitch = pitch < FINE_PITCH_MM;
            let clearance = session.clearance_for(net);
            let w = session.width_for(net, width);
            // Reuse a coincident same-net stitch via dropped for an earlier pad.
            let extra: Vec<Vec2> = out
                .vias
                .iter()
                .filter(|(_, n)| n == net)
                .map(|(p, _)| *p)
                .collect();
            let mut spans: Vec<SpanId> = Vec::new();
            match escape_endpoint(
                session, pcb, net, pad_pt, pad_layer, w, clearance, via_r, &copper, placed, &extra,
                &mut spans, fine_pitch,
            ) {
                Some(e) => {
                    if let Some((a, b)) = e.stub {
                        out.stubs.push((a, b, net.clone(), pad_layer));
                    }
                    if let Some(v) = e.via {
                        out.vias.push((v, net.clone()));
                    }
                    out.nets.insert(net.clone());
                    // spans stay committed — the stitching copper is part of the board.
                }
                None => {
                    rollback(session, &spans);
                    out.failed_pads.push((net.clone(), pad_pt));
                }
            }
        }
    }
    out
}

/// Bounded single-level rip-up-and-reroute.
///
/// For each connection the greedy pass couldn't route, find the other-net
/// copper directly in its way (the blockers `probe` reports along the direct
/// path), rip those connections out of the session, route the failed
/// connection, then re-route the ripped victims so they avoid the new copper.
/// A victim that can no longer be routed becomes unrouted in its place — the
/// DRC-clean invariant always holds because every (re)route goes through
/// [`try_route`]. Returns the connections still unrouted after the pass.
#[allow(clippy::too_many_arguments)]
fn ripup_pass(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    placed: &mut Vec<Placed>,
    unrouted: Vec<Conn>,
    cong: &Congestion,
    use_push_shove: bool,
) -> Vec<Conn> {
    let hw = width / 2.0;
    let mut still: Vec<Conn> = Vec::new();

    for (net, from, to) in unrouted {
        // Other-net copper crossing a CORRIDOR around the direct path — not the
        // hairline segment. The maze router detours around copper, so the
        // connections worth ripping are everything in the band a detour would
        // thread; a hairline probe finds only what sits exactly on the straight
        // line and leaves congested-but-offset nets with an empty victim set
        // (abandoned without trying). A few-trace-wide corridor surfaces them.
        let clearance = session.clearance_for(&net);
        let corridor_hw = hw + clearance + width * 3.0;
        let seg = CopperGeom::Segment {
            a: from,
            b: to,
            half_w: corridor_hw,
        };
        let mut blocker_spans: HashSet<SpanId> = HashSet::new();
        for &layer in &LAYERS {
            for b in session.probe(&seg, layer, &net, clearance).blockers {
                blocker_spans.insert(b.span);
            }
        }

        // Which placed connections own those blocking spans (other nets only).
        let victim_set: HashSet<usize> = placed
            .iter()
            .enumerate()
            .filter(|(_, p)| p.net != net && p.spans.iter().any(|s| blocker_spans.contains(s)))
            .map(|(i, _)| i)
            .collect();
        if victim_set.is_empty() {
            still.push((net, from, to));
            continue;
        }

        // Rip the victims out of `placed` and the session.
        let mut victims = Vec::new();
        let mut kept = Vec::new();
        for (i, p) in std::mem::take(placed).into_iter().enumerate() {
            if victim_set.contains(&i) {
                victims.push(p);
            } else {
                kept.push(p);
            }
        }
        *placed = kept;
        for v in &victims {
            for &s in &v.spans {
                session.remove(s);
            }
        }

        // Route the previously-failed connection into the freed space.
        let routed_target = try_route(
            session,
            pcb,
            width,
            &net,
            from,
            to,
            placed,
            cong,
            use_push_shove,
        );
        if let Some(p) = routed_target {
            placed.push(p);
        } else {
            still.push((net, from, to));
        }

        // Re-route every victim; one that can't be placed becomes unrouted.
        for v in victims {
            match try_route(
                session,
                pcb,
                width,
                &v.net,
                v.from,
                v.to,
                placed,
                cong,
                use_push_shove,
            ) {
                Some(p) => placed.push(p),
                None => still.push((v.net, v.from, v.to)),
            }
        }
    }

    still
}

/// Synthesize a netlist from pad net assignments for ratsnest computation.
fn netlist_from_pads(pcb: &Pcb) -> Netlist {
    let mut map: BTreeMap<String, Vec<NetConnection>> = BTreeMap::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(net) = &pad.net {
                map.entry(net.clone()).or_default().push(NetConnection {
                    component_ref: fp.reference.clone(),
                    pin_number: pad.number.clone(),
                });
            }
        }
    }
    Netlist {
        nets: map
            .into_iter()
            .map(|(name, connections)| NetlistNet { name, connections })
            .collect(),
    }
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::check_drc;
    use vcad_ir::ecad::*;

    fn pad(num: &str, x: f64, y: f64, net: &str) -> Pad {
        Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 1.0,
                height: 1.0,
            },
            position: Vec2::new(x, y),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        }
    }

    fn fp(reference: &str, x: f64, y: f64, pads: Vec<Pad>) -> Footprint {
        Footprint {
            reference: reference.into(),
            value: "x".into(),
            footprint_name: "test".into(),
            position: Vec2::new(x, y),
            rotation: 0.0,
            front: true,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    fn board(footprints: Vec<Footprint>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".into()),
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
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
            footprints,
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    /// Apply the router's output to the board (as the MCP tool does).
    fn apply(pcb: &mut Pcb, r: &RouteAllResult) {
        for t in &r.traces {
            pcb.traces.push(Trace {
                start: t.start,
                end: t.end,
                width: t.width,
                layer: t.layer,
                net: t.net.clone(),
            });
        }
        for v in &r.vias {
            pcb.vias.push(Via {
                position: v.position,
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: v.net.clone(),
            });
        }
    }

    #[test]
    fn routes_two_crossing_nets_drc_clean() {
        // Two nets whose straight connections overlap on one layer; the router
        // must use the back layer for one of them. The applied board must be
        // free of shorts and clearance violations.
        let pcb0 = board(vec![
            fp("R1", 10.0, 10.0, vec![pad("1", 0.0, 0.0, "A")]),
            fp("R2", 40.0, 10.0, vec![pad("1", 0.0, 0.0, "A")]),
            fp("R3", 10.0, 20.0, vec![pad("1", 0.0, 0.0, "B")]),
            fp("R4", 40.0, 20.0, vec![pad("1", 0.0, 0.0, "B")]),
            // Force a crossing: A also reaches a pad at bottom-right, B top-right.
            fp("R5", 40.0, 20.0, vec![pad("1", 0.0, 5.0, "A")]),
            fp("R6", 40.0, 10.0, vec![pad("1", 0.0, -5.0, "B")]),
        ]);
        let r = route_all(&pcb0, 0.25, &[]);
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);

        let viols = check_drc(&pcb);
        let bad: Vec<_> = viols
            .iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .collect();
        assert!(
            bad.is_empty(),
            "router output must be short/clearance clean, got: {:?}",
            bad.iter().map(|v| &v.message).collect::<Vec<_>>()
        );
        assert!(!r.traces.is_empty(), "should have routed something");
    }

    #[test]
    fn never_emits_illegal_copper_even_when_unroutable() {
        // A board where a net genuinely cannot be routed still produces zero
        // short/clearance violations — it reports the net unrouted instead.
        let pcb0 = board(vec![
            fp("R1", 5.0, 15.0, vec![pad("1", 0.0, 0.0, "X")]),
            fp("R2", 45.0, 15.0, vec![pad("1", 0.0, 0.0, "X")]),
        ]);
        let r = route_all(&pcb0, 0.25, &[]);
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);
        let bad = check_drc(&pcb)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count();
        assert_eq!(bad, 0, "must never emit shorting copper");
    }

    #[test]
    fn congested_crossing_nets_all_route() {
        // N nets whose connections all cross through the board center — a
        // rip-up stress test. The greedy pass plus a single rip-up round leaves
        // several unrouted; iterative rip-up with corridor blocker detection
        // should place them all, DRC-clean.
        let n = 16usize;
        let mut fps = Vec::new();
        for i in 0..n {
            let net = format!("N{i}");
            // Top row left→right, bottom row right→left: every net crosses center.
            fps.push(fp(
                &format!("T{i}"),
                4.0 + 2.8 * i as f64,
                24.0,
                vec![pad("1", 0.0, 0.0, &net)],
            ));
            fps.push(fp(
                &format!("B{i}"),
                4.0 + 2.8 * (n - 1 - i) as f64,
                6.0,
                vec![pad("1", 0.0, 0.0, &net)],
            ));
        }
        let pcb0 = board(fps);
        let r = route_all(&pcb0, 0.25, &[]);
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);

        let bad = check_drc(&pcb)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count();
        assert_eq!(bad, 0, "router output must be short/clearance clean");
        assert_eq!(
            r.routed_nets.len(),
            n,
            "expected all {n} nets routed, unrouted: {:?}",
            r.unrouted_nets
        );
    }

    #[test]
    fn power_net_routes_at_its_class_width() {
        let mut pcb = board(vec![
            fp("R1", 10.0, 15.0, vec![pad("1", 0.0, 0.0, "PWR")]),
            fp("R2", 40.0, 15.0, vec![pad("1", 0.0, 0.0, "PWR")]),
        ]);
        // Put PWR in a wide net class.
        pcb.rules.class_rules.push(NetClassRules {
            name: "Power".into(),
            trace_width: 0.6,
            clearance: 0.2,
            via_diameter: 0.8,
            via_drill: 0.4,
            diff_pair_gap: None,
            diff_pair_width: None,
        });
        pcb.rules
            .net_class_assignments
            .insert("Power".into(), vec!["PWR".into()]);

        // Default width 0.25, but PWR's class says 0.6 — every PWR trace is wide.
        let r = route_all(&pcb, 0.25, &[]);
        assert!(!r.traces.is_empty());
        assert!(
            r.traces.iter().all(|t| (t.width - 0.6).abs() < 1e-9),
            "PWR should route at its 0.6mm class width, got {:?}",
            r.traces.iter().map(|t| t.width).collect::<Vec<_>>()
        );
    }

    /// A fine-pitch pad whose at-pad transition via would short its neighbour,
    /// behind an FCu wall that forces the back layer: plain routing leaves it
    /// unrouted, the fan-out dog-bone rescues it (issue #289 part 1).
    fn small_pad(num: &str, x: f64, y: f64, net: &str) -> Pad {
        Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 0.25,
                height: 0.25,
            },
            position: Vec2::new(x, y),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        }
    }

    #[test]
    fn fanout_via_escapes_a_fine_pitch_pad() {
        // SIG's source pad sits 0.5 mm from a different-net pad, so a via landing
        // *on* it overlaps the neighbour — the at-pad transition via is illegal.
        // A GND wall down the board centre blocks every FCu route, so SIG can
        // only reach its far pad on BCu. With no way to drop a legal via at the
        // pad, plain routing abandons it; the dog-bone fan-out offsets the via
        // away from the neighbour and routes on the back layer.
        let pcb0 = board(vec![
            fp("U1", 10.0, 15.0, vec![small_pad("1", 0.0, 0.0, "SIG")]),
            fp("U1b", 10.5, 15.0, vec![small_pad("1", 0.0, 0.0, "BLK")]),
            fp("U2", 40.0, 15.0, vec![small_pad("1", 0.0, 0.0, "SIG")]),
            // Full-height GND wall on FCu at x=25 — no FCu route can cross it.
            fp(
                "W",
                25.0,
                0.0,
                vec![Pad {
                    number: "1".into(),
                    pad_type: PadType::SMD,
                    shape: PadShape::Rect {
                        width: 0.4,
                        height: 30.0,
                    },
                    position: Vec2::new(0.0, 15.0),
                    rotation: 0.0,
                    drill: None,
                    net: Some("GND".into()),
                    layers: vec![PcbLayer::FCu],
                }],
            ),
        ]);

        let r = route_all(&pcb0, 0.25, &[]);
        assert!(
            r.routed_nets.iter().any(|n| n == "SIG"),
            "SIG must route via fan-out, unrouted: {:?}",
            r.unrouted_nets
        );
        // The rescue is a back-layer route reached through a via.
        assert!(
            !r.vias.is_empty(),
            "fan-out must place at least one transition via"
        );
        // Proof it dog-boned: a SIG via sits *off* the trapped pad (10,15), not
        // on it — an at-pad via there is illegal, so this can only be the escape.
        let trapped = Vec2::new(10.0, 15.0);
        assert!(
            r.vias
                .iter()
                .filter(|v| v.net == "SIG")
                .any(|v| dist(v.position, trapped) > 0.1),
            "expected a SIG escape via offset from the trapped pad, got {:?}",
            r.vias
                .iter()
                .filter(|v| v.net == "SIG")
                .map(|v| v.position)
                .collect::<Vec<_>>()
        );

        // And the applied board is short/clearance clean.
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);
        let bad = check_drc(&pcb)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count();
        assert_eq!(bad, 0, "fan-out route must be short/clearance clean");
    }

    /// A 4-layer board with inner GND/VCC planes, SMD power pads on FCu.
    fn board4(footprints: Vec<Footprint>, zones: Vec<Zone>) -> Pcb {
        let mut pcb = board(footprints);
        pcb.stackup = LayerStackup {
            layers: vec![
                StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(0.2),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                },
                StackupLayer {
                    layer: PcbLayer::In1Cu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.1),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                },
                StackupLayer {
                    layer: PcbLayer::In2Cu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(0.2),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                },
                StackupLayer {
                    layer: PcbLayer::BCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                },
            ],
        };
        pcb.zones = zones;
        pcb
    }

    fn plane(net: &str, layer: PcbLayer) -> Zone {
        Zone {
            // Whole-board pour (matches add_zone fill_board).
            outline: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(50.0, 0.0),
                Vec2::new(50.0, 30.0),
                Vec2::new(0.0, 30.0),
            ],
            holes: vec![],
            net: net.into(),
            layer,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: None,
            thermal_spoke_width: None,
            priority: 0,
        }
    }

    #[test]
    fn stitches_power_pads_to_inner_planes() {
        // GND plane on In1Cu, V3V3 plane on In2Cu; SMD power pads on FCu. Without
        // stitching the planes are dead copper and every power pad is its own
        // disjoint group. route_all must drop a via from each pad to its plane
        // (issue #289 part 2) so the net reads connected — and never route a
        // star of power traces on a signal layer.
        let pcb0 = board4(
            vec![
                fp("U1", 8.0, 8.0, vec![pad("1", 0.0, 0.0, "GND")]),
                fp("U2", 25.0, 8.0, vec![pad("1", 0.0, 0.0, "GND")]),
                fp("U3", 42.0, 22.0, vec![pad("1", 0.0, 0.0, "GND")]),
                fp("C1", 14.0, 22.0, vec![pad("1", 0.0, 0.0, "V3V3")]),
                fp("C2", 36.0, 8.0, vec![pad("1", 0.0, 0.0, "V3V3")]),
            ],
            vec![
                plane("GND", PcbLayer::In1Cu),
                plane("V3V3", PcbLayer::In2Cu),
            ],
        );

        let r = route_all(&pcb0, 0.25, &[]);

        // Both planed nets are reported routed, and every power pad got a via.
        assert!(
            r.routed_nets.iter().any(|n| n == "GND"),
            "GND unrouted: {:?}",
            r.unrouted_nets
        );
        assert!(
            r.routed_nets.iter().any(|n| n == "V3V3"),
            "V3V3 unrouted: {:?}",
            r.unrouted_nets
        );
        assert_eq!(
            r.vias.iter().filter(|v| v.net == "GND").count(),
            3,
            "one stitching via per GND pad"
        );
        assert_eq!(
            r.vias.iter().filter(|v| v.net == "V3V3").count(),
            2,
            "one stitching via per V3V3 pad"
        );
        // Power nets must NOT consume a signal layer with a star of traces.
        assert!(
            r.traces.iter().all(|t| t.net != "GND" && t.net != "V3V3"),
            "planed nets should be stitched, not trace-routed"
        );

        // The applied board: planes now connect their pads, no shorts, no
        // unrouted GND/V3V3.
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);
        let viols = check_drc(&pcb);
        let bad = viols
            .iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count();
        assert_eq!(bad, 0, "stitched board must be short/clearance clean");
        let unconnected: Vec<_> = viols
            .iter()
            .filter(|v| matches!(v.rule, crate::drc::DrcRuleType::UnconnectedNet))
            .filter(|v| v.message.contains("GND") || v.message.contains("V3V3"))
            .collect();
        assert!(
            unconnected.is_empty(),
            "planes must connect their pads, still unconnected: {:?}",
            unconnected.iter().map(|v| &v.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn stitches_fine_pitch_pad_with_escape_fanout_via() {
        // Three 0.5 mm-pitch VCC pads over a VCC plane on In1Cu. Their neighbours
        // share the net, so an *at-pad* stitching via would probe clearance-legal
        // — yet a 0.6 mm via can't physically sit on a 0.5 mm-pitch pad. Stitching
        // must therefore escape into a dog-bone: every via lands OFF its pad, and
        // the plane still connects every pad.
        let pcb0 = board4(
            vec![fp(
                "U1",
                25.0,
                15.0,
                vec![
                    small_pad("1", -0.5, 0.0, "VCC"),
                    small_pad("2", 0.0, 0.0, "VCC"),
                    small_pad("3", 0.5, 0.0, "VCC"),
                ],
            )],
            vec![plane("VCC", PcbLayer::In1Cu)],
        );

        let r = route_all(&pcb0, 0.25, &[]);

        let vias: Vec<_> = r.vias.iter().filter(|v| v.net == "VCC").collect();
        assert_eq!(
            vias.len(),
            3,
            "one stitch via per fine-pitch VCC pad, got {:?}",
            vias.iter().map(|v| v.position).collect::<Vec<_>>()
        );
        // Each stitch via must sit OFF every pad — the escape-in-fanout, not on-pad.
        let pads = [
            Vec2::new(24.5, 15.0),
            Vec2::new(25.0, 15.0),
            Vec2::new(25.5, 15.0),
        ];
        for v in &vias {
            assert!(
                pads.iter().all(|p| dist(v.position, *p) > 0.1),
                "fine-pitch stitch via must dog-bone off the pad, got on-pad {:?}",
                v.position
            );
        }
        // Dog-bone stub traces (pad → via) are emitted on net VCC.
        assert!(
            r.traces.iter().any(|t| t.net == "VCC"),
            "expected dog-bone stub traces for the fanned VCC stitches"
        );

        // Applied board: short/clearance clean and every VCC pad is stitched.
        let mut pcb = pcb0.clone();
        apply(&mut pcb, &r);
        let viols = check_drc(&pcb);
        let bad = viols
            .iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count();
        assert_eq!(
            bad, 0,
            "fine-pitch stitched board must be short/clearance clean"
        );
        let unstitched = viols
            .iter()
            .filter(|v| {
                v.rule == crate::drc::DrcRuleType::UnstitchedPad && v.message.contains("VCC")
            })
            .count();
        assert_eq!(unstitched, 0, "every fine-pitch VCC pad must be stitched");
    }

    /// A square board of the given side (mm), 2-layer, default rules.
    fn square_board(side: f64, footprints: Vec<Footprint>) -> Pcb {
        let mut pcb = board(footprints);
        pcb.outline.vertices = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(side, 0.0),
            Vec2::new(side, side),
            Vec2::new(0.0, side),
        ];
        pcb
    }

    /// A dense crossing bus: `n` nets from a left column to a right column whose
    /// order is reversed, so every net crosses every other near the centre. With
    /// the columns packed tight on a small board the centre saturates — greedy +
    /// rip-up strands several connections, and only negotiated congestion (which
    /// prices the contested centre up so flexible nets bow out to the perimeter)
    /// reclaims them. Returns the board plus the net names.
    fn crossing_bus(side: f64, n: usize, pitch: f64) -> Pcb {
        let mut fps = Vec::new();
        let span = pitch * (n as f64 - 1.0);
        let y0 = (side - span) / 2.0;
        for i in 0..n {
            let net = format!("N{i}");
            let yl = y0 + pitch * i as f64;
            let yr = y0 + pitch * (n - 1 - i) as f64;
            fps.push(fp(
                &format!("L{i}"),
                3.0,
                yl,
                vec![pad("1", 0.0, 0.0, &net)],
            ));
            fps.push(fp(
                &format!("R{i}"),
                side - 3.0,
                yr,
                vec![pad("1", 0.0, 0.0, &net)],
            ));
        }
        square_board(side, fps)
    }

    fn bad_drc(pcb: &Pcb, r: &RouteAllResult) -> usize {
        let mut b = pcb.clone();
        apply(&mut b, r);
        check_drc(&b)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    crate::drc::DrcRuleType::Short | crate::drc::DrcRuleType::Clearance
                )
            })
            .count()
    }

    /// The headline benchmark: PathFinder negotiated congestion closes strictly
    /// more nets than plain greedy + rip-up on a congested crossing bus, and the
    /// board it produces is still short/clearance clean — with per-net
    /// diagnostics and a routability score for whatever stays open.
    ///
    /// The fixture is a dense bus on a ~32 mm board whose nets all cross through
    /// the centre. Greedy + rip-up grabs the centre channel suboptimally and
    /// strands several connections; negotiation prices the contested centre up
    /// round over round so flexible nets bow out to the perimeter, reclaiming
    /// the stranded ones.
    #[test]
    fn negotiation_closes_more_nets_than_baseline() {
        let n = 16usize;
        let pcb = crossing_bus(32.0, n, 1.7);

        // The historical router: a single greedy + rip-up pass, no negotiation,
        // no push-shove (negotiation_rounds == 1 reduces to exactly that).
        let baseline = route_all_with_opts(
            &pcb,
            0.25,
            &[],
            &RouteOptions {
                negotiation_rounds: 1,
                use_push_shove: false,
            },
        );
        // The shipped default: negotiated congestion + validated push-shove.
        let negotiated = route_all_with_opts(&pcb, 0.25, &[], &RouteOptions::default());

        // The fixture is genuinely congested — the baseline can't finish it.
        assert!(
            baseline.routed_nets.len() < n,
            "fixture must be congested, but baseline routed all {} nets",
            baseline.routed_nets.len()
        );
        // ...and negotiation closes strictly more of it.
        assert!(
            negotiated.routed_nets.len() > baseline.routed_nets.len(),
            "negotiated routed {} nets, baseline {} — negotiation must close more",
            negotiated.routed_nets.len(),
            baseline.routed_nets.len()
        );

        // The DRC-clean invariant holds through negotiation AND the push-shove
        // fallback: both outputs are short/clearance clean once applied.
        assert_eq!(bad_drc(&pcb, &baseline), 0, "baseline must be DRC-clean");
        assert_eq!(
            bad_drc(&pcb, &negotiated),
            0,
            "negotiated must be DRC-clean"
        );

        // Routability is the routed fraction (one 2-pad connection per net here)
        // and rises with the extra closed nets.
        let expected = negotiated.routed_nets.len() as f64 / n as f64;
        assert!(
            (negotiated.routability - expected).abs() < 1e-9,
            "routability {} should equal routed/total {expected}",
            negotiated.routability
        );
        assert!(
            negotiated.routability > baseline.routability,
            "routability must improve: {} vs {}",
            negotiated.routability,
            baseline.routability
        );

        // Every still-open net carries an actionable diagnostic, and every
        // diagnostic names a still-open net. A multi-connection net can yield
        // several diagnostics (one per failed connection), so assert the net
        // *sets* coincide rather than the raw counts — the latter only happens to
        // match here because every net in this fixture is a single connection.
        let diag_nets: BTreeSet<&String> = negotiated.diagnostics.iter().map(|d| &d.net).collect();
        let unrouted_set: BTreeSet<&String> = negotiated.unrouted_nets.iter().collect();
        assert_eq!(
            diag_nets, unrouted_set,
            "diagnostics must cover exactly the unrouted nets"
        );
        for d in &negotiated.diagnostics {
            assert!(
                negotiated.unrouted_nets.contains(&d.net),
                "diagnostic net {} must be in the unrouted set",
                d.net
            );
            assert!(
                !d.reason.is_empty(),
                "diagnostic must explain the obstruction"
            );
            assert!(
                !d.blocking_nets.is_empty() || d.suggested_layer.is_some(),
                "diagnostic must name a blocker or suggest a layer: {d:?}"
            );
            // The congested region is a non-degenerate box around the corridor.
            assert!(d.region_max.x >= d.region_min.x && d.region_max.y >= d.region_min.y);
        }
    }

    /// Baseline fidelity: on a board that routes fully, the negotiated default
    /// (`route_all`) reduces *exactly* to the historical greedy + rip-up baseline
    /// (`negotiation_rounds == 1, use_push_shove == false`). Negotiation only
    /// engages when a round leaves nets unrouted, so an easy board stops after
    /// round 0 — which IS the baseline path — and the default adds nothing. This
    /// pins the "purely additive over a faithful baseline" claim to an observable
    /// equality, not merely to determinism.
    #[test]
    fn negotiation_default_reduces_to_baseline_on_easy_board() {
        // Two well-separated nets, each a clear front-layer shot — no crossing,
        // no congestion, so both paths route fully and must coincide.
        let pcb = board(vec![
            fp("R1", 10.0, 8.0, vec![pad("1", 0.0, 0.0, "A")]),
            fp("R2", 40.0, 8.0, vec![pad("1", 0.0, 0.0, "A")]),
            fp("R3", 10.0, 22.0, vec![pad("1", 0.0, 0.0, "B")]),
            fp("R4", 40.0, 22.0, vec![pad("1", 0.0, 0.0, "B")]),
        ]);
        let baseline = route_all_with_opts(
            &pcb,
            0.25,
            &[],
            &RouteOptions {
                negotiation_rounds: 1,
                use_push_shove: false,
            },
        );
        let default = route_all(&pcb, 0.25, &[]);

        // Precondition: the board routes fully on both paths (so negotiation
        // never engages and the two are expected to be identical).
        assert!(
            baseline.unrouted_nets.is_empty() && default.unrouted_nets.is_empty(),
            "easy board must route fully on both paths"
        );
        // Same outcome AND the same copper — the default laid nothing extra.
        assert_eq!(default.routed_nets, baseline.routed_nets);
        assert_eq!(default.unrouted_nets, baseline.unrouted_nets);
        assert_eq!(
            default.traces.len(),
            baseline.traces.len(),
            "default must lay the same copper as the baseline on an easy board"
        );
        assert_eq!(default.vias.len(), baseline.vias.len());
    }

    /// Routing is deterministic — same board, same options, byte-stable result.
    /// (Load-bearing for reproducible builds and route caching.)
    #[test]
    fn routing_is_deterministic() {
        let pcb = crossing_bus(32.0, 16, 1.7);
        let a = route_all(&pcb, 0.25, &[]);
        let b = route_all(&pcb, 0.25, &[]);
        assert_eq!(a.routed_nets, b.routed_nets);
        assert_eq!(a.unrouted_nets, b.unrouted_nets);
        assert_eq!(a.traces.len(), b.traces.len());
        assert_eq!(a.vias.len(), b.vias.len());
    }
}
