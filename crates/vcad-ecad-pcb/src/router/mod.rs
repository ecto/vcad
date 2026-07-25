//! Trace routing algorithms for PCB layout.
//!
//! This module provides multiple routing strategies:
//!
//! - [`grid`] -- Lee/wave BFS-based grid router
//! - [`maze`] -- Single-net A* that avoids real copper via the incremental oracle
//! - [`push_shove`] -- Interactive push-and-shove router with visibility-graph pathfinding
//! - [`diff_pair`] -- Differential pair router with phase matching
//! - [`length_tune`] -- Length tuning meander generator with DRC-aware clearance checking
//! - [`length_match`] -- Group length matching: meander shorter nets to the longest/target
//! - [`congestion`] -- PathFinder-style negotiated-congestion history-cost field

pub mod auto;
pub mod classes;
pub mod descent;
#[cfg(feature = "gpu")]
pub mod gpu_bridge;
pub mod si_claims;

/// Wall-clock timer that is a no-op on wasm32 (where `Instant::now` traps).
/// Elapsed milliseconds, or 0.0 when timing is unavailable.
pub(crate) struct Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    start: std::time::Instant,
}

impl Stopwatch {
    pub(crate) fn start() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            start: std::time::Instant::now(),
        }
    }

    pub(crate) fn ms(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.start.elapsed().as_secs_f64() * 1000.0
        }
        #[cfg(target_arch = "wasm32")]
        {
            0.0
        }
    }
}

pub mod complete;
pub mod congestion;
pub mod diff_pair;
pub(crate) mod escape;
pub mod global;
pub mod grid;
pub(crate) mod legalize;
pub mod length_match;
pub mod length_tune;
pub mod maze;
pub(crate) mod pair;
pub mod push_shove;
pub(crate) mod shove;

pub use auto::{
    route_all, route_all_with_opts, RouteAllResult, RouteOptions, RoutedTrace, RoutedVia,
    UnroutedDiagnostic,
};
pub use congestion::Congestion;
pub use diff_pair::route_diff_pair;
pub use length_match::{
    check_length_match, match_lengths, LengthMatchOptions, LengthMatchResult, NetLengthReport,
};
pub use maze::route_net_maze;
pub use descent::{descend_board, DescentReport};
pub use pair::{census_pairs, polish_pairs, PairBail, PairCensus, PairCensusRow};
pub use si_claims::coupled_fraction as pair_coupled_fraction;

/// What [`si_finish`] changed.
#[derive(Debug, Clone, Copy, Default)]
pub struct SiFinishReport {
    /// Uncoupled pairs re-routed coupled by the polish stage.
    pub polished: usize,
    /// Uncoupled pairs the polish stage attempted.
    pub polish_attempted: usize,
    /// Differentiable-descent outcome.
    pub descent: DescentReport,
    /// Pairs whose residual skew was meandered out.
    pub meandered: usize,
    /// Pairs still over the skew tolerance after every stage.
    pub over_tolerance: usize,
}

