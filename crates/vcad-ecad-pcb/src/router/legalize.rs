//! Post-route legalization: the router's own output, made legal before it is
//! returned.
//!
//! The autorouter composes copper from several independent emitters (maze
//! routes, fan-out rescue vias, plane stitching, joint repair). Each emitter is
//! probed against the board, but their *combined* output can still contain
//! copper no one would ship — the three recurring field failures:
//!
//! 1. **Duplicate / overlapping vias** — two emitters drop a via for the same
//!    net at (nearly) the same spot, and the drills intersect: a `HoleToHole`
//!    DRC error with negative spacing.
//! 2. **Same-net bypass noodles** — a route brushes its own net's copper far
//!    from any intended junction, short-circuiting the conductor between the
//!    touch points: a `SameNetBypass` DRC warning.
//! 3. **Dangling copper** — traces and vias whose island reaches no pad and no
//!    pour, stranded by rip-up/restore: `NetIslands`, "copper only, no pads".
//!
//! [`legalize`] runs after routing settles and before the result is returned:
//! it merges same-net vias whose holes are hole-to-hole illegal (reconnecting
//! their traces), prunes redundant self-touching trace segments while a
//! continuity oracle proves the net stays connected, and finally demotes —
//! fail-closed — any net whose new copper is still hole-to-hole illegal.
//! [`prune_dangling`] then gets the last word, dropping the dead islands. The
//! invariant they enforce together: an autoroute pass on a clean placement
//! never introduces `HoleToHole` or `SameNetBypass` violations of its own
//! making, and never returns copper connected to nothing.
//!
//! Demotion is per *net*, which is the right granularity for a routed net whose
//! copper is one connected path — but not for a plane-stitched power net, whose
//! copper is dozens of independent pad→plane vias. There, one bad drill would
//! strip every stitch and leave the pour connected to nothing (measured on the
//! moteus fixture: a single hole conflict cost all 32 GND stitches). A stitch
//! via is therefore dropped on its own, taking only its dog-bone stub with it;
//! the pad it served is then honestly reported by the DRC's `UnstitchedPad`.

use std::collections::BTreeSet;

use vcad_ir::ecad::{CopperSource, Pcb, PcbLayer, Trace, Via};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::CopperGeom;

use super::auto::{copper_layers, RoutedTrace, RoutedVia};
use crate::drc::{
    analyze_net_continuity, check_drc_in_region, dangling_copper_mask, DrcRuleType, DrcViolation,
};

/// Two points closer than this are the same routing coordinate.
const POS_EPS: f64 = 1e-4;

/// Iteration cap for the DRC-driven fix loop.
///
/// A round clears *every* independently-provable bypass it sees (see
/// [`legalize`]), so the cap bounds the number of times a fix has to unmask
/// another one — not the number of violations. It was 4, which combined with a
/// re-judge after every single fix capped the whole pass at 4 repairs: a full
/// CM5 route arrived with 21 same-net bypasses and left with 17, the loop
/// having spent its budget on the first four.
const MAX_FIX_ROUNDS: usize = 16;

/// What [`legalize`] did to the router's output.
#[derive(Debug, Default)]
pub(super) struct LegalizeReport {
    /// Vias removed by merging into a coincident/overlapping same-net via.
    pub merged_vias: usize,
    /// Trace segments pruned as redundant same-net bypass copper.
    pub pruned_traces: usize,
    /// Nets whose new copper was removed entirely because it remained
    /// hole-to-hole illegal after merging (fail-closed: better unrouted than
    /// unmanufacturable). Each entry carries the position of the offending via.
    pub demoted: Vec<(String, Vec2)>,
    /// Plane-stitch vias dropped individually (with their dog-bone stub) rather
    /// than demoting the whole net. Each entry is the via position.
    pub dropped_stitches: Vec<Vec2>,
}

/// Legalize the router's flattened output in place. See the module docs.
///
/// `prune_dead` runs [`prune_dangling`] at the top of every fix round. Both
/// oracles this pass depends on read the board as it will actually ship, and
/// the speculative emitters' debris is not on it: on the CM5 the router hands
/// over ~1855 dead traces that the caller's prune deletes moments later, and
/// deleting them *lengthens* the surviving conductor chains — a contact 5 hops
/// apart through dead copper is 9 hops apart once it is gone. Judging before
/// the prune therefore misses bypasses that the shipped board really has.
pub(super) fn legalize(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
    stitch_vias: &[Vec2],
    prune_dead: bool,
) -> LegalizeReport {
    let mut report = LegalizeReport::default();
    if traces.is_empty() && vias.is_empty() {
        return report;
    }

    report.merged_vias = merge_same_net_vias(pcb, traces, vias);

    // DRC-driven cleanup: judge the candidate board (existing copper + the
    // router's output) with the real checker and act only on violations the
    // new copper participates in. Loop because one fix can unmask another.
    let mut unfixable: BTreeSet<(u64, u64)> = BTreeSet::new();
    for _ in 0..MAX_FIX_ROUNDS {
        if prune_dead {
            prune_dangling(pcb, traces, vias);
        }
        let candidate = candidate_pcb(pcb, traces, vias);
        let (min, max) = new_copper_bbox(traces, vias);
        let violations = check_drc_in_region(&candidate, min, max);

        let mut progressed = false;
        // Bypasses first, and as many as the round can prove independent.
        // Independence is per net: a same-net bypass is a contact between two
        // pieces of one net's copper, and its hop count is measured along that
        // net's own adjacency graph, so pruning net X's copper cannot change
        // any verdict on net Y. Two violations on the *same* net are not
        // independent — the first prune may well clear the second — so only
        // the first is acted on and the rest wait for the next round, where
        // they are re-judged rather than assumed.
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for v in violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
        {
            let key = pos_key(v.position);
            if unfixable.contains(&key) {
                continue;
            }
            let Some(net) = bypass_net(traces, v) else {
                unfixable.insert(key);
                continue;
            };
            if !touched.insert(net) {
                continue; // same net already changed this round — re-judge first
            }
            if prune_bypass(pcb, traces, vias, v) {
                report.pruned_traces += 1;
                progressed = true;
            } else {
                unfixable.insert(key);
            }
        }
        // Then at most one hole conflict: unlike a prune, a demotion strips a
        // whole net, so it is never applied against a stale violation list.
        for v in violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::HoleToHole)
        {
            // Only act when one of the holes is a via we placed; a
            // pad-vs-pad conflict is the placement's, not ours.
            let Some(idx) = new_via_at_holes(v, vias) else {
                continue;
            };
            // A plane stitch is an independent pad→plane connection:
            // drop just that via (and its dog-bone stub) so the net's
            // other stitches survive. Anything else is part of a routed
            // path, where half a path is worse than none.
            if stitch_vias
                .iter()
                .any(|p| d2(*p, vias[idx].position) <= POS_EPS * POS_EPS)
            {
                drop_stitch_via(idx, traces, vias, &mut report);
            } else {
                let net = vias[idx].net.clone();
                demote_net(&net, v.position, traces, vias, &mut report);
            }
            progressed = true;
            break; // copper changed — re-judge from scratch
        }
        if !progressed {
            break;
        }
    }
    report
}

