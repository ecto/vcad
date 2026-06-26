//! Dev harness: render a representative sample PCB to SVG so the 2D board
//! renderer can be eyeballed without a full MCP round-trip.
//!
//! ```sh
//! cargo run -p vcad-render --example pcb_preview -- /tmp/pcb.svg
//! rsvg-convert -w 1600 -o /tmp/pcb.png /tmp/pcb.svg   # or @resvg/resvg-js
//! ```
//!
//! Exercises: front + back copper, vias, SMD + through-hole pads, oval/round
//! pads, silkscreen reference designators, footprint values, courtyards, and a
//! deliberately-unrouted net (so the ratsnest air-wires show).

use std::collections::HashMap;

use vcad_ir::ecad::{
    BoardOutline, DesignRules, DrillSpec, Footprint, FootprintGraphic, LayerStackup, Net,
    NetClassRules, Pad, PadShape, PadType, Pcb, PcbLayer, ThermalReliefStyle, Trace, TraceArc, Via,
    Zone, ZoneFillType,
};
use vcad_ir::Vec2;
use vcad_render::pcb::{render_pcb_svg_opts, Highlight, PcbRenderOpts, Theme};

fn refdes(text: &str, x: f64, y: f64) -> FootprintGraphic {
    FootprintGraphic::Text {
        text: text.to_string(),
        position: Vec2::new(x, y),
        rotation: 0.0,
        height: 0.9,
        width: 0.12,
        layer: PcbLayer::FSilkS,
    }
}

fn courtyard(hw: f64, hh: f64) -> FootprintGraphic {
    FootprintGraphic::Rect {
        start: Vec2::new(-hw, -hh),
        end: Vec2::new(hw, hh),
        width: 0.05,
        layer: PcbLayer::FCrtYd,
    }
}

fn smd_rect(num: &str, net: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::RoundRect {
            width: w,
            height: h,
            corner_ratio: 0.25,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: Some(net.to_string()),
        layers: vec![PcbLayer::FCu],
    }
}

fn th_pad(num: &str, net: &str, x: f64, y: f64, dia: f64, drill: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::THT,
        shape: PadShape::Circle { diameter: dia },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: Some(DrillSpec {
            diameter: drill,
            oval: false,
            oval_height: None,
        }),
        net: Some(net.to_string()),
        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
    }
}

