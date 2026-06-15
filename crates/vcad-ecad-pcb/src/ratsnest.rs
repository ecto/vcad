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

        // Create sequential ratsnest between consecutive pads
        for i in 0..net.connections.len() - 1 {
            let a = &net.connections[i];
            let b = &net.connections[i + 1];
            let key_a = format!("{}:{}", a.component_ref, a.pin_number);
            let key_b = format!("{}:{}", b.component_ref, b.pin_number);
            if let (Some(&pos_a), Some(&pos_b)) =
                (pad_positions.get(&key_a), pad_positions.get(&key_b))
            {
                lines.push(RatsnestLine {
                    net: net.name.clone(),
                    from: pos_a,
                    to: pos_b,
                    fp_ref: a.component_ref.clone(),
                    pad_num: a.pin_number.clone(),
                });
            }
        }
    }

    lines
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
