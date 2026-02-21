//! Electrical Rule Checking (ERC) for schematic sheets.
//!
//! Detects common wiring errors:
//! - Unconnected pins (pins with no wire reaching them)
//! - Pin type conflicts (e.g. output driving output)
//! - Duplicate reference designators
//! - Power pins not on power nets

use std::collections::{HashMap, HashSet};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

use crate::{generate_netlist, pin_world_position, points_coincident};

/// ERC violation severity.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum ErcSeverity {
    /// A hard error that must be fixed.
    Error,
    /// A warning that should be reviewed.
    Warning,
}

/// An ERC violation found in the schematic.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ErcViolation {
    /// Severity of the violation.
    pub severity: ErcSeverity,
    /// Human-readable description of the problem.
    pub message: String,
    /// Optional position on the schematic where the violation occurs.
    pub position: Option<Vec2>,
}

/// Run all ERC checks on a schematic sheet.
///
/// Returns a list of violations sorted by severity (errors first) then by
/// message for deterministic output.
pub fn check_erc(sheet: &SchematicSheet) -> Vec<ErcViolation> {
    let mut violations = Vec::new();

    check_duplicate_references(sheet, &mut violations);
    check_unconnected_pins(sheet, &mut violations);
    check_pin_type_conflicts(sheet, &mut violations);
    check_power_pins_on_power_nets(sheet, &mut violations);

    // Sort: errors first, then warnings, then by message.
    violations.sort_by(|a, b| {
        let sev_a = match a.severity {
            ErcSeverity::Error => 0,
            ErcSeverity::Warning => 1,
        };
        let sev_b = match b.severity {
            ErcSeverity::Error => 0,
            ErcSeverity::Warning => 1,
        };
        sev_a.cmp(&sev_b).then_with(|| a.message.cmp(&b.message))
    });

    violations
}

/// Check for duplicate reference designators.
fn check_duplicate_references(sheet: &SchematicSheet, violations: &mut Vec<ErcViolation>) {
    let mut seen: HashMap<&str, Vec2> = HashMap::new();
    for comp in &sheet.components {
        if let Some(&first_pos) = seen.get(comp.reference.as_str()) {
            violations.push(ErcViolation {
                severity: ErcSeverity::Error,
                message: format!("Duplicate reference designator '{}'", comp.reference),
                position: Some(comp.position),
            });
            // Also flag the first occurrence if we haven't already.
            // We only flag the duplicates, not the original, to avoid
            // double-reporting when there are 2 of the same ref.
            let _ = first_pos; // original position tracked but not re-reported
        } else {
            seen.insert(&comp.reference, comp.position);
        }
    }
}

/// Check for unconnected pins: pins that have no wire endpoint or junction
/// at their world position.
fn check_unconnected_pins(sheet: &SchematicSheet, violations: &mut Vec<ErcViolation>) {
    // Collect all wire endpoints.
    let mut wire_points: Vec<Vec2> = Vec::new();
    for wire in &sheet.wires {
        wire_points.push(wire.start);
        wire_points.push(wire.end);
    }

    // Collect junction positions too (junctions imply connectivity).
    for junction in &sheet.junctions {
        wire_points.push(junction.position);
    }

    // Also collect label positions (a label at a pin counts as connected).
    for label in &sheet.labels {
        wire_points.push(label.position);
    }

    // Also collect all other pin positions (pin-to-pin direct connection).
    let mut all_pin_positions: Vec<Vec2> = Vec::new();
    for comp in &sheet.components {
        for pin in &comp.pins {
            all_pin_positions.push(pin_world_position(comp, pin));
        }
    }

    for comp in &sheet.components {
        for pin in &comp.pins {
            // Skip NotConnected pins — they are intentionally unconnected.
            if pin.pin_type == PinType::NotConnected {
                continue;
            }

            let world_pos = pin_world_position(comp, pin);

            // Check if any wire endpoint, junction, or label touches this pin.
            let has_wire = wire_points
                .iter()
                .any(|wp| points_coincident(wp, &world_pos));

            // Check if another pin is at the same position (direct pin-to-pin).
            let has_other_pin = all_pin_positions
                .iter()
                .filter(|pp| points_coincident(pp, &world_pos))
                .count()
                > 1; // More than 1 because we'll match ourselves

            if !has_wire && !has_other_pin {
                violations.push(ErcViolation {
                    severity: ErcSeverity::Warning,
                    message: format!(
                        "Unconnected pin: {} pin {} ({})",
                        comp.reference, pin.number, pin.name
                    ),
                    position: Some(world_pos),
                });
            }
        }
    }
}

