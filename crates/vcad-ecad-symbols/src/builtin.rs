//! Builtin symbol and footprint library.
//!
//! Hardcoded symbol definitions for schematic component placement, plus
//! parametric footprint generators based on IPC-7351B standards.

use vcad_ir::ecad::*;
use vcad_ir::Vec2;

// ============================================================================
// Pad helpers
// ============================================================================

fn smd_pad(num: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: w,
            height: h,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: None,
        layers: vec![PcbLayer::FCu],
    }
}

fn tht_pad(num: &str, x: f64, y: f64, pad_dia: f64, drill_dia: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::THT,
        shape: PadShape::Circle { diameter: pad_dia },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: Some(DrillSpec {
            diameter: drill_dia,
            oval: false,
            oval_height: None,
        }),
        net: None,
        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
    }
}

fn silk_line(x1: f64, y1: f64, x2: f64, y2: f64) -> FootprintGraphic {
    FootprintGraphic::Line {
        start: Vec2::new(x1, y1),
        end: Vec2::new(x2, y2),
        width: 0.12,
        layer: PcbLayer::FSilkS,
    }
}

fn silk_rect(x1: f64, y1: f64, x2: f64, y2: f64) -> Vec<FootprintGraphic> {
    vec![
        silk_line(x1, y1, x2, y1),
        silk_line(x2, y1, x2, y2),
        silk_line(x2, y2, x1, y2),
        silk_line(x1, y2, x1, y1),
    ]
}

fn pin1_dot(x: f64, y: f64, r: f64) -> FootprintGraphic {
    FootprintGraphic::Circle {
        center: Vec2::new(x, y),
        radius: r,
        width: 0.12,
        layer: PcbLayer::FSilkS,
    }
}

// ============================================================================
// Footprint generators
// ============================================================================

/// Chip size identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipSize {
    /// 0402 (1.0 x 0.5 mm)
    C0402,
    /// 0603 (1.6 x 0.8 mm)
    C0603,
    /// 0805 (2.0 x 1.25 mm)
    C0805,
    /// 1206 (3.2 x 1.6 mm)
    C1206,
}

impl ChipSize {
    fn name(self) -> &'static str {
        match self {
            Self::C0402 => "0402",
            Self::C0603 => "0603",
            Self::C0805 => "0805",
            Self::C1206 => "1206",
        }
    }

    fn params(self) -> (f64, f64, f64, f64) {
        // (padW, padH, gap, silkY)
        match self {
            Self::C0402 => (0.5, 0.5, 0.5, 0.35),
            Self::C0603 => (0.8, 0.9, 0.8, 0.55),
            Self::C0805 => (1.0, 1.2, 1.0, 0.7),
            Self::C1206 => (1.6, 1.8, 1.0, 1.0),
        }
    }
}

/// Generate a chip (0402/0603/0805/1206) footprint.
pub fn fp_chip(size: ChipSize) -> FootprintTemplate {
    let (pad_w, pad_h, gap, silk_y) = size.params();
    let cx = (gap + pad_w) / 2.0;
    FootprintTemplate {
        name: size.name().to_string(),
        pads: vec![
            smd_pad("1", -cx, 0.0, pad_w, pad_h),
            smd_pad("2", cx, 0.0, pad_w, pad_h),
        ],
        graphics: vec![
            silk_line(-gap / 2.0, -silk_y, gap / 2.0, -silk_y),
            silk_line(-gap / 2.0, silk_y, gap / 2.0, silk_y),
        ],
    }
}

/// Generate a SOIC (Small Outline IC) footprint.
pub fn fp_soic(pins: u32) -> FootprintTemplate {
    let pitch = 1.27;
    let body_w = 3.9;
    let pad_w = 0.6;
    let pad_h = 2.2;
    let row_x = 2.7;
    let half = pins / 2;
    let mut pads = Vec::new();

    for i in 0..half {
        let y = (i as f64 - (half - 1) as f64 / 2.0) * pitch;
        pads.push(smd_pad(&(i + 1).to_string(), -row_x, y, pad_h, pad_w));
        pads.push(smd_pad(&(pins - i).to_string(), row_x, y, pad_h, pad_w));
    }

    let half_len = (half as f64 * pitch) / 2.0;
    let mut graphics = silk_rect(-body_w / 2.0, -half_len, body_w / 2.0, half_len);
    graphics.push(pin1_dot(-body_w / 2.0 + 0.5, -half_len + 0.5, 0.25));

    FootprintTemplate {
        name: format!("SOIC-{pins}"),
        pads,
        graphics,
    }
}