#[allow(clippy::too_many_arguments)]
fn fp(
    reference: &str,
    value: &str,
    name: &str,
    x: f64,
    y: f64,
    rot: f64,
    pads: Vec<Pad>,
    hw: f64,
    hh: f64,
) -> Footprint {
    Footprint {
        reference: reference.to_string(),
        value: value.to_string(),
        footprint_name: name.to_string(),
        position: Vec2::new(x, y),
        rotation: rot,
        front: true,
        pads,
        graphics: vec![refdes(reference, 0.0, hh + 0.8), courtyard(hw, hh)],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn two_pad(
    reference: &str,
    value: &str,
    x: f64,
    y: f64,
    rot: f64,
    n1: &str,
    n2: &str,
) -> Footprint {
    // 0805-ish chip part.
    fp(
        reference,
        value,
        "C_0805",
        x,
        y,
        rot,
        vec![
            smd_rect("1", n1, -0.95, 0.0, 1.0, 1.3),
            smd_rect("2", n2, 0.95, 0.0, 1.0, 1.3),
        ],
        1.6,
        0.9,
    )
}

fn soic8(reference: &str, value: &str, x: f64, y: f64, pin_nets: [&str; 8]) -> Footprint {
    let mut pads = Vec::new();
    let pitch = 1.27;
    // Left column 1..4 (top to bottom), right column 5..8 (bottom to top).
    for (i, net) in pin_nets[..4].iter().enumerate() {
        let py = (1.5 - i as f64) * pitch;
        pads.push(Pad {
            number: (i + 1).to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::RoundRect {
                width: 1.5,
                height: 0.6,
                corner_ratio: 0.2,
            },
            position: Vec2::new(-2.7, py),
            rotation: 0.0,
            drill: None,
            net: Some((*net).to_string()),
            layers: vec![PcbLayer::FCu],
        });
    }
    for (i, net) in pin_nets[4..].iter().enumerate() {
        let py = (-1.5 + i as f64) * pitch;
        pads.push(Pad {
            number: (i + 5).to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::RoundRect {
                width: 1.5,
                height: 0.6,
                corner_ratio: 0.2,
            },
            position: Vec2::new(2.7, py),
            rotation: 0.0,
            drill: None,
            net: Some((*net).to_string()),
            layers: vec![PcbLayer::FCu],
        });
    }
    Footprint {
        reference: reference.to_string(),
        value: value.to_string(),
        footprint_name: "SOIC-8".to_string(),
        position: Vec2::new(x, y),
        rotation: 0.0,
        front: true,
        pads,
        graphics: vec![refdes(reference, 0.0, 3.2), courtyard(3.6, 2.6)],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn header(reference: &str, x: f64, y: f64, n: usize, nets: &[&str]) -> Footprint {
    let mut pads = Vec::new();
    let pitch = 2.54;
    for i in 0..n {
        let px = (i as f64 - (n as f64 - 1.0) / 2.0) * pitch;
        let net = nets.get(i).copied().unwrap_or("NC");
        let mut pad = th_pad(&(i + 1).to_string(), net, px, 0.0, 1.6, 0.9);
        if i == 0 {
            // Pin 1 marker: square pad.
            pad.shape = PadShape::Rect {
                width: 1.6,
                height: 1.6,
            };
        }
        pads.push(pad);
    }
    let hw = (n as f64) * pitch / 2.0 + 0.6;
    Footprint {
        reference: reference.to_string(),
        value: format!("1x{n}"),
        footprint_name: format!("PinHeader_1x{n:02}"),
        position: Vec2::new(x, y),
        rotation: 0.0,
        front: true,
        pads,
        graphics: vec![refdes(reference, 0.0, 2.2), courtyard(hw, 1.5)],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn trace(net: &str, layer: PcbLayer, x1: f64, y1: f64, x2: f64, y2: f64, w: f64) -> Trace {
    Trace {
        start: Vec2::new(x1, y1),
        end: Vec2::new(x2, y2),
        width: w,
        layer,
        net: net.to_string(),
    }
}

fn sample_board() -> Pcb {
    let footprints = vec![
        soic8(
            "U1",
            "ATtiny85",
            25.0,
            18.0,
            ["VCC", "PB3", "PB4", "GND", "PB0", "PB1", "PB2", "RST"],
        ),
        two_pad("R1", "10k", 14.0, 26.0, 0.0, "RST", "VCC"),
        two_pad("R2", "330", 36.0, 26.0, 0.0, "PB0", "LED_A"),
        two_pad("C1", "100n", 14.0, 12.0, 90.0, "VCC", "GND"),
        two_pad("C2", "1u", 36.0, 12.0, 90.0, "VCC", "GND"),
        two_pad("D1", "LED", 44.0, 26.0, 0.0, "LED_A", "GND"),
        header("J1", 9.0, 18.0, 4, &["VCC", "GND", "PB1", "PB2"]),
        header("J2", 41.0, 18.0, 2, &["PB3", "PB4"]),
    ];

    // Route most nets; leave PB1/PB2/PB3/PB4 unrouted so the ratsnest shows.
    let traces = vec![
        // VCC (front)
        trace("VCC", PcbLayer::FCu, 11.54, 18.0, 13.0, 12.0, 0.4),
        trace("VCC", PcbLayer::FCu, 13.0, 12.0, 14.0, 12.95, 0.4),
        trace("VCC", PcbLayer::FCu, 14.0, 11.05, 22.3, 14.31, 0.4),
        // GND (back)
        trace("GND", PcbLayer::BCu, 9.0, 15.46, 14.0, 11.05, 0.5),
        trace("GND", PcbLayer::BCu, 14.0, 11.05, 27.7, 14.31, 0.5),
        trace("GND", PcbLayer::BCu, 27.7, 14.31, 36.0, 11.05, 0.5),
        trace("GND", PcbLayer::BCu, 44.95, 26.0, 36.0, 13.0, 0.5),
        // PB0 -> R2
        trace("PB0", PcbLayer::FCu, 27.7, 19.69, 34.4, 26.0, 0.3),
        // LED_A
        trace("LED_A", PcbLayer::FCu, 37.6, 26.0, 43.05, 26.0, 0.3),
        // RST
        trace("RST", PcbLayer::FCu, 22.3, 16.41, 14.95, 26.0, 0.3),
        trace("RST", PcbLayer::FCu, 13.05, 26.0, 11.54, 19.5, 0.3),
    ];

    let vias = vec![
        Via {
            position: Vec2::new(14.0, 11.05),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "GND".to_string(),
        },
        Via {
            position: Vec2::new(27.7, 14.31),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "GND".to_string(),
        },
    ];

    let nets: Vec<Net> = [
        "VCC", "GND", "RST", "PB0", "PB1", "PB2", "PB3", "PB4", "LED_A",
    ]
    .iter()
    .map(|n| Net {
        id: (*n).to_string(),
        name: (*n).to_string(),
    })
    .collect();

    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(50.0, 0.0),
                Vec2::new(50.0, 35.0),
                Vec2::new(0.0, 35.0),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup { layers: vec![] },
        nets,
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
            net_class_assignments: HashMap::new(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.2,
        },
        footprints,
        traces,
        trace_arcs: vec![],
        vias,
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

// ─── complex / edge-case boards ──────────────────────────────────────────────

fn base_rules() -> DesignRules {
    DesignRules {
        default_rules: NetClassRules {
            name: "Default".to_string(),
            trace_width: 0.2,
            clearance: 0.2,
            via_diameter: 0.7,
            via_drill: 0.35,
            diff_pair_gap: None,
            diff_pair_width: None,
        },
        class_rules: vec![],
        net_class_assignments: HashMap::new(),
        edge_clearance: 0.4,
        hole_to_hole: 0.4,
        min_annular_ring: 0.13,
        min_drill: 0.2,
    }
}

fn qfp_pad(n: usize, x: f64, y: f64, w: f64, h: f64, net: String) -> Pad {
    Pad {
        number: (n + 1).to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::RoundRect {
            width: w,
            height: h,
            corner_ratio: 0.2,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: Some(net),
        layers: vec![PcbLayer::FCu],
    }
}

/// A fine-pitch QFP footprint, `per_side` pads per edge (numbered CCW from the
/// left side, KiCad-style), with a per-pin net assignment.
fn qfp(
    reference: &str,
    value: &str,
    cx: f64,
    cy: f64,
    per_side: usize,
    pitch: f64,
    pin_net: impl Fn(usize) -> String,
) -> Footprint {
    let span = (per_side as f64 - 1.0) * pitch;
    let half = span / 2.0;
    let off = half + 1.3;
    let pl = 1.3;
    let pw = pitch * 0.55;
    let mut pads = Vec::new();
    let mut num = 0usize;
    for i in 0..per_side {
        pads.push(qfp_pad(
            num,
            -off,
            half - i as f64 * pitch,
            pl,
            pw,
            pin_net(num),
        ));
        num += 1;
    }
    for i in 0..per_side {
        pads.push(qfp_pad(
            num,
            -half + i as f64 * pitch,
            -off,
            pw,
            pl,
            pin_net(num),
        ));
        num += 1;
    }
    for i in 0..per_side {
        pads.push(qfp_pad(
            num,
            off,
            -half + i as f64 * pitch,
            pl,
            pw,
            pin_net(num),
        ));
        num += 1;
    }
    for i in 0..per_side {
        pads.push(qfp_pad(
            num,
            half - i as f64 * pitch,
            off,
            pw,
            pl,
            pin_net(num),
        ));
        num += 1;
    }
    let hw = off + 0.4;
    Footprint {
        reference: reference.to_string(),
        value: value.to_string(),
        footprint_name: format!("QFP-{}", per_side * 4),
        position: Vec2::new(cx, cy),
        rotation: 0.0,
        front: true,
        pads,
        graphics: vec![refdes(reference, 0.0, hw + 0.9), courtyard(hw, hw)],
        model_3d: None,
        properties: HashMap::new(),
    }
}

/// World position + outward unit direction of QFP pad `n` (matches `qfp`).
fn qfp_pad_world(cx: f64, cy: f64, per: usize, pitch: f64, n: usize) -> (Vec2, Vec2) {
    let half = (per as f64 - 1.0) * pitch / 2.0;
    let off = half + 1.3;
    let i = n % per;
    match n / per {
        0 => (
            Vec2::new(cx - off, cy + half - i as f64 * pitch),
            Vec2::new(-1.0, 0.0),
        ),
        1 => (
            Vec2::new(cx - half + i as f64 * pitch, cy - off),
            Vec2::new(0.0, -1.0),
        ),
        2 => (
            Vec2::new(cx + off, cy - half + i as f64 * pitch),
            Vec2::new(1.0, 0.0),
        ),
        _ => (
            Vec2::new(cx + half - i as f64 * pitch, cy + off),
            Vec2::new(0.0, 1.0),
        ),
    }
}

fn pour(net: &str, layer: PcbLayer, w: f64, h: f64, inset: f64) -> Zone {
    Zone {
        outline: vec![
            Vec2::new(inset, inset),
            Vec2::new(w - inset, inset),
            Vec2::new(w - inset, h - inset),
            Vec2::new(inset, h - inset),
        ],
        holes: vec![],
        net: net.to_string(),
        layer,
        clearance: 0.3,
        min_area: 0.2,
        fill_type: ZoneFillType::Solid,
        thermal_relief: ThermalReliefStyle::Relief,
        thermal_gap: Some(0.3),
        thermal_spoke_width: Some(0.3),
        priority: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn arc(net: &str, layer: PcbLayer, cx: f64, cy: f64, r: f64, a0: f64, a1: f64, w: f64) -> TraceArc {
    TraceArc {
        center: Vec2::new(cx, cy),
        radius: r,
        start_angle: a0,
        end_angle: a1,
        width: w,
        layer,
        net: net.to_string(),
    }
}

/// A dense "kitchen sink": fine-pitch QFP-48 fan-out, passive ring, a TH header,
/// F.Cu + B.Cu routing with vias, a B.Cu GND pour (clearance voids + thermal
/// relief), arc-routed traces, and a board cutout.
fn complex_board() -> Pcb {
    let w = 64.0;
    let h = 46.0;
    let cx = w * 0.5;
    let cy = h * 0.5;
    let per = 12usize;
    let pitch = 0.5;
    let qfp_net = |n: usize| -> String {
        if n.is_multiple_of(6) {
            "GND".to_string()
        } else if n.is_multiple_of(5) {
            "VCC".to_string()
        } else {
            format!("S{n}")
        }
    };

    let mut footprints = vec![qfp("U1", "STM32F030", cx, cy, per, pitch, qfp_net)];
    let mut traces: Vec<Trace> = Vec::new();
    let mut vias: Vec<Via> = Vec::new();

    // Fan-out: every pad gets an F.Cu stub; every 3rd drops to B.Cu via a via.
    for n in 0..(per * 4) {
        let (p, d) = qfp_pad_world(cx, cy, per, pitch, n);
        let len = 2.4 + (n % 4) as f64 * 1.1;
        let end = Vec2::new(p.x + d.x * len, p.y + d.y * len);
        let net = qfp_net(n);
        traces.push(Trace {
            start: p,
            end,
            width: 0.16,
            layer: PcbLayer::FCu,
            net: net.clone(),
        });
        if n.is_multiple_of(3) {
            vias.push(Via {
                position: end,
                diameter: 0.6,
                drill: 0.3,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: net.clone(),
            });
            let end2 = Vec2::new(end.x + d.x * 4.5, end.y + d.y * 4.5);
            traces.push(Trace {
                start: end,
                end: end2,
                width: 0.25,
                layer: PcbLayer::BCu,
                net,
            });
        }
    }

    // Passive ring (0603), pin1 → a signal, pin2 → GND (gives the pour thermals).
    let ring: [(f64, f64, f64); 10] = [
        (14.0, 40.0, 0.0),
        (24.0, 40.5, 0.0),
        (40.0, 40.5, 0.0),
        (50.0, 40.0, 0.0),
        (14.0, 6.0, 0.0),
        (24.0, 5.5, 0.0),
        (40.0, 5.5, 0.0),
        (50.0, 6.0, 0.0),
        (7.0, 23.0, 90.0),
        (57.0, 23.0, 90.0),
    ];
    for (k, (px, py, rot)) in ring.iter().enumerate() {
        let r = two_pad(
            &format!("R{}", k + 1),
            "4k7",
            *px,
            *py,
            *rot,
            &format!("S{}", k * 3 + 1),
            "GND",
        );
        footprints.push(r);
    }
    // A through-hole header (TH drill halos at the board edge).
    footprints.push(header(
        "J1",
        7.0,
        12.0,
        5,
        &["VCC", "GND", "S2", "S7", "S13"],
    ));

    // Arc-routed traces (curved copper).
    let trace_arcs = vec![
        arc("S2", PcbLayer::FCu, cx, 9.0, 15.0, 205.0, 335.0, 0.22),
        arc("VCC", PcbLayer::FCu, cx, h - 9.0, 17.0, 18.0, 162.0, 0.3),
    ];

    // B.Cu GND pour — carves clearance voids around non-GND B.Cu copper + vias.
    let zones = vec![pour("GND", PcbLayer::BCu, w, h, 1.6)];

    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ],
            // Mounting slot cutout near the top-right corner.
            cutouts: vec![vec![
                Vec2::new(w - 9.0, h - 6.5),
                Vec2::new(w - 3.5, h - 6.5),
                Vec2::new(w - 3.5, h - 3.5),
                Vec2::new(w - 9.0, h - 3.5),
            ]],
            thickness: 1.6,
        },
        stackup: LayerStackup { layers: vec![] },
        nets: vec![],
        rules: base_rules(),
        footprints,
        traces,
        trace_arcs,
        vias,
        zones,
        keepouts: vec![],
        net_ties: vec![],
    }
}

/// A 6-layer board exercising the neon inner-copper spread (In1Cu..In6Cu).
fn inner_layers_board() -> Pcb {
    let w = 40.0;
    let h = 28.0;
    let mut traces = Vec::new();
    let layers = [
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::In3Cu,
        PcbLayer::In4Cu,
        PcbLayer::In5Cu,
        PcbLayer::In6Cu,
    ];
    for (i, layer) in layers.iter().enumerate() {
        let y = 3.0 + i as f64 * 3.5;
        traces.push(Trace {
            start: Vec2::new(3.0, y),
            end: Vec2::new(w - 3.0, y),
            width: 0.6,
            layer: *layer,
            net: format!("L{i}"),
        });
    }
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup { layers: vec![] },
        nets: vec![],
        rules: base_rules(),
        footprints: vec![],
        traces,
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp".to_string());
    let scale = 12.0;

    let complex = complex_board();
    let layers_all = [
        PcbLayer::BCu,
        PcbLayer::FCu,
        PcbLayer::FCrtYd,
        PcbLayer::FSilkS,
        PcbLayer::EdgeCuts,
    ];
    let layers_top = [
        PcbLayer::FCu,
        PcbLayer::FCrtYd,
        PcbLayer::FSilkS,
        PcbLayer::EdgeCuts,
    ];

    let inner = [
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::In3Cu,
        PcbLayer::In4Cu,
        PcbLayer::In5Cu,
        PcbLayer::In6Cu,
        PcbLayer::EdgeCuts,
    ];

    // (name, pcb, layers, opts)
    let jobs: Vec<(&str, &Pcb, &[PcbLayer], PcbRenderOpts)> = vec![
        ("complex", &complex, &layers_all, PcbRenderOpts::default()),
        (
            "complex_top",
            &complex,
            &layers_top,
            PcbRenderOpts::default(),
        ),
        (
            "complex_hl",
            &complex,
            &layers_all,
            PcbRenderOpts {
                highlight: Highlight {
                    nets: vec!["GND".to_string()],
                    refs: vec![],
                },
                ..Default::default()
            },
        ),
        (
            "complex_labels",
            &complex,
            &layers_all,
            PcbRenderOpts {
                show_net_labels: true,
                ..Default::default()
            },
        ),
    ];
    for (name, pcb, layers, opts) in &jobs {
        let svg = render_pcb_svg_opts(pcb, layers, scale, opts);
        let path = format!("{dir}/pcb_{name}.svg");
        std::fs::write(&path, &svg).expect("write svg");
        eprintln!("wrote {} ({} bytes)", path, svg.len());
    }

    let inner_board = inner_layers_board();
    let svg = render_pcb_svg_opts(&inner_board, &inner, scale, &PcbRenderOpts::default());
    std::fs::write(format!("{dir}/pcb_inner.svg"), &svg).expect("write svg");
    eprintln!("wrote {dir}/pcb_inner.svg ({} bytes)", svg.len());

    let sample = sample_board();
    let light_svg = render_pcb_svg_opts(
        &sample,
        &layers_all,
        scale,
        &PcbRenderOpts {
            theme: Theme::Light,
            ..Default::default()
        },
    );
    std::fs::write(format!("{dir}/pcb_sample_light.svg"), &light_svg).expect("write svg");
    eprintln!(
        "wrote {dir}/pcb_sample_light.svg ({} bytes)",
        light_svg.len()
    );
}