/// The net a `SameNetBypass` violation is on, recovered geometrically (the
/// message names it, but parsing prose is not a contract): the new segment
/// whose copper lies nearest the contact point is copper of that net.
///
/// Nearest rather than covering, because the reported point need not be *on*
/// either conductor — see [`bypass_candidates`].
fn bypass_net(traces: &[RoutedTrace], v: &DrcViolation) -> Option<String> {
    traces
        .iter()
        .min_by(|a, b| dist_to_trace(v.position, a).total_cmp(&dist_to_trace(v.position, b)))
        .filter(|t| dist_to_trace(v.position, t) <= t.width) // sanity bound
        .map(|t| t.net.clone())
}

/// The new segments of `net` that could be the redundant half of a bypass at
/// `v.position`, longest first.
///
/// The DRC reports a segment-vs-segment contact at the *midpoint of the two
/// centerlines' closest points*, which is not a point on either centerline: it
/// is offset from each by at most half the centerline gap, and two segments
/// touch while that gap is as wide as the sum of their half-widths. So the
/// point can sit off the conductor it belongs to. Testing `point_on_trace` —
/// distance ≤ `hw_self` — therefore misses contacts involving copper wider than
/// the trace being tested, and was doing so on the CM5: the last surviving
/// bypass sat 0.0437mm from a 0.08mm trace whose half-width test allowed
/// 0.041mm, so the pruner saw no candidate at all and gave up without ever
/// consulting the continuity oracle.
///
/// `hw_self + hw_other` is an envelope, not a tight bound — the exact offset
/// depends on which geometry pair the DRC matched (segment/segment, disc, pad
/// rect, pour), and each has its own contact-point construction. Being generous
/// here is safe: every candidate is still filtered to the violation's own net,
/// and no candidate is *removed* unless the continuity oracle proves the net is
/// no worse off without it.
fn bypass_candidates(traces: &[RoutedTrace], net: &str, at: Vec2) -> Vec<usize> {
    /// Radius around the contact point in which to look for the other conductor.
    const NEARBY: f64 = 1.0;
    let hw_other = traces
        .iter()
        .filter(|t| t.net == net && dist_to_trace(at, t) <= NEARBY)
        .map(|t| t.width / 2.0)
        .fold(0.0f64, f64::max);
    let mut candidates: Vec<usize> = traces
        .iter()
        .enumerate()
        .filter(|(_, t)| t.net == net && dist_to_trace(at, t) <= t.width / 2.0 + hw_other + 1e-3)
        .map(|(i, _)| i)
        .collect();
    // Longest first: drop the noodle, keep the shortest connecting subtree.
    candidates.sort_by(|&a, &b| {
        d2(traces[b].start, traces[b].end).total_cmp(&d2(traces[a].start, traces[a].end))
    });
    candidates
}

/// Distance from `p` to trace `t`'s centerline.
fn dist_to_trace(p: Vec2, t: &RoutedTrace) -> f64 {
    let (a, b) = (t.start, t.end);
    let ab = Vec2::new(b.x - a.x, b.y - a.y);
    let len2 = ab.x * ab.x + ab.y * ab.y;
    let u = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0)
    };
    d2(p, Vec2::new(a.x + ab.x * u, a.y + ab.y * u)).sqrt()
}

/// Clear same-net bypasses on an already-committed board, in place; returns the
/// number of trace segments removed.
///
/// [`legalize`] enforces this invariant on `route_all`'s output, but that is not
/// the last word on the board: [`super::si_finish`] runs afterwards and rips,
/// re-routes and prunes copper of its own (on the full CM5, 483 dead traces and
/// 92 dead vias removed *after* legalization). Both moves create bypasses that
/// nothing then judges — a re-routed pair brushing its own net, and, more often,
/// a prune that lengthens a surviving conductor chain until an existing contact
/// crosses the hop limit. A full CM5 route reached the drill file with 8 such
/// bypasses even once legalization itself was clearing every one it could see.
///
/// Same policy as [`crate::drc::prune_dangling_copper`], which `si_finish`
/// already applies board-wide: the whole board is in scope, and the continuity
/// oracle is what keeps it honest. A segment is removed only when the net it
/// belongs to ends up with no more islands and no less coverage than it had —
/// so this can never clear a violation by deleting connectivity, which is the
/// trap `fab-prep`'s arrival-vs-completion guard exists to catch.
pub fn repair_same_net_bypass(pcb: &mut Pcb) -> usize {
    let mut removed = 0;
    let mut unfixable: BTreeSet<(u64, u64)> = BTreeSet::new();
    for _ in 0..MAX_FIX_ROUNDS {
        // Dead copper first, for the same reason `legalize` does it: the hop
        // counts this rule measures are the ones on the board that ships.
        crate::drc::prune_dangling_copper(pcb);
        let violations: Vec<DrcViolation> = crate::drc::check_drc(pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .collect();
        let view = trace_view(pcb);

        let mut progressed = false;
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for v in &violations {
            let key = pos_key(v.position);
            if unfixable.contains(&key) {
                continue;
            }
            let Some(net) = bypass_net(&view, v) else {
                unfixable.insert(key);
                continue;
            };
            // Bypasses on distinct nets are independent (see `legalize`); two on
            // one net are not, so the rest of that net waits for a re-judge.
            if !touched.insert(net.clone()) {
                continue;
            }
            if prune_bypass_in_place(pcb, &view, &net, v) {
                removed += 1;
                progressed = true;
            } else {
                unfixable.insert(key);
            }
        }
        if !progressed {
            break;
        }
    }
    removed
}

/// The board's traces as the geometry the bypass helpers read.
fn trace_view(pcb: &Pcb) -> Vec<RoutedTrace> {
    pcb.traces
        .iter()
        .map(|t| RoutedTrace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
        })
        .collect()
}

/// [`prune_bypass`] against a committed board: delete the redundant segment at
/// the contact point, keeping it only if the net is no worse off without it.
///
/// `view` indexes `pcb.traces` one-for-one, so a candidate index addresses the
/// same segment in both.
fn prune_bypass_in_place(pcb: &mut Pcb, view: &[RoutedTrace], net: &str, v: &DrcViolation) -> bool {
    for i in bypass_candidates(view, net, v.position) {
        let before = analyze_net_continuity(pcb, net);
        let removed = pcb.traces.remove(i);
        let after = analyze_net_continuity(pcb, net);
        if after.islands <= before.islands && after.coverage >= before.coverage - 1e-9 {
            return true;
        }
        pcb.traces.insert(i, removed);
    }
    false
}