/// Insert outward bumps into `points` adding about `deficit` mm of length,
/// each bulging AWAY from `twin` copper.
///
/// This is phase compensation, and it is the only shape that works on a
/// coupled leg. A generic meander generator bends to whichever side its
/// pattern says, which on half the bends is straight into the partner trace
/// sitting one gap away. Bulging away from the twin uses the open board on
/// the far side of the pair instead.
///
/// It is needed because a coupled pair's skew is not an accident of routing:
/// the two legs are offsets of ONE centerline, so at every bend the outer leg
/// takes the long way round and the inner the short way. The difference is
/// `(w + gap) · Σ 2·tan(θ/2)` over signed turns — about 0.9mm for a single net
/// right-angle turn at this board's 0.2/0.25 geometry, against a 1.1mm claim
/// bound. It cannot be tuned out by shifting the pair sideways either: a
/// lateral shift changes both legs by the same amount and their difference
/// depends only on the total separation. Length has to be added back.
fn bump_away(
    points: &[Vec2],
    twin: &[(Vec2, Vec2)],
    deficit: f64,
    amplitude: f64,
) -> Option<Vec<Vec2>> {
    if points.len() < 2 || deficit <= 0.0 {
        return None;
    }
    let pt_seg = |p: Vec2, a: Vec2, b: Vec2| -> f64 {
        let ab = b - a;
        let l2 = ab.x * ab.x + ab.y * ab.y;
        if l2 < 1e-18 {
            return (p - a).length();
        }
        let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
        (p - (a + ab.scale(t))).length()
    };
    let twin_dist = |p: Vec2| -> f64 {
        twin.iter()
            .map(|&(a, b)| pt_seg(p, a, b))
            .fold(f64::INFINITY, f64::min)
    };

    // Bumps are tiled by ARCLENGTH, not per segment.
    //
    // A routed leg is a polyline of many short segments (a simplified maze
    // staircase, ~0.45mm a side). Tiling per segment caps the achievable
    // length at one small cell per segment — measured, that is ~0.03mm per
    // segment, so a 10.66mm run reported "cannot reach 1.312mm" despite
    // having ample copper. Resampling the path at a fixed arclength step
    // decouples the pattern from the original vertices and restores the true
    // capacity, which is 2·(√2 − 1)/3 ≈ 0.276mm of added length per mm of run
    // at cell = 3·amplitude — and, notably, independent of amplitude.
    let cell = (3.0 * amplitude).max(0.3);
    let step = cell / 3.0;
    let gain_per_cell = 2.0 * ((step * step + amplitude * amplitude).sqrt() - step);
    if gain_per_cell < 1e-4 {
        return None;
    }
    let total: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    if total < 2.0 * cell {
        return None;
    }
    // Only rework the prefix needed to absorb the deficit (plus margin); the
    // rest of the run keeps its original vertices, so the trace count stays
    // bounded.
    let need_len = (deficit / (gain_per_cell / cell) * 1.25).min(total);

    let mut out: Vec<Vec2> = vec![points[0]];
    let mut added = 0.0f64;
    let mut travelled = 0.0f64;
    let mut phase = 0usize; // 0 = on path, 1 and 2 = offset (one cell = 3 steps)
    let mut carry = 0.0f64; // distance already consumed inside the current segment

    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b - a).length();
        if len < 1e-9 {
            continue;
        }
        let dir = (b - a).scale(1.0 / len);
        let nrm = Vec2::new(-dir.y, dir.x);
        // Bulge to whichever side is farther from the twin.
        let mid = a + dir.scale(len / 2.0);
        let sign = if twin_dist(mid + nrm.scale(amplitude)) >= twin_dist(mid - nrm.scale(amplitude))
        {
            1.0
        } else {
            -1.0
        };
        let off = nrm.scale(sign * amplitude);

        let mut t = carry;
        while t < len {
            if travelled + (t - carry) >= need_len || added >= deficit {
                break;
            }
            let p = a + dir.scale(t);
            if phase == 0 {
                out.push(p);
            } else {
                out.push(p + off);
                if phase == 2 {
                    added += gain_per_cell;
                }
            }
            phase = (phase + 1) % 3;
            t += step;
        }
        travelled += len - carry;
        carry = (t - len).max(0.0);
        // Land back on the path at the vertex before moving on.
        if added >= deficit || travelled >= need_len {
            out.push(b);
            phase = 0;
            carry = 0.0;
        }
    }
    out.push(*points.last().unwrap());
    // Only worth committing if it closed most of the gap; a partial bump just
    // moves the skew around.
    if added >= deficit * 0.6 {
        Some(out)
    } else {
        None
    }
}