/// Check for pin type conflicts on shared nets.
///
/// Specifically:
/// - Two output pins on the same net (output driving output)
/// - Two power output pins on the same net
fn check_pin_type_conflicts(sheet: &SchematicSheet, violations: &mut Vec<ErcViolation>) {
    let netlist = generate_netlist(sheet);

    // Build a lookup: (component_ref, pin_number) -> PinType
    let mut pin_types: HashMap<(String, String), PinType> = HashMap::new();
    for comp in &sheet.components {
        for pin in &comp.pins {
            pin_types.insert((comp.reference.clone(), pin.number.clone()), pin.pin_type);
        }
    }

    for net in &netlist.nets {
        let mut output_pins: Vec<&str> = Vec::new();
        let mut power_output_pins: Vec<&str> = Vec::new();

        for conn in &net.connections {
            let key = (conn.component_ref.clone(), conn.pin_number.clone());
            if let Some(&pin_type) = pin_types.get(&key) {
                match pin_type {
                    PinType::Output => output_pins.push(&conn.component_ref),
                    PinType::PowerOutput => power_output_pins.push(&conn.component_ref),
                    _ => {}
                }
            }
        }

        // Two or more outputs on the same net.
        if output_pins.len() > 1 {
            violations.push(ErcViolation {
                severity: ErcSeverity::Error,
                message: format!(
                    "Pin conflict on net '{}': multiple outputs ({})",
                    net.name,
                    output_pins.join(", ")
                ),
                position: None,
            });
        }

        // Two or more power outputs on the same net.
        if power_output_pins.len() > 1 {
            violations.push(ErcViolation {
                severity: ErcSeverity::Error,
                message: format!(
                    "Pin conflict on net '{}': multiple power outputs ({})",
                    net.name,
                    power_output_pins.join(", ")
                ),
                position: None,
            });
        }
    }
}

