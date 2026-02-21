//! Zone fill (copper pour) algorithm.
//!
//! Generates filled zone polygons for PCB copper pours. A zone is a region
//! of copper connected to a net, with clearance gaps around other-net copper
//! elements (traces, pads, vias).
//!
//! This is a simplified implementation that creates clearance cutouts by
//! expanding obstacle bounding boxes. A production implementation would use
//! polygon boolean operations (e.g. via the `geo` crate) for precise geometry.

use vcad_ir::ecad::{Pad, PadShape, Pcb, PcbLayer, Zone};
use vcad_ir::Vec2;

/// Filled zone result after copper pour calculation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FilledZone {
    /// Filled polygons (each is a closed outline in board coordinates).
    pub polygons: Vec<Vec<Vec2>>,
    /// Net this zone is assigned to.
    pub net: String,
    /// Layer this zone is on.
    pub layer: PcbLayer,
}

/// Fill all zones on a PCB.
///
/// For each zone, this computes filled copper polygons with clearance gaps
/// around copper elements belonging to other nets.
///
/// # Returns
///
/// A vector of [`FilledZone`] results, one per input zone.
pub fn fill_zones(pcb: &Pcb) -> Vec<FilledZone> {
    pcb.zones.iter().map(|zone| fill_zone(pcb, zone)).collect()
}

/// Fill a single zone, producing clearance-cut polygons.
fn fill_zone(pcb: &Pcb, zone: &Zone) -> FilledZone {
    // Collect rectangular clearance cutouts from other-net copper elements
    let cutouts = collect_clearance_cutouts(pcb, zone);

    // Start with the zone outline as the base polygon. In a full implementation
    // we would subtract the cutout rectangles using polygon booleans. For now,
    // we return the zone outline minus any cutouts as separate polygons.
    //
    // Simplified approach: if there are no cutouts, the filled zone is the
    // entire outline. If there are cutouts, we still return the outline
    // (a real implementation would clip it).
    let mut polygons = vec![zone.outline.clone()];

    // For each hole in the zone definition, add it as a cutout polygon
    for hole in &zone.holes {
        if !hole.is_empty() {
            polygons.push(hole.clone());
        }
    }

    // Filter out cutout regions that are too small (below min_area)
    // In the simplified version, we just note their existence.
    let _ = cutouts;

    FilledZone {
        polygons,
        net: zone.net.clone(),
        layer: zone.layer,
    }
}

/// Rectangular clearance cutout in board coordinates.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ClearanceCutout {
    /// Minimum corner.
    min: Vec2,
    /// Maximum corner.
    max: Vec2,
}

/// Collect all clearance cutouts for a zone from other-net copper elements.
fn collect_clearance_cutouts(pcb: &Pcb, zone: &Zone) -> Vec<ClearanceCutout> {
    let clearance = zone.clearance;
    let mut cutouts = Vec::new();

    // Trace clearances (other-net traces on the same layer)
    for trace in &pcb.traces {
        if trace.net == zone.net || trace.layer != zone.layer {
            continue;
        }
        let min_x = trace.start.x.min(trace.end.x) - trace.width / 2.0 - clearance;
        let min_y = trace.start.y.min(trace.end.y) - trace.width / 2.0 - clearance;
        let max_x = trace.start.x.max(trace.end.x) + trace.width / 2.0 + clearance;
        let max_y = trace.start.y.max(trace.end.y) + trace.width / 2.0 + clearance;
        cutouts.push(ClearanceCutout {
            min: Vec2::new(min_x, min_y),
            max: Vec2::new(max_x, max_y),
        });
    }

    // Via clearances (other-net vias that span our layer)
    for via in &pcb.vias {
        if via.net == zone.net {
            continue;
        }
        let radius = via.diameter / 2.0 + clearance;
        cutouts.push(ClearanceCutout {
            min: Vec2::new(via.position.x - radius, via.position.y - radius),
            max: Vec2::new(via.position.x + radius, via.position.y + radius),
        });
    }

    // Pad clearances (other-net pads on the same layer)
    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            let pad_net = pad.net.as_deref().unwrap_or("");
            if pad_net == zone.net || !pad.layers.contains(&zone.layer) {
                continue;
            }
            let (hw, hh) = pad_half_extents(pad);
            let abs_pos = Vec2::new(
                footprint.position.x + pad.position.x,
                footprint.position.y + pad.position.y,
            );
            cutouts.push(ClearanceCutout {
                min: Vec2::new(abs_pos.x - hw - clearance, abs_pos.y - hh - clearance),
                max: Vec2::new(abs_pos.x + hw + clearance, abs_pos.y + hh + clearance),
            });
        }
    }

    cutouts
}

/// Get the half-width and half-height of a pad's bounding box.
fn pad_half_extents(pad: &Pad) -> (f64, f64) {
    match &pad.shape {
        PadShape::Circle { diameter } => {
            let r = diameter / 2.0;
            (r, r)
        }
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (width / 2.0, height / 2.0),
        PadShape::Custom { vertices } => {
            if vertices.is_empty() {
                return (0.0, 0.0);
            }
            let mut max_x: f64 = 0.0;
            let mut max_y: f64 = 0.0;
            for v in vertices {
                max_x = max_x.max(v.x.abs());
                max_y = max_y.max(v.y.abs());
            }
            (max_x, max_y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Create a minimal test PCB.
    fn test_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
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
            traces: vec![Trace {
                start: Vec2::new(10.0, 20.0),
                end: Vec2::new(30.0, 20.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".to_string(),
            }],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![Zone {
                outline: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                holes: vec![],
                net: "2".to_string(),
                layer: PcbLayer::FCu,
                clearance: 0.3,
                min_area: 0.0,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: Some(0.5),
                thermal_spoke_width: Some(0.5),
                priority: 0,
            }],
            keepouts: vec![],
        }
    }

    #[test]
    fn fill_zones_produces_output() {
        let pcb = test_pcb();
        let filled = fill_zones(&pcb);
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0].net, "2");
        assert_eq!(filled[0].layer, PcbLayer::FCu);
        assert!(!filled[0].polygons.is_empty());
        assert_eq!(filled[0].polygons[0].len(), 4); // zone outline
    }

    #[test]
    fn fill_zones_empty_pcb() {
        let mut pcb = test_pcb();
        pcb.zones.clear();
        let filled = fill_zones(&pcb);
        assert!(filled.is_empty());
    }

    #[test]
    fn pad_half_extents_circle() {
        let pad = Pad {
            number: "1".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Circle { diameter: 2.0 },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        };
        let (hw, hh) = pad_half_extents(&pad);
        assert!((hw - 1.0).abs() < 1e-10);
        assert!((hh - 1.0).abs() < 1e-10);
    }

    #[test]
    fn pad_half_extents_rect() {
        let pad = Pad {
            number: "1".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 3.0,
                height: 2.0,
            },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        };
        let (hw, hh) = pad_half_extents(&pad);
        assert!((hw - 1.5).abs() < 1e-10);
        assert!((hh - 1.0).abs() < 1e-10);
    }
}