/// Add `deficit` mm to `net` by meandering one of its runs, preferring runs
/// that have room. Returns the modified board, or `None` if no run could
/// absorb it.
///
/// Run choice is the whole point. The obvious candidate — the net's longest
/// run — is the coupled leg, and a coupled leg is the one place on the board
/// where a meander provably cannot go: its twin sits a gap away for the run's
/// whole length, so every amplitude the generator tries collides with it (on
/// the CM5 this refused 4 of 7 pairs outright with "meanders do not fit").
/// The breakout copper is the opposite: it is thinner than a leg, it is
/// uncoupled by construction, and it has open board around it. So candidates
/// are ordered narrowest-first, and the coupled legs are tried only as a last
/// resort.
fn compensate_run(pcb: &Pcb, net: &str, twin_net: &str, deficit: f64) -> Option<Pcb> {
    use length_tune::{generate_meanders_checked, LengthTuneParams, MeanderStyle};
    use vcad_ir::ecad::Trace;

    // Pair geometry, for the coupling-preserving amplitude cap below.
    let dp = pcb
        .rules
        .class_rules
        .iter()
        .find(|r| r.name == classes::DIFF_PAIR_CLASS);
    let leg_w = dp
        .and_then(|r| r.diff_pair_width)
        .or(pcb.rules.default_rules.diff_pair_width)
        .unwrap_or(0.2);
    let gap = dp
        .and_then(|r| r.diff_pair_gap)
        .or(pcb.rules.default_rules.diff_pair_gap)
        .unwrap_or(0.25);
    let pair_pitch = leg_w + gap;
    let max_sep = pair_pitch * 1.75;

    // Candidate runs: longest unbranched chain per (layer, width) class.
    let mut classes: Vec<(PcbLayer, f64)> = pcb
        .traces
        .iter()
        .filter(|t| t.net == net)
        .map(|t| (t.layer, (t.width * 1000.0).round() / 1000.0))
        .collect();
    classes.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
    classes.dedup();
    // Narrowest first: breakout copper before coupled legs.
    classes.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    for (layer, width) in classes {
        let segs: Vec<&Trace> = pcb
            .traces
            .iter()
            .filter(|t| t.net == net && t.layer == layer && (t.width - width).abs() < 1e-6)
            .collect();
        let Some(points) = length_match::longest_chain(&segs) else {
            continue;
        };
        let run_len: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        if points.len() < 2 || run_len < 0.5 {
            continue;
        }
        // Obstacles for the amplitude search: every other net's copper on
        // this layer near the run, plus the twin (which is same-net-adjacent
        // but a hard obstacle for a meander).
        let session = RouteSession::from_pcb(pcb);
        let clearance = session.clearance_for(net);
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &points {
            lo[0] = lo[0].min(p.x - 3.0);
            lo[1] = lo[1].min(p.y - 3.0);
            hi[0] = hi[0].max(p.x + 3.0);
            hi[1] = hi[1].max(p.y + 3.0);
        }
        let mut obstacles: Vec<(Vec2, Vec2, f64)> = Vec::new();
        session.for_each_blocking(layer, net, lo, hi, |geom, emin, emax, req| {
            match geom {
                crate::spatial::CopperGeom::Segment { a, b, half_w } => {
                    obstacles.push((*a, *b, req + half_w + width / 2.0))
                }
                crate::spatial::CopperGeom::Disc { center, r } => {
                    obstacles.push((*center, *center, req + r + width / 2.0))
                }
                _ => {
                    let c = Vec2::new((emin[0] + emax[0]) / 2.0, (emin[1] + emax[1]) / 2.0);
                    let hd =
                        ((emax[0] - emin[0]).powi(2) + (emax[1] - emin[1]).powi(2)).sqrt() / 2.0;
                    obstacles.push((c, c, req + hd + width / 2.0));
                }
            }
        });
        // Validation session: the board without THIS net's copper, so the
        // meander is judged against everything else including its twin.
        let mut bare = pcb.clone();
        bare.traces.retain(|t| t.net != net);
        let vsession = RouteSession::from_pcb(&bare);

        let on_run = |t: &Trace| {
            t.net == net
                && t.layer == layer
                && (t.width - width).abs() < 1e-6
                && points.windows(2).any(|w| {
                    ((t.start - w[0]).length() < 1e-6 && (t.end - w[1]).length() < 1e-6)
                        || ((t.start - w[1]).length() < 1e-6 && (t.end - w[0]).length() < 1e-6)
                })
        };

        // Amplitude retry loop. `generate_meanders_checked` only clearance-
        // checks meander WAYPOINTS, so a bend can pass its check while the
        // copper between two waypoints crosses an obstacle. Validate each
        // candidate with the exact oracle instead and shrink the amplitude
        // until one is genuinely legal.
        // The twin's copper on this layer, for the away-side decision.
        let twin: Vec<(Vec2, Vec2)> = pcb
            .traces
            .iter()
            .filter(|t| t.net == twin_net && t.layer == layer)
            .map(|t| (t.start, t.end))
            .collect();

        // On a COUPLED leg the amplitude is capped by the coupling window:
        // `min_pair_coupled_fraction` counts a P sample as coupled only while
        // some N copper is within 1.75x the pair pitch, so a bump swinging
        // further than that buys skew by destroying coupling — measured, and
        // it cost 0.777 -> 0.252 on the subset board. Breakout copper is
        // uncoupled already and keeps the generous amplitude.
        let is_leg = (width - leg_w).abs() < 0.01;
        let cap = if is_leg {
            ((max_sep - pair_pitch) * 0.7).max(0.05)
        } else {
            1.2
        };
        // Climb: smallest disruption that works wins.
        let ladder: [f64; 6] = [0.06, 0.10, 0.16, 0.24, 0.4, 1.2];
        for &rung_amp in ladder.iter() {
            let amp = rung_amp.min(cap);
            let params = LengthTuneParams {
                target_length: run_len + deficit,
                max_amplitude: amp,
                spacing: 0.5,
                style: MeanderStyle::Trombone,
            };
            let meanders = generate_meanders_checked(&points, &params, clearance, &obstacles);
            // Two candidate shapes per amplitude: the generic meander, and —
            // for coupled legs, where the generic one bends into the twin —
            // bumps that bulge away from it.
            let mut candidates: Vec<Vec<Vec2>> = Vec::new();
            if let Some(meanders) = meanders {
                if !meanders.is_empty() {
                    let mut tuned: Vec<Vec2> = Vec::new();
                    for (i, w) in points.windows(2).enumerate() {
                        tuned.push(w[0]);
                        if let Some(m) = meanders.iter().find(|m| m.segment_index == i) {
                            tuned.extend(m.points.iter().copied());
                        }
                    }
                    tuned.push(*points.last().unwrap());
                    candidates.push(tuned);
                }
            }
            if !twin.is_empty() {
                match bump_away(&points, &twin, deficit, amp) {
                    Some(bumped) => candidates.push(bumped),
                    None => log::trace!(
                        "compensate {net}: {layer:?} w={width} amp={amp:.3} run={run_len:.2} — bumps cannot reach {deficit:.3}mm"
                    ),
                }
            }
            for tuned in candidates {
            let legal = tuned.windows(2).all(|w| {
                vsession
                    .probe(
                        &crate::spatial::CopperGeom::Segment {
                            a: w[0],
                            b: w[1],
                            half_w: width / 2.0,
                        },
                        layer,
                        net,
                        clearance,
                    )
                    .legal
            });
            if !legal {
                log::trace!(
                    "compensate {net}: {layer:?} w={width} amp={amp:.3} candidate illegal"
                );
                continue;
            }
            let mut work = pcb.clone();
            work.traces.retain(|t| !on_run(t));
            work.traces.extend(tuned.windows(2).map(|w| Trace {
                start: w[0],
                end: w[1],
                width,
                layer,
                net: net.to_string(),
                source: None,
            }));
            return Some(work);
            }
        }
    }
    None
}

