#![warn(missing_docs)]
//! Netlist generation and ERC (electrical rule checking) for vcad schematics.
//!
//! Given a [`vcad_ir::ecad::SchematicSheet`], this crate can:
//! - Extract a [`Netlist`] by tracing wire connectivity via union-find
//! - Run ERC checks to detect wiring errors via [`erc::check_erc`]

use std::collections::{HashMap, HashSet};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

pub mod erc;
pub mod geometry;

// ============================================================================
// Netlist types
// ============================================================================

/// A connection point in a netlist: a specific pin on a specific component.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NetConnection {
    /// Reference designator of the component (e.g. "R1", "U3").
    pub component_ref: String,
    /// Pin number on that component (e.g. "1", "A1").
    pub pin_number: String,
}

/// A net in the netlist: a named set of connected pins.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NetlistNet {
    /// Net name (from a label, or auto-generated as "NET-001" etc.).
    pub name: String,
    /// All component pins connected to this net.
    pub connections: Vec<NetConnection>,
}

/// A complete netlist extracted from a schematic.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Netlist {
    /// All nets in the design.
    pub nets: Vec<NetlistNet>,
}

// ============================================================================
// Union-Find
// ============================================================================

/// Simple union-find (disjoint set) structure for merging connected points.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
    }
}

// ============================================================================
// Geometry helpers
// ============================================================================

/// Tolerance for matching positions on a schematic (in schematic units).
const POSITION_TOLERANCE: f64 = 0.01;

/// Check if two 2D points are coincident within tolerance.
pub(crate) fn points_coincident(a: &Vec2, b: &Vec2) -> bool {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy) < POSITION_TOLERANCE * POSITION_TOLERANCE
}

/// Whether point `p` lies on the segment `a`--`b` (within tolerance), including
/// its interior — i.e. a T-tap or a pin/junction sitting on a wire. This is
/// deliberately a point-vs-segment test, never segment-vs-segment: two wires
/// that merely *cross* (each through the other's interior, with no endpoint,
/// pin, or junction at the crossing) are NOT connected.
pub(crate) fn point_on_segment(p: &Vec2, a: &Vec2, b: &Vec2) -> bool {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 < POSITION_TOLERANCE * POSITION_TOLERANCE {
        // Degenerate (zero-length) wire: fall back to coincidence.
        return points_coincident(p, a);
    }
    // Project p onto the segment, clamped to [0, 1].
    let t = ((p.x - a.x) * abx + (p.y - a.y) * aby) / len2;
    if !(0.0..=1.0).contains(&t) {
        return false;
    }
    let cx = a.x + t * abx;
    let cy = a.y + t * aby;
    let dx = p.x - cx;
    let dy = p.y - cy;
    (dx * dx + dy * dy) < POSITION_TOLERANCE * POSITION_TOLERANCE
}

/// Compute the absolute position of a pin given the parent component's
/// position, rotation (degrees), and mirror state.
pub fn pin_world_position(comp: &SchematicComponent, pin: &SchematicPin) -> Vec2 {
    let angle_rad = comp.rotation.to_radians();
    let cos = angle_rad.cos();
    let sin = angle_rad.sin();

    let px = pin.position.x;
    let py = pin.position.y;

    // Apply mirror (horizontal flip before rotation).
    let mx = if comp.mirror { -px } else { px };
    let my = py;

    // Rotate then translate.
    Vec2::new(
        comp.position.x + mx * cos - my * sin,
        comp.position.y + mx * sin + my * cos,
    )
}

// ============================================================================
// Netlist generation
// ============================================================================

/// A point in the connectivity graph, with an index into the union-find.
#[derive(Debug)]
struct ConnPoint {
    pos: Vec2,
    uf_index: usize,
}

