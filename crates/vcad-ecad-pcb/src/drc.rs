//! Design Rule Checking (DRC) engine.
//!
//! Validates a PCB layout against its design rules and reports violations.
//! Uses the spatial index from [`crate::spatial`] for efficient proximity queries.

use std::collections::HashMap;

use vcad_ir::ecad::{Pad, PadShape, PadType, Pcb};
use vcad_ir::Vec2;

use crate::spatial::{pad_half_extents, SpatialIndex};

/// DRC rule type.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum DrcRuleType {
    /// Copper-to-copper clearance violation.
    Clearance,
    /// Trace width below minimum.
    MinTraceWidth,
    /// Drill diameter below minimum.
    MinDrill,
    /// Annular ring too narrow.
    AnnularRing,
    /// Copper too close to board edge.
    EdgeClearance,
    /// Hole-to-hole distance too small.
    HoleToHole,
    /// Net has unconnected terminals.
    UnconnectedNet,
    /// Silkscreen overlapping pads.
    SilkscreenClearance,
    /// Component courtyards overlapping.
    CourtyardOverlap,
    /// Acute angle copper creating acid trap.
    AcidTrap,
}

/// DRC violation severity.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum DrcSeverity {
    /// Must be fixed before fabrication.
    Error,
    /// Should be reviewed but may be acceptable.
    Warning,
}

/// A DRC violation found during checking.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DrcViolation {
    /// Which rule was violated.
    pub rule: DrcRuleType,
    /// Severity of the violation.
    pub severity: DrcSeverity,
    /// Location of the violation on the board (mm).
    pub position: Vec2,
    /// Human-readable description.
    pub message: String,
    /// Actual measured value (mm).
    pub actual: f64,
    /// Required value from design rules (mm).
    pub required: f64,
}

/// Run all DRC checks on a PCB and return violations.
///
/// Checks performed:
/// - Copper clearance between different-net elements
/// - Minimum trace width
/// - Minimum drill diameter
/// - Edge clearance
/// - Hole-to-hole spacing
/// - Annular ring width
pub fn check_drc(pcb: &Pcb) -> Vec<DrcViolation> {
    let mut violations = Vec::new();
    let index = SpatialIndex::from_pcb(pcb);

    check_clearance(pcb, &index, &mut violations);
    check_pad_clearance(pcb, &mut violations);
    check_min_trace_width(pcb, &mut violations);
    check_min_drill(pcb, &mut violations);
    check_edge_clearance(pcb, &mut violations);
    check_hole_to_hole(pcb, &mut violations);
    check_annular_ring(pcb, &mut violations);

    violations
}

/// Check copper-to-copper clearance between different-net elements.
fn check_clearance(pcb: &Pcb, index: &SpatialIndex, violations: &mut Vec<DrcViolation>) {
    let default_clearance = pcb.rules.default_rules.clearance;

    // Build net class clearance lookup
    let net_clearance = build_net_clearance_map(pcb);

    // Check each trace against nearby elements on the same layer
    for trace in &pcb.traces {
        let clearance = net_clearance
            .get(&trace.net)
            .copied()
            .unwrap_or(default_clearance);

        let half_w = trace.width / 2.0;
        let search_margin = clearance + half_w + 1.0; // extra margin for search
        let min_x = trace.start.x.min(trace.end.x) - search_margin;
        let min_y = trace.start.y.min(trace.end.y) - search_margin;
        let max_x = trace.start.x.max(trace.end.x) + search_margin;
        let max_y = trace.start.y.max(trace.end.y) + search_margin;

        let nearby = index.query_region([min_x, min_y], [max_x, max_y]);

        for elem in nearby {
            if elem.net == trace.net || elem.layer != trace.layer {
                continue;
            }

            // Compute approximate distance between trace bbox and element bbox
            let dist = bbox_distance(
                [
                    trace.start.x.min(trace.end.x) - half_w,
                    trace.start.y.min(trace.end.y) - half_w,
                    trace.start.x.max(trace.end.x) + half_w,
                    trace.start.y.max(trace.end.y) + half_w,
                ],
                [elem.min[0], elem.min[1], elem.max[0], elem.max[1]],
            );

            if dist < clearance {
                let pos = Vec2::new(
                    (trace.start.x + trace.end.x) / 2.0,
                    (trace.start.y + trace.end.y) / 2.0,
                );
                violations.push(DrcViolation {
                    rule: DrcRuleType::Clearance,
                    severity: DrcSeverity::Error,
                    position: pos,
                    message: format!(
                        "Clearance violation: trace net '{}' to net '{}': {:.3}mm < {:.3}mm",
                        trace.net, elem.net, dist, clearance
                    ),
                    actual: dist,
                    required: clearance,
                });
            }
        }
    }
}