/// Meander the shorter leg of any pair still outside `tolerance` of its twin.
///
/// The last stage of [`si_finish`], and the one that closes
/// `worst_intra_pair_skew`. A coupled pair's two legs are offsets of one
/// centerline, so they match by construction — the residual skew lives in the
/// pad breakout connectors, which are independent maze routes and can differ
/// by millimetres. Descent cannot fix that: on a well-coupled pair the gap
/// springs and clearance hinges both resist moving the run, which is exactly
/// why it rejects these. Adding the deficit back as a meander is the standard
/// answer and the one the human board uses.
///
/// Fail-closed like every other stage: tuned copper is re-probed against the
/// exact oracle with the net's old copper removed, and anything illegal or
/// non-improving is dropped.
fn meander_pair_skew(pcb: &mut Pcb, tolerance: f64) -> (usize, usize) {
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
    let classifier = classes::classify_nets(&nets);
    let (mut fixed, mut over) = (0usize, 0usize);
    for (pn, nn) in &classifier.pairs {
        let lp = length_match::net_routed_length(pcb, pn);
        let ln = length_match::net_routed_length(pcb, nn);
        if lp <= 0.0 || ln <= 0.0 {
            continue;
        }
        let before = (lp - ln).abs();
        if before <= tolerance {
            continue;
        }
        // Which leg is short, and by how much.
        let (short_net, deficit) = if lp < ln { (pn, ln - lp) } else { (nn, lp - ln) };
        let twin_net = if short_net == pn { nn } else { pn };
        let Some(work) = compensate_run(pcb, short_net, twin_net, deficit) else {
            log::debug!("si-finish: {pn} skew {before:.3}mm — no run could absorb {deficit:.3}mm");
            over += 1;
            continue;
        };
        let after = (length_match::net_routed_length(&work, pn)
            - length_match::net_routed_length(&work, nn))
        .abs();
        if after >= before {
            over += 1;
            continue;
        }
        // The two pair claims pull against each other: length added to a
        // coupled leg has to go somewhere, and "somewhere" is away from the
        // twin, which costs coupled fraction. Demanding zero loss deadlocks
        // (it refused 5 of 9 pairs outright and skew stayed at 1.499mm), so
        // spend the slack the claim actually leaves: keep the pair well clear
        // of the 0.5 bound — COUPLING_FLOOR is the human board's own margin,
        // not a relaxed bound — and let skew take the rest. A pair already
        // below the floor may not be pushed lower.
        const COUPLING_FLOOR: f64 = 0.7;
        let gap_c = pcb.rules.default_rules.diff_pair_gap.unwrap_or(0.25);
        let w_c = pcb
            .rules
            .default_rules
            .diff_pair_width
            .unwrap_or(pcb.rules.default_rules.trace_width);
        let sep = (w_c + gap_c) * 1.75;
        let (cf_before, cf_after) = (
            si_claims::coupled_fraction(pcb, pn, nn, sep),
            si_claims::coupled_fraction(&work, pn, nn, sep),
        );
        if cf_after < COUPLING_FLOOR.min(cf_before) - 1e-9 {
            log::debug!(
                "si-finish: {pn} compensation would cost coupling ({cf_before:.3} -> {cf_after:.3}) — skipped"
            );
            over += 1;
            continue;
        }
        // Exact oracle over every new segment. Only the net under test has
        // its own copper removed — the TWIN stays in the obstacle set, since
        // a meander that swings into its partner is exactly the failure this
        // stage could otherwise introduce (the session applies the declared
        // pair gap to it rather than the base clearance).
        let legal = [pn, nn].iter().all(|net| {
            let mut bare = work.clone();
            bare.traces.retain(|t| t.net != **net);
            let vsession = RouteSession::from_pcb(&bare);
            work.traces
                .iter()
                .filter(|t| t.net == **net)
                .all(|t| {
                    vsession
                        .probe(
                            &crate::spatial::CopperGeom::Segment {
                                a: t.start,
                                b: t.end,
                                half_w: t.width / 2.0,
                            },
                            t.layer,
                            &t.net,
                            vsession.clearance_for(&t.net),
                        )
                        .legal
                })
        });
        if !legal {
            log::debug!("si-finish: {pn} meander rejected by oracle");
            over += 1;
            continue;
        }
        *pcb = work;
        fixed += 1;
        log::info!("si-finish: meandered {pn} skew {before:.3} -> {after:.3} mm");
        if after > tolerance {
            over += 1;
        }
    }
    (fixed, over)
}