/// Drop the router's own dead copper: new traces and vias whose galvanic
/// island on the candidate board holds no pad and no pour fragment.
///
/// The debris comes from the emitters that add copper speculatively — dog-bone
/// escape vias for a connection rip-up later abandoned, fan-out stubs whose
/// route was replaced, stitching vias whose plane never materialized — and it
/// shows up in DRC as `NetIslands` "copper only, no pads" (4185 traces and 130
/// vias on one CM5 pass, 172 island violations down to 18 once removed).
///
/// Soundness — this can only remove electrically dead copper:
///
/// * Islands are *maximal* connected components, so they are disjoint. Deleting
///   an entire unanchored island cannot disconnect anything that stays, and the
///   single pass is already the fixpoint.
/// * Every connection the router counts as placed runs pad to pad, so its
///   copper lives in a pad-anchored island and is kept. `routability`, the
///   routed/unrouted net split, and the `pending` diagnostics therefore mean
///   exactly what they meant before the prune.
/// * Only *new* copper is dropped. Board copper — including anything the caller
///   placed by hand — is judged (it anchors islands) but never removed, which is
///   also what keeps the arc blind spot harmless: connectivity does not model
///   `trace_arcs`, but the router emits none and never deletes the board's.
///
/// Returns `(traces_removed, vias_removed)`.
pub(super) fn prune_dangling(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
) -> (usize, usize) {
    if traces.is_empty() && vias.is_empty() {
        return (0, 0);
    }
    // Judge the board as the caller will commit it: `candidate_pcb` appends the
    // new copper after the existing copper, so the new traces occupy
    // `pcb.traces.len()..` of the mask and the new vias `pcb.vias.len()..`.
    let candidate = candidate_pcb(pcb, traces, vias);
    // NOTE: [`crate::drc::spur_copper_mask`] is the finer version of this — it
    // also strips dead-end branches hanging off islands that *are* pad-anchored,
    // which this pass keeps by design ("the island is the unit"). It is measured
    // and sound (it never changes any board's `UnconnectedNet` count) and it
    // recovers `vias_per_si_net` on the CM5, but it is deliberately NOT wired in
    // here yet: for a net the router only partially reached, every piece of
    // copper is a dead end, so enabling it would reclassify those nets as
    // unrouted and move `routability`. That reconciliation needs its own
    // full-board before/after, so it is a separate change.
    let (keep_trace, keep_via) = dangling_copper_mask(&candidate);
    let (t0, v0) = (pcb.traces.len(), pcb.vias.len());

    let mut ti = 0;
    traces.retain(|_| {
        let k = keep_trace[t0 + ti];
        ti += 1;
        k
    });
    let mut vi = 0;
    vias.retain(|_| {
        let k = keep_via[v0 + vi];
        vi += 1;
        k
    });
    (
        keep_trace[t0..].iter().filter(|k| !**k).count(),
        keep_via[v0..].iter().filter(|k| !**k).count(),
    )
}

/// Quantized position, as a set key for the unfixable-violation cache.
fn pos_key(p: Vec2) -> (u64, u64) {
    (
        ((p.x * 1e4).round() as i64) as u64,
        ((p.y * 1e4).round() as i64) as u64,
    )
}

use super::classes::via_geom_for;

fn d2(a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy
}