/// Check copper clearance between pads of different nets.
///
/// The trace pass covers trace↔copper pairs; this covers pad↔pad shorts
/// (overlapping footprints or stacked pads), which that pass never sees.
fn check_pad_clearance(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let default_clearance = pcb.rules.default_rules.clearance;
    let net_clearance = build_net_clearance_map(pcb);

    struct PadBox<'a> {
        bbox: [f64; 4],
        net: &'a str,
        layers: &'a [vcad_ir::ecad::PcbLayer],
        fp_ref: &'a str,
        number: &'a str,
    }

    let mut boxes: Vec<PadBox> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            // Pads without a net can't short two nets together.
            let Some(net) = pad.net.as_deref() else {
                continue;
            };
            let (hw, hh) = pad_half_extents(pad);
            let x = fp.position.x + pad.position.x;
            let y = fp.position.y + pad.position.y;
            boxes.push(PadBox {
                bbox: [x - hw, y - hh, x + hw, y + hh],
                net,
                layers: &pad.layers,
                fp_ref: &fp.reference,
                number: &pad.number,
            });
        }
    }

    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let (a, b) = (&boxes[i], &boxes[j]);
            if a.net == b.net {
                continue;
            }
            let share_copper = a
                .layers
                .iter()
                .any(|la| la.is_copper() && b.layers.contains(la));
            if !share_copper {
                continue;
            }

            let clearance = net_clearance
                .get(a.net)
                .copied()
                .unwrap_or(default_clearance)
                .max(net_clearance.get(b.net).copied().unwrap_or(default_clearance));

            let dist = bbox_distance(a.bbox, b.bbox);
            if dist < clearance {
                let pos = Vec2::new(
                    (a.bbox[0] + a.bbox[2] + b.bbox[0] + b.bbox[2]) / 4.0,
                    (a.bbox[1] + a.bbox[3] + b.bbox[1] + b.bbox[3]) / 4.0,
                );
                violations.push(DrcViolation {
                    rule: DrcRuleType::Clearance,
                    severity: DrcSeverity::Error,
                    position: pos,
                    message: format!(
                        "Clearance violation: pad {}.{} net '{}' to pad {}.{} net '{}': {:.3}mm < {:.3}mm",
                        a.fp_ref, a.number, a.net, b.fp_ref, b.number, b.net, dist, clearance
                    ),
                    actual: dist,
                    required: clearance,
                });
            }
        }
    }
}

/// Check that all traces meet the minimum trace width.
fn check_min_trace_width(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let net_width = build_net_trace_width_map(pcb);
    let default_width = pcb.rules.default_rules.trace_width;

    for trace in &pcb.traces {
        let min_width = net_width.get(&trace.net).copied().unwrap_or(default_width);

        if trace.width < min_width - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::MinTraceWidth,
                severity: DrcSeverity::Error,
                position: Vec2::new(
                    (trace.start.x + trace.end.x) / 2.0,
                    (trace.start.y + trace.end.y) / 2.0,
                ),
                message: format!(
                    "Trace width {:.3}mm below minimum {:.3}mm for net '{}'",
                    trace.width, min_width, trace.net
                ),
                actual: trace.width,
                required: min_width,
            });
        }
    }
}

/// Check that all drills meet the minimum drill diameter.
fn check_min_drill(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_drill = pcb.rules.min_drill;

    // Check via drills
    for via in &pcb.vias {
        if via.drill < min_drill - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::MinDrill,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!(
                    "Via drill {:.3}mm below minimum {:.3}mm",
                    via.drill, min_drill
                ),
                actual: via.drill,
                required: min_drill,
            });
        }
    }

    // Check pad drills
    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                if drill.diameter < min_drill - 1e-6 {
                    let abs_pos = Vec2::new(
                        footprint.position.x + pad.position.x,
                        footprint.position.y + pad.position.y,
                    );
                    violations.push(DrcViolation {
                        rule: DrcRuleType::MinDrill,
                        severity: DrcSeverity::Error,
                        position: abs_pos,
                        message: format!(
                            "Pad {} drill {:.3}mm below minimum {:.3}mm on {}",
                            pad.number, drill.diameter, min_drill, footprint.reference
                        ),
                        actual: drill.diameter,
                        required: min_drill,
                    });
                }
            }
        }
    }
}

