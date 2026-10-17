//! `RouteOptions::through_vias_only` (its own test binary: the mode is process-wide).
use std::collections::HashMap;
use vcad_ecad_pcb::router::auto::{route_all_with_opts, RouteOptions};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

fn pad(num: &str, w: f64, h: f64, net: &str) -> Pad {
    Pad {
        number: num.into(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: w,
            height: h,
        },
        position: Vec2::new(0.0, 0.0),
        rotation: 0.0,
        drill: None,
        net: Some(net.into()),
        layers: vec![PcbLayer::FCu],
    }
}

fn fp(r: &str, x: f64, y: f64, p: Pad) -> Footprint {
    Footprint {
        reference: r.into(),
        value: String::new(),
        footprint_name: String::new(),
        position: Vec2::new(x, y),
        rotation: 0.0,
        front: true,
        pads: vec![p],
        graphics: vec![],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn board() -> Pcb {
    let layers = [
        PcbLayer::FCu,
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::BCu,
    ];
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(40.0, 20.0),
                Vec2::new(0.0, 20.0),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: layers
                .iter()
                .map(|l| StackupLayer {
                    layer: *l,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(0.4),
                    dielectric_er: Some(4.4),
                    material: None,
                })
                .collect(),
        },
        nets: vec![
            Net {
                id: "X".into(),
                name: "X".into(),
            },
            Net {
                id: "W".into(),
                name: "W".into(),
            },
        ],
        rules: DesignRules {
            inferred_floors: Vec::new(),
            min_microvia_drill: 0.1,
            min_microvia_diameter: 0.2,
            default_rules: NetClassRules {
                neck_width: None,
                neck_length: None,
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
            net_class_assignments: HashMap::new(),
            edge_clearance: 0.3,
            hole_to_hole: 0.25,
            min_annular_ring: 0.1,
            min_drill: 0.2,
        },
        footprints: vec![
            fp("A", 5.0, 10.0, pad("1", 1.0, 1.0, "X")),
            fp("B", 35.0, 10.0, pad("1", 1.0, 1.0, "X")),
            // a wall of another net across the whole board on F.Cu: X must dive
            fp("W", 20.0, 10.0, pad("1", 1.0, 19.6, "W")),
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
fn through_vias_only_never_lays_a_blind_via() {
    let pcb = board();
    let opts = RouteOptions {
        through_vias_only: true,
        ..Default::default()
    };
    let r = route_all_with_opts(&pcb, 0.2, &["X".to_string()], &opts);
    assert!(r.unrouted_nets.is_empty(), "{:?}", r.unrouted_nets);
    assert!(!r.vias.is_empty(), "X has to leave F.Cu to cross the wall");
    for v in &r.vias {
        let span = [v.start_layer, v.end_layer];
        assert!(
            span.contains(&PcbLayer::FCu) && span.contains(&PcbLayer::BCu),
            "blind/buried via {:?}..{:?}",
            v.start_layer,
            v.end_layer
        );
    }
}