/// Generate a QFP (Quad Flat Package) footprint.
pub fn fp_qfp(pins: u32, pitch: f64) -> FootprintTemplate {
    let pins_per_side = pins / 4;
    let body_size: f64 = match pins {
        32 => 7.0,
        48 => 9.0,
        _ => 12.0,
    };
    let pad_w = 0.4;
    let pad_h = 1.5;
    let row_offset = body_size / 2.0 + pad_h / 2.0 - 0.3;
    let mut pads = Vec::new();
    let mut num = 1u32;

    // Bottom side (left to right)
    for i in 0..pins_per_side {
        let x = (i as f64 - (pins_per_side - 1) as f64 / 2.0) * pitch;
        pads.push(smd_pad(&num.to_string(), x, row_offset, pad_w, pad_h));
        num += 1;
    }
    // Right side (bottom to top)
    for i in 0..pins_per_side {
        let y = ((pins_per_side - 1) as f64 / 2.0 - i as f64) * pitch;
        pads.push(smd_pad(&num.to_string(), row_offset, y, pad_h, pad_w));
        num += 1;
    }
    // Top side (right to left)
    for i in 0..pins_per_side {
        let x = ((pins_per_side - 1) as f64 / 2.0 - i as f64) * pitch;
        pads.push(smd_pad(&num.to_string(), x, -row_offset, pad_w, pad_h));
        num += 1;
    }
    // Left side (top to bottom)
    for i in 0..pins_per_side {
        let y = (i as f64 - (pins_per_side - 1) as f64 / 2.0) * pitch;
        pads.push(smd_pad(&num.to_string(), -row_offset, y, pad_h, pad_w));
        num += 1;
    }

    let hs = body_size / 2.0;
    let mut graphics = silk_rect(-hs, -hs, hs, hs);
    graphics.push(pin1_dot(-hs + 0.8, hs - 0.8, 0.3));

    FootprintTemplate {
        name: format!("QFP-{pins}"),
        pads,
        graphics,
    }
}

/// Generate a DIP (Dual In-Line Package) footprint.
pub fn fp_dip(pins: u32) -> FootprintTemplate {
    let pitch = 2.54;
    let row_spacing = 7.62;
    let pad_dia = 1.6;
    let drill_dia = 0.8;
    let half = pins / 2;
    let mut pads = Vec::new();

    for i in 0..half {
        let y = (i as f64 - (half - 1) as f64 / 2.0) * pitch;
        pads.push(tht_pad(
            &(i + 1).to_string(),
            -row_spacing / 2.0,
            y,
            pad_dia,
            drill_dia,
        ));
        pads.push(tht_pad(
            &(pins - i).to_string(),
            row_spacing / 2.0,
            y,
            pad_dia,
            drill_dia,
        ));
    }

    let half_len = (half as f64 * pitch) / 2.0 + 0.5;
    let half_w = row_spacing / 2.0 + 1.5;
    let mut graphics = silk_rect(-half_w, -half_len, half_w, half_len);
    graphics.push(pin1_dot(-row_spacing / 2.0, -half_len + 1.0, 0.5));

    FootprintTemplate {
        name: format!("DIP-{pins}"),
        pads,
        graphics,
    }
}

/// Generate a SOT-23 (3-pin) footprint.
pub fn fp_sot23() -> FootprintTemplate {
    FootprintTemplate {
        name: "SOT-23".to_string(),
        pads: vec![
            smd_pad("1", -0.95, 1.1, 0.6, 0.7),
            smd_pad("2", 0.95, 1.1, 0.6, 0.7),
            smd_pad("3", 0.0, -1.1, 0.6, 0.7),
        ],
        graphics: vec![
            silk_line(-1.3, -0.6, 1.3, -0.6),
            silk_line(-1.3, 0.6, -1.3, -0.6),
            silk_line(1.3, 0.6, 1.3, -0.6),
        ],
    }
}

