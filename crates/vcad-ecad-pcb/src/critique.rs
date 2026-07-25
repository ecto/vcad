//! Read-only route audit — `critique_route`.
//!
//! Reports the quality of a single net's routing without mutating anything: how
//! long it is, how many layer changes (vias) it takes, how much clearance
//! margin it has to other-net copper (via the same oracle the router uses), and
//! any DRC issues it's involved in. The agent-UX "audit before you commit"
//! verb — you inspect a net's route before trusting it.

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::drc::{check_drc, DrcRuleType};
use crate::session::RouteSession;
use crate::spatial::CopperGeom;

/// A read-only quality report for one net's routing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetCritique {
    /// Net name.
    pub net: String,
    /// Whether the net has any routed copper.
    pub routed: bool,
    /// Total routed trace length (mm).
    pub routed_length_mm: f64,
    /// Number of trace segments.
    pub segment_count: usize,
    /// Number of vias on this net (layer changes).
    pub via_count: usize,
    /// Distinct copper layers the net uses.
    pub layers: Vec<PcbLayer>,
    /// Smallest edge-to-edge clearance to any other-net copper (mm), or `None`
    /// when nothing else is nearby.
    pub min_clearance_mm: Option<f64>,
    /// The clearance this net is required to hold (mm).
    pub required_clearance_mm: f64,
    /// DRC issues this net is involved in (clearance, short, unconnected).
    pub drc_issues: Vec<String>,
}

/// Audit one net's routing on `pcb`. Mutates nothing.
pub fn critique_net(pcb: &Pcb, net: &str) -> NetCritique {
    let segs: Vec<_> = pcb.traces.iter().filter(|t| t.net == net).collect();
    let via_count = pcb.vias.iter().filter(|v| v.net == net).count();
    let routed_length_mm: f64 = segs.iter().map(|t| dist(t.start, t.end)).sum();

    let mut layers: Vec<PcbLayer> = Vec::new();
    for t in &segs {
        if !layers.contains(&t.layer) {
            layers.push(t.layer);
        }
    }

    // Closest approach to any other-net copper, via the router's own oracle.
    // Probe with a generous reach (not just the required clearance) so the
    // audit reports the true nearest-copper distance even when it comfortably
    // clears — the broadphase only looks as far as the value passed in.
    let session = RouteSession::from_pcb(pcb);
    let required_clearance_mm = session.clearance_for(net);
    let audit_reach = required_clearance_mm.max(2.0);
    let mut min_clear = f64::INFINITY;
    for t in &segs {
        let g = CopperGeom::Segment {
            a: t.start,
            b: t.end,
            half_w: t.width / 2.0,
        };
        let r = session.probe(&g, t.layer, net, audit_reach);
        if r.min_clearance < min_clear {
            min_clear = r.min_clearance;
        }
    }
    let min_clearance_mm = min_clear.is_finite().then_some(min_clear);

    // DRC issues mentioning this net (substring match on the message).
    let drc_issues: Vec<String> = check_drc(pcb)
        .into_iter()
        .filter(|v| {
            matches!(
                v.rule,
                DrcRuleType::Clearance | DrcRuleType::Short | DrcRuleType::UnconnectedNet
            )
        })
        .filter(|v| v.message.contains(net))
        .map(|v| v.message)
        .collect();

    NetCritique {
        net: net.to_string(),
        routed: !segs.is_empty(),
        routed_length_mm,
        segment_count: segs.len(),
        via_count,
        layers,
        min_clearance_mm,
        required_clearance_mm,
        drc_issues,
    }
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn pcb_with(traces: Vec<Trace>, vias: Vec<Via>) -> Pcb {
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
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                }],
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
            traces,
            trace_arcs: vec![],
            vias,
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn trace(a: (f64, f64), b: (f64, f64), layer: PcbLayer, net: &str) -> Trace {
        Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer,
            net: net.into(),
            source: None,
        }
    }

    #[test]
    fn critique_reports_length_layers_and_margin() {
        let pcb = pcb_with(
            vec![
                trace((10.0, 20.0), (20.0, 20.0), PcbLayer::FCu, "SIG"),
                trace((20.0, 20.0), (20.0, 30.0), PcbLayer::BCu, "SIG"),
                // An other-net trace 0.5mm away from the first SIG segment.
                trace((10.0, 20.5), (20.0, 20.5), PcbLayer::FCu, "GND"),
            ],
            vec![Via {
                position: Vec2::new(20.0, 20.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "SIG".into(),
                source: None,
            }],
        );
        let c = critique_net(&pcb, "SIG");
        assert!(c.routed);
        assert_eq!(c.segment_count, 2);
        assert_eq!(c.via_count, 1);
        assert_eq!(c.layers.len(), 2, "SIG spans FCu + BCu");
        assert!((c.routed_length_mm - 20.0).abs() < 1e-9);
        // Edge-to-edge to the GND trace: 0.5 centerline - 0.125 - 0.125 = 0.25.
        let m = c.min_clearance_mm.expect("GND is nearby");
        assert!((m - 0.25).abs() < 1e-6, "min clearance {m} should be ~0.25");
    }

    #[test]
    fn critique_unrouted_net() {
        let c = critique_net(&pcb_with(vec![], vec![]), "NOPE");
        assert!(!c.routed);
        assert_eq!(c.segment_count, 0);
        assert!(c.min_clearance_mm.is_none());
    }
}