/// Signal-integrity finishing pass for a fully routed board.
///
/// The two claims that judge differential pairs — `min_pair_coupled_fraction`
/// and `worst_intra_pair_skew` — are worst-case over EVERY routed pair, so a
/// single bad pair breaks them however good the rest is. Detailed routing
/// optimizes for completion, not for those, and leaves a tail. This pass
/// works that tail in the order that matters:
///
/// 1. [`polish_pairs`] rips each still-uncoupled pair off the settled board
///    and re-routes it coupled, ripping crossing single-ended traces out of
///    its corridor and re-routing them after. This is *reroute*-then-descend:
///    descent is a local optimizer, so a pair that started far off-target has
///    to be re-routed before descending it is worth anything.
/// 2. [`descend_board`] then drives residual intra-pair skew out of the
///    geometry that is already close.
/// 3. [`meander_pair_skew`] meanders whatever skew survives — the breakout
///    mismatch descent structurally cannot reach.
///
/// Every stage is strictly non-regressive and gated on the exact oracle: a
/// pair that fails restores its original copper, so this can only improve the
/// board or leave it alone.
pub fn si_finish(pcb: &mut Pcb, expansions: usize, descent_iters: usize) -> SiFinishReport {
    let (polished, polish_attempted) = polish_pairs(pcb, expansions);
    log::info!("si-finish: polish re-coupled {polished}/{polish_attempted} pairs");
    let descent = descend_board(pcb, descent_iters);
    log::info!(
        "si-finish: descent tuned {}/{} pairs ({} rejected)",
        descent.tuned,
        descent.attempted,
        descent.rejected
    );
    // Tolerance well inside the 1.1mm claim bound: the claim is a maximum
    // over every pair, so leaving a pair at 1.05mm is one reroute away from
    // breaking it.
    //
    // Iterated: one pass can only absorb what the run's straight stretches
    // have room for at one amplitude, so a large deficit closes over several
    // passes. Stops as soon as a pass fixes nothing, so a board that is
    // already matched pays for exactly one pass.
    let (mut meandered, mut over_tolerance) = (0usize, 0usize);
    for _ in 0..6 {
        let (fixed, over) = meander_pair_skew(pcb, 0.6);
        meandered += fixed;
        over_tolerance = over;
        if fixed == 0 {
            break;
        }
    }
    log::info!("si-finish: meandered {meandered} pairs, {over_tolerance} still over tolerance");
    SiFinishReport {
        polished,
        polish_attempted,
        descent,
        meandered,
        over_tolerance,
    }
}

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use push_shove::{Obstacle, PushShoveRouter};