/// Generate a SOT-223 (4-pin with thermal tab) footprint.
pub fn fp_sot223() -> FootprintTemplate {
    let pitch = 2.3;
    let graphics = silk_rect(-3.3, -2.3, 3.3, 2.3);
    FootprintTemplate {
        name: "SOT-223".to_string(),
        pads: vec![
            smd_pad("1", -pitch, 3.15, 0.7, 1.5),
            smd_pad("2", 0.0, 3.15, 0.7, 1.5),
            smd_pad("3", pitch, 3.15, 0.7, 1.5),
            smd_pad("4", 0.0, -3.15, 3.5, 1.5),
        ],
        graphics,
    }
}

/// Generate a pin header footprint.
pub fn fp_pin_header(rows: u32, cols: u32) -> FootprintTemplate {
    let pitch = 2.54;
    let pad_dia = 2.5;
    let drill_dia = 1.0;
    let mut pads = Vec::new();
    let mut num = 1u32;

    for c in 0..cols {
        for r in 0..rows {
            let x = if rows == 1 {
                0.0
            } else {
                (r as f64 - 0.5) * pitch
            };
            let y = (c as f64 - (cols - 1) as f64 / 2.0) * pitch;
            pads.push(tht_pad(&num.to_string(), x, y, pad_dia, drill_dia));
            num += 1;
        }
    }

    let half_w = if rows == 1 { 1.5 } else { pitch };
    let half_h = ((cols - 1) as f64 * pitch) / 2.0 + 1.5;
    let graphics = silk_rect(-half_w, -half_h, half_w, half_h);

    FootprintTemplate {
        name: format!("PinHeader_{rows}x{cols}"),
        pads,
        graphics,
    }
}

// ============================================================================
// Footprint name resolution
// ============================================================================

/// Resolve a KiCad-style footprint name to a parametric footprint template.
///
/// Thin wrapper over [`crate::footprint::resolve_footprint`], which parses the
/// package family, pin count, pitch, and body size out of the id and
/// synthesizes IPC-7351-style pads (QFN/DFN, QFP, SOIC/SSOP/TSSOP, SOT, DPAK,
/// SOD, chips, DIP, headers, screw terminals, electrolytics, ...). Returns
/// `None` only when the name is unrecognized and `pin_count` is zero. Call
/// `resolve_footprint` directly when you also need to know whether the id was a
/// real family match or a generic placeholder.
pub fn footprint_for_name(name: &str, pin_count: u32) -> Option<FootprintTemplate> {
    crate::footprint::resolve_footprint(name, pin_count).template
}

// ============================================================================
// Symbol definitions
// ============================================================================

fn pin(num: &str, name: &str, pin_type: PinType, x: f64, y: f64) -> SchematicPin {
    SchematicPin {
        number: num.to_string(),
        name: name.to_string(),
        pin_type,
        position: Vec2::new(x, y),
    }
}

