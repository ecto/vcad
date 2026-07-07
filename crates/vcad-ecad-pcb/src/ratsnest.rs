//! Ratsnest computation for unrouted net connections.
//!
//! Given a PCB and netlist, computes "air wires" between same-net pads that
//! are not yet connected by traces.

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::Pcb;
use vcad_ir::Vec2;

/// A single ratsnest line (unrouted connection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatsnestLine {
    /// Net name.
    pub net: String,
    /// Start position (world coordinates).
    pub from: Vec2,
    /// End position (world coordinates).
    pub to: Vec2,
    /// Footprint reference of the start pad.
    pub fp_ref: String,
    /// Pad number of the start pad.
    pub pad_num: String,
}

/// Netlist net with connections (matches the schematic crate's output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetlistNet {
    /// Net name.
    pub name: String,
    /// Component pin connections.
    pub connections: Vec<NetConnection>,
}

/// A connection point in a netlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetConnection {
    /// Reference designator.
    pub component_ref: String,
    /// Pin number.
    pub pin_number: String,
}

/// A netlist (re-used to avoid coupling to vcad-ecad-schematic directly).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Netlist {
    /// All nets.
    pub nets: Vec<NetlistNet>,
}

/// Compute ratsnest lines: unrouted connections between same-net pads.
///
/// Each net's air-wires form a Euclidean **minimum spanning tree** over its
/// pad positions (Prim's algorithm), not a sequential chain — so the displayed
/// ratsnest is the shortest set of connections that ties every pad together,
/// independent of the order pins appear in the netlist. A 2-pin net yields one
/// line; an n-pin net yields exactly n-1.
pub fn compute_ratsnest(pcb: &Pcb, netlist: &Netlist) -> Vec<RatsnestLine> {
    let mut lines = Vec::new();

    // Build pad position lookup: "REF:NUM" -> world position
    let mut pad_positions = std::collections::HashMap::new();
    for fp in &pcb.footprints {
        let fp_rot = fp.rotation.to_radians();
        let cos_r = fp_rot.cos();
        let sin_r = fp_rot.sin();
        for pad in &fp.pads {
            let key = format!("{}:{}", fp.reference, pad.number);
            let wx = fp.position.x + pad.position.x * cos_r - pad.position.y * sin_r;
            let wy = fp.position.y + pad.position.x * sin_r + pad.position.y * cos_r;
            pad_positions.insert(key, Vec2::new(wx, wy));
        }
    }

    for net in &netlist.nets {
        if net.connections.len() < 2 {
            continue;
        }

        // Skip nets that already have traces
        let has_trace = pcb.traces.iter().any(|t| t.net == net.name);
        if has_trace {
            continue;
        }

        // Gather the connection points whose pad position is known, preserving
        // netlist order so MST tie-breaking is deterministic.
        let nodes: Vec<(Vec2, &NetConnection)> = net
            .connections
            .iter()
            .filter_map(|c| {
                let key = format!("{}:{}", c.component_ref, c.pin_number);
                pad_positions.get(&key).map(|&pos| (pos, c))
            })
            .collect();
        if nodes.len() < 2 {
            continue;
        }

        for (from_idx, to_idx) in mst_edges(&nodes) {
            let (from_pos, from_conn) = nodes[from_idx];
            let (to_pos, _) = nodes[to_idx];
            lines.push(RatsnestLine {
                net: net.name.clone(),
                from: from_pos,
                to: to_pos,
                fp_ref: from_conn.component_ref.clone(),
                pad_num: from_conn.pin_number.clone(),
            });
        }
    }

    lines
}