/// Check that all copper elements maintain edge clearance.
fn check_edge_clearance(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let edge_clearance = pcb.rules.edge_clearance;
    let outline = &pcb.outline.vertices;

    if outline.is_empty() {
        return;
    }

    // Check traces against board edges
    for trace in &pcb.traces {
        let mid = Vec2::new(
            (trace.start.x + trace.end.x) / 2.0,
            (trace.start.y + trace.end.y) / 2.0,
        );
        // Check start and end points
        for point in [&trace.start, &trace.end] {
            let dist = min_distance_to_polygon(point, outline);
            let effective_dist = dist - trace.width / 2.0;
            if effective_dist < edge_clearance - 1e-6 {
                violations.push(DrcViolation {
                    rule: DrcRuleType::EdgeClearance,
                    severity: DrcSeverity::Error,
                    position: mid,
                    message: format!(
                        "Trace net '{}' edge clearance {:.3}mm < {:.3}mm",
                        trace.net, effective_dist, edge_clearance
                    ),
                    actual: effective_dist,
                    required: edge_clearance,
                });
                break; // one violation per trace
            }
        }
    }

    // Check vias against board edges
    for via in &pcb.vias {
        let dist = min_distance_to_polygon(&via.position, outline);
        let effective_dist = dist - via.diameter / 2.0;
        if effective_dist < edge_clearance - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::EdgeClearance,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!(
                    "Via net '{}' edge clearance {:.3}mm < {:.3}mm",
                    via.net, effective_dist, edge_clearance
                ),
                actual: effective_dist,
                required: edge_clearance,
            });
        }
    }
}

/// Check hole-to-hole spacing.
fn check_hole_to_hole(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_spacing = pcb.rules.hole_to_hole;

    // Collect all hole positions and radii
    let mut holes: Vec<(Vec2, f64)> = Vec::new();

    for via in &pcb.vias {
        holes.push((via.position, via.drill / 2.0));
    }

    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                let abs_pos = Vec2::new(
                    footprint.position.x + pad.position.x,
                    footprint.position.y + pad.position.y,
                );
                holes.push((abs_pos, drill.diameter / 2.0));
            }
        }
    }

    // O(n^2) check — fine for typical PCB sizes; use spatial index for large boards
    for i in 0..holes.len() {
        for j in (i + 1)..holes.len() {
            let dx = holes[i].0.x - holes[j].0.x;
            let dy = holes[i].0.y - holes[j].0.y;
            let center_dist = (dx * dx + dy * dy).sqrt();
            let edge_dist = center_dist - holes[i].1 - holes[j].1;

            if edge_dist < min_spacing - 1e-6 {
                let mid = Vec2::new(
                    (holes[i].0.x + holes[j].0.x) / 2.0,
                    (holes[i].0.y + holes[j].0.y) / 2.0,
                );
                violations.push(DrcViolation {
                    rule: DrcRuleType::HoleToHole,
                    severity: DrcSeverity::Error,
                    position: mid,
                    message: format!(
                        "Hole-to-hole spacing {:.3}mm < {:.3}mm",
                        edge_dist, min_spacing
                    ),
                    actual: edge_dist,
                    required: min_spacing,
                });
            }
        }
    }
}

/// Check annular ring width on through-hole pads and vias.
fn check_annular_ring(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_ring = pcb.rules.min_annular_ring;

    // Check vias
    for via in &pcb.vias {
        let ring = (via.diameter - via.drill) / 2.0;
        if ring < min_ring - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::AnnularRing,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!("Via annular ring {:.3}mm < {:.3}mm", ring, min_ring),
                actual: ring,
                required: min_ring,
            });
        }
    }

    // Check THT pads
    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if pad.pad_type != PadType::THT {
                continue;
            }
            if let Some(drill) = &pad.drill {
                let pad_min_dim = pad_min_dimension(pad);
                let ring = (pad_min_dim - drill.diameter) / 2.0;
                if ring < min_ring - 1e-6 {
                    let abs_pos = Vec2::new(
                        footprint.position.x + pad.position.x,
                        footprint.position.y + pad.position.y,
                    );
                    violations.push(DrcViolation {
                        rule: DrcRuleType::AnnularRing,
                        severity: DrcSeverity::Error,
                        position: abs_pos,
                        message: format!(
                            "Pad {} on {} annular ring {:.3}mm < {:.3}mm",
                            pad.number, footprint.reference, ring, min_ring
                        ),
                        actual: ring,
                        required: min_ring,
                    });
                }
            }
        }
    }
}