/// Route a single net on a board with the avoiding A* maze router.
///
/// Builds a [`RouteSession`] from `pcb` (so the route avoids every trace, pad,
/// and via already on `layer`, not just other-net trace bounding boxes) and
/// searches for a clearance-legal path. Convenience wrapper over
/// [`route_net_maze`] for one-shot single-net routing; to route many nets while
/// each avoids the ones before it, hold a `RouteSession` and commit between
/// calls.
pub fn route_net_maze_pcb(
    pcb: &Pcb,
    layer: PcbLayer,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
) -> RouteResult {
    let session = RouteSession::from_pcb(pcb);
    route_net_maze(
        &session,
        &pcb.outline.vertices,
        layer,
        net,
        start,
        end,
        width,
    )
}

/// Unique identifier for a net within the router.
pub type NetId = u32;

/// Route result for a single net.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteResult {
    /// Net name that was routed.
    pub net: String,
    /// Routed trace segments as (start, end) pairs in board coordinates (mm).
    pub segments: Vec<(Vec2, Vec2)>,
    /// Via locations where the route changes layers.
    pub vias: Vec<Vec2>,
    /// Whether routing succeeded.
    pub success: bool,
}

/// Common routing configuration shared across algorithms.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    /// Default trace width in mm.
    pub trace_width: f64,
    /// Default clearance in mm.
    pub clearance: f64,
    /// Default via diameter in mm.
    pub via_diameter: f64,
    /// Default via drill in mm.
    pub via_drill: f64,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            trace_width: 0.25,
            clearance: 0.2,
            via_diameter: 0.8,
            via_drill: 0.4,
        }
    }
}

/// Route a single net on a board with the push-and-shove router.
///
/// Existing traces on **other** nets become rectangular obstacles, so the new
/// route detours around copper already on the board instead of crossing it —
/// the continuous-space counterpart to the grid router used by the basic
/// autorouter. Coordinates are board-space millimetres (the returned segments
/// are in the same frame), so callers don't need the grid router's
/// origin-offset bookkeeping.
///
/// `width` is the new trace's width; clearance is taken from the board's
/// default net-class rules.
pub fn route_net_push_shove(
    pcb: &Pcb,
    net: &str,
    start: Vec2,
    end: Vec2,
    width: f64,
) -> RouteResult {
    let clearance = pcb.rules.default_rules.clearance;
    let mut router = PushShoveRouter::new(width, clearance);

    for trace in &pcb.traces {
        if trace.net == net {
            continue;
        }
        let hw = trace.width * 0.5;
        let min = Vec2::new(
            trace.start.x.min(trace.end.x) - hw,
            trace.start.y.min(trace.end.y) - hw,
        );
        let max = Vec2::new(
            trace.start.x.max(trace.end.x) + hw,
            trace.start.y.max(trace.end.y) + hw,
        );
        router.add_obstacle(Obstacle::new(min, max));
    }

    router.route_net(net, start, end)
}

#[cfg(test)]
mod compensation_tests {
    use super::*;

