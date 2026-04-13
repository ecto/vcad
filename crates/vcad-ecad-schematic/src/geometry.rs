//! Schematic geometry helpers.
//!
//! Grid snapping, pin proximity, point-on-segment, and net lookup utilities
//! used by the schematic canvas.

use vcad_ir::ecad::{SchematicComponent, SchematicWire};
use vcad_ir::Vec2;

use crate::{pin_world_position, Netlist};

/// Result of snapping a position to the grid or a nearby pin.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapResult {
    /// Snapped position.
    pub position: Vec2,
    /// Whether the position snapped to a pin (rather than the grid).
    pub is_pin: bool,
}

/// Snap a value to the nearest grid point.
pub fn snap_to_grid(v: f64, grid: f64) -> f64 {
    (v / grid).round() * grid
}

/// Snap to the nearest component pin if within threshold, otherwise grid-snap.
pub fn snap_to_grid_or_pin(
    pos: Vec2,
    components: &[SchematicComponent],
    grid: f64,
    threshold: f64,
) -> SnapResult {
    let mut best_dist = threshold;
    let mut result = SnapResult {
        position: Vec2::new(snap_to_grid(pos.x, grid), snap_to_grid(pos.y, grid)),
        is_pin: false,
    };

    for comp in components {
        for pin in &comp.pins {
            let p = pin_world_position(comp, pin);
            let d = ((pos.x - p.x).powi(2) + (pos.y - p.y).powi(2)).sqrt();
            if d < best_dist {
                best_dist = d;
                result = SnapResult {
                    position: p,
                    is_pin: true,
                };
            }
        }
    }

    result
}

/// Check if point `p` lies on segment `(a, b)` — excluding endpoints — within
/// tolerance.
pub fn point_on_segment(p: Vec2, a: Vec2, b: Vec2, tol: f64) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 0.01 {
        return false;
    }
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    if !(0.01..=0.99).contains(&t) {
        return false;
    }
    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    ((p.x - proj_x).powi(2) + (p.y - proj_y).powi(2)).sqrt() < tol
}

/// Determine which net a component pin belongs to.
pub fn net_for_pin(ref_: &str, pin_num: &str, netlist: &Netlist) -> Option<String> {
    for net in &netlist.nets {
        for conn in &net.connections {
            if conn.component_ref == ref_ && conn.pin_number == pin_num {
                return Some(net.name.clone());
            }
        }
    }
    None
}

/// Get all nets connected to a component.
pub fn nets_for_component(ref_: &str, netlist: &Netlist) -> Vec<String> {
    let mut nets = Vec::new();
    for net in &netlist.nets {
        for conn in &net.connections {
            if conn.component_ref == ref_ {
                nets.push(net.name.clone());
                break;
            }
        }
    }
    nets
}