/// Get the minimum dimension of a pad (for annular ring calculation).
fn pad_min_dimension(pad: &Pad) -> f64 {
    match &pad.shape {
        PadShape::Circle { diameter } => *diameter,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.min(*height),
        PadShape::Custom { vertices } => {
            // Approximate as bounding box minimum dimension
            if vertices.is_empty() {
                return 0.0;
            }
            let mut min_x = f64::MAX;
            let mut max_x = f64::MIN;
            let mut min_y = f64::MAX;
            let mut max_y = f64::MIN;
            for v in vertices {
                min_x = min_x.min(v.x);
                max_x = max_x.max(v.x);
                min_y = min_y.min(v.y);
                max_y = max_y.max(v.y);
            }
            (max_x - min_x).min(max_y - min_y)
        }
    }
}

/// Build a map of net ID to clearance from design rules.
fn build_net_clearance_map(pcb: &Pcb) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    for rule in &pcb.rules.class_rules {
        if let Some(nets) = pcb.rules.net_class_assignments.get(&rule.name) {
            for net_id in nets {
                map.insert(net_id.clone(), rule.clearance);
            }
        }
    }
    map
}

/// Build a map of net ID to minimum trace width from design rules.
fn build_net_trace_width_map(pcb: &Pcb) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    for rule in &pcb.rules.class_rules {
        if let Some(nets) = pcb.rules.net_class_assignments.get(&rule.name) {
            for net_id in nets {
                map.insert(net_id.clone(), rule.trace_width);
            }
        }
    }
    map
}