/// Check that power input pins are on nets with a power-related label or
/// a power output pin providing the power.
fn check_power_pins_on_power_nets(sheet: &SchematicSheet, violations: &mut Vec<ErcViolation>) {
    let netlist = generate_netlist(sheet);

    // Build lookups.
    let mut pin_types: HashMap<(String, String), PinType> = HashMap::new();
    for comp in &sheet.components {
        for pin in &comp.pins {
            pin_types.insert((comp.reference.clone(), pin.number.clone()), pin.pin_type);
        }
    }

    // Common power net name patterns.
    let power_names: HashSet<&str> = [
        "VCC", "VDD", "V+", "V-", "GND", "VSS", "VBUS", "3V3", "5V", "12V", "+3V3", "+5V", "+12V",
        "AVCC", "AVDD", "AGND", "DGND", "DVDD",
    ]
    .iter()
    .copied()
    .collect();

    for net in &netlist.nets {
        let has_power_input = net.connections.iter().any(|c| {
            let key = (c.component_ref.clone(), c.pin_number.clone());
            matches!(pin_types.get(&key), Some(PinType::PowerInput))
        });

        if !has_power_input {
            continue;
        }

        // Check if net has a power output pin.
        let has_power_output = net.connections.iter().any(|c| {
            let key = (c.component_ref.clone(), c.pin_number.clone());
            matches!(pin_types.get(&key), Some(PinType::PowerOutput))
        });

        // Check if net name looks like a power net.
        let name_upper = net.name.to_uppercase();
        let is_power_name = power_names.contains(name_upper.as_str());

        if !has_power_output && !is_power_name {
            violations.push(ErcViolation {
                severity: ErcSeverity::Warning,
                message: format!("Power input pin on net '{}' has no power source", net.name),
                position: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Vec2;

    fn make_simple_component(
        reference: &str,
        position: Vec2,
        pins: Vec<SchematicPin>,
    ) -> SchematicComponent {
        SchematicComponent {
            reference: reference.to_string(),
            value: "test".to_string(),
            footprint_id: "test:test".to_string(),
            position,
            rotation: 0.0,
            mirror: false,
            pins,
            properties: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn clean_schematic_no_violations() {
        // R1 and R2 connected by a wire, with VCC and GND labels.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_simple_component(
                    "R1",
                    Vec2::new(10.0, 0.0),
                    vec![
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
                ),
                make_simple_component(
                    "R2",
                    Vec2::new(25.0, 0.0),
                    vec![
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
                ),
            ],
            wires: vec![
                // Connect R1 pin 1 to VCC label
                SchematicWire {
                    start: Vec2::new(5.0, 0.0),
                    end: Vec2::new(0.0, 0.0),
                },
                // Connect R1 pin 2 to R2 pin 1
                SchematicWire {
                    start: Vec2::new(15.0, 0.0),
                    end: Vec2::new(20.0, 0.0),
                },
                // Connect R2 pin 2 to GND label
                SchematicWire {
                    start: Vec2::new(30.0, 0.0),
                    end: Vec2::new(35.0, 0.0),
                },
            ],
            junctions: vec![],
            labels: vec![
                SchematicLabel {
                    name: "VCC".to_string(),
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    scope: LabelScope::Global,
                },
                SchematicLabel {
                    name: "GND".to_string(),
                    position: Vec2::new(35.0, 0.0),
                    rotation: 0.0,
                    scope: LabelScope::Global,
                },
            ],
        };

        let violations = check_erc(&sheet);
        assert!(
            violations.is_empty(),
            "Expected no violations, got: {:?}",
            violations
        );
    }

    #[test]
    fn detect_duplicate_reference() {
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_simple_component(
                    "R1",
                    Vec2::new(0.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
                make_simple_component(
                    "R1", // duplicate!
                    Vec2::new(20.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        let dup = violations
            .iter()
            .find(|v| v.message.contains("Duplicate reference"));
        assert!(dup.is_some(), "Expected duplicate reference violation");
        assert_eq!(dup.unwrap().severity, ErcSeverity::Error);
    }

    #[test]
    fn detect_unconnected_pin() {
        // R1 with two pins, only pin 1 is wired.
        let sheet = SchematicSheet {
            title: None,
            components: vec![make_simple_component(
                "R1",
                Vec2::new(10.0, 0.0),
                vec![
                    SchematicPin {
                        number: "1".to_string(),
                        name: "A".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(-5.0, 0.0),
                    },
                    SchematicPin {
                        number: "2".to_string(),
                        name: "B".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(5.0, 0.0),
                    },
                ],
            )],
            wires: vec![SchematicWire {
                start: Vec2::new(5.0, 0.0), // connects to pin 1
                end: Vec2::new(0.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        let uncon = violations
            .iter()
            .find(|v| v.message.contains("Unconnected pin"));
        assert!(uncon.is_some(), "Expected unconnected pin warning");
        assert_eq!(uncon.unwrap().severity, ErcSeverity::Warning);
        // Pin 2 at world (15,0) should be the unconnected one.
        assert!(uncon.unwrap().message.contains("pin 2"));
    }

    #[test]
    fn not_connected_pin_type_skipped() {
        // A pin explicitly marked NotConnected should not trigger a warning.
        let sheet = SchematicSheet {
            title: None,
            components: vec![make_simple_component(
                "U1",
                Vec2::new(0.0, 0.0),
                vec![SchematicPin {
                    number: "3".to_string(),
                    name: "NC".to_string(),
                    pin_type: PinType::NotConnected,
                    position: Vec2::new(5.0, 0.0),
                }],
            )],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        let uncon = violations
            .iter()
            .find(|v| v.message.contains("Unconnected"));
        assert!(
            uncon.is_none(),
            "NotConnected pin should not trigger unconnected warning"
        );
    }

    #[test]
    fn detect_output_driving_output() {
        // Two output pins on the same net.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_simple_component(
                    "U1",
                    Vec2::new(0.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "OUT".to_string(),
                        pin_type: PinType::Output,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
                make_simple_component(
                    "U2",
                    Vec2::new(20.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "OUT".to_string(),
                        pin_type: PinType::Output,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        let conflict = violations
            .iter()
            .find(|v| v.message.contains("multiple outputs"));
        assert!(conflict.is_some(), "Expected output-output conflict");
        assert_eq!(conflict.unwrap().severity, ErcSeverity::Error);
    }

    #[test]
    fn detect_power_pin_without_source() {
        // IC with a PowerInput pin on a net that has no power source and
        // no power-related label.
        let sheet = SchematicSheet {
            title: None,
            components: vec![make_simple_component(
                "U1",
                Vec2::new(0.0, 0.0),
                vec![SchematicPin {
                    number: "1".to_string(),
                    name: "VCC".to_string(),
                    pin_type: PinType::PowerInput,
                    position: Vec2::new(0.0, 0.0),
                }],
            )],
            wires: vec![SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(-5.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "RANDOM_SIGNAL".to_string(),
                position: Vec2::new(-5.0, 0.0),
                rotation: 0.0,
                scope: LabelScope::Local,
            }],
        };

        let violations = check_erc(&sheet);
        let pwr = violations
            .iter()
            .find(|v| v.message.contains("no power source"));
        assert!(pwr.is_some(), "Expected power pin warning");
        assert_eq!(pwr.unwrap().severity, ErcSeverity::Warning);
    }

    #[test]
    fn power_pin_on_vcc_net_is_ok() {
        // Power input pin on a net labeled "VCC" — should be fine.
        let sheet = SchematicSheet {
            title: None,
            components: vec![make_simple_component(
                "U1",
                Vec2::new(0.0, 0.0),
                vec![SchematicPin {
                    number: "1".to_string(),
                    name: "VCC".to_string(),
                    pin_type: PinType::PowerInput,
                    position: Vec2::new(0.0, 0.0),
                }],
            )],
            wires: vec![SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(-5.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "VCC".to_string(),
                position: Vec2::new(-5.0, 0.0),
                rotation: 0.0,
                scope: LabelScope::Global,
            }],
        };

        let violations = check_erc(&sheet);
        let pwr = violations
            .iter()
            .find(|v| v.message.contains("no power source"));
        assert!(pwr.is_none(), "Power pin on VCC net should not be flagged");
    }

    #[test]
    fn power_pin_with_power_output_is_ok() {
        // Power input pin on a net with a power output pin — should be fine.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                make_simple_component(
                    "U1",
                    Vec2::new(0.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "VCC".to_string(),
                        pin_type: PinType::PowerInput,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
                make_simple_component(
                    "VR1",
                    Vec2::new(20.0, 0.0),
                    vec![SchematicPin {
                        number: "3".to_string(),
                        name: "OUT".to_string(),
                        pin_type: PinType::PowerOutput,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(20.0, 0.0),
            }],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        let pwr = violations
            .iter()
            .find(|v| v.message.contains("no power source"));
        assert!(
            pwr.is_none(),
            "Power pin with power output should not be flagged"
        );
    }

    #[test]
    fn empty_schematic_no_violations() {
        let sheet = SchematicSheet {
            title: None,
            components: vec![],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
        };
        let violations = check_erc(&sheet);
        assert!(violations.is_empty());
    }

    #[test]
    fn violations_sorted_errors_first() {
        // Create a schematic with both errors and warnings.
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                // Duplicate ref (error)
                make_simple_component(
                    "R1",
                    Vec2::new(0.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
                make_simple_component(
                    "R1",
                    Vec2::new(20.0, 0.0),
                    vec![SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(0.0, 0.0),
                    }],
                ),
            ],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
        };

        let violations = check_erc(&sheet);
        assert!(!violations.is_empty());

        // Errors should come before warnings.
        let mut saw_warning = false;
        for v in &violations {
            if v.severity == ErcSeverity::Warning {
                saw_warning = true;
            }
            if v.severity == ErcSeverity::Error && saw_warning {
                panic!("Error appeared after warning — sorting is broken");
            }
        }
    }
}