fn sym_rect(x: f64, y: f64, w: f64, h: f64) -> SymbolGraphic {
    SymbolGraphic::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn sym_line(x1: f64, y1: f64, x2: f64, y2: f64) -> SymbolGraphic {
    SymbolGraphic::Line { x1, y1, x2, y2 }
}

fn sym_circle(cx: f64, cy: f64, r: f64) -> SymbolGraphic {
    SymbolGraphic::Circle { cx, cy, r }
}

fn sym_polyline(pts: &[(f64, f64)]) -> SymbolGraphic {
    SymbolGraphic::Polyline {
        points: pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect(),
    }
}

/// Return all builtin symbol definitions.
pub fn builtin_symbols() -> Vec<SymbolDef> {
    vec![
        // Resistor (0805 default)
        SymbolDef {
            id: "resistor".into(),
            name: "Resistor".into(),
            prefix: "R".into(),
            default_value: "10k".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 15.0),
                pin("2", "2", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_rect(5.0, 5.0, 30.0, 20.0),
                sym_line(-8.0, 15.0, 5.0, 15.0),
                sym_line(35.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C0805)),
        },
        // Capacitor (0805 default)
        SymbolDef {
            id: "capacitor".into(),
            name: "Capacitor".into(),
            prefix: "C".into(),
            default_value: "100nF".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 15.0),
                pin("2", "2", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_line(-8.0, 15.0, 16.0, 15.0),
                sym_line(16.0, 3.0, 16.0, 27.0),
                sym_line(24.0, 3.0, 24.0, 27.0),
                sym_line(24.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C0805)),
        },
        // LED
        SymbolDef {
            id: "led".into(),
            name: "LED".into(),
            prefix: "D".into(),
            default_value: "Red".into(),
            pins: vec![
                pin("A", "A", PinType::Passive, -8.0, 15.0),
                pin("K", "K", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_line(-8.0, 15.0, 15.0, 15.0),
                sym_polyline(&[(15.0, 5.0), (15.0, 25.0), (30.0, 15.0), (15.0, 5.0)]),
                sym_line(30.0, 5.0, 30.0, 25.0),
                sym_line(30.0, 15.0, 48.0, 15.0),
                sym_line(25.0, 3.0, 30.0, 0.0),
                sym_line(28.0, 5.0, 33.0, 2.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C0805)),
        },
        // Diode
        SymbolDef {
            id: "diode".into(),
            name: "Diode".into(),
            prefix: "D".into(),
            default_value: "1N4148".into(),
            pins: vec![
                pin("A", "A", PinType::Passive, -8.0, 15.0),
                pin("K", "K", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_line(-8.0, 15.0, 15.0, 15.0),
                sym_polyline(&[(15.0, 5.0), (15.0, 25.0), (30.0, 15.0), (15.0, 5.0)]),
                sym_line(30.0, 5.0, 30.0, 25.0),
                sym_line(30.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(FootprintTemplate {
                name: "SOD-323".into(),
                pads: vec![
                    smd_pad("A", -1.15, 0.0, 0.9, 0.6),
                    smd_pad("K", 1.15, 0.0, 0.9, 0.6),
                ],
                graphics: vec![
                    silk_line(-0.5, -0.5, 0.5, -0.5),
                    silk_line(-0.5, 0.5, 0.5, 0.5),
                    silk_line(0.3, -0.5, 0.3, 0.5),
                ],
            }),
        },
        // DC Motor (electromechanical — spins under simulation)
        SymbolDef {
            id: "motor".into(),
            name: "Motor".into(),
            prefix: "M".into(),
            default_value: "DC".into(),
            pins: vec![
                pin("1", "+", PinType::Passive, -8.0, 15.0),
                pin("2", "-", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_circle(20.0, 15.0, 14.0),
                sym_line(-8.0, 15.0, 6.0, 15.0),
                sym_line(34.0, 15.0, 48.0, 15.0),
                // an "M" inside the circle
                sym_line(14.0, 22.0, 14.0, 8.0),
                sym_line(14.0, 8.0, 20.0, 15.0),
                sym_line(20.0, 15.0, 26.0, 8.0),
                sym_line(26.0, 8.0, 26.0, 22.0),
            ],
            footprint_template: None,
        },
        // NPN Transistor
        SymbolDef {
            id: "npn".into(),
            name: "NPN Transistor".into(),
            prefix: "Q".into(),
            default_value: "2N2222".into(),
            pins: vec![
                pin("B", "B", PinType::Input, -8.0, 15.0),
                pin("C", "C", PinType::Output, 35.0, 0.0),
                pin("E", "E", PinType::Output, 35.0, 30.0),
            ],
            graphics: vec![
                sym_line(-8.0, 15.0, 10.0, 15.0),
                sym_line(10.0, 5.0, 10.0, 25.0),
                sym_line(10.0, 10.0, 35.0, 0.0),
                sym_line(10.0, 20.0, 35.0, 30.0),
                sym_circle(18.0, 15.0, 14.0),
            ],
            footprint_template: Some(fp_sot23()),
        },
        // IC Header 8-pin
        SymbolDef {
            id: "ic8".into(),
            name: "IC Header 8-pin".into(),
            prefix: "U".into(),
            default_value: "IC".into(),
            pins: vec![
                pin("1", "1", PinType::Bidirectional, -8.0, 10.0),
                pin("2", "2", PinType::Bidirectional, -8.0, 24.0),
                pin("3", "3", PinType::Bidirectional, -8.0, 38.0),
                pin("4", "4", PinType::Bidirectional, -8.0, 52.0),
                pin("5", "5", PinType::Bidirectional, 48.0, 52.0),
                pin("6", "6", PinType::Bidirectional, 48.0, 38.0),
                pin("7", "7", PinType::Bidirectional, 48.0, 24.0),
                pin("8", "8", PinType::Bidirectional, 48.0, 10.0),
            ],
            graphics: vec![sym_rect(0.0, 0.0, 40.0, 62.0), sym_circle(6.0, 4.0, 2.0)],
            footprint_template: Some(fp_dip(8)),
        },
        // VCC Power Symbol
        SymbolDef {
            id: "vcc".into(),
            name: "VCC".into(),
            prefix: "PWR".into(),
            default_value: "VCC".into(),
            pins: vec![pin("1", "1", PinType::PowerOutput, 20.0, 30.0)],
            graphics: vec![
                sym_line(20.0, 30.0, 20.0, 10.0),
                sym_polyline(&[(10.0, 10.0), (20.0, 0.0), (30.0, 10.0)]),
            ],
            footprint_template: None,
        },
        // GND Power Symbol
        SymbolDef {
            id: "gnd".into(),
            name: "GND".into(),
            prefix: "PWR".into(),
            default_value: "GND".into(),
            pins: vec![pin("1", "1", PinType::PowerInput, 20.0, 0.0)],
            graphics: vec![
                sym_line(20.0, 0.0, 20.0, 15.0),
                sym_line(8.0, 15.0, 32.0, 15.0),
                sym_line(12.0, 20.0, 28.0, 20.0),
                sym_line(16.0, 25.0, 24.0, 25.0),
            ],
            footprint_template: None,
        },
        // Voltage Regulator (LDO)
        SymbolDef {
            id: "ldo".into(),
            name: "Voltage Regulator (LDO)".into(),
            prefix: "U".into(),
            default_value: "AMS1117-3.3".into(),
            pins: vec![
                pin("1", "IN", PinType::PowerInput, -8.0, 15.0),
                pin("2", "GND", PinType::PowerInput, 20.0, 40.0),
                pin("3", "OUT", PinType::PowerOutput, 48.0, 15.0),
            ],
            graphics: vec![
                sym_rect(0.0, 0.0, 40.0, 30.0),
                sym_line(-8.0, 15.0, 0.0, 15.0),
                sym_line(40.0, 15.0, 48.0, 15.0),
                sym_line(20.0, 30.0, 20.0, 40.0),
            ],
            footprint_template: Some(fp_sot223()),
        },
        // Op-Amp
        SymbolDef {
            id: "opamp".into(),
            name: "Op-Amp".into(),
            prefix: "U".into(),
            default_value: "LM358".into(),
            pins: vec![
                pin("2", "IN+", PinType::Input, -8.0, 10.0),
                pin("3", "IN-", PinType::Input, -8.0, 24.0),
                pin("1", "OUT", PinType::Output, 48.0, 17.0),
                pin("4", "VCC", PinType::PowerInput, 20.0, -5.0),
                pin("8", "GND", PinType::PowerInput, 20.0, 40.0),
            ],
            graphics: vec![
                sym_polyline(&[(5.0, 0.0), (5.0, 34.0), (38.0, 17.0), (5.0, 0.0)]),
                sym_line(-8.0, 10.0, 5.0, 10.0),
                sym_line(-8.0, 24.0, 5.0, 24.0),
                sym_line(38.0, 17.0, 48.0, 17.0),
            ],
            footprint_template: Some(fp_soic(8)),
        },
        // Microcontroller 32-pin
        SymbolDef {
            id: "mcu32".into(),
            name: "Microcontroller 32-pin".into(),
            prefix: "U".into(),
            default_value: "STM32F0".into(),
            pins: {
                let mut pins = Vec::new();
                for i in 0..8 {
                    pins.push(pin(
                        &(i + 1).to_string(),
                        &format!("P{}", i + 1),
                        PinType::Bidirectional,
                        -8.0,
                        8.0 + i as f64 * 8.0,
                    ));
                    pins.push(pin(
                        &(24 - i).to_string(),
                        &format!("P{}", 24 - i),
                        PinType::Bidirectional,
                        58.0,
                        8.0 + i as f64 * 8.0,
                    ));
                }
                for i in 0..8 {
                    pins.push(pin(
                        &(9 + i).to_string(),
                        &format!("P{}", 9 + i),
                        PinType::Bidirectional,
                        12.0 + i as f64 * 5.0,
                        75.0,
                    ));
                    pins.push(pin(
                        &(32 - i).to_string(),
                        &format!("P{}", 32 - i),
                        PinType::Bidirectional,
                        12.0 + i as f64 * 5.0,
                        -5.0,
                    ));
                }
                pins
            },
            graphics: vec![sym_rect(0.0, 0.0, 50.0, 70.0), sym_circle(6.0, 4.0, 2.0)],
            footprint_template: Some(fp_qfp(32, 0.8)),
        },
        // Resistor 0402
        SymbolDef {
            id: "resistor_0402".into(),
            name: "Resistor (0402)".into(),
            prefix: "R".into(),
            default_value: "10k".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 15.0),
                pin("2", "2", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_rect(5.0, 5.0, 30.0, 20.0),
                sym_line(-8.0, 15.0, 5.0, 15.0),
                sym_line(35.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C0402)),
        },
        // Capacitor 0603
        SymbolDef {
            id: "capacitor_0603".into(),
            name: "Capacitor (0603)".into(),
            prefix: "C".into(),
            default_value: "100nF".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 15.0),
                pin("2", "2", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_line(-8.0, 15.0, 16.0, 15.0),
                sym_line(16.0, 3.0, 16.0, 27.0),
                sym_line(24.0, 3.0, 24.0, 27.0),
                sym_line(24.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C0603)),
        },
        // Resistor 1206
        SymbolDef {
            id: "resistor_1206".into(),
            name: "Resistor (1206)".into(),
            prefix: "R".into(),
            default_value: "10k".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 15.0),
                pin("2", "2", PinType::Passive, 48.0, 15.0),
            ],
            graphics: vec![
                sym_rect(5.0, 5.0, 30.0, 20.0),
                sym_line(-8.0, 15.0, 5.0, 15.0),
                sym_line(35.0, 15.0, 48.0, 15.0),
            ],
            footprint_template: Some(fp_chip(ChipSize::C1206)),
        },
        // Pin Header 1x4
        SymbolDef {
            id: "pinheader_1x4".into(),
            name: "Pin Header 1x4".into(),
            prefix: "J".into(),
            default_value: "1x4".into(),
            pins: vec![
                pin("1", "1", PinType::Passive, -8.0, 8.0),
                pin("2", "2", PinType::Passive, -8.0, 22.0),
                pin("3", "3", PinType::Passive, -8.0, 36.0),
                pin("4", "4", PinType::Passive, -8.0, 50.0),
            ],
            graphics: vec![
                sym_rect(0.0, 0.0, 20.0, 58.0),
                sym_line(-8.0, 8.0, 0.0, 8.0),
                sym_line(-8.0, 22.0, 0.0, 22.0),
                sym_line(-8.0, 36.0, 0.0, 36.0),
                sym_line(-8.0, 50.0, 0.0, 50.0),
            ],
            footprint_template: Some(fp_pin_header(1, 4)),
        },
        // Pin Header 2x5
        SymbolDef {
            id: "pinheader_2x5".into(),
            name: "Pin Header 2x5".into(),
            prefix: "J".into(),
            default_value: "2x5".into(),
            pins: {
                let mut pins = Vec::new();
                for i in 0..5 {
                    pins.push(pin(
                        &(i * 2 + 1).to_string(),
                        &(i * 2 + 1).to_string(),
                        PinType::Passive,
                        -8.0,
                        8.0 + i as f64 * 14.0,
                    ));
                    pins.push(pin(
                        &(i * 2 + 2).to_string(),
                        &(i * 2 + 2).to_string(),
                        PinType::Passive,
                        48.0,
                        8.0 + i as f64 * 14.0,
                    ));
                }
                pins
            },
            graphics: vec![sym_rect(0.0, 0.0, 40.0, 66.0)],
            footprint_template: Some(fp_pin_header(2, 5)),
        },
    ]
}