/// Edges of a Euclidean minimum spanning tree over `nodes` via Prim's algorithm.
///
/// Returns `n-1` `(parent, child)` index pairs. Uses squared distance (monotonic
/// in true distance, so the MST is identical) and starts from node 0 with strict
/// `<` tie-breaking, making the result deterministic for a given node order.
fn mst_edges(nodes: &[(Vec2, &NetConnection)]) -> Vec<(usize, usize)> {
    let n = nodes.len();
    if n < 2 {
        return Vec::new();
    }
    let dist2 = |i: usize, j: usize| -> f64 {
        let a = nodes[i].0;
        let b = nodes[j].0;
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        dx * dx + dy * dy
    };

    let mut in_tree = vec![false; n];
    let mut best_dist = vec![f64::INFINITY; n];
    let mut best_parent = vec![0usize; n];
    in_tree[0] = true;
    for (j, d) in best_dist.iter_mut().enumerate().skip(1) {
        *d = dist2(0, j);
    }

    let mut edges = Vec::with_capacity(n - 1);
    for _ in 1..n {
        // Closest not-yet-connected node to the growing tree.
        let mut u = usize::MAX;
        let mut bd = f64::INFINITY;
        for j in 0..n {
            if !in_tree[j] && best_dist[j] < bd {
                bd = best_dist[j];
                u = j;
            }
        }
        if u == usize::MAX {
            break;
        }
        in_tree[u] = true;
        edges.push((best_parent[u], u));
        for j in 0..n {
            if !in_tree[j] {
                let d = dist2(u, j);
                if d < best_dist[j] {
                    best_dist[j] = d;
                    best_parent[j] = u;
                }
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn minimal_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 50.0),
                    Vec2::new(0.0, 50.0),
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
            footprints: vec![
                Footprint {
                    reference: "R1".into(),
                    value: "10k".into(),
                    footprint_name: "0805".into(),
                    position: Vec2::new(10.0, 20.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![
                        Pad {
                            number: "1".into(),
                            pad_type: PadType::SMD,
                            shape: PadShape::Rect {
                                width: 1.0,
                                height: 1.2,
                            },
                            position: Vec2::new(-1.0, 0.0),
                            rotation: 0.0,
                            drill: None,
                            net: Some("VCC".into()),
                            layers: vec![PcbLayer::FCu],
                        },
                        Pad {
                            number: "2".into(),
                            pad_type: PadType::SMD,
                            shape: PadShape::Rect {
                                width: 1.0,
                                height: 1.2,
                            },
                            position: Vec2::new(1.0, 0.0),
                            rotation: 0.0,
                            drill: None,
                            net: Some("GND".into()),
                            layers: vec![PcbLayer::FCu],
                        },
                    ],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
                Footprint {
                    reference: "C1".into(),
                    value: "100nF".into(),
                    footprint_name: "0805".into(),
                    position: Vec2::new(30.0, 20.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![
                        Pad {
                            number: "1".into(),
                            pad_type: PadType::SMD,
                            shape: PadShape::Rect {
                                width: 1.0,
                                height: 1.2,
                            },
                            position: Vec2::new(-1.0, 0.0),
                            rotation: 0.0,
                            drill: None,
                            net: Some("VCC".into()),
                            layers: vec![PcbLayer::FCu],
                        },
                        Pad {
                            number: "2".into(),
                            pad_type: PadType::SMD,
                            shape: PadShape::Rect {
                                width: 1.0,
                                height: 1.2,
                            },
                            position: Vec2::new(1.0, 0.0),
                            rotation: 0.0,
                            drill: None,
                            net: Some("GND".into()),
                            layers: vec![PcbLayer::FCu],
                        },
                    ],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
            ],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn conn(refdes: &str) -> NetConnection {
        NetConnection {
            component_ref: refdes.into(),
            pin_number: "1".into(),
        }
    }

    /// PCB with one single-pad footprint per `(ref, x, y)`, all on net "SIG".
    fn line_pcb(positions: &[(&str, f64, f64)]) -> Pcb {
        let mut pcb = minimal_pcb();
        pcb.footprints.clear();
        for (refdes, x, y) in positions {
            pcb.footprints.push(Footprint {
                reference: (*refdes).into(),
                value: "x".into(),
                footprint_name: "0402".into(),
                position: Vec2::new(*x, *y),
                rotation: 0.0,
                front: true,
                pads: vec![Pad {
                    number: "1".into(),
                    pad_type: PadType::SMD,
                    shape: PadShape::Rect {
                        width: 0.5,
                        height: 0.5,
                    },
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    drill: None,
                    net: Some("SIG".into()),
                    layers: vec![PcbLayer::FCu],
                }],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            });
        }
        pcb
    }

    fn sig_netlist(order: &[&str]) -> Netlist {
        Netlist {
            nets: vec![NetlistNet {
                name: "SIG".into(),
                connections: order.iter().map(|r| conn(r)).collect(),
            }],
        }
    }

    fn total_len(lines: &[RatsnestLine]) -> f64 {
        lines
            .iter()
            .map(|l| (l.to.x - l.from.x).hypot(l.to.y - l.from.y))
            .sum()
    }

    #[test]
    fn ratsnest_three_pin_is_minimum_spanning_tree() {
        // A at x=0, B at x=10, C at x=11 — B and C nearly coincident.
        let pcb = line_pcb(&[("A", 0.0, 0.0), ("B", 10.0, 0.0), ("C", 11.0, 0.0)]);
        // Netlist order A, C, B — deliberately NOT the nearest-neighbour order.
        let lines = compute_ratsnest(&pcb, &sig_netlist(&["A", "C", "B"]));
        assert_eq!(lines.len(), 2, "3-pin net -> exactly 2 edges");
        // MST total = 10 (A-B) + 1 (B-C) = 11; the old chain A-C-B would be 11+1=12.
        assert!(
            (total_len(&lines) - 11.0).abs() < 1e-9,
            "MST length {} should be 11, not the chain's 12",
            total_len(&lines)
        );
    }

    #[test]
    fn ratsnest_is_order_independent() {
        let pcb = line_pcb(&[("A", 0.0, 0.0), ("B", 10.0, 0.0), ("C", 11.0, 0.0)]);
        let len_of = |order: &[&str]| total_len(&compute_ratsnest(&pcb, &sig_netlist(order)));
        let baseline = len_of(&["A", "B", "C"]);
        // Any permutation of the pins yields the same minimum-spanning-tree length.
        assert!((len_of(&["C", "A", "B"]) - baseline).abs() < 1e-9);
        assert!((len_of(&["B", "C", "A"]) - baseline).abs() < 1e-9);
    }

    #[test]
    fn ratsnest_with_connections() {
        let pcb = minimal_pcb();
        let netlist = Netlist {
            nets: vec![NetlistNet {
                name: "VCC".into(),
                connections: vec![
                    NetConnection {
                        component_ref: "R1".into(),
                        pin_number: "1".into(),
                    },
                    NetConnection {
                        component_ref: "C1".into(),
                        pin_number: "1".into(),
                    },
                ],
            }],
        };

        let lines = compute_ratsnest(&pcb, &netlist);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].net, "VCC");
    }

    #[test]
    fn ratsnest_skips_routed_nets() {
        let mut pcb = minimal_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(9.0, 20.0),
            end: Vec2::new(29.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "VCC".into(),
            source: None,
        });

        let netlist = Netlist {
            nets: vec![NetlistNet {
                name: "VCC".into(),
                connections: vec![
                    NetConnection {
                        component_ref: "R1".into(),
                        pin_number: "1".into(),
                    },
                    NetConnection {
                        component_ref: "C1".into(),
                        pin_number: "1".into(),
                    },
                ],
            }],
        };

        let lines = compute_ratsnest(&pcb, &netlist);
        assert!(lines.is_empty());
    }

    #[test]
    fn ratsnest_empty_netlist() {
        let pcb = minimal_pcb();
        let netlist = Netlist { nets: vec![] };
        let lines = compute_ratsnest(&pcb, &netlist);
        assert!(lines.is_empty());
    }
}