/// Get the net for a wire based on endpoint proximity to component pins.
///
/// Checks if either wire endpoint is within 2 units of any component pin and
/// returns the first matching net.
pub fn net_for_wire(
    wire: &SchematicWire,
    netlist: &Netlist,
    components: &[SchematicComponent],
) -> Option<String> {
    for comp in components {
        for pin in &comp.pins {
            let p = pin_world_position(comp, pin);
            let d1 = ((wire.start.x - p.x).powi(2) + (wire.start.y - p.y).powi(2)).sqrt();
            let d2 = ((wire.end.x - p.x).powi(2) + (wire.end.y - p.y).powi(2)).sqrt();
            if d1 < 2.0 || d2 < 2.0 {
                if let Some(net) = net_for_pin(&comp.reference, &pin.number, netlist) {
                    return Some(net);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::{PinType, SchematicPin};

    fn make_component(reference: &str, pos: Vec2, pins: Vec<(Vec2, &str)>) -> SchematicComponent {
        SchematicComponent {
            reference: reference.to_string(),
            value: "10k".to_string(),
            footprint_id: "R_0805".to_string(),
            position: pos,
            rotation: 0.0,
            mirror: false,
            pins: pins
                .into_iter()
                .map(|(p, num)| SchematicPin {
                    number: num.to_string(),
                    name: "~".to_string(),
                    pin_type: PinType::Passive,
                    position: p,
                })
                .collect(),
            properties: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn snap_to_grid_rounds_correctly() {
        assert_eq!(snap_to_grid(7.0, 10.0), 10.0);
        assert_eq!(snap_to_grid(3.0, 10.0), 0.0);
        assert_eq!(snap_to_grid(15.0, 10.0), 20.0);
        assert_eq!(snap_to_grid(25.0, 10.0), 30.0);
    }

    #[test]
    fn snap_to_pin_when_close() {
        let comps = vec![make_component(
            "R1",
            Vec2::new(10.0, 10.0),
            vec![(Vec2::new(-5.0, 0.0), "1"), (Vec2::new(5.0, 0.0), "2")],
        )];
        // Cursor at (14.0, 10.0) — close to pin 2 at (15.0, 10.0)
        let result = snap_to_grid_or_pin(Vec2::new(14.0, 10.0), &comps, 10.0, 12.0);
        assert!(result.is_pin);
        assert!((result.position.x - 15.0).abs() < 0.001);
        assert!((result.position.y - 10.0).abs() < 0.001);
    }

    #[test]
    fn snap_to_grid_when_far_from_pins() {
        let comps = vec![make_component(
            "R1",
            Vec2::new(10.0, 10.0),
            vec![(Vec2::new(-5.0, 0.0), "1")],
        )];
        // Cursor at (50.0, 50.0) — far from pin at (5.0, 10.0)
        let result = snap_to_grid_or_pin(Vec2::new(53.0, 47.0), &comps, 10.0, 12.0);
        assert!(!result.is_pin);
        assert!((result.position.x - 50.0).abs() < 0.001);
        assert!((result.position.y - 50.0).abs() < 0.001);
    }

    #[test]
    fn point_on_segment_interior() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        assert!(point_on_segment(Vec2::new(5.0, 0.5), a, b, 2.0));
        assert!(!point_on_segment(Vec2::new(5.0, 3.0), a, b, 2.0));
    }

    #[test]
    fn point_on_segment_excludes_endpoints() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        // Very close to endpoint a
        assert!(!point_on_segment(Vec2::new(0.05, 0.0), a, b, 2.0));
        // Very close to endpoint b
        assert!(!point_on_segment(Vec2::new(9.95, 0.0), a, b, 2.0));
    }

    #[test]
    fn net_for_pin_found() {
        let netlist = crate::Netlist {
            nets: vec![crate::NetlistNet {
                name: "VCC".to_string(),
                connections: vec![crate::NetConnection {
                    component_ref: "R1".to_string(),
                    pin_number: "1".to_string(),
                }],
            }],
        };
        assert_eq!(net_for_pin("R1", "1", &netlist), Some("VCC".to_string()));
        assert_eq!(net_for_pin("R1", "2", &netlist), None);
        assert_eq!(net_for_pin("R2", "1", &netlist), None);
    }

    #[test]
    fn nets_for_component_collects_all() {
        let netlist = crate::Netlist {
            nets: vec![
                crate::NetlistNet {
                    name: "VCC".to_string(),
                    connections: vec![crate::NetConnection {
                        component_ref: "R1".to_string(),
                        pin_number: "1".to_string(),
                    }],
                },
                crate::NetlistNet {
                    name: "GND".to_string(),
                    connections: vec![crate::NetConnection {
                        component_ref: "R1".to_string(),
                        pin_number: "2".to_string(),
                    }],
                },
                crate::NetlistNet {
                    name: "SIG".to_string(),
                    connections: vec![crate::NetConnection {
                        component_ref: "R2".to_string(),
                        pin_number: "1".to_string(),
                    }],
                },
            ],
        };
        let mut nets = nets_for_component("R1", &netlist);
        nets.sort();
        assert_eq!(nets, vec!["GND", "VCC"]);
    }

    #[test]
    fn net_for_wire_by_proximity() {
        use vcad_ir::ecad::SchematicWire;

        let comps = vec![make_component(
            "R1",
            Vec2::new(10.0, 10.0),
            vec![(Vec2::new(-5.0, 0.0), "1"), (Vec2::new(5.0, 0.0), "2")],
        )];
        let netlist = crate::Netlist {
            nets: vec![crate::NetlistNet {
                name: "NET-001".to_string(),
                connections: vec![crate::NetConnection {
                    component_ref: "R1".to_string(),
                    pin_number: "2".to_string(),
                }],
            }],
        };
        // Wire start at (15.0, 10.0) — coincides with pin 2 at (15.0, 10.0)
        let wire = SchematicWire {
            start: Vec2::new(15.0, 10.0),
            end: Vec2::new(30.0, 10.0),
        };
        assert_eq!(
            net_for_wire(&wire, &netlist, &comps),
            Some("NET-001".to_string())
        );

        // Wire far from any pin
        let wire2 = SchematicWire {
            start: Vec2::new(100.0, 100.0),
            end: Vec2::new(200.0, 100.0),
        };
        assert_eq!(net_for_wire(&wire2, &netlist, &comps), None);
    }
}