    /// A run's compensation capacity is set by its LENGTH, not by how many
    /// vertices it happens to have.
    ///
    /// This is the property that decides whether `worst_intra_pair_skew` can
    /// be closed at all. A routed leg is a simplified maze staircase, so the
    /// same 20mm of copper arrives as either a couple of long segments or
    /// forty short ones; tiling bumps per segment made capacity track the
    /// vertex count and left real runs unable to absorb ~1mm deficits. Both
    /// shapes must now absorb the same deficit, and the added length must
    /// match the analytic 2·(√2−1)/3 ≈ 0.276mm per mm.
    #[test]
    fn bump_capacity_follows_run_length_not_vertex_count() {
        let twin = [(Vec2::new(0.0, -0.45), Vec2::new(20.0, -0.45))];
        let len = |p: &[Vec2]| -> f64 { p.windows(2).map(|w| (w[1] - w[0]).length()).sum() };

        // Same 20mm path, two segments vs forty.
        let coarse = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(20.0, 0.0)];
        let fine: Vec<Vec2> = (0..=40).map(|i| Vec2::new(i as f64 * 0.5, 0.0)).collect();

        for (name, run) in [("coarse", &coarse), ("fine", &fine)] {
            let out = bump_away(run, &twin, 2.0, 0.06)
                .unwrap_or_else(|| panic!("{name} run must absorb 2mm over 20mm of copper"));
            let added = len(&out) - len(run);
            assert!(
                added >= 2.0 * 0.6,
                "{name}: added only {added:.3}mm of a 2mm deficit"
            );
            // Endpoints pinned.
            assert_eq!(out[0], run[0]);
            assert_eq!(*out.last().unwrap(), *run.last().unwrap());
            // Every bump goes AWAY from the twin (which sits at y = -0.45).
            assert!(
                out.iter().all(|p| p.y >= -1e-9),
                "{name}: a bump crossed toward the twin"
            );
        }

        // Capacity is independent of amplitude: a 10x smaller bump over the
        // same run still reaches the same deficit.
        assert!(
            bump_away(&fine, &twin, 2.0, 0.6).is_some(),
            "large amplitude must also reach"
        );
    }

    /// Nothing is emitted when the run genuinely cannot absorb the deficit —
    /// the caller relies on `None` to fall through to another run rather than
    /// commit a partial fix that just moves the skew around.
    #[test]
    fn bump_refuses_when_run_is_too_short() {
        let twin = [(Vec2::new(0.0, -0.45), Vec2::new(2.0, -0.45))];
        let short = vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)];
        assert!(bump_away(&short, &twin, 5.0, 0.06).is_none());
    }
}

#[cfg(test)]
mod pcb_route_tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Bare board with a configurable trace list — enough to exercise the
    /// push-and-shove integration without the full footprint scaffolding.
    fn board(traces: Vec<Trace>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(20.0, 0.0),
                    Vec2::new(20.0, 20.0),
                    Vec2::new(0.0, 20.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                }],
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

    #[test]
    fn straight_route_when_board_is_empty() {
        let pcb = board(vec![]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert_eq!(r.segments.len(), 1, "no obstacles → one straight segment");
    }

    #[test]
    fn detours_around_a_trace_on_another_net() {
        // A GND trace straddles the straight path from start to end; the
        // router must shove the SIG route around it.
        let blocker = trace("GND", Vec2::new(7.5, 2.0), Vec2::new(7.5, 8.0));
        let pcb = board(vec![blocker]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert!(
            r.segments.len() > 1,
            "should detour around the GND trace, got {} segment(s)",
            r.segments.len()
        );
        // Endpoints are preserved.
        assert!((r.segments[0].0.x - 0.0).abs() < 1e-6);
        let last = r.segments.last().unwrap();
        assert!((last.1.x - 15.0).abs() < 1e-6);
    }

    #[test]
    fn ignores_obstacles_on_the_same_net() {
        // A same-net trace is not an obstacle — co-net copper may touch.
        let same = trace("SIG", Vec2::new(7.5, 2.0), Vec2::new(7.5, 8.0));
        let pcb = board(vec![same]);
        let r = route_net_push_shove(&pcb, "SIG", Vec2::new(0.0, 5.0), Vec2::new(15.0, 5.0), 0.25);
        assert!(r.success);
        assert_eq!(r.segments.len(), 1, "same-net copper is not shoved around");
    }
}
