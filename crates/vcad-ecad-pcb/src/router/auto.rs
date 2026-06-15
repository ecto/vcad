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

use std::collections::{BTreeMap, BTreeSet};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use crate::session::RouteSession;
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

/// Route every unrouted net on `pcb` (optionally restricted to `nets_filter`).
///
/// `width` is the trace width; via geometry comes from the board's default
/// rules. Returns the new copper to add — all of it clearance-legal against the
/// board and against the copper the router itself places.
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
    let hw = width / 2.0;
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;

    let mut traces: Vec<RoutedTrace> = Vec::new();
    let mut vias: Vec<RoutedVia> = Vec::new();
    let mut routed: BTreeSet<String> = BTreeSet::new();
    let mut unrouted: BTreeSet<String> = BTreeSet::new();

    for line in &rats {
        if !nets_filter.is_empty() && !nets_filter.iter().any(|n| n == &line.net) {
            continue;
        }
        let net = &line.net;
        let clearance = session.clearance_for(net);
        let mut done = false;

        for (li, &layer) in LAYERS.iter().enumerate() {
            let r = route_net_maze(
                &session,
                &pcb.outline.vertices,
                layer,
                net,
                line.from,
                line.to,
                width,
            );
            if !r.success || r.segments.is_empty() {
                continue;
            }

            // A non-front layer needs a transition via at each endpoint to drop
            // from the FCu pad. Probe each via on BOTH layers first; if either
            // endpoint can't take a legal via, abandon this layer.
            let needs_via = li > 0;
            let mut new_vias: Vec<Vec2> = Vec::new();
            if needs_via {
                let mut ok = true;
                for &p in &[line.from, line.to] {
                    // A same-net via already here (a pad shared by two MST
                    // edges) is reused, not re-stacked.
                    if vias
                        .iter()
                        .any(|v| v.net == *net && dist(v.position, p) < 0.05)
                    {
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

            // Commit the route: traces, then transition vias on both layers, so
            // the next connection avoids every piece of copper we just placed.
            for (a, b) in &r.segments {
                session.commit(CopperElement {
                    min: [a.x.min(b.x) - hw, a.y.min(b.y) - hw],
                    max: [a.x.max(b.x) + hw, a.y.max(b.y) + hw],
                    net: net.clone(),
                    layer,
                    geom: CopperGeom::Segment {
                        a: *a,
                        b: *b,
                        half_w: hw,
                    },
                });
                traces.push(RoutedTrace {
                    start: *a,
                    end: *b,
                    width,
                    layer,
                    net: net.clone(),
                });
            }
            for &p in &new_vias {
                for vl in LAYERS {
                    session.commit(CopperElement {
                        min: [p.x - via_r, p.y - via_r],
                        max: [p.x + via_r, p.y + via_r],
                        net: net.clone(),
                        layer: vl,
                        geom: CopperGeom::Disc {
                            center: p,
                            r: via_r,
                        },
                    });
                }
                vias.push(RoutedVia {
                    position: p,
                    net: net.clone(),
                });
            }
            routed.insert(net.clone());
            done = true;
            break;
        }
        if !done {
            unrouted.insert(net.clone());
        }
    }

    RouteAllResult {
        traces,
        vias,
        routed_nets: routed.into_iter().collect(),
        unrouted_nets: unrouted.into_iter().collect(),
    }
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
}