/// Compute the minimum distance between two axis-aligned bounding boxes.
/// Each box is represented as `[min_x, min_y, max_x, max_y]`.
/// Returns 0.0 if they overlap.
fn bbox_distance(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dx = (a[0] - b[2]).max(b[0] - a[2]).max(0.0);
    let dy = (a[1] - b[3]).max(b[1] - a[3]).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Compute minimum distance from a point to a closed polygon (edge segments).
fn min_distance_to_polygon(point: &Vec2, polygon: &[Vec2]) -> f64 {
    if polygon.is_empty() {
        return f64::MAX;
    }

    let mut min_dist = f64::MAX;
    let n = polygon.len();
    for i in 0..n {
        let a = &polygon[i];
        let b = &polygon[(i + 1) % n];
        let dist = point_to_segment_distance(point, a, b);
        if dist < min_dist {
            min_dist = dist;
        }
    }
    min_dist
}

/// Distance from a point to a line segment.
fn point_to_segment_distance(p: &Vec2, a: &Vec2, b: &Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;

    if len_sq < 1e-12 {
        // Degenerate segment (point)
        let ex = p.x - a.x;
        let ey = p.y - a.y;
        return (ex * ex + ey * ey).sqrt();
    }

    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);

    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    let ex = p.x - proj_x;
    let ey = p.y - proj_y;
    (ex * ex + ey * ey).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Create a minimal clean PCB (no violations expected).
    fn clean_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
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
                        material: Some("FR4".to_string()),
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
            nets: vec![
                Net {
                    id: "1".to_string(),
                    name: "VCC".to_string(),
                },
                Net {
                    id: "2".to_string(),
                    name: "GND".to_string(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".to_string(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![
                Trace {
                    start: Vec2::new(20.0, 40.0),
                    end: Vec2::new(50.0, 40.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: "1".to_string(),
                },
                Trace {
                    start: Vec2::new(20.0, 50.0),
                    end: Vec2::new(50.0, 50.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: "2".to_string(),
                },
            ],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(50.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
            }],
            zones: vec![],
            keepouts: vec![],
        }
    }

    #[test]
    fn clean_pcb_no_violations() {
        let pcb = clean_pcb();
        let violations = check_drc(&pcb);
        // The clean PCB should have no violations — traces are 10mm apart
        assert!(
            violations.is_empty(),
            "expected no violations, got: {:?}",
            violations
        );
    }

    #[test]
    fn detect_min_trace_width_violation() {
        let mut pcb = clean_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 60.0),
            end: Vec2::new(50.0, 60.0),
            width: 0.1, // below 0.25 minimum
            layer: PcbLayer::FCu,
            net: "1".to_string(),
        });

        let violations = check_drc(&pcb);
        let trace_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::MinTraceWidth)
            .collect();
        assert!(
            !trace_violations.is_empty(),
            "should detect min trace width violation"
        );
        assert!((trace_violations[0].actual - 0.1).abs() < 1e-6);
        assert!((trace_violations[0].required - 0.25).abs() < 1e-6);
    }

    #[test]
    fn detect_pad_to_pad_short() {
        let mut pcb = clean_pcb();
        let pad = |num: &str, x: f64, net: &str| Pad {
            number: num.to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 1.0,
                height: 1.2,
            },
            position: Vec2::new(x, 0.0),
            rotation: 0.0,
            drill: None,
            net: Some(net.to_string()),
            layers: vec![PcbLayer::FCu],
        };
        // Two stacked pads on different nets — a hard short.
        pcb.footprints.push(Footprint {
            reference: "U1".to_string(),
            value: "IC".to_string(),
            footprint_name: "broken".to_string(),
            position: Vec2::new(60.0, 60.0),
            rotation: 0.0,
            front: true,
            pads: vec![pad("1", 0.0, "1"), pad("2", 0.0, "2")],
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        });

        let violations = check_drc(&pcb);
        let pad_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance && v.message.contains("pad"))
            .collect();
        assert!(
            !pad_violations.is_empty(),
            "should detect pad-to-pad short, got: {:?}",
            violations
        );
    }

    #[test]
    fn detect_min_drill_violation() {
        let mut pcb = clean_pcb();
        pcb.vias.push(Via {
            position: Vec2::new(30.0, 60.0),
            diameter: 0.6,
            drill: 0.15, // below 0.2 minimum
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
        });

        let violations = check_drc(&pcb);
        let drill_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::MinDrill)
            .collect();
        assert!(
            !drill_violations.is_empty(),
            "should detect min drill violation"
        );
        assert!((drill_violations[0].actual - 0.15).abs() < 1e-6);
    }

    #[test]
    fn detect_edge_clearance_violation() {
        let mut pcb = clean_pcb();
        // Place a trace very close to the board edge
        pcb.traces.push(Trace {
            start: Vec2::new(0.1, 40.0),
            end: Vec2::new(0.1, 60.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "1".to_string(),
        });

        let violations = check_drc(&pcb);
        let edge_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::EdgeClearance)
            .collect();
        assert!(
            !edge_violations.is_empty(),
            "should detect edge clearance violation"
        );
    }

    #[test]
    fn detect_hole_to_hole_violation() {
        let mut pcb = clean_pcb();
        // Place two vias very close together
        pcb.vias.push(Via {
            position: Vec2::new(50.5, 40.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
        });

        let violations = check_drc(&pcb);
        let hole_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::HoleToHole)
            .collect();
        assert!(
            !hole_violations.is_empty(),
            "should detect hole-to-hole violation"
        );
    }

    #[test]
    fn detect_annular_ring_violation() {
        let mut pcb = clean_pcb();
        // Via with very thin annular ring
        pcb.vias.push(Via {
            position: Vec2::new(70.0, 40.0),
            diameter: 0.5,
            drill: 0.4, // ring = 0.05mm < 0.15mm
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "1".to_string(),
        });

        let violations = check_drc(&pcb);
        let ring_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::AnnularRing)
            .collect();
        assert!(
            !ring_violations.is_empty(),
            "should detect annular ring violation"
        );
        assert!((ring_violations[0].actual - 0.05).abs() < 1e-6);
    }

    #[test]
    fn detect_clearance_violation() {
        let mut pcb = clean_pcb();
        // Remove existing well-spaced traces
        pcb.traces.clear();
        // Add two traces on the same layer, same Y, different nets, very close
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 40.0),
            end: Vec2::new(80.0, 40.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "1".to_string(),
        });
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 40.3),
            end: Vec2::new(80.0, 40.3),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "2".to_string(),
        });

        let violations = check_drc(&pcb);
        let clearance_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !clearance_violations.is_empty(),
            "should detect clearance violation between close traces"
        );
    }

    #[test]
    fn point_to_segment() {
        let p = Vec2::new(1.0, 1.0);
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(2.0, 0.0);
        let dist = point_to_segment_distance(&p, &a, &b);
        assert!((dist - 1.0).abs() < 1e-10);
    }

    #[test]
    fn bbox_distance_separated() {
        let dist = bbox_distance([0.0, 0.0, 1.0, 1.0], [3.0, 0.0, 4.0, 1.0]);
        assert!((dist - 2.0).abs() < 1e-10);
    }

    #[test]
    fn bbox_distance_overlapping() {
        let dist = bbox_distance([0.0, 0.0, 2.0, 2.0], [1.0, 1.0, 3.0, 3.0]);
        assert!((dist - 0.0).abs() < 1e-10);
    }
}
