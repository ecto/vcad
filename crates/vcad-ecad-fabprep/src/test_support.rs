//! Small hand-built boards for the unit tests.
//!
//! Deliberately tiny: the pipeline's behaviour under a fixture baseline, a
//! strippable offender, and a fail-closed stall is all reproducible on a 2-layer
//! 10×10 mm board, and a test that takes a minute is a test nobody runs.

use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, NetClassRules, Pad, PadShape, PadType, Pcb,
    PcbLayer, StackupLayer, Trace,
};
use vcad_ir::Vec2;

/// An empty 10×10 mm two-layer board with ordinary rules.
pub fn test_board() -> Pcb {
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(0.0, 10.0),
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
                    material: Some("FR4".into()),
                },
            ],
        },
        nets: vec![],
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.2,
                clearance: 0.2,
                via_diameter: 0.6,
                via_drill: 0.3,
                diff_pair_gap: None,
                diff_pair_width: None,
                target_impedance: None,
                target_diff_impedance: None,
            },
            class_rules: vec![],
            net_class_assignments: Default::default(),
            edge_clearance: 0.2,
            hole_to_hole: 0.25,
            min_annular_ring: 0.1,
            min_drill: 0.2,
        },
        footprints: vec![],
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

fn footprint_mut<'a>(pcb: &'a mut Pcb, reference: &str) -> &'a mut Footprint {
    if let Some(i) = pcb.footprints.iter().position(|f| f.reference == reference) {
        return &mut pcb.footprints[i];
    }
    pcb.footprints.push(Footprint {
        reference: reference.to_string(),
        value: "TEST".into(),
        footprint_name: "Test:Pad".into(),
        position: Vec2::new(0.0, 0.0),
        rotation: 0.0,
        front: true,
        pads: vec![],
        graphics: vec![],
        model_3d: None,
        properties: Default::default(),
    });
    pcb.footprints.last_mut().expect("just pushed")
}

/// Add a 0.5 mm square SMD pad on the front copper layer.
pub fn with_smd_pad(pcb: &mut Pcb, reference: &str, number: &str, x: f64, y: f64, net: &str) {
    let fp = footprint_mut(pcb, reference);
    fp.pads.push(Pad {
        number: number.to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: 0.5,
            height: 0.5,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: Some(net.to_string()),
        layers: vec![PcbLayer::FCu],
    });
}

/// Add a through-hole pad with the given drill diameter.
pub fn with_tht_pad(pcb: &mut Pcb, reference: &str, number: &str, x: f64, y: f64, drill: f64) {
    let fp = footprint_mut(pcb, reference);
    fp.pads.push(Pad {
        number: number.to_string(),
        pad_type: PadType::THT,
        shape: PadShape::Circle {
            diameter: drill + 0.3,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: Some(vcad_ir::ecad::DrillSpec {
            diameter: drill,
            oval: false,
            oval_height: None,
        }),
        net: Some(format!("N-{reference}-{number}")),
        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
    });
}

/// Add a front-copper trace at the default class width.
pub fn with_trace(pcb: &mut Pcb, net: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
    let width = pcb.rules.default_rules.trace_width;
    pcb.traces.push(Trace {
        start: Vec2::new(x0, y0),
        end: Vec2::new(x1, y1),
        width,
        layer: PcbLayer::FCu,
        net: net.to_string(),
        source: None,
    });
}