/// Merge same-net new vias whose drilled holes are closer than the
/// hole-to-hole rule allows (including exact duplicates). The survivor takes
/// the union of the cluster's layer spans; traces that ended on a dropped via
/// are reconnected to the survivor with a short same-net jog on their own
/// layer. Returns the number of vias removed.
///
/// Both of those moves place copper that the router's oracle never saw. The
/// widened span puts a barrel on layers the original via did not occupy, and
/// the jog is a brand new trace; the old justification ("the jog stays inside
/// copper the via already claimed") only holds on the layers the via already
/// claimed. So both are probed here against the candidate board, and a cluster
/// whose merge would land on foreign copper is left alone — its vias stay
/// hole-to-hole illegal and the fail-closed demotion below strips the net,
/// which is the honest outcome. Unchecked, this was the second source of exact
/// 0.000mm overlaps on a full CM5 route: foreign traces sitting inside a
/// merged via's newly-claimed inner-layer pad.
fn merge_same_net_vias(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
) -> usize {
    if vias.len() < 2 {
        return 0;
    }
    let min_spacing = pcb.rules.hole_to_hole;
    let stack = copper_layers(pcb);
    let layer_idx = |l: PcbLayer| stack.iter().position(|&s| s == l).unwrap_or(0);
    // The oracle for this stage: the board as the caller will actually commit
    // it. Same-net copper is excluded by the probe, so a cluster is judged
    // purely on the foreign copper it would newly touch — but only on copper
    // that will still be there. At this point the output still carries the
    // speculative emitters' debris (abandoned fan-out stubs, escape vias for
    // routes rip-up replaced: on the CM5, 1855 dead traces), which
    // `prune_dangling` deletes moments later. Judging against phantom
    // obstacles refused every merge on the board and left the duplicate vias
    // for the demotion pass to strip.
    let session = {
        let mut cand = candidate_pcb(pcb, traces, vias);
        let (keep_trace, keep_via) = dangling_copper_mask(&cand);
        // Only the NEW copper may be dropped — `candidate_pcb` appends it after
        // the board's own, which is never pruned and stays an obstacle here.
        let (t0, v0) = (pcb.traces.len(), pcb.vias.len());
        let mut ti = 0;
        cand.traces.retain(|_| {
            let k = ti < t0 || keep_trace[ti];
            ti += 1;
            k
        });
        let mut vi = 0;
        cand.vias.retain(|_| {
            let k = vi < v0 || keep_via[vi];
            vi += 1;
            k
        });
        RouteSession::from_pcb(&cand)
    };

    // Union-find over the new vias: same net + illegal hole spacing → cluster.
    let mut parent: Vec<usize> = (0..vias.len()).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = i;
        while parent[c] != r {
            let next = parent[c];
            parent[c] = r;
            c = next;
        }
        r
    }
    for i in 0..vias.len() {
        let (_, drill_i) = via_geom_for(pcb, &vias[i].net);
        for j in (i + 1)..vias.len() {
            if vias[i].net != vias[j].net {
                continue;
            }
            let (_, drill_j) = via_geom_for(pcb, &vias[j].net);
            let edge =
                d2(vias[i].position, vias[j].position).sqrt() - drill_i / 2.0 - drill_j / 2.0;
            if edge < min_spacing - 1e-6 {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Resolve each cluster: keep the member nearest the cluster centroid.
    let mut clusters: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..vias.len() {
        let r = find(&mut parent, i);
        clusters.entry(r).or_default().push(i);
    }

    let mut drop: BTreeSet<usize> = BTreeSet::new();
    let mut connectors: Vec<RoutedTrace> = Vec::new();
    for members in clusters.values() {
        if members.len() < 2 {
            continue;
        }
        let n = members.len() as f64;
        let cx = members.iter().map(|&i| vias[i].position.x).sum::<f64>() / n;
        let cy = members.iter().map(|&i| vias[i].position.y).sum::<f64>() / n;
        let centroid = Vec2::new(cx, cy);
        // Survivor preference: nearest the centroid. The legality gate below
        // can veto a survivor (its widened barrel or a reconnect jog would
        // land on foreign copper), and a different member of the same cluster
        // often passes — so this is an ordering, not a single choice.
        let mut order: Vec<usize> = members.clone();
        order.sort_by(|&a, &b| {
            d2(vias[a].position, centroid)
                .partial_cmp(&d2(vias[b].position, centroid))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Union of layer spans, in stackup order.
        let lo_all = members
            .iter()
            .map(|&i| layer_idx(vias[i].start_layer))
            .min()
            .unwrap_or(0);
        let hi_all = members
            .iter()
            .map(|&i| layer_idx(vias[i].end_layer))
            .max()
            .unwrap_or(stack.len().saturating_sub(1));

        let mut chosen: Option<(usize, Vec<RoutedTrace>)> = None;
        for &keep in &order {
            let (lo, hi) = (lo_all, hi_all);
            let keep_pos = vias[keep].position;
            let net = vias[keep].net.clone();
            let clr = session.clearance_for(&net);
            let (via_d, _) = via_geom_for(pcb, &net);
            // Layers the survivor would newly occupy: everything in the union span
            // outside its own original span.
            let (own_lo, own_hi) = {
                let (a, b) = (
                    layer_idx(vias[keep].start_layer),
                    layer_idx(vias[keep].end_layer),
                );
                (a.min(b), a.max(b))
            };
            let disc = CopperGeom::Disc {
                center: keep_pos,
                r: via_d / 2.0,
            };
            let span_ok = (lo..=hi)
                .filter(|&li| li < own_lo || li > own_hi)
                .all(|li| session.probe(&disc, stack[li], &net, clr).legal);
            if !span_ok {
                log::debug!(
                    "legalize: not merging {net} via cluster at ({:.3},{:.3}) — the widened \
                 barrel would land on foreign copper",
                    keep_pos.x,
                    keep_pos.y
                );
                continue;
            }

            // Same for the reconnect jogs: build them first, probe them all, and
            // only then commit the cluster.
            let mut cluster_jogs: Vec<RoutedTrace> = Vec::new();
            let mut jog_ok = true;
            for &i in members {
                if i == keep {
                    continue;
                }
                let gone = vias[i].position;
                if d2(gone, keep_pos).sqrt() <= POS_EPS {
                    continue;
                }
                let mut jogged: BTreeSet<usize> = BTreeSet::new();
                for t in traces.iter() {
                    if t.net != vias[i].net {
                        continue;
                    }
                    if d2(t.start, gone).sqrt() > POS_EPS && d2(t.end, gone).sqrt() > POS_EPS {
                        continue;
                    }
                    let li = layer_idx(t.layer);
                    if jogged.insert(li) {
                        cluster_jogs.push(RoutedTrace {
                            start: gone,
                            end: keep_pos,
                            width: t.width,
                            layer: t.layer,
                            net: t.net.clone(),
                        });
                    }
                }
            }
            for j in &cluster_jogs {
                let g = CopperGeom::Segment {
                    a: j.start,
                    b: j.end,
                    half_w: j.width / 2.0,
                };
                if !session
                    .probe(&g, j.layer, &j.net, session.clearance_for(&j.net))
                    .legal
                {
                    jog_ok = false;
                    break;
                }
            }
            if !jog_ok {
                log::debug!(
                    "legalize: not merging {net} via cluster at ({:.3},{:.3}) — a reconnect \
                 jog would cross foreign copper",
                    keep_pos.x,
                    keep_pos.y
                );
                continue;
            }

            chosen = Some((keep, cluster_jogs));
            break;
        }
        let Some((keep, cluster_jogs)) = chosen else {
            continue;
        };
        vias[keep].start_layer = stack[lo_all];
        vias[keep].end_layer = stack[hi_all];
        connectors.extend(cluster_jogs);

        for &i in members {
            if i != keep {
                drop.insert(i);
            }
        }
    }

    let removed = drop.len();
    if removed > 0 {
        let mut idx = 0;
        vias.retain(|_| {
            let keep = !drop.contains(&idx);
            idx += 1;
            keep
        });
        traces.extend(connectors);
    }
    removed
}

/// The board plus the router's output, as the caller would commit it — the
/// subject the DRC judges.
fn candidate_pcb(pcb: &Pcb, traces: &[RoutedTrace], vias: &[RoutedVia]) -> Pcb {
    let mut c = pcb.clone();
    for t in traces {
        c.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: Some(CopperSource::Autoroute),
        });
    }
    for v in vias {
        let (diameter, drill) = via_geom_for(pcb, &v.net);
        c.vias.push(Via {
            position: v.position,
            diameter,
            drill,
            start_layer: v.start_layer,
            end_layer: v.end_layer,
            net: v.net.clone(),
            source: Some(CopperSource::Autoroute),
        });
    }
    c
}

/// Bounding box of the router's new copper, inflated so scoped DRC sees every
/// interaction with neighboring board copper.
fn new_copper_bbox(traces: &[RoutedTrace], vias: &[RoutedVia]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut grow = |p: Vec2| {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    };
    for t in traces {
        grow(t.start);
        grow(t.end);
    }
    for v in vias {
        grow(v.position);
    }
    const MARGIN: f64 = 5.0;
    (
        Vec2::new(min.x - MARGIN, min.y - MARGIN),
        Vec2::new(max.x + MARGIN, max.y + MARGIN),
    )
}

/// If either hole of a `HoleToHole` violation is a via the router placed,
/// return that via's net. The violation's position is the midpoint between the
/// two hole centers, so membership is tested against both actual hole centers
/// via the message-independent geometry: a new via whose center is within its
/// pad diameter of the reported midpoint and whose hole pair distance matches.
fn new_via_at_holes(v: &DrcViolation, vias: &[RoutedVia]) -> Option<usize> {
    // The midpoint sits between the two holes; each hole center is within
    // (center distance)/2 of it. Center distance = actual + r_a + r_b, and all
    // router drills are sub-millimeter, so a generous radius bound suffices.
    let reach = (v.actual.abs() + 2.0).max(2.0);
    vias.iter()
        .enumerate()
        .filter(|(_, via)| d2(via.position, v.position).sqrt() <= reach)
        .min_by(|a, b| d2(a.1.position, v.position).total_cmp(&d2(b.1.position, v.position)))
        .map(|(i, _)| i)
}

/// Drop one plane-stitch via and the dog-bone stub that fed it: any same-net
/// trace with an endpoint on the via is that stub, and without the via it is
/// copper leading nowhere.
fn drop_stitch_via(
    idx: usize,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
    report: &mut LegalizeReport,
) {
    let via = vias.remove(idx);
    traces.retain(|t| {
        t.net != via.net
            || (d2(t.start, via.position) > POS_EPS * POS_EPS
                && d2(t.end, via.position) > POS_EPS * POS_EPS)
    });
    report.dropped_stitches.push(via.position);
}

/// Fail-closed: strip every piece of new copper on `net` and record the
/// demotion. An unrouted net is honest; hole-to-hole-illegal copper is not.
fn demote_net(
    net: &str,
    at: Vec2,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
    report: &mut LegalizeReport,
) {
    traces.retain(|t| t.net != net);
    vias.retain(|v| v.net != net);
    report.demoted.push((net.to_string(), at));
}

/// Try to clear one `SameNetBypass` violation by deleting a redundant new
/// trace segment at the contact point. Candidates are the new segments whose
/// copper covers the violation position, longest first (drop the noodle, keep
/// the shortest connecting subtree). A deletion is accepted only when the
/// continuity oracle proves the net is no worse off without the segment.
fn prune_bypass(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &[RoutedVia],
    v: &DrcViolation,
) -> bool {
    // The net is named in the message; recover it geometrically instead of
    // parsing, then take that net's segments at the contact point.
    let Some(net) = bypass_net(traces, v) else {
        return false;
    };
    let candidates = bypass_candidates(traces, &net, v.position);
    if candidates.is_empty() {
        return false;
    }

    for &i in &candidates {
        let net = traces[i].net.clone();
        let before = analyze_net_continuity(&candidate_pcb(pcb, traces, vias), &net);
        let removed = traces.remove(i);
        let after = analyze_net_continuity(&candidate_pcb(pcb, traces, vias), &net);
        let ok = after.islands <= before.islands && after.coverage >= before.coverage - 1e-9;
        if ok {
            return true;
        }
        traces.insert(i, removed);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board() -> Pcb {
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
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn rvia(x: f64, y: f64, net: &str) -> RoutedVia {
        RoutedVia {
            position: Vec2::new(x, y),
            net: net.into(),
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
        }
    }

    fn rtrace(a: (f64, f64), b: (f64, f64), net: &str, layer: PcbLayer) -> RoutedTrace {
        RoutedTrace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer,
            net: net.into(),
        }
    }

    /// Two same-net vias at the exact same spot collapse to one.
    #[test]
    fn coincident_same_net_vias_deduped() {
        let pcb = board();
        let mut traces = vec![rtrace((5.0, 5.0), (10.0, 10.0), "A", PcbLayer::FCu)];
        let mut vias = vec![rvia(10.0, 10.0, "A"), rvia(10.0, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert_eq!(report.merged_vias, 1);
        assert_eq!(vias.len(), 1);
        assert!(report.demoted.is_empty());
    }

    /// Overlapping same-net drills (negative hole-to-hole spacing) merge, and
    /// the trace that ended on the dropped via is reconnected with a jog.
    #[test]
    fn overlapping_same_net_vias_merged_and_reconnected() {
        let pcb = board();
        // 0.3mm apart: drills (0.4) overlap outright — edge dist = -0.1mm.
        let mut traces = vec![
            rtrace((5.0, 10.0), (10.0, 10.0), "A", PcbLayer::FCu),
            rtrace((10.3, 10.0), (20.0, 10.0), "A", PcbLayer::BCu),
        ];
        let mut vias = vec![rvia(10.0, 10.0, "A"), rvia(10.3, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert_eq!(report.merged_vias, 1);
        assert_eq!(vias.len(), 1);
        // The BCu trace's end at the dropped via must still reach the survivor:
        // a jog on BCu connects the old position to the kept one.
        let kept = vias[0].position;
        assert!(
            traces.iter().any(|t| t.layer == PcbLayer::BCu
                && (d2(t.start, kept).sqrt() <= POS_EPS || d2(t.end, kept).sqrt() <= POS_EPS)),
            "dropped via's trace must be jogged to the surviving via, traces: {traces:?}"
        );
        // And the merged output is hole-to-hole clean.
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        let h2h: Vec<_> = crate::drc::check_drc(&candidate)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::HoleToHole)
            .collect();
        assert!(h2h.is_empty(), "merge must clear HoleToHole, got {h2h:?}");
    }

    /// A merge that would drive copper through another net is refused, and the
    /// still-illegal cluster is then stripped fail-closed rather than returned.
    ///
    /// The reconnect jog used to be emitted unconditionally on the theory that
    /// it "stays inside copper the via already claimed" — which says nothing
    /// about what else is on that layer. Here a foreign trace sits between the
    /// two drills, so the jog would cross it at 0.000mm. Both vias carry BCu
    /// copper, so neither can be the survivor — whichever is kept, the jog
    /// that reconnects the other runs along the blocked layer.
    #[test]
    fn merge_refused_when_the_reconnect_jog_would_short() {
        let mut pcb = board();
        // Foreign BOARD copper straight across the jog's path on BCu. It has to
        // be board copper: new copper with no pad anchor is dead by
        // construction and the prune deletes it, so the gate rightly ignores it.
        pcb.traces.push(Trace {
            start: Vec2::new(10.15, 8.0),
            end: Vec2::new(10.15, 12.0),
            width: 0.25,
            layer: PcbLayer::BCu,
            net: "B".into(),
            source: None,
        });
        let mut traces = vec![
            rtrace((5.0, 10.0), (10.0, 10.0), "A", PcbLayer::BCu),
            rtrace((10.3, 10.0), (20.0, 10.0), "A", PcbLayer::BCu),
        ];
        let mut vias = vec![rvia(10.0, 10.0, "A"), rvia(10.3, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert_eq!(report.merged_vias, 0, "the merge must be refused");
        assert!(
            !traces.iter().any(|t| t.net == "A"
                && t.layer == PcbLayer::BCu
                && (t.start - Vec2::new(10.15, 10.0)).length() < 0.2
                && (t.end - Vec2::new(10.15, 10.0)).length() < 0.2),
            "no jog may be emitted across the foreign trace"
        );
        // Fail-closed: the cluster is still hole-to-hole illegal, so the DRC
        // loop strips A rather than returning an unmanufacturable board.
        assert_eq!(report.demoted.len(), 1);
        assert_eq!(report.demoted[0].0, "A");
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        let hard: Vec<_> = crate::drc::check_drc(&candidate)
            .into_iter()
            .filter(|v| {
                matches!(
                    v.rule,
                    DrcRuleType::HoleToHole | DrcRuleType::Clearance | DrcRuleType::Short
                )
            })
            .collect();
        assert!(hard.is_empty(), "output must be clean, got {hard:?}");
    }

    /// Different-net vias can't merge: the net whose via is illegally close to
    /// a fixed board hole is stripped, fail-closed.
    #[test]
    fn different_net_hole_conflict_demotes_net() {
        let mut pcb = board();
        // A pre-existing (manual) via on net B — a fixed hole the router must
        // respect.
        pcb.vias.push(Via {
            position: Vec2::new(10.0, 10.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "B".into(),
            source: Some(CopperSource::Manual),
        });
        let mut traces = vec![rtrace((10.3, 10.0), (20.0, 10.0), "A", PcbLayer::FCu)];
        let mut vias = vec![rvia(10.3, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert_eq!(report.demoted.len(), 1);
        assert_eq!(report.demoted[0].0, "A");
        assert!(vias.is_empty() && traces.is_empty());
    }

    /// A board with two pads on net A and a trace running between them, plus a
    /// floating stub the router left behind somewhere else on the same net.
    fn board_with_pads() -> Pcb {
        let mut pcb = board();
        let pad = |x: f64, y: f64, num: &str, net: &str| Footprint {
            reference: format!("U{num}"),
            value: String::new(),
            footprint_name: "test".into(),
            position: Vec2::new(x, y),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: num.into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.0,
                    height: 1.0,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                layers: vec![PcbLayer::FCu],
                net: Some(net.into()),
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        };
        pcb.footprints.push(pad(5.0, 5.0, "1", "A"));
        pcb.footprints.push(pad(20.0, 5.0, "2", "A"));
        pcb
    }

    /// Copper that reaches a pad stays; a floating island of the same net —
    /// trace plus the via that hangs off it — is removed.
    #[test]
    fn dangling_island_pruned_live_copper_kept() {
        let pcb = board_with_pads();
        let mut traces = vec![
            // Pad-to-pad: the live route.
            rtrace((5.0, 5.0), (20.0, 5.0), "A", PcbLayer::FCu),
            // Orphan: touches neither pad nor pour.
            rtrace((5.0, 20.0), (12.0, 20.0), "A", PcbLayer::FCu),
        ];
        let mut vias = vec![rvia(12.0, 20.0, "A")];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (1, 1));
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].start, Vec2::new(5.0, 5.0));
        assert!(vias.is_empty());
    }

    /// The prune only judges the router's own copper: board traces stay even
    /// when they are the dangling ones.
    #[test]
    fn existing_board_copper_never_pruned() {
        let mut pcb = board_with_pads();
        pcb.traces.push(Trace {
            start: Vec2::new(5.0, 20.0),
            end: Vec2::new(12.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Manual),
        });
        let mut traces = vec![rtrace((5.0, 5.0), (20.0, 5.0), "A", PcbLayer::FCu)];
        let mut vias = vec![];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (0, 0));
        assert_eq!(traces.len(), 1);
        assert_eq!(pcb.traces.len(), 1, "board copper is judged, never removed");
    }

    /// A chain of new copper is kept as a whole when any link reaches a pad —
    /// the island is the unit, not the individual segment.
    #[test]
    fn chained_copper_anchored_through_via_kept() {
        let pcb = board_with_pads();
        let mut traces = vec![
            rtrace((5.0, 5.0), (10.0, 5.0), "A", PcbLayer::FCu),
            rtrace((10.0, 5.0), (20.0, 5.0), "A", PcbLayer::BCu),
        ];
        let mut vias = vec![rvia(10.0, 5.0, "A")];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (0, 0));
        assert_eq!((traces.len(), vias.len()), (2, 1));
    }

    /// A plane-stitch via that lands hole-to-hole illegal is dropped on its own
    /// — one bad drill must not cost a poured net every other stitch it has.
    #[test]
    fn illegal_plane_stitch_is_dropped_without_demoting_the_net() {
        let mut pcb = board();
        pcb.vias.push(Via {
            position: Vec2::new(10.0, 10.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "B".into(),
            source: Some(CopperSource::Manual),
        });
        // Three GND plane stitches; only the first conflicts with the fixed
        // hole above, and it carries a dog-bone stub.
        let bad = Vec2::new(10.3, 10.0);
        let mut vias = vec![
            rvia(bad.x, bad.y, "GND"),
            rvia(30.0, 10.0, "GND"),
            rvia(30.0, 20.0, "GND"),
        ];
        let mut traces = vec![rtrace((9.0, 10.0), (bad.x, bad.y), "GND", PcbLayer::FCu)];
        let stitches = vec![bad, Vec2::new(30.0, 10.0), Vec2::new(30.0, 20.0)];
        let report = legalize(&pcb, &mut traces, &mut vias, &stitches, false);

        assert!(
            report.demoted.is_empty(),
            "the net must survive: {report:?}"
        );
        assert_eq!(report.dropped_stitches.len(), 1);
        assert_eq!(vias.len(), 2, "the other two stitches are kept");
        assert!(
            vias.iter().all(|v| d2(v.position, bad) > POS_EPS * POS_EPS),
            "the offending via is gone"
        );
        assert!(traces.is_empty(), "its dog-bone stub goes with it");

        // And the result really is hole-to-hole clean.
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        assert!(
            !crate::drc::check_drc(&candidate)
                .iter()
                .any(|v| v.rule == DrcRuleType::HoleToHole),
            "legalized output must be hole-to-hole clean"
        );
    }

    /// Diagnostic harness (not a gate): re-legalize a saved routed board.
    ///
    /// `VCAD_LEGALIZE_FIXTURE=/tmp/cm5.pcb.json cargo test --release -p
    /// vcad-ecad-pcb -- --ignored legalize_fixture`
    ///
    /// A fresh `cm5_bench` run clears the board's copper before routing, so
    /// every trace and via in the saved board is router output: stripping them
    /// back out reconstructs `legalize`'s own input.
    #[test]
    #[ignore = "needs a routed-board fixture"]
    fn legalize_fixture() {
        let Ok(path) = std::env::var("VCAD_LEGALIZE_FIXTURE") else {
            return;
        };
        let full: Pcb = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut base = full.clone();
        base.traces.clear();
        base.trace_arcs.clear();
        base.vias.clear();

        let mut traces: Vec<RoutedTrace> = full
            .traces
            .iter()
            .map(|t| RoutedTrace {
                start: t.start,
                end: t.end,
                width: t.width,
                layer: t.layer,
                net: t.net.clone(),
            })
            .collect();
        let mut vias: Vec<RoutedVia> = full
            .vias
            .iter()
            .map(|v| RoutedVia {
                position: v.position,
                net: v.net.clone(),
                start_layer: v.start_layer,
                end_layer: v.end_layer,
            })
            .collect();

        let count = |p: &Pcb, rule: DrcRuleType| {
            crate::drc::check_drc(p)
                .into_iter()
                .filter(|v| v.rule == rule)
                .count()
        };
        eprintln!(
            "BEFORE: {} traces, {} vias, bypass={}, h2h={}",
            traces.len(),
            vias.len(),
            count(&full, DrcRuleType::SameNetBypass),
            count(&full, DrcRuleType::HoleToHole),
        );

        let t0 = std::time::Instant::now();
        let report = legalize(&base, &mut traces, &mut vias, &[], true);
        let (dt, dv) = prune_dangling(&base, &mut traces, &mut vias);
        let after = candidate_pcb(&base, &traces, &vias);
        eprintln!(
            "AFTER ({:.1}s): {report:?}\n  pruned dangling {dt} traces / {dv} vias\n  \
             {} traces, {} vias, bypass={}, h2h={}",
            t0.elapsed().as_secs_f64(),
            traces.len(),
            vias.len(),
            count(&after, DrcRuleType::SameNetBypass),
            count(&after, DrcRuleType::HoleToHole),
        );
        for v in crate::drc::check_drc(&after)
            .iter()
            .filter(|v| matches!(v.rule, DrcRuleType::SameNetBypass | DrcRuleType::HoleToHole))
        {
            eprintln!("  REMAINS {:?} {}", v.rule, v.message);
            eprintln!(
                "    candidates covering the point: {:?}",
                traces
                    .iter()
                    .filter(|t| dist_to_trace(v.position, t) <= t.width)
                    .map(|t| (t.net.as_str(), t.layer, t.start, t.end))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Board-level repair clears a redundant bypass on a committed board —
    /// the invariant `si_finish` needs, since it rips, re-routes and prunes
    /// after `route_all`'s legalization has finished.
    #[test]
    fn board_repair_clears_a_redundant_bypass() {
        let mut pcb = board_with_pads();
        let t = |a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Autoroute),
        };
        // Pad (5,5) → pad (20,5) the long way round, as a hop chain...
        for k in 0..5 {
            pcb.traces
                .push(t((5.0 + k as f64 * 3.0, 5.0), (8.0 + k as f64 * 3.0, 5.0)));
        }
        // ...plus a redundant noodle shorting across the middle of it.
        // Landing mid-segment, not on a chain vertex: an end-to-end meeting is
        // an intended junction, and the rule rightly ignores those.
        pcb.traces.push(t((5.0, 5.0), (5.0, 8.0)));
        pcb.traces.push(t((5.0, 8.0), (12.5, 8.0)));
        pcb.traces.push(t((12.5, 8.0), (12.5, 5.0)));

        let before = crate::drc::check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .count();
        assert!(before > 0, "fixture must present a bypass");
        let unconnected = |p: &Pcb| analyze_net_continuity(p, "A").islands;
        let islands_before = unconnected(&pcb);

        let removed = repair_same_net_bypass(&mut pcb);
        assert!(removed > 0, "the redundant noodle must go");
        assert!(
            !crate::drc::check_drc(&pcb)
                .iter()
                .any(|v| v.rule == DrcRuleType::SameNetBypass),
            "repair must clear the bypass"
        );
        assert!(
            unconnected(&pcb) <= islands_before,
            "and must not clear it by disconnecting the net"
        );
    }

    /// The repair refuses when both sides of the contact are load-bearing: two
    /// legs of one net crossing, each the only path to a pad. Deleting either
    /// would clear the violation *by disconnecting the net* — the exact
    /// clean-by-deletion trap `fab-prep`'s connectivity guard exists to catch.
    /// These have to be prevented at routing time, not repaired after.
    #[test]
    fn board_repair_refuses_to_clean_by_deletion() {
        let mut pcb = board_with_pads();
        // Two more pads, so the net has four and each crossing leg is the sole
        // path to one of them.
        let mut extra = pcb.footprints[0].clone();
        extra.reference = "U3".into();
        extra.position = Vec2::new(5.0, 20.0);
        pcb.footprints.push(extra);
        let mut extra2 = pcb.footprints[1].clone();
        extra2.reference = "U4".into();
        extra2.position = Vec2::new(20.0, 20.0);
        pcb.footprints.push(extra2);
        let t = |a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Autoroute),
        };
        // Two chains that cross in the middle, each ending at its own pad.
        for k in 0..3 {
            let f = k as f64;
            pcb.traces.push(t(
                (5.0 + f * 5.0, 5.0 + f * 5.0),
                (10.0 + f * 5.0, 10.0 + f * 5.0),
            ));
            pcb.traces.push(t(
                (5.0 + f * 5.0, 20.0 - f * 5.0),
                (10.0 + f * 5.0, 15.0 - f * 5.0),
            ));
        }

        let islands_before = analyze_net_continuity(&pcb, "A").islands;
        let coverage_before = analyze_net_continuity(&pcb, "A").coverage;
        repair_same_net_bypass(&mut pcb);
        let after = analyze_net_continuity(&pcb, "A");
        assert!(
            after.islands <= islands_before && after.coverage >= coverage_before - 1e-9,
            "the repair must never trade connectivity for a clean DRC table: \
             islands {islands_before} -> {}, coverage {coverage_before} -> {}",
            after.islands,
            after.coverage
        );
    }

    /// Diagnostic harness (not a gate): board-level bypass repair on a saved
    /// routed board, which is exactly what `si_finish` hands back.
    ///
    /// `VCAD_LEGALIZE_FIXTURE=/tmp/cm5.pcb.json cargo test --release -p
    /// vcad-ecad-pcb -- --ignored repair_fixture`
    #[test]
    #[ignore = "needs a routed-board fixture"]
    fn repair_fixture() {
        let Ok(path) = std::env::var("VCAD_LEGALIZE_FIXTURE") else {
            return;
        };
        let mut pcb: Pcb = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let count = |p: &Pcb, rule: DrcRuleType| {
            crate::drc::check_drc(p)
                .into_iter()
                .filter(|v| v.rule == rule)
                .count()
        };
        let conn = |p: &Pcb| {
            crate::drc::check_drc(p)
                .into_iter()
                .filter(|v| {
                    matches!(
                        v.rule,
                        DrcRuleType::UnconnectedNet | DrcRuleType::NetIslands
                    )
                })
                .count()
        };
        eprintln!(
            "BEFORE: {} traces, bypass={}, h2h={}, connectivity={}",
            pcb.traces.len(),
            count(&pcb, DrcRuleType::SameNetBypass),
            count(&pcb, DrcRuleType::HoleToHole),
            conn(&pcb),
        );
        let t0 = std::time::Instant::now();
        let removed = repair_same_net_bypass(&mut pcb);
        eprintln!(
            "AFTER ({:.1}s): removed {removed} segment(s); {} traces, bypass={}, h2h={}, \
             connectivity={}",
            t0.elapsed().as_secs_f64(),
            pcb.traces.len(),
            count(&pcb, DrcRuleType::SameNetBypass),
            count(&pcb, DrcRuleType::HoleToHole),
            conn(&pcb),
        );
    }

    /// Every redundant bypass on the board is cleared, not just the first
    /// handful. The loop used to re-judge the whole board after each single
    /// repair under a 4-round cap, so a board with more than four bypasses
    /// always returned dirty — the CM5 arrived with 21 and left with 17.
    /// Bypasses on distinct nets are independent (a same-net contact's hop
    /// count is measured along that net's own copper), so one round clears
    /// them all.
    #[test]
    fn every_independent_bypass_is_cleared_not_just_the_first_few() {
        const NETS: usize = 7;
        let mut pcb = board();
        // One U-shaped conductor chain per net, spaced well apart, each with a
        // redundant noodle shorting across its own body.
        for n in 0..NETS {
            let net = format!("N{n}");
            let x = 3.0 + n as f64 * 6.0;
            for k in 0..4 {
                pcb.traces.push(Trace {
                    start: Vec2::new(x, 5.0 + k as f64 * 2.0),
                    end: Vec2::new(x, 7.0 + k as f64 * 2.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: net.clone(),
                    source: Some(CopperSource::Manual),
                });
            }
        }
        let mut traces: Vec<RoutedTrace> = Vec::new();
        for n in 0..NETS {
            let net = format!("N{n}");
            let x = 3.0 + n as f64 * 6.0;
            traces.push(rtrace((x, 5.0), (x + 2.0, 5.0), &net, PcbLayer::FCu));
            traces.push(rtrace((x + 2.0, 5.0), (x + 2.0, 10.0), &net, PcbLayer::FCu));
            traces.push(rtrace((x + 2.0, 10.0), (x, 10.0), &net, PcbLayer::FCu));
        }
        let mut vias = vec![];

        let before = crate::drc::check_drc(&candidate_pcb(&pcb, &traces, &vias))
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .count();
        /// The cap this loop used to run under, one repair per round.
        const OLD_CAP: usize = 4;
        assert!(
            before > OLD_CAP,
            "the fixture must present more bypasses than the old one-per-round loop \
             could ever clear, got {before}"
        );

        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert!(report.pruned_traces >= NETS, "{report:?}");
        let bypass: Vec<_> = crate::drc::check_drc(&candidate_pcb(&pcb, &traces, &vias))
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "all {before} must clear, left {bypass:?}"
        );
    }

    /// The contact point a segment-vs-segment bypass reports is the midpoint of
    /// the two centerlines' closest points — a point that lies on *neither*
    /// conductor. Candidate selection must still find the trace, or the pruner
    /// silently gives up. On the CM5 this left one bypass standing: the point
    /// sat 0.0439mm from a 0.08mm trace whose half-width is 0.04mm.
    #[test]
    fn bypass_candidates_found_when_the_contact_point_is_off_centerline() {
        let net = "A";
        // Two crossing fine traces, in the CM5's geometry: the reported contact
        // point falls between the centerlines, outside both half-widths.
        let a = RoutedTrace {
            start: Vec2::new(2.5, 6.6),
            end: Vec2::new(3.447, 6.992),
            width: 0.08,
            layer: PcbLayer::FCu,
            net: net.into(),
        };
        let b = RoutedTrace {
            start: Vec2::new(3.15, 6.6),
            end: Vec2::new(3.466, 7.362),
            width: 0.08,
            layer: PcbLayer::FCu,
            net: net.into(),
        };
        let traces = vec![a.clone(), b.clone()];
        let at = Vec2::new(3.390, 7.016);
        assert!(
            dist_to_trace(at, &a) > a.width / 2.0,
            "the fixture must place the point off the conductor, else it proves nothing"
        );
        let found = bypass_candidates(&traces, net, at);
        assert_eq!(found.len(), 2, "both crossing segments are candidates");
        assert_eq!(
            bypass_net(&traces, &bypass_violation(at)).as_deref(),
            Some(net)
        );
    }

    fn bypass_violation(at: Vec2) -> DrcViolation {
        DrcViolation {
            rule: DrcRuleType::SameNetBypass,
            severity: crate::drc::DrcSeverity::Warning,
            position: at,
            message: String::new(),
            actual: 0.0,
            required: 0.0,
            provenance: crate::drc::DrcProvenance::Routing,
            generated: false,
        }
    }

    /// A redundant same-net segment that brushes distant copper of its own net
    /// (SameNetBypass) is pruned when the net stays connected without it.
    #[test]
    fn bypass_noodle_pruned_when_redundant() {
        let mut pcb = board();
        // A long conductor chain on net A already on the board: a U shape.
        let t = |a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Manual),
        };
        pcb.traces.push(t((5.0, 5.0), (5.0, 7.0)));
        pcb.traces.push(t((5.0, 7.0), (5.0, 9.0)));
        pcb.traces.push(t((5.0, 9.0), (5.0, 11.0)));
        pcb.traces.push(t((5.0, 11.0), (5.0, 13.0)));
        // The router adds a noodle attached at the chain's start that wanders
        // around and lands mid-body on the chain's third segment — a same-net
        // contact 5 conductor hops from its own attachment point.
        let mut traces = vec![
            rtrace((5.0, 5.0), (8.0, 5.0), "A", PcbLayer::FCu),
            rtrace((8.0, 5.0), (8.0, 10.0), "A", PcbLayer::FCu),
            rtrace((8.0, 10.0), (5.0, 10.0), "A", PcbLayer::FCu),
        ];
        let mut vias = vec![];
        let report = legalize(&pcb, &mut traces, &mut vias, &[], false);
        assert!(report.pruned_traces >= 1, "the noodle must be pruned");
        // The result must be bypass-free.
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        let bypass: Vec<_> = crate::drc::check_drc(&candidate)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "prune must clear the bypass, got {bypass:?}"
        );
    }
}