/// Generate a netlist from a schematic sheet.
///
/// Algorithm:
/// 1. Collect all relevant points: wire endpoints, pin positions, junction
///    positions, and label positions.
/// 2. Assign each point an index in a union-find structure.
/// 3. Merge points that share the same position (within tolerance).
/// 4. Additionally, merge each wire's start and end points (they are
///    electrically connected through the wire).
/// 5. Group the resulting connected components and assign names from labels
///    (or auto-generate).
/// 6. Map component pins to their connected nets.
pub fn generate_netlist(sheet: &SchematicSheet) -> Netlist {
    // Phase 1: Collect all connectivity points.
    let mut points: Vec<ConnPoint> = Vec::new();
    let mut next_idx: usize = 0;

    // Track wire endpoint indices so we can union start<->end per wire.
    let mut wire_pairs: Vec<(usize, usize)> = Vec::new();

    for wire in &sheet.wires {
        let start_idx = next_idx;
        points.push(ConnPoint {
            pos: wire.start,
            uf_index: start_idx,
        });
        next_idx += 1;

        let end_idx = next_idx;
        points.push(ConnPoint {
            pos: wire.end,
            uf_index: end_idx,
        });
        next_idx += 1;

        wire_pairs.push((start_idx, end_idx));
    }

    // Track pin indices for later mapping: (component_ref, pin_number) -> uf_index.
    let mut pin_indices: Vec<(String, String, usize)> = Vec::new();

    for comp in &sheet.components {
        for pin in &comp.pins {
            let world_pos = pin_world_position(comp, pin);
            let idx = next_idx;
            points.push(ConnPoint {
                pos: world_pos,
                uf_index: idx,
            });
            pin_indices.push((comp.reference.clone(), pin.number.clone(), idx));
            next_idx += 1;
        }
    }

    // Junction points.
    let mut junction_indices: Vec<usize> = Vec::new();
    for junction in &sheet.junctions {
        let idx = next_idx;
        points.push(ConnPoint {
            pos: junction.position,
            uf_index: idx,
        });
        junction_indices.push(idx);
        next_idx += 1;
    }

    // Label points.
    let mut label_indices: Vec<(String, usize)> = Vec::new();
    for label in &sheet.labels {
        let idx = next_idx;
        points.push(ConnPoint {
            pos: label.position,
            uf_index: idx,
        });
        label_indices.push((label.name.clone(), idx));
        next_idx += 1;
    }

    // Phase 2: Build union-find and merge.
    let total = next_idx;
    if total == 0 {
        return Netlist { nets: Vec::new() };
    }

    let mut uf = UnionFind::new(total);

    // Merge wire start and end (they are the same net through the wire).
    for &(s, e) in &wire_pairs {
        uf.union(s, e);
    }

    // Merge all points that share the same position.
    // For small-to-medium schematics an O(n^2) approach is fine.
    // For very large schematics a spatial index could be used.
    let n = points.len();
    for i in 0..n {
        for j in (i + 1)..n {
            if points_coincident(&points[i].pos, &points[j].pos) {
                uf.union(points[i].uf_index, points[j].uf_index);
            }
        }
    }

    // Connect points that lie on a wire's interior (T-taps): a wire endpoint,
    // pin, junction, or label sitting on another wire joins that wire's net.
    // This is what makes bus-style wiring work — route one wire through a row
    // of pins and they all connect. Crucially it is point-on-segment only, so
    // two wires that merely cross (no point at the intersection) stay on
    // separate nets; a real connection there needs an explicit junction dot.
    for (k, wire) in sheet.wires.iter().enumerate() {
        let wire_root_idx = wire_pairs[k].0;
        for p in &points {
            // Skip this wire's own endpoints (already unioned start<->end).
            if p.uf_index == wire_pairs[k].0 || p.uf_index == wire_pairs[k].1 {
                continue;
            }
            if point_on_segment(&p.pos, &wire.start, &wire.end) {
                uf.union(p.uf_index, wire_root_idx);
            }
        }
    }

    // Phase 3: Group into nets.
    // Map root -> set of pin connections and labels.
    let mut net_pins: HashMap<usize, Vec<NetConnection>> = HashMap::new();
    let mut net_labels: HashMap<usize, HashSet<String>> = HashMap::new();

    for &(ref comp_ref, ref pin_num, idx) in &pin_indices {
        let root = uf.find(idx);
        net_pins.entry(root).or_default().push(NetConnection {
            component_ref: comp_ref.clone(),
            pin_number: pin_num.clone(),
        });
    }

    for &(ref name, idx) in &label_indices {
        let root = uf.find(idx);
        net_labels.entry(root).or_default().insert(name.clone());
    }

    // Collect all unique roots that have at least one pin connection.
    let all_roots: HashSet<usize> = net_pins.keys().copied().collect();

    // Build output nets, sorted by name for deterministic output.
    let mut auto_counter: u32 = 1;
    let mut nets: Vec<NetlistNet> = Vec::new();

    let mut sorted_roots: Vec<usize> = all_roots.into_iter().collect();
    sorted_roots.sort();

    for root in sorted_roots {
        let name = if let Some(labels) = net_labels.get(&root) {
            // Pick first label alphabetically for determinism.
            let mut sorted: Vec<&String> = labels.iter().collect();
            sorted.sort();
            sorted[0].clone()
        } else {
            let name = format!("NET-{:03}", auto_counter);
            auto_counter += 1;
            name
        };

        let mut connections = net_pins.remove(&root).unwrap_or_default();
        // Sort connections for deterministic output.
        connections.sort_by(|a, b| {
            a.component_ref
                .cmp(&b.component_ref)
                .then_with(|| a.pin_number.cmp(&b.pin_number))
        });

        nets.push(NetlistNet { name, connections });
    }

    // Sort nets by name.
    nets.sort_by(|a, b| a.name.cmp(&b.name));

    Netlist { nets }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Vec2;

    /// Helper: build a simple resistor component.
    fn make_resistor(reference: &str, position: Vec2) -> SchematicComponent {
        SchematicComponent {
            reference: reference.to_string(),
            value: "10k".to_string(),
            footprint_id: "Resistor_SMD:R_0805".to_string(),
            position,
            rotation: 0.0,
            mirror: false,
            pins: vec![
                SchematicPin {
                    number: "1".to_string(),
                    name: "~".to_string(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(-5.0, 0.0),
                },
                SchematicPin {
                    number: "2".to_string(),
                    name: "~".to_string(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(5.0, 0.0),
                },
            ],
            properties: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn empty_schematic_produces_empty_netlist() {
        let sheet = SchematicSheet {
            title: None,
            components: vec![],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
        };
        let netlist = generate_netlist(&sheet);
        assert!(netlist.nets.is_empty());
    }

    #[test]
    fn single_wire_connects_two_pins() {
        // R1 at (10, 0) with pins at (-5,0) and (5,0) => world (5,0) and (15,0)
        // R2 at (25, 0) with pins at (-5,0) and (5,0) => world (20,0) and (30,0)
        // Wire from (15,0) to (20,0) connects R1 pin 2 to R2 pin 1
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_resistor("R1", Vec2::new(10.0, 0.0)),
                make_resistor("R2", Vec2::new(25.0, 0.0)),
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(15.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let netlist = generate_netlist(&sheet);

        // There should be 3 nets:
        // - R1 pin 1 (unconnected, still forms a single-pin net)
        // - R1 pin 2 + R2 pin 1 (connected by wire)
        // - R2 pin 2 (unconnected)
        assert_eq!(netlist.nets.len(), 3);

        // Find the net with 2 connections
        let connected_net = netlist.nets.iter().find(|n| n.connections.len() == 2);
        assert!(connected_net.is_some());
        let net = connected_net.unwrap();
        assert_eq!(net.connections.len(), 2);

        // Should contain R1 pin 2 and R2 pin 1
        let has_r1_p2 = net
            .connections
            .iter()
            .any(|c| c.component_ref == "R1" && c.pin_number == "2");
        let has_r2_p1 = net
            .connections
            .iter()
            .any(|c| c.component_ref == "R2" && c.pin_number == "1");
        assert!(has_r1_p2);
        assert!(has_r2_p1);
    }

    #[test]
    fn tap_onto_wire_interior_connects() {
        // A vertical bus rises from R1 pin 1; R2 pin 1 sits on the bus interior
        // and should join the same net (T-tap), not form its own.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_resistor("R1", Vec2::new(10.0, 0.0)), // pins world (5,0),(15,0)
                make_resistor("R2", Vec2::new(10.0, 20.0)), // pins world (5,20),(15,20)
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(5.0, 0.0), // R1 pin 1
                end: Vec2::new(5.0, 40.0),  // vertical bus, passes through (5,20) = R2 pin 1
            }],
            junctions: vec![],
            labels: vec![],
        };

        let netlist = generate_netlist(&sheet);
        let net = netlist
            .nets
            .iter()
            .find(|n| {
                n.connections
                    .iter()
                    .any(|c| c.component_ref == "R1" && c.pin_number == "1")
            })
            .expect("R1 pin 1 net");
        assert!(
            net.connections
                .iter()
                .any(|c| c.component_ref == "R2" && c.pin_number == "1"),
            "R2 pin 1 should tap onto the bus and share R1 pin 1's net, got {:?}",
            net.connections,
        );
    }

    #[test]
    fn crossing_wires_stay_separate() {
        // A horizontal wire and a vertical wire cross at (20,0) with no pin or
        // junction there. They must NOT merge: R1 (on the horizontal) and R2
        // (on the vertical) stay on different nets.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_resistor("R1", Vec2::new(5.0, 0.0)), // pins (0,0),(10,0) on the horizontal
                make_resistor("R2", Vec2::new(15.0, -10.0)), // pin 2 (20,-10) on the vertical
            ],
            wires: vec![
                SchematicWire {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(40.0, 0.0),
                },
                SchematicWire {
                    start: Vec2::new(20.0, -10.0),
                    end: Vec2::new(20.0, 10.0),
                },
            ],
            junctions: vec![],
            labels: vec![],
        };

        let netlist = generate_netlist(&sheet);
        let r1_net = netlist
            .nets
            .iter()
            .find(|n| {
                n.connections
                    .iter()
                    .any(|c| c.component_ref == "R1" && c.pin_number == "1")
            })
            .expect("R1 pin 1 net");
        assert!(
            !r1_net.connections.iter().any(|c| c.component_ref == "R2"),
            "crossing wires must not connect; R1 net = {:?}",
            r1_net.connections,
        );
    }

    #[test]
    fn label_assigns_net_name() {
        // R1 at (10, 0): pin 1 at world (5,0), pin 2 at world (15,0)
        // Wire from (5,0) to (0,0)
        // Label "VCC" at (0,0)
        let sheet = SchematicSheet {
            title: None,
            components: vec![make_resistor("R1", Vec2::new(10.0, 0.0))],
            wires: vec![SchematicWire {
                start: Vec2::new(5.0, 0.0),
                end: Vec2::new(0.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "VCC".to_string(),
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                scope: LabelScope::Global,
            }],
        };

        let netlist = generate_netlist(&sheet);

        // R1 pin 1 should be on the "VCC" net
        let vcc_net = netlist.nets.iter().find(|n| n.name == "VCC");
        assert!(vcc_net.is_some(), "Expected a net named VCC");
        let vcc = vcc_net.unwrap();
        assert!(vcc
            .connections
            .iter()
            .any(|c| c.component_ref == "R1" && c.pin_number == "1"));
    }

    #[test]
    fn auto_naming_when_no_label() {
        // Two resistors connected by a wire, no labels
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_resistor("R1", Vec2::new(10.0, 0.0)),
                make_resistor("R2", Vec2::new(25.0, 0.0)),
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(15.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let netlist = generate_netlist(&sheet);
        // All nets should be auto-named NET-001, NET-002, ...
        for net in &netlist.nets {
            assert!(
                net.name.starts_with("NET-"),
                "Expected auto-generated name, got: {}",
                net.name
            );
        }
    }

    #[test]
    fn junction_merges_wires() {
        // Three wires meeting at a junction at (10,0):
        //  wire A: (0,0) -> (10,0)
        //  wire B: (10,0) -> (20,0)
        //  wire C: (10,0) -> (10,10)
        // R1 at (0,0) with pin offsets, but let's place pins directly:
        // Use components with pin at (0,0), (20,0), (10,10)
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                SchematicComponent {
                    reference: "R1".to_string(),
                    value: "10k".to_string(),
                    footprint_id: "R_0805".to_string(),
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    mirror: false,
                    pins: vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                    properties: std::collections::HashMap::new(),
                },
                SchematicComponent {
                    reference: "R2".to_string(),
                    value: "10k".to_string(),
                    footprint_id: "R_0805".to_string(),
                    position: Vec2::new(20.0, 0.0),
                    rotation: 0.0,
                    mirror: false,
                    pins: vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                    properties: std::collections::HashMap::new(),
                },
                SchematicComponent {
                    reference: "R3".to_string(),
                    value: "10k".to_string(),
                    footprint_id: "R_0805".to_string(),
                    position: Vec2::new(10.0, 10.0),
                    rotation: 0.0,
                    mirror: false,
                    pins: vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                    properties: std::collections::HashMap::new(),
                },
            ],
            wires: vec![
                SchematicWire {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(10.0, 0.0),
                },
                SchematicWire {
                    start: Vec2::new(10.0, 0.0),
                    end: Vec2::new(20.0, 0.0),
                },
                SchematicWire {
                    start: Vec2::new(10.0, 0.0),
                    end: Vec2::new(10.0, 10.0),
                },
            ],
            junctions: vec![SchematicJunction {
                position: Vec2::new(10.0, 0.0),
            }],
            labels: vec![],
        };

        let netlist = generate_netlist(&sheet);

        // All three pins should be on the same net.
        assert_eq!(netlist.nets.len(), 1);
        assert_eq!(netlist.nets[0].connections.len(), 3);
    }

    #[test]
    fn rotated_component_pin_positions() {
        // R1 at (10, 10) rotated 90 degrees.
        // Pin 1 relative (-5, 0) => after 90deg rotation => (0, -5) => world (10, 5)
        // Pin 2 relative (5, 0) => after 90deg rotation => (0, 5) => world (10, 15)
        let mut r1 = make_resistor("R1", Vec2::new(10.0, 10.0));
        r1.rotation = 90.0;

        // Wire from (10, 5) to (0, 5) connecting pin 1 to a label
        let sheet = SchematicSheet {
            title: None,
            components: vec![r1],
            wires: vec![SchematicWire {
                start: Vec2::new(10.0, 5.0),
                end: Vec2::new(0.0, 5.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "SIG".to_string(),
                position: Vec2::new(0.0, 5.0),
                rotation: 0.0,
                scope: LabelScope::Local,
            }],
        };

        let netlist = generate_netlist(&sheet);

        let sig_net = netlist.nets.iter().find(|n| n.name == "SIG");
        assert!(sig_net.is_some(), "Expected SIG net");
        let sig = sig_net.unwrap();
        assert!(sig
            .connections
            .iter()
            .any(|c| c.component_ref == "R1" && c.pin_number == "1"));
    }

    #[test]
    fn mirrored_component_pin_positions() {
        // R1 at (10, 0) mirrored. Pin 1 relative (-5, 0) => mirror => (5, 0) => world (15, 0)
        let mut r1 = make_resistor("R1", Vec2::new(10.0, 0.0));
        r1.mirror = true;

        let sheet = SchematicSheet {
            title: None,
            components: vec![r1],
            wires: vec![SchematicWire {
                start: Vec2::new(15.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "TEST".to_string(),
                position: Vec2::new(20.0, 0.0),
                rotation: 0.0,
                scope: LabelScope::Local,
            }],
        };

        let netlist = generate_netlist(&sheet);
        let test_net = netlist.nets.iter().find(|n| n.name == "TEST");
        assert!(test_net.is_some(), "Expected TEST net");
        // Mirrored pin 1 is at (15,0) which is connected via wire to label
        assert!(test_net
            .unwrap()
            .connections
            .iter()
            .any(|c| c.component_ref == "R1" && c.pin_number == "1"));
    }

    #[test]
    fn multiple_labels_same_net_picks_alphabetical() {
        // Two labels at the same position
        let sheet = SchematicSheet {
            title: None,
            components: vec![SchematicComponent {
                reference: "R1".to_string(),
                value: "10k".to_string(),
                footprint_id: "R_0805".to_string(),
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                mirror: false,
                pins: vec![SchematicPin {
                    number: "1".to_string(),
                    name: "~".to_string(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(0.0, 0.0),
                }],
                properties: std::collections::HashMap::new(),
            }],
            wires: vec![],
            junctions: vec![],
            labels: vec![
                SchematicLabel {
                    name: "ZEBRA".to_string(),
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    scope: LabelScope::Local,
                },
                SchematicLabel {
                    name: "ALPHA".to_string(),
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    scope: LabelScope::Local,
                },
            ],
        };

        let netlist = generate_netlist(&sheet);
        // Should pick "ALPHA" (first alphabetically)
        let net = netlist.nets.iter().find(|n| n.connections.len() == 1);
        assert!(net.is_some());
        assert_eq!(net.unwrap().name, "ALPHA");
    }
}
