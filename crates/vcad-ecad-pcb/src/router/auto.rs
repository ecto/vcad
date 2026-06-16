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

use crate::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use crate::session::{RouteSession, SpanId};
use crate::spatial::{CopperElement, CopperGeom};

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
}

/// Copper layers tried per connection, in order (front first, then back).
const LAYERS: [PcbLayer; 2] = [PcbLayer::FCu, PcbLayer::BCu];

/// Maximum rip-up-and-reroute rounds before accepting the still-unrouted set.
/// The loop also stops the instant a round places nothing new, so this only
/// bounds worst-case work on a genuinely over-constrained board.
const MAX_RIPUP_ROUNDS: usize = 8;

/// A connection that has been routed, plus the session spans it occupies —
/// enough to rip it back out and re-route it.
struct Placed {
    net: String,
    from: Vec2,
    to: Vec2,
    layer: PcbLayer,
    width: f64,
    segments: Vec<(Vec2, Vec2)>,
    via_pts: Vec<Vec2>,
    spans: Vec<SpanId>,
}

/// Route every unrouted net on `pcb` (optionally restricted to `nets_filter`).
///
/// Routes greedily (longest connection first) against one growing
/// [`RouteSession`], then runs a single-level rip-up pass to place connections
/// that were blocked. `width` is the trace width; via geometry comes from the
/// board's default rules. Returns the new copper to add — all of it
/// clearance-legal against the board and against the copper the router places.
pub fn route_all(pcb: &Pcb, width: f64, nets_filter: &[String]) -> RouteAllResult {
    let netlist = netlist_from_pads(pcb);
    let mut rats = compute_ratsnest(pcb, &netlist);
    // Route the longest connections first. They span the most board and have
    // the least routing freedom, so giving them the emptier board up front
    // leaves fewer dead-ends for the short connections that fill in around them.
    rats.sort_by(|a, b| {
        dist(b.from, b.to)
            .partial_cmp(&dist(a.from, a.to))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut session = RouteSession::from_pcb(pcb);
    let mut placed: Vec<Placed> = Vec::new();
    let mut unrouted_conns: Vec<(String, Vec2, Vec2)> = Vec::new();

    // Greedy pass: route each connection against the growing session.
    for line in &rats {
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
        pending = ripup_pass(&mut session, pcb, width, &mut placed, pending);
        if placed.len() <= placed_before {
            break;
        }
    }
    let still_unrouted = pending;

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
        for &pt in &p.via_pts {
            vias.push(RoutedVia {
                position: pt,
                net: p.net.clone(),
            });
        }
    }
    let unrouted: BTreeSet<String> = still_unrouted.into_iter().map(|(n, _, _)| n).collect();

    RouteAllResult {
        traces,
        vias,
        routed_nets: routed.into_iter().collect(),
        unrouted_nets: unrouted.into_iter().collect(),
    }
}

/// Try to route one connection on FCu then BCu against `session`. On success,
/// commits the copper (traces, plus transition vias for a back-layer route) to
/// `session` and returns the [`Placed`] record; otherwise returns `None`
/// without mutating the session. Every committed segment and via is probed —
/// there is no path here that commits illegal copper.
fn try_route(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &[Placed],
) -> Option<Placed> {
    // Net-class width if the net has one (wider power/ground), else the caller's
    // default. The same width drives the maze search, the committed copper, and
    // the reported trace.
    let w = session.width_for(net, width);
    let hw = w / 2.0;
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let clearance = session.clearance_for(net);

    for (li, &layer) in LAYERS.iter().enumerate() {
        let r = route_net_maze(session, &pcb.outline.vertices, layer, net, from, to, w);
        if !r.success || r.segments.is_empty() {
            continue;
        }

        // A back-layer route needs a transition via at each endpoint. Probe
        // each on BOTH layers before committing; reuse a same-net via already
        // dropped at a shared pad rather than stacking a coincident drill.
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
                let legal = session.probe(&disc, PcbLayer::FCu, net, clearance).legal
                    && session.probe(&disc, PcbLayer::BCu, net, clearance).legal;
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
        for (a, b) in &r.segments {
            spans.push(session.commit(CopperElement {
                min: [a.x.min(b.x) - hw, a.y.min(b.y) - hw],
                max: [a.x.max(b.x) + hw, a.y.max(b.y) + hw],
                net: net.to_string(),
                layer,
                geom: CopperGeom::Segment {
                    a: *a,
                    b: *b,
                    half_w: hw,
                },
            }));
        }
        for &p in &new_vias {
            for vl in LAYERS {
                spans.push(session.commit(CopperElement {
                    min: [p.x - via_r, p.y - via_r],
                    max: [p.x + via_r, p.y + via_r],
                    net: net.to_string(),
                    layer: vl,
                    geom: CopperGeom::Disc {
                        center: p,
                        r: via_r,
                    },
                }));
            }
        }
        return Some(Placed {
            net: net.to_string(),
            from,
            to,
            layer,
            width: w,
            segments: r.segments,
            via_pts: new_vias,
            spans,
        });
    }
    None
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
fn ripup_pass(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    placed: &mut Vec<Placed>,
    unrouted: Vec<(String, Vec2, Vec2)>,
) -> Vec<(String, Vec2, Vec2)> {
    let hw = width / 2.0;
    let mut still: Vec<(String, Vec2, Vec2)> = Vec::new();

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
        let routed_target = try_route(session, pcb, width, &net, from, to, placed);
        if let Some(p) = routed_target {
            placed.push(p);
        } else {
            still.push((net, from, to));
        }

        // Re-route every victim; one that can't be placed becomes unrouted.
        for v in victims {
            match try_route(session, pcb, width, &v.net, v.from, v.to, placed) {
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
}