/// Look up a symbol by ID.
pub fn get_symbol(id: &str) -> Option<SymbolDef> {
    builtin_symbols().into_iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_symbols_count() {
        let symbols = builtin_symbols();
        assert_eq!(symbols.len(), 17); // + Motor
    }

    #[test]
    fn motor_symbol_present() {
        let m = get_symbol("motor").expect("motor symbol");
        assert_eq!(m.prefix, "M");
        assert_eq!(m.pins.len(), 2);
    }

    #[test]
    fn get_symbol_found() {
        assert!(get_symbol("resistor").is_some());
        assert!(get_symbol("capacitor").is_some());
        assert!(get_symbol("npn").is_some());
        assert!(get_symbol("mcu32").is_some());
    }

    #[test]
    fn get_symbol_not_found() {
        assert!(get_symbol("nonexistent").is_none());
    }

    #[test]
    fn chip_footprint_pads() {
        let fp = fp_chip(ChipSize::C0805);
        assert_eq!(fp.name, "0805");
        assert_eq!(fp.pads.len(), 2);
        assert_eq!(fp.graphics.len(), 2);
    }

    #[test]
    fn soic_footprint_pads() {
        let fp = fp_soic(8);
        assert_eq!(fp.name, "SOIC-8");
        assert_eq!(fp.pads.len(), 8);
    }

    #[test]
    fn qfp_footprint_pads() {
        let fp = fp_qfp(32, 0.8);
        assert_eq!(fp.name, "QFP-32");
        assert_eq!(fp.pads.len(), 32);
    }

    #[test]
    fn dip_footprint_pads() {
        let fp = fp_dip(8);
        assert_eq!(fp.name, "DIP-8");
        assert_eq!(fp.pads.len(), 8);
    }

    #[test]
    fn pin_header_footprint() {
        let fp = fp_pin_header(2, 5);
        assert_eq!(fp.name, "PinHeader_2x5");
        assert_eq!(fp.pads.len(), 10);
    }

    #[test]
    fn symbols_roundtrip_json() {
        let symbols = builtin_symbols();
        let json = serde_json::to_string(&symbols).unwrap();
        let restored: Vec<SymbolDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(symbols.len(), restored.len());
        for (orig, rest) in symbols.iter().zip(restored.iter()) {
            assert_eq!(orig.id, rest.id);
            assert_eq!(orig.pins.len(), rest.pins.len());
        }
    }

    #[test]
    fn power_symbols_have_no_footprint() {
        let vcc = get_symbol("vcc").unwrap();
        assert!(vcc.footprint_template.is_none());
        let gnd = get_symbol("gnd").unwrap();
        assert!(gnd.footprint_template.is_none());
    }

    #[test]
    fn footprint_for_name_resolves_kicad_names() {
        let soic = footprint_for_name("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm", 8).unwrap();
        assert_eq!(soic.name, "SOIC-8");
        assert_eq!(soic.pads.len(), 8);
        // Pads must not stack: all positions unique.
        let mut positions: Vec<(i64, i64)> = soic
            .pads
            .iter()
            .map(|p| {
                (
                    (p.position.x * 1000.0) as i64,
                    (p.position.y * 1000.0) as i64,
                )
            })
            .collect();
        positions.sort();
        positions.dedup();
        assert_eq!(positions.len(), 8);

        let chip = footprint_for_name("Resistor_SMD:R_0805_2012Metric", 2).unwrap();
        assert_eq!(chip.name, "0805");

        let hdr = footprint_for_name(
            "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
            2,
        )
        .unwrap();
        assert_eq!(hdr.name, "PinHeader_1x2");
        assert_eq!(hdr.pads.len(), 2);

        let dip = footprint_for_name("Package_DIP:DIP-14_W7.62mm", 14).unwrap();
        assert_eq!(dip.name, "DIP-14");

        let qfp = footprint_for_name("Package_QFP:LQFP-32_7x7mm_P0.8mm", 32).unwrap();
        assert_eq!(qfp.name, "QFP-32");

        let sot = footprint_for_name("Package_TO_SOT_SMD:SOT-23", 3).unwrap();
        assert_eq!(sot.name, "SOT-23");

        let sot223 = footprint_for_name("Package_TO_SOT_SMD:SOT-223-3_TabPin2", 4).unwrap();
        assert_eq!(sot223.name, "SOT-223");
    }

    #[test]
    fn footprint_for_name_fallbacks_spread_pads() {
        // Unknown 2-pin part → chip.
        let two = footprint_for_name("Mystery:Part", 2).unwrap();
        assert_eq!(two.pads.len(), 2);
        // Unknown 5-pin part → single-row header, one pad per pin.
        let five = footprint_for_name("Mystery:Part5", 5).unwrap();
        assert_eq!(five.pads.len(), 5);
        // Nothing to go on.
        assert!(footprint_for_name("Mystery:Unknowable", 0).is_none());
    }
}
