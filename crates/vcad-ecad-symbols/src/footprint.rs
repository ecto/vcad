//! Parametric footprint engine.
//!
//! Resolves a KiCad-style footprint identifier (e.g.
//! `"Package_DFN_QFN:QFN-40_5x5mm_P0.4mm"`) into a real land pattern by
//! parsing the *package family*, *pin count*, *pitch*, and *body size* out of
//! the id and synthesizing IPC-7351-style pads — instead of the previous
//! marker-table that silently fell back to a single 2.54mm column of
//! through-hole pads (which ran off the board for anything it didn't know).
//!
//! The public entry point is [`resolve_footprint`], which returns a
//! [`FootprintResolution`] carrying *both* the generated template and whether
//! it was a real family match or a generic placeholder — so callers can warn
//! loudly instead of discovering off-board pads three steps later in DRC.
//!
//! Supported families: chip passives (0201–2512), SOT-23/-5/-6/-223 and
//! SC-70/SOT-353/-363, SOIC/SO/SOP/SSOP/TSSOP/MSOP/VSSOP, QFP/LQFP/TQFP/PQFP,
//! QFN/DFN/SON (with thermal pad), HTSSOP/TSSOP-EP (PowerPad, with thermal
//! pad), DPAK/D2PAK (TO-252/TO-263), SOD/DO-214 (SMA/SMB/SMC) two-terminal
//! SMD, DIP, SMD crystals (2-pad 5032/6035/7050, 4-pad 3225/2520/2016/1612),
//! pin headers/sockets, screw terminals, radial electrolytic caps, and
//! wire-to-board / programming / USB connectors: JST PH/XH/EH (THT) and SH/GH
//! (SMD), Molex Pico-Blade (SMD), Tag-Connect TC2030/TC2050 spring-pin pads,
//! a USB micro-B receptacle, and a simplified USB-C receptacle. Anything
//! else falls back to a *compact grid* of pads sized to stay on the board,
//! flagged `matched: false`.

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

// ============================================================================
// Resolution result
// ============================================================================

/// Outcome of resolving a footprint identifier.
///
/// Distinguishes a real package-family land pattern from a generic pin-count
/// placeholder, so a caller (e.g. `place_components`) can surface unresolved
/// footprints to the user rather than silently substituting wrong geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintResolution {
    /// The generated land pattern. `None` only when the id is unrecognized
    /// *and* `pin_count` is zero (nothing to synthesize from).
    pub template: Option<FootprintTemplate>,
    /// `true` when a real package-family land pattern was generated; `false`
    /// when a generic placeholder was substituted (the caller should warn).
    pub matched: bool,
    /// Recognized package family (e.g. `"QFN"`, `"SOIC"`, `"Chip"`, `"DPAK"`),
    /// or `None` for the generic fallback.
    pub family: Option<String>,
    /// Human-readable explanation of what was generated or why it fell back.
    pub note: String,
}

// ============================================================================
// Pad / graphic helpers
// ============================================================================

fn rect_smd(num: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
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
        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
    }
}

fn circle_tht(num: &str, x: f64, y: f64, pad_dia: f64, drill_dia: f64) -> Pad {
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
        layers: vec![
            PcbLayer::FCu,
            PcbLayer::BCu,
            PcbLayer::FMask,
            PcbLayer::BMask,
        ],
    }
}

/// Rectangular through-hole pad (a drilled hole inside a square/rect copper
/// land). Used to square off pin 1 of a connector for polarity, the classic
/// "pin 1 is the only non-round pad" convention.
fn rect_tht(num: &str, x: f64, y: f64, w: f64, h: f64, drill_dia: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::THT,
        shape: PadShape::Rect {
            width: w,
            height: h,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: Some(DrillSpec {
            diameter: drill_dia,
            oval: false,
            oval_height: None,
        }),
        net: None,
        layers: vec![
            PcbLayer::FCu,
            PcbLayer::BCu,
            PcbLayer::FMask,
            PcbLayer::BMask,
        ],
    }
}

/// Finished plated-through-hole drill for a round lead of nominal diameter
/// `lead_dia` (mm).
///
/// IPC-2222 Class-2 wants the finished hole ≈ `lead + 0.2 mm` (a ~0.1 mm radial
/// gap), floored at a fab-practical 0.3 mm. Sizing the drill off the *lead*
/// (not the pad pitch) is what keeps drilled holes from ballooning past the
/// hole-to-hole rule on standard parts — the old `pitch * 0.4` heuristic emitted
/// oversized holes (e.g. 1.0 mm on a 0.5 mm JST lead) that produced phantom
/// hole-to-hole DRC violations when the auto-placer packed parts together.
fn tht_drill(lead_dia: f64) -> f64 {
    (lead_dia + 0.2).max(0.3)
}

/// Copper land diameter for a through-hole pad: its drill plus an annular ring
/// of `ring` mm per side. A `ring >= 0.2` keeps the pad clear of the
/// min-annular-ring DRC by construction.
fn tht_pad(drill: f64, ring: f64) -> f64 {
    drill + 2.0 * ring
}

/// Bare-copper round SMD test pad: front copper + mask opening but **no paste**
/// — a spring-pin (pogo) target that should never get a solder-stencil aperture.
fn smd_test_pad(num: &str, x: f64, y: f64, dia: f64) -> Pad {
    Pad {
        number: num.to_string(),
        pad_type: PadType::SMD,
        shape: PadShape::Circle { diameter: dia },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: None,
        layers: vec![PcbLayer::FCu, PcbLayer::FMask],
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

fn pin1_dot(x: f64, y: f64) -> FootprintGraphic {
    FootprintGraphic::Circle {
        center: Vec2::new(x, y),
        radius: 0.25,
        width: 0.12,
        layer: PcbLayer::FSilkS,
    }
}

fn courtyard(hx: f64, hy: f64) -> FootprintGraphic {
    FootprintGraphic::Rect {
        start: Vec2::new(-hx, -hy),
        end: Vec2::new(hx, hy),
        width: 0.05,
        layer: PcbLayer::FCrtYd,
    }
}

/// Body silk rectangle + pin-1 dot (top-left) + courtyard, for IC packages.
fn ic_graphics(body_w: f64, body_h: f64) -> Vec<FootprintGraphic> {
    let mut g = silk_rect(-body_w / 2.0, -body_h / 2.0, body_w / 2.0, body_h / 2.0);
    g.push(pin1_dot(-body_w / 2.0 - 0.4, -body_h / 2.0 - 0.4));
    g.push(courtyard(body_w / 2.0 + 0.25, body_h / 2.0 + 0.25));
    g
}

// ============================================================================
// Chip passives
// ============================================================================

/// (pad_w, pad_h, gap, silk_y) per imperial chip code. `gap` is inner edge to
/// inner edge; pads sit at ±(gap + pad_w)/2.
fn chip_params(code: &str) -> Option<(f64, f64, f64, f64)> {
    Some(match code {
        "0201" => (0.46, 0.42, 0.28, 0.3),
        "0402" => (0.5, 0.5, 0.5, 0.35),
        "0603" => (0.8, 0.9, 0.8, 0.55),
        "0805" => (1.0, 1.2, 1.0, 0.7),
        "1206" => (1.6, 1.8, 1.0, 1.0),
        "1210" => (1.6, 2.7, 1.0, 1.45),
        "2010" => (1.4, 2.7, 2.0, 1.45),
        "2512" => (1.6, 3.4, 2.4, 1.8),
        // 1008 (2520 metric) inductors: IPC-7351 nominal, KiCad 9
        // `L_1008_2520Metric` (1.25 × 2.2 lands at ±1.075).
        "1008" => (1.25, 2.2, 0.9, 1.25),
        _ => return None,
    })
}

fn chip(code: &str) -> FootprintTemplate {
    let (pad_w, pad_h, gap, silk_y) = chip_params(code).unwrap_or((1.0, 1.2, 1.0, 0.7));
    let cx = (gap + pad_w) / 2.0;
    FootprintTemplate {
        name: code.to_string(),
        pads: vec![
            rect_smd("1", -cx, 0.0, pad_w, pad_h),
            rect_smd("2", cx, 0.0, pad_w, pad_h),
        ],
        graphics: vec![
            silk_line(-gap / 2.0, -silk_y, gap / 2.0, -silk_y),
            silk_line(-gap / 2.0, silk_y, gap / 2.0, silk_y),
        ],
    }
}

// ============================================================================
// Dual-row gull-wing (SOIC / SOP / SSOP / TSSOP / MSOP), pins on left & right
// ============================================================================

fn dual_lr(
    name: String,
    pins: u32,
    pitch: f64,
    body_w: f64,
    lead_len: f64,
    lead_wid: f64,
) -> FootprintTemplate {
    let half = pins / 2;
    // Pad center column just outside the body, leads tucked partly under it.
    let row_center = body_w / 2.0 + lead_len / 2.0 - 0.3;
    let top = (half as f64 - 1.0) / 2.0 * pitch;
    let mut pads = Vec::new();
    // Left column, top -> bottom: pins 1..half.
    for i in 0..half {
        let y = top - i as f64 * pitch;
        pads.push(rect_smd(
            &(i + 1).to_string(),
            -row_center,
            y,
            lead_len,
            lead_wid,
        ));
    }
    // Right column, bottom -> top: pins half+1..pins.
    for i in 0..half {
        let y = -top + i as f64 * pitch;
        pads.push(rect_smd(
            &(half + i + 1).to_string(),
            row_center,
            y,
            lead_len,
            lead_wid,
        ));
    }
    let body_h = half as f64 * pitch + (pitch * 0.5).max(0.6);
    FootprintTemplate {
        name,
        pads,
        graphics: ic_graphics(body_w, body_h),
    }
}

// ============================================================================
// Dual-row, pins on top & bottom (SOT-23 family, SC-70, ...)
// ============================================================================

fn dual_tb(
    name: String,
    bottom: &[u32],
    top: &[u32],
    pitch: f64,
    row_y: f64,
    pad_w: f64,
    pad_h: f64,
) -> FootprintTemplate {
    let span = bottom.len().max(top.len()) as u32;
    let mut pads = Vec::new();
    let place_row = |pads: &mut Vec<Pad>, nums: &[u32], y: f64| {
        let n = nums.len();
        if n == 0 {
            return;
        }
        // Spread `n` pads symmetrically across the full `span` width.
        let full = (span as f64 - 1.0) * pitch;
        for (k, num) in nums.iter().enumerate() {
            let x = if n == 1 {
                0.0
            } else {
                -full / 2.0 + (k as f64) * (full / (n as f64 - 1.0))
            };
            pads.push(rect_smd(&num.to_string(), x, y, pad_w, pad_h));
        }
    };
    place_row(&mut pads, bottom, row_y); // bottom row (+y)
    place_row(&mut pads, top, -row_y); // top row (-y)
    let body_w = (span as f64) * pitch + 0.4;
    let body_h = 2.0 * row_y - pad_h + 0.2;
    FootprintTemplate {
        name,
        pads,
        graphics: ic_graphics(body_w, body_h.max(1.0)),
    }
}

/// SOT-23 / SOT-23-5 / SOT-23-6 / SOT-23-8 (and SC-70 / SOT-353 / SOT-363).
fn sot_dual(
    name: String,
    pins: u32,
    pitch: f64,
    row_y: f64,
    pad_w: f64,
    pad_h: f64,
) -> FootprintTemplate {
    let bottom_n = pins.div_ceil(2);
    let bottom: Vec<u32> = (1..=bottom_n).collect();
    // Top row numbered right-to-left, continuing CCW.
    let top: Vec<u32> = ((bottom_n + 1)..=pins).rev().collect();
    dual_tb(name, &bottom, &top, pitch, row_y, pad_w, pad_h)
}

// ============================================================================
// Quad (QFP / LQFP / TQFP / QFN / DFN-as-quad), CCW from pin 1 (top-left)
// ============================================================================

fn quad(
    name: String,
    pins: u32,
    pitch: f64,
    row_center: f64,
    lead_len: f64,
    lead_wid: f64,
    thermal: Option<f64>,
) -> FootprintTemplate {
    let pps = pins / 4;
    let edge = (pps as f64 - 1.0) / 2.0 * pitch;
    let mut pads = Vec::new();
    let mut num = 1u32;
    // Left side, top -> bottom.
    for i in 0..pps {
        let y = edge - i as f64 * pitch;
        pads.push(rect_smd(
            &num.to_string(),
            -row_center,
            y,
            lead_len,
            lead_wid,
        ));
        num += 1;
    }
    // Bottom side, left -> right.
    for i in 0..pps {
        let x = -edge + i as f64 * pitch;
        pads.push(rect_smd(
            &num.to_string(),
            x,
            row_center,
            lead_wid,
            lead_len,
        ));
        num += 1;
    }
    // Right side, bottom -> top.
    for i in 0..pps {
        let y = -edge + i as f64 * pitch;
        pads.push(rect_smd(
            &num.to_string(),
            row_center,
            y,
            lead_len,
            lead_wid,
        ));
        num += 1;
    }
    // Top side, right -> left.
    for i in 0..pps {
        let x = edge - i as f64 * pitch;
        pads.push(rect_smd(
            &num.to_string(),
            x,
            -row_center,
            lead_wid,
            lead_len,
        ));
        num += 1;
    }
    if let Some(ep) = thermal {
        pads.push(rect_smd("EP", 0.0, 0.0, ep, ep));
    }
    let body = 2.0 * (row_center - lead_len / 2.0 + 0.2);
    FootprintTemplate {
        name,
        pads,
        graphics: ic_graphics(body, body),
    }
}

// ============================================================================
// DIP (through-hole dual in-line)
// ============================================================================

fn dip(pins: u32) -> FootprintTemplate {
    let pitch = 2.54;
    let row_spacing = 7.62;
    let half = pins / 2;
    let top = (half as f64 - 1.0) / 2.0 * pitch;
    // DIP IC leads are ~0.5mm round; drill the IPC gap (→ 0.7mm) with a wide
    // hand-insert pad.
    let drill = tht_drill(0.5);
    let pad = tht_pad(drill, 0.45);
    let mut pads = Vec::new();
    for i in 0..half {
        let y = top - i as f64 * pitch;
        pads.push(circle_tht(
            &(i + 1).to_string(),
            -row_spacing / 2.0,
            y,
            pad,
            drill,
        ));
    }
    for i in 0..half {
        let y = -top + i as f64 * pitch;
        pads.push(circle_tht(
            &(half + i + 1).to_string(),
            row_spacing / 2.0,
            y,
            pad,
            drill,
        ));
    }
    let body_h = half as f64 * pitch + 1.0;
    let mut graphics = silk_rect(
        -row_spacing / 2.0 - 1.5,
        -body_h / 2.0,
        row_spacing / 2.0 + 1.5,
        body_h / 2.0,
    );
    graphics.push(pin1_dot(-row_spacing / 2.0, -body_h / 2.0 + 1.0));
    FootprintTemplate {
        name: format!("DIP-{pins}"),
        pads,
        graphics,
    }
}

// ============================================================================
// Tab packages (DPAK / D2PAK = TO-252 / TO-263) and TO-220/TO-247 (THT)
// ============================================================================

/// `leads` = the small signal-lead pin numbers on the bottom edge; `tab_num`
/// = the large heat-tab pad number (often the drain). SMD by default.
fn tab_package(
    name: String,
    leads: &[u32],
    tab_num: u32,
    pitch: f64,
    lead_dims: (f64, f64),
    tab_dims: (f64, f64),
    tht: bool,
) -> FootprintTemplate {
    let (lw, lh) = lead_dims;
    let (tw, th) = tab_dims;
    let mut pads = Vec::new();
    let n = leads.len();
    let lead_y = th / 2.0 + 1.0; // leads project below the tab
    let lead_drill = tht_drill((lw.min(lh) - 0.4).max(0.6)); // lead + 0.2mm IPC gap
    for (k, num) in leads.iter().enumerate() {
        let x = if n == 1 {
            0.0
        } else {
            (k as f64 - (n as f64 - 1.0) / 2.0) * pitch
        };
        if tht {
            pads.push(circle_tht(
                &num.to_string(),
                x,
                lead_y,
                lw.max(lh),
                lead_drill,
            ));
        } else {
            pads.push(rect_smd(&num.to_string(), x, lead_y, lw, lh));
        }
    }
    // The tab.
    if tht {
        pads.push(circle_tht(
            &tab_num.to_string(),
            0.0,
            -lead_y,
            tw.min(th),
            1.5,
        ));
    } else {
        pads.push(rect_smd(&tab_num.to_string(), 0.0, -th / 2.0, tw, th));
    }
    let body_w = (tw).max((n as f64) * pitch) + 0.5;
    let body_h = th + lh + 2.5;
    FootprintTemplate {
        name,
        pads,
        graphics: ic_graphics(body_w, body_h),
    }
}

// ============================================================================
// Two-terminal SMD (SOD diodes, DO-214 / SMA / SMB / SMC)
// ============================================================================

fn two_pad(name: String, span: f64, pad_w: f64, pad_h: f64) -> FootprintTemplate {
    FootprintTemplate {
        name,
        pads: vec![
            rect_smd("1", -span / 2.0, 0.0, pad_w, pad_h),
            rect_smd("2", span / 2.0, 0.0, pad_w, pad_h),
        ],
        graphics: vec![
            silk_line(-0.6, -pad_h / 2.0 - 0.2, 0.6, -pad_h / 2.0 - 0.2),
            silk_line(-0.6, pad_h / 2.0 + 0.2, 0.6, pad_h / 2.0 + 0.2),
            // Cathode bar near pin 1.
            silk_line(
                -span / 2.0 + pad_w,
                -pad_h / 2.0,
                -span / 2.0 + pad_w,
                pad_h / 2.0,
            ),
        ],
    }
}

// ============================================================================
// SMD crystals (2-pad 5032/6035/7050, 4-pad 3225/2520/2016/1612)
// ============================================================================

/// Two-terminal SMD crystal (e.g. 5032 = 5.0 × 3.2 mm body): one pad at each
/// end, sized off the body per IPC-7351 chip-like proportions with a small toe
/// past the body edge.
fn crystal_2pad(code: &str, body_w: f64, body_h: f64) -> FootprintTemplate {
    let pad_w = body_w * 0.38;
    let pad_h = body_h * 0.75;
    let cx = body_w / 2.0 - pad_w / 2.0 + 0.45;
    let mut graphics = silk_rect(-body_w / 2.0, -body_h / 2.0, body_w / 2.0, body_h / 2.0);
    graphics.push(pin1_dot(-cx - pad_w / 2.0 - 0.4, 0.0));
    graphics.push(courtyard(cx + pad_w / 2.0 + 0.25, pad_h / 2.0 + 0.25));
    FootprintTemplate {
        name: format!("Crystal_{code}"),
        pads: vec![
            rect_smd("1", -cx, 0.0, pad_w, pad_h),
            rect_smd("2", cx, 0.0, pad_w, pad_h),
        ],
        graphics,
    }
}

/// Four-terminal SMD crystal (e.g. 3225 = 3.2 × 2.5 mm body): one pad per
/// corner, numbered CCW from pin 1 at the bottom-left (KiCad convention —
/// pins 2 and 4 are usually the grounded case pads).
fn crystal_4pad(code: &str, body_w: f64, body_h: f64) -> FootprintTemplate {
    let pad_w = body_w * 0.44;
    let pad_h = body_h * 0.46;
    let cx = body_w / 2.0 - pad_w / 2.0 + 0.2;
    let cy = body_h / 2.0 - pad_h / 2.0 + 0.2;
    let mut graphics = silk_rect(-body_w / 2.0, -body_h / 2.0, body_w / 2.0, body_h / 2.0);
    graphics.push(pin1_dot(-cx - pad_w / 2.0 - 0.4, cy + pad_h / 2.0 + 0.4));
    graphics.push(courtyard(cx + pad_w / 2.0 + 0.25, cy + pad_h / 2.0 + 0.25));
    FootprintTemplate {
        name: format!("Crystal_{code}"),
        pads: vec![
            rect_smd("1", -cx, cy, pad_w, pad_h),
            rect_smd("2", cx, cy, pad_w, pad_h),
            rect_smd("3", cx, -cy, pad_w, pad_h),
            rect_smd("4", -cx, -cy, pad_w, pad_h),
        ],
        graphics,
    }
}

// ============================================================================
// Connectors: pin headers / sockets, screw terminals, radial electrolytics
// ============================================================================

fn pin_header(rows: u32, cols: u32, pitch: f64) -> FootprintTemplate {
    // Round-equivalent lead of the square header post: 2.54mm strips use ~0.8mm
    // (a 0.64mm post needs the diagonal cleared), finer pitches proportionally
    // smaller. Drill the IPC gap over that, pad keeps a 0.35mm ring — yielding
    // the standard 1.0mm drill / 1.7mm pad on classic 2.54mm headers instead of
    // the old pitch-scaled 1.016mm drill / 1.524mm pad.
    let lead = (pitch * 0.315).clamp(0.5, 0.8);
    let drill = tht_drill(lead);
    let pad_dia = tht_pad(drill, 0.35);
    let mut pads = Vec::new();
    let mut num = 1u32;
    for c in 0..cols {
        for r in 0..rows {
            let x = if rows == 1 {
                0.0
            } else {
                (r as f64 - (rows as f64 - 1.0) / 2.0) * pitch
            };
            let y = (c as f64 - (cols as f64 - 1.0) / 2.0) * pitch;
            pads.push(circle_tht(&num.to_string(), x, y, pad_dia, drill));
            num += 1;
        }
    }
    let half_w = if rows == 1 {
        1.27
    } else {
        (rows as f64 - 1.0) * pitch / 2.0 + 1.0
    };
    let half_h = (cols as f64 - 1.0) * pitch / 2.0 + 1.0;
    FootprintTemplate {
        name: format!("PinHeader_{rows}x{cols}"),
        pads,
        graphics: silk_rect(-half_w, -half_h, half_w, half_h),
    }
}

fn screw_terminal(positions: u32, pitch: f64) -> FootprintTemplate {
    // Terminal blocks take a stripped wire ferrule (~1.1mm) — drill the gap,
    // wide pad for the larger solder fillet.
    let drill = tht_drill(1.1);
    let pad = tht_pad(drill, 0.65);
    let mut pads = Vec::new();
    for i in 0..positions {
        let x = (i as f64 - (positions as f64 - 1.0) / 2.0) * pitch;
        pads.push(circle_tht(&(i + 1).to_string(), x, 0.0, pad, drill));
    }
    let half_w = (positions as f64) * pitch / 2.0 + 0.5;
    FootprintTemplate {
        name: format!("ScrewTerminal_1x{positions}"),
        pads,
        graphics: silk_rect(-half_w, -pitch / 2.0 - 1.0, half_w, pitch / 2.0 + 1.0),
    }
}

fn radial_electrolytic(pitch: f64, body_dia: f64) -> FootprintTemplate {
    // Radial electrolytic leads are ~0.6mm round.
    let drill = tht_drill(0.6);
    let pad = tht_pad(drill, 0.4);
    FootprintTemplate {
        name: format!("CP_Radial_D{body_dia:.1}mm_P{pitch:.2}mm"),
        pads: vec![
            circle_tht("1", -pitch / 2.0, 0.0, pad, drill),
            circle_tht("2", pitch / 2.0, 0.0, pad, drill),
        ],
        graphics: vec![FootprintGraphic::Circle {
            center: Vec2::new(0.0, 0.0),
            radius: body_dia / 2.0,
            width: 0.12,
            layer: PcbLayer::FSilkS,
        }],
    }
}

// ============================================================================
// Wire-to-board connectors (JST PH/XH/EH/SH/GH, Molex Pico-Blade): single row
// ============================================================================

/// Pad style for a single-row wire-to-board connector.
#[derive(Clone, Copy)]
enum ConnPad {
    /// Through-hole: round pads (pin 1 squared off), drilled `drill`.
    Tht {
        /// Copper land diameter (and pin-1 square edge), mm.
        pad: f64,
        /// Drill hole diameter, mm.
        drill: f64,
    },
    /// Surface-mount: rectangular pads, `w` across the row × `h` along the body.
    Smd {
        /// Pad width (across the pin row), mm.
        w: f64,
        /// Pad height (along the connector body), mm.
        h: f64,
    },
}

/// One row of `pins` contacts spaced `pitch` apart along x, pin 1 at the left
/// (−x). Pin 1 is squared off (THT) and flagged with a silk dot so the polarity
/// survives assembly. Mounting/boss tabs are omitted — the contact lands are
/// what routing and DRC operate on.
fn linear_connector(name: String, pins: u32, pitch: f64, pad: ConnPad) -> FootprintTemplate {
    let n = pins.max(1);
    let span = (n as f64 - 1.0) * pitch;
    let mut pads = Vec::new();
    for i in 0..n {
        let x = -span / 2.0 + i as f64 * pitch;
        let num = (i + 1).to_string();
        match pad {
            ConnPad::Tht { pad, drill } => {
                if i == 0 {
                    pads.push(rect_tht(&num, x, 0.0, pad, pad, drill));
                } else {
                    pads.push(circle_tht(&num, x, 0.0, pad, drill));
                }
            }
            ConnPad::Smd { w, h } => pads.push(rect_smd(&num, x, 0.0, w, h)),
        }
    }
    let pad_h = match pad {
        ConnPad::Tht { pad, .. } => pad,
        ConnPad::Smd { h, .. } => h,
    };
    let half_w = span / 2.0 + pitch * 0.7;
    let half_h = pad_h / 2.0 + 0.9;
    let mut graphics = silk_rect(-half_w, -half_h, half_w, half_h);
    graphics.push(pin1_dot(-span / 2.0 - pitch * 0.45, half_h + 0.3));
    graphics.push(courtyard(half_w + 0.25, half_h + 0.25));
    FootprintTemplate {
        name,
        pads,
        graphics,
    }
}

// ============================================================================
// Tag-Connect spring-pin programming pads (TC2030 = 6, TC2050 = 10)
// ============================================================================

/// Two columns of bare-copper SMD pads at 1.27 mm pitch that a Tag-Connect
/// spring-pin cable lands on. `pins` is rounded down to an even count; pads are
/// numbered 1,2 / 3,4 / … down the rows (KiCad 2×N order). The locating/leg
/// holes are mechanical and omitted — the copper is what gets routed.
fn tag_connect(pins: u32) -> FootprintTemplate {
    let pitch = 1.27;
    let per_col = (pins.max(2) / 2).max(1);
    let col_x = pitch / 2.0;
    let top = (per_col as f64 - 1.0) / 2.0 * pitch;
    // Round contacts a touch under the pitch so neighbours keep a real gap
    // (no overlap → no spurious clearance flags inside the dense pad field).
    let dia = 0.8;
    let mut pads = Vec::new();
    let mut num = 1u32;
    for r in 0..per_col {
        let y = top - r as f64 * pitch;
        pads.push(smd_test_pad(&num.to_string(), -col_x, y, dia));
        num += 1;
        pads.push(smd_test_pad(&num.to_string(), col_x, y, dia));
        num += 1;
    }
    let half_w = col_x + dia / 2.0 + 0.6;
    let half_h = top + dia / 2.0 + 0.6;
    let mut graphics = silk_rect(-half_w, -half_h, half_w, half_h);
    graphics.push(pin1_dot(-col_x - dia / 2.0 - 0.4, top));
    graphics.push(courtyard(half_w + 0.25, half_h + 0.25));
    FootprintTemplate {
        name: format!("TagConnect-{}", per_col * 2),
        pads,
        graphics,
    }
}

// ============================================================================
// USB micro-B receptacle: 5 SMD contacts + 4 THT shield/retention posts
// ============================================================================

/// USB micro-B receptacle: five 0.65 mm-pitch SMD signal contacts
/// (VBUS/D−/D+/ID/GND, pins 1–5) plus four through-hole shield/retention
/// posts numbered SH1–SH4 (mechanical — no declared pin, so they stay clear
/// of the signal nets). Land dims follow the common Molex/Amphenol 5-pin SMD
/// receptacle; use inline `pads` when an exact vendor drawing matters.
fn usb_micro_b() -> FootprintTemplate {
    let pitch = 0.65;
    let span = 4.0 * pitch;
    let row_y = -1.25;
    let mut pads = Vec::new();
    for i in 0..5u32 {
        let x = -span / 2.0 + i as f64 * pitch;
        pads.push(rect_smd(&(i + 1).to_string(), x, row_y, 0.4, 1.35));
    }
    // Shield / retention posts (through-hole), one per corner.
    let drill = tht_drill(0.65);
    let pad_dia = tht_pad(drill, 0.3);
    let post_x = 2.9;
    for (k, (sx, sy)) in [(-1.0, -1.55), (1.0, -1.55), (-1.0, 1.65), (1.0, 1.65)]
        .iter()
        .enumerate()
    {
        pads.push(circle_tht(
            &format!("SH{}", k + 1),
            sx * post_x,
            *sy,
            pad_dia,
            drill,
        ));
    }
    let half_w = post_x + 1.0;
    let half_h = 2.6;
    let mut graphics = silk_rect(-half_w, -half_h, half_w, half_h);
    graphics.push(pin1_dot(-span / 2.0 - 0.5, row_y - 0.8));
    graphics.push(courtyard(half_w + 0.25, half_h + 0.25));
    FootprintTemplate {
        name: "USB-Micro-B".to_string(),
        pads,
        graphics,
    }
}

// ============================================================================
// USB-C receptacle (simplified): 2 SMD contact rows + 4 THT shield posts
// ============================================================================

/// Simplified USB-C receptacle: `signal` SMD contacts split across two rows
/// (front/back) plus four through-hole shield/retention posts at the corners.
/// Contact pitch tightens with the count (≤8 → 1.0, ≤16 → 0.8, else 0.5 mm) to
/// approximate the common 6/16/24-pin variants. Posts are numbered SH1–SH4 so
/// they map to no declared pin and stay clear of the signal nets (mechanical).
/// This is an approximation — use inline `pads` when exact A1/B1 geometry and
/// numbering matter.
fn usb_c(signal: u32) -> FootprintTemplate {
    let n = signal.max(2);
    let per_row = n.div_ceil(2);
    let pitch = if n <= 8 {
        1.0
    } else if n <= 16 {
        0.8
    } else {
        0.5
    };
    let row_y = 1.45;
    let span = (per_row as f64 - 1.0) * pitch;
    let pad_w = (pitch * 0.6).clamp(0.3, 0.7);
    let mut pads = Vec::new();
    let mut num = 1u32;
    // Front row (−y) then back row (+y), each left→right; numbering continues.
    for &ry in &[-row_y, row_y] {
        for i in 0..per_row {
            if num > n {
                break;
            }
            let x = -span / 2.0 + i as f64 * pitch;
            pads.push(rect_smd(&num.to_string(), x, ry, pad_w, 1.1));
            num += 1;
        }
    }
    // Shield / mounting posts (through-hole), one per corner.
    let post_x = span / 2.0 + 1.7;
    let post_y = 1.9;
    for (k, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .iter()
        .enumerate()
    {
        pads.push(circle_tht(
            &format!("SH{}", k + 1),
            sx * post_x,
            sy * post_y,
            1.7,
            1.0,
        ));
    }
    let half_w = post_x + 1.0;
    let half_h = post_y + 1.0;
    let mut graphics = silk_rect(-half_w, -half_h, half_w, half_h);
    graphics.push(pin1_dot(-span / 2.0 - 0.6, -row_y - 0.7));
    graphics.push(courtyard(half_w + 0.25, half_h + 0.25));
    FootprintTemplate {
        name: format!("USB-C-{signal}"),
        pads,
        graphics,
    }
}

// ============================================================================
// Generic compact fallback (replaces the off-board single-column placeholder)
// ============================================================================

fn grid_fallback(pins: u32) -> FootprintTemplate {
    let cols = (pins as f64).sqrt().ceil() as u32;
    let cols = cols.max(1);
    let pitch = 2.54;
    // Unknown lead — assume a generic 0.6mm pin so the placeholder drills a
    // fab-buildable hole rather than an oversized one.
    let drill = tht_drill(0.6);
    let pad = tht_pad(drill, 0.3);
    let mut pads = Vec::new();
    let rows = pins.div_ceil(cols);
    for n in 0..pins {
        let r = n / cols;
        let c = n % cols;
        let x = (c as f64 - (cols as f64 - 1.0) / 2.0) * pitch;
        let y = (r as f64 - (rows as f64 - 1.0) / 2.0) * pitch;
        pads.push(circle_tht(&(n + 1).to_string(), x, y, pad, drill));
    }
    FootprintTemplate {
        name: format!("Generic-{pins}pad"),
        pads,
        graphics: vec![],
    }
}

// ============================================================================
// Parsing helpers
// ============================================================================

/// First unsigned integer that follows `marker` in `s`.
fn uint_after(s: &str, marker: &str) -> Option<u32> {
    let idx = s.find(marker)? + marker.len();
    s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// First decimal number that follows `marker` in `s` (e.g. pitch from `_P0.4mm`).
fn float_after(s: &str, marker: &str) -> Option<f64> {
    let idx = s.find(marker)? + marker.len();
    s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .ok()
}

/// Body dimensions from a `"<w>x<h>mm"` token (e.g. `5x5mm`, `3.9x4.9mm`).
/// Deliberately requires the trailing `m` so `RxC` connector tokens like
/// `1x02_` are not mistaken for a body size.
fn body_mm(s: &str) -> Option<(f64, f64)> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'x' {
            continue;
        }
        let lstart = s[..i]
            .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map(|p| p + 1)
            .unwrap_or(0);
        let lstr = &s[lstart..i];
        let after = &s[i + 1..];
        let rlen = after
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(after.len());
        let rstr = &after[..rlen];
        if lstr.is_empty() || rstr.is_empty() {
            continue;
        }
        if bytes.get(i + 1 + rlen).copied() != Some(b'm') {
            continue; // not "...mm" — likely an RxC connector token
        }
        if let (Ok(w), Ok(h)) = (lstr.parse::<f64>(), rstr.parse::<f64>()) {
            return Some((w, h));
        }
    }
    None
}

/// Parse a `RxC` pattern (e.g. the `1x02` in `PinHeader_1x02_P2.54mm`) where
/// both digit runs are underscore/boundary delimited (so floats like
/// `3.9x4.9mm` are rejected).
fn rows_cols(s: &str) -> Option<(u32, u32)> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'x' {
            continue;
        }
        let run_start = s[..i]
            .rfind(|c: char| !c.is_ascii_digit())
            .map(|p| p + 1)
            .unwrap_or(0);
        let run_end = i
            + 1
            + s[i + 1..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(s.len() - i - 1);
        if run_start == i || run_end == i + 1 {
            continue;
        }
        let before_ok = run_start == 0 || bytes[run_start - 1] == b'_';
        // Accept a trailing `_` *or* `-` so KiCad's mounting-pad suffix
        // (`1x02-1MP`) still parses to its 2 contacts. Body sizes like
        // `5x5mm`/`3.9x4.9mm` are still rejected — `mm`/`.` fail the digit-run
        // boundary on the other side.
        let after_ok = run_end == s.len() || bytes[run_end] == b'_' || bytes[run_end] == b'-';
        if !before_ok || !after_ok {
            continue;
        }
        let rows: u32 = s[run_start..i].parse().ok()?;
        let cols: u32 = s[i + 1..run_end].parse().ok()?;
        return Some((rows, cols));
    }
    None
}

/// Contact count for a connector: prefer a KiCad `RxC` token (`1x02`, `2x05`),
/// then a count immediately after `count_marker` (the bare `JST_PH_2` form),
/// then the caller-declared `pin_count`. `count_marker` must include the
/// trailing `_` (e.g. `"JST_PH_"`) so it only fires on the bare-count form and
/// never grabs a part-number digit run.
fn connector_pins(base: &str, count_marker: &str, declared: u32) -> u32 {
    if let Some((r, c)) = rows_cols(base) {
        let n = r * c;
        if n > 0 {
            return n;
        }
    }
    if let Some(n) = uint_after(base, count_marker) {
        if n > 0 {
            return n;
        }
    }
    declared
}

/// Pin count from a family marker (`"QFN-40"` → 40), falling back to the
/// caller-declared `pin_count` when the id carries no count.
fn family_pins(base: &str, markers: &[&str], declared: u32) -> u32 {
    for m in markers {
        if let Some(n) = uint_after(base, m) {
            if n > 0 {
                return n;
            }
        }
    }
    declared
}

// ============================================================================
// Family dispatch
// ============================================================================

/// Like `s.contains(token)` but only when the match is bounded by `_` or a
/// string boundary on both sides. Keeps a bare chip code like `0805` matching
/// `R_0805_2012Metric` while rejecting an embedded run such as the `2512` in
/// `MAX2512_SOT-23-6` or the `1206` in `BGA-1206` (which would otherwise be
/// hijacked into a 2-pad chip before the real family marker is seen).
fn delimited_contains(s: &str, token: &str) -> bool {
    let bytes = s.as_bytes();
    let mut from = 0;
    while let Some(rel) = s[from..].find(token) {
        let i = from + rel;
        let before_ok = i == 0 || bytes[i - 1] == b'_';
        let after = i + token.len();
        let after_ok = after == s.len() || bytes[after] == b'_';
        if before_ok && after_ok {
            return true;
        }
        from = i + 1;
    }
    false
}

/// Build a QFN land pattern via the unified parametric generator
/// ([`vcad_ecad_package::derive`]). Returns `None` if derivation fails, so the
/// caller can fall back to the legacy table.
///
/// `pins` is the (multiple-of-4) total lead count; `ep` is the exposed-pad
/// edge length. Terminal contact dims (0.4 × 0.2 mm) plus the no-lead IPC
/// fillet goals reproduce a standard fine-pitch QFN land.
fn qfn_via_derive(pins: u32, pitch: f64, body: f64, ep: f64) -> Option<FootprintTemplate> {
    let pc = vcad_ecad_package::presets::qfn(pins, pitch, body, ep);
    vcad_ecad_package::derive(&pc).ok().map(|d| d.footprint)
}

// ============================================================================
// Power MOSFETs, power diodes, IMUs and high-current wire-to-board connectors
// ============================================================================
//
// Every land pattern in this section is transcribed from the vendor document
// cited on its function (document number + revision), in KiCad's top-view,
// Y-down frame. Where a vendor drawing gives only the package (no land
// pattern) the derivation is spelled out next to the numbers.
//
// Power FETs collapse their multi-lead source/drain rows into one net per
// terminal, numbered 1 = G, 2 = S, 3 = D (the netlist model schematics use for
// discrete FETs). A terminal whose vendor land is not a rectangle (combed
// source/drain lands) is emitted as several abutting rectangles that share the
// terminal's number, exactly like KiCad's multi-pad terminals.
//
// Paste: pads carry full-size paste. The vendors' reduced stencil windows for
// the big FET drain/source lands (e.g. the TDSON-8 FL aperture drawing) are not
// modelled — apply them in the stencil step for production.

/// Mechanical (non-plated) hole, e.g. a connector's plastic locating peg.
fn npth(x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        number: String::new(),
        pad_type: PadType::NPTH,
        shape: if (w - h).abs() < 1e-9 {
            PadShape::Circle { diameter: w }
        } else {
            PadShape::Rect {
                width: w,
                height: h,
            }
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: Some(DrillSpec {
            diameter: w.min(h),
            oval: (w - h).abs() > 1e-9,
            oval_height: if (w - h).abs() > 1e-9 {
                Some(w.max(h))
            } else {
                None
            },
        }),
        net: None,
        layers: vec![PcbLayer::FMask, PcbLayer::BMask],
    }
}

/// Axis-aligned extent of a pad's copper (or hole, for NPTH).
fn pad_extent(p: &Pad) -> (f64, f64, f64, f64) {
    let (w, h) = match p.shape {
        PadShape::Rect { width, height } => (width, height),
        PadShape::Circle { diameter } => (diameter, diameter),
        _ => (0.0, 0.0),
    };
    (
        p.position.x - w / 2.0,
        p.position.y - h / 2.0,
        p.position.x + w / 2.0,
        p.position.y + h / 2.0,
    )
}

/// Body silk outline `(x1, y1, x2, y2)` + pin-1 dot + a courtyard enclosing
/// the body AND every pad (IPC-7351 0.25 mm excess). Connectors and LGAs whose
/// pads overhang the body, or whose body is not centred on the origin, need the
/// union — `ic_graphics` assumes a centred body that covers its pads.
fn outlined_graphics(
    pads: &[Pad],
    body: (f64, f64, f64, f64),
    pin1: (f64, f64),
) -> Vec<FootprintGraphic> {
    let (bx1, by1, bx2, by2) = body;
    let mut g = silk_rect(bx1, by1, bx2, by2);
    g.push(pin1_dot(pin1.0, pin1.1));
    let (mut x1, mut y1, mut x2, mut y2) = (bx1, by1, bx2, by2);
    for p in pads {
        let (a, b, c, d) = pad_extent(p);
        x1 = x1.min(a);
        y1 = y1.min(b);
        x2 = x2.max(c);
        y2 = y2.max(d);
    }
    g.push(FootprintGraphic::Rect {
        start: Vec2::new(x1 - 0.25, y1 - 0.25),
        end: Vec2::new(x2 + 0.25, y2 + 0.25),
        width: 0.05,
        layer: PcbLayer::FCrtYd,
    });
    g
}

/// Infineon PG-HSOF-8-1 (TOLL, 9.9 × 11.7 mm). Package: outline
/// Z8B00169619 rev 02 (IPT015N10N5 datasheet Rev. 2.2, 2016-10-13, fig. 1):
/// 8 leads at e = 1.20, gate = lead 1, leads 2–8 source, tab = drain. Infineon
/// publishes the land pattern only as a separate footprint drawing on the
/// PG-HSOF-8-1 package page (not in the datasheet); the numbers below are that
/// drawing as captured in KiCad 9's `Infineon_PG-HSOF-8-1.kicad_mod`, rotated
/// 90° so the leads sit on the +Y edge:
/// - lead lands 0.8 × 2.8 at x = −4.2 + 1.2·i, y 3.85…6.65;
/// - source leads 2–8 bridged by a 0.8 mm bar (y 3.85…4.65);
/// - drain land 9.7 × 6.6 (y −5.25…1.35) + 10.1 × 1.4 tab strip (y −6.65…−5.25);
/// - 2.5 mm source-to-drain gap; courtyard ±5.3 × ±6.9.
fn toll() -> FootprintTemplate {
    let mut pads = vec![rect_smd("1", -4.2, 5.25, 0.8, 2.8)];
    // Source: bar + 7 fingers (the fingers' inner 0.8 mm is the bar).
    pads.push(rect_smd("2", 0.6, 4.25, 8.0, 0.8));
    for i in 1..8 {
        pads.push(rect_smd("2", -4.2 + 1.2 * i as f64, 5.65, 0.8, 2.0));
    }
    pads.push(rect_smd("3", 0.0, -1.95, 9.7, 6.6));
    pads.push(rect_smd("3", 0.0, -5.95, 10.1, 1.4));
    let graphics = outlined_graphics(&pads, (-4.95, -5.84, 4.95, 5.84), (-5.3, 6.4));
    FootprintTemplate {
        name: "TOLL".into(),
        pads,
        graphics,
    }
}

/// Infineon PG-TDSON-8 FL (SuperSO8, 5.15 × 6.15 mm). Land pattern:
/// "PG-TDSON-8-FL: Recommended Boardpads & Apertures", BSC010N04LS datasheet
/// Rev. 2.4 (2020-03-13), fig. 2. Leads 1–3 source, 4 gate (+Y edge, pin 1 at
/// −X); leads 5–8 + exposed pad drain. Pads at the 1.27 mm lead pitch:
/// - gate 0.5 × 1.1, 0.77 from the source land;
/// - source: 3.2 × 0.6 bar (y 2.225…2.825) + three 0.5 × 0.5 fingers to y 3.325;
/// - drain: 4.41 × 3.75 (y −2.525…1.225) + four 0.6 × 0.8 fingers to y −3.325;
/// - 1.0 mm source-to-drain gap.
fn tdson8() -> FootprintTemplate {
    let p = 1.27;
    let mut pads = vec![rect_smd("1", 1.5 * p, 2.775, 0.5, 1.1)];
    pads.push(rect_smd("2", -0.715, 2.525, 3.2, 0.6));
    for i in 0..3 {
        pads.push(rect_smd("2", (i as f64 - 1.5) * p, 3.075, 0.5, 0.5));
    }
    pads.push(rect_smd("3", 0.0, -0.65, 4.41, 3.75));
    for i in 0..4 {
        pads.push(rect_smd("3", (i as f64 - 1.5) * p, -2.925, 0.6, 0.8));
    }
    let graphics = outlined_graphics(&pads, (-2.575, -3.075, 2.575, 3.075), (-2.9, 3.6));
    FootprintTemplate {
        name: "TDSON-8".into(),
        pads,
        graphics,
    }
}

/// Infineon PG-TSDSON-8 FL (S3O8, 3.3 × 3.3 mm). Land pattern: outline
/// Z8B00158553 rev 02 (BSZ018N04LS6 datasheet Rev. 2.1, 2020-03-31, fig. 1,
/// F-dimensions), rotated so the source/gate leads sit on the +Y edge:
/// - overall F1 = 3.90 across the land, F2 = 2.29 wide;
/// - drain F7 = 2.51 deep incl. F5 = 0.80 lead fingers (0.34 wide = F4,
///   0.65 pitch);
/// - gate 0.34 × ~1.0 (F6 = 1.00), F3 = 0.31 gap to source;
/// - source F8 = 1.64 wide incl. F9 = 0.50 lead fingers.
///
/// F10 (source/gate land depth) is not legible in the datasheet's table; it
/// is taken as F1 − F7 − 0.40 = 0.99 (0.40 = the drawn drain-to-source gap,
/// to scale against F1/F7) — consistent with F6 = 1.00.
fn tsdson8() -> FootprintTemplate {
    let p = 0.65;
    let mut pads = vec![rect_smd("1", 1.5 * p, 1.455, 0.34, 0.99)];
    pads.push(rect_smd("2", -0.325, 1.205, 1.64, 0.49));
    for i in 0..3 {
        pads.push(rect_smd("2", (i as f64 - 1.5) * p, 1.70, 0.34, 0.5));
    }
    pads.push(rect_smd("3", 0.0, -0.295, 2.29, 1.71));
    for i in 0..4 {
        pads.push(rect_smd("3", (i as f64 - 1.5) * p, -1.55, 0.34, 0.8));
    }
    let graphics = outlined_graphics(&pads, (-1.65, -1.65, 1.65, 1.65), (-1.9, 2.2));
    FootprintTemplate {
        name: "TSDSON-8".into(),
        pads,
        graphics,
    }
}

/// Vishay SMPC / TO-277A (e.g. SS10P10-class Schottkys, 4.6 × 6.5 mm).
/// Mounting pad layout from Vishay SS5PH102 datasheet, doc. 98856 rev.
/// 04-Dec-2025, p. 4 ("Conform to JEDEC TO-277A"): cathode tab land
/// 4.80 × 4.72 min, two anode lands 1.40 × 1.27 min on ±1.04 mm centres, 6.80
/// overall. Numbered like the other two-terminal diodes here: 1 = cathode
/// (tab), 2 = anode (both anode lands — they are one terminal).
fn to277a() -> FootprintTemplate {
    let pads = vec![
        rect_smd("1", 0.0, -1.04, 4.80, 4.72),
        rect_smd("2", -1.04, 2.765, 1.40, 1.27),
        rect_smd("2", 1.04, 2.765, 1.40, 1.27),
    ];
    let graphics = outlined_graphics(&pads, (-2.3, -3.25, 2.3, 3.25), (-2.9, -3.5));
    FootprintTemplate {
        name: "TO-277A".into(),
        pads,
        graphics,
    }
}

/// Bosch BMI088 IMU, LGA-16 3.0 × 4.5 mm, 0.5 mm pitch. Land pattern:
/// BMI088 datasheet BST-BMI088-DS000-19, rev. 1.9 (January 2024), §8.2 fig. 11
/// (top view): 0.675 × 0.25 lands; pins 1–7 down the right column, 9–15 up the
/// left (inner edges 0.925 from the centre line), pin 8 bottom / 16 top
/// (inner edges 1.675 from centre). Pin 1 top-right.
fn bmi088() -> FootprintTemplate {
    let (len, w, xc, yc) = (0.675, 0.25, 0.925 + 0.675 / 2.0, 1.675 + 0.675 / 2.0);
    let mut pads = Vec::new();
    for n in 1..=16u32 {
        let pad = match n {
            1..=7 => rect_smd(&n.to_string(), xc, -1.5 + 0.5 * (n - 1) as f64, len, w),
            8 => rect_smd("8", 0.0, yc, w, len),
            9..=15 => rect_smd(&n.to_string(), -xc, 1.5 - 0.5 * (n - 9) as f64, len, w),
            _ => rect_smd("16", 0.0, -yc, w, len),
        };
        pads.push(pad);
    }
    let graphics = outlined_graphics(&pads, (-1.5, -2.25, 1.5, 2.25), (1.9, -2.6));
    FootprintTemplate {
        name: "BMI088_LGA-16".into(),
        pads,
        graphics,
    }
}

/// TDK InvenSense ICM-42688-P IMU, LGA-14 3.0 × 2.5 mm, 0.5 mm pitch.
/// Package: datasheet DS-000347 rev. 1.5, §10.2 (lead W 0.25, L 0.475, leads
/// inset 0.10 from the body edge) and pin-out §4.1 fig. 5 (top view): pins
/// 1–4 down the left edge, 5–7 along the bottom, 8–11 up the right, 12–14
/// along the top right-to-left. TDK gives no land pattern, so lands follow
/// IPC-7351 no-lead practice on that lead: 0.30 wide (W + 0.05), heel 0.05
/// inside the lead, toe 0.10 past the body edge (Bosch's LGA convention above)
/// → 0.725 long.
fn icm42688() -> FootprintTemplate {
    let (len, w) = (0.725, 0.30);
    let xc = 1.6 - len / 2.0; // long-edge (x = ±1.5) leads: toe at ±1.60
    let yc = 1.35 - len / 2.0; // short-edge (y = ±1.25) leads: toe at ±1.35
    let mut pads = Vec::new();
    for n in 1..=14u32 {
        let k = n as f64;
        let pad = match n {
            1..=4 => rect_smd(&n.to_string(), -xc, -0.75 + 0.5 * (k - 1.0), len, w),
            5..=7 => rect_smd(&n.to_string(), -0.5 + 0.5 * (k - 5.0), yc, w, len),
            8..=11 => rect_smd(&n.to_string(), xc, 0.75 - 0.5 * (k - 8.0), len, w),
            _ => rect_smd(&n.to_string(), 0.5 - 0.5 * (k - 12.0), -yc, w, len),
        };
        pads.push(pad);
    }
    let graphics = outlined_graphics(&pads, (-1.5, -1.25, 1.5, 1.25), (-1.9, -1.6));
    FootprintTemplate {
        name: "ICM-42688-P_LGA-14".into(),
        pads,
        graphics,
    }
}

/// Plated power hole with a fab-safe 0.65 mm annular ring (pin 1 square).
fn power_tht(num: &str, x: f64, y: f64, drill: f64) -> Pad {
    let d = drill + 1.3;
    if num == "1" {
        rect_tht(num, x, y, d, d, drill)
    } else {
        circle_tht(num, x, y, d, drill)
    }
}

/// AMASS XT30PW-M (horizontal). AMASS spec "XT30PW-M" V1.2, mounting hole
/// diagram: 2 × Ø1.7 power holes on 5.00 mm, 2 × Ø1.0 housing pegs 11.00 mm
/// apart, 10.00 mm behind the power row (toward the mating face).
fn xt30pw() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -2.5, 0.0, 1.7),
        power_tht("2", 2.5, 0.0, 1.7),
        npth(-5.5, -10.0, 1.0, 1.0),
        npth(5.5, -10.0, 1.0, 1.0),
    ];
    // Body 13.30 wide (incl. peg ears) × 9.90 deep, ending ~2.0 behind the row.
    let graphics = outlined_graphics(&pads, (-6.65, -11.9, 6.65, 2.0), (-4.8, 1.8));
    FootprintTemplate {
        name: "XT30PW".into(),
        pads,
        graphics,
    }
}

/// AMASS XT30PW(2+2)-M.G.B (right angle, 2 power + 2 signal; LCSC C19268030).
/// Land pattern from the LCSC/EasyEDA footprint `CONN-TH_4P-XT30PW-M`
/// (no AMASS drawing in hand): 2 × Ø1.90 power holes on 5.00 mm (EasyEDA
/// draws 5.08; 5.00 is the XT30 family pitch), two Ø0.91 signal holes
/// 3.85 mm beyond the "−" hole, 1.23 and 3.23 mm toward the mating face.
/// Housing 14.00 wide, from 2.86 to 11.66 in front of the power row. Pad 3/4
/// follow the EasyEDA numbering — check against the moulded "1"/"2" marks.
fn xt30pw_2p2() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -2.5, 0.0, 1.9),
        power_tht("2", 2.5, 0.0, 1.9),
        circle_tht("3", 6.35, -1.23, 1.5, 1.0),
        circle_tht("4", 6.35, -3.23, 1.5, 1.0),
    ];
    let graphics = outlined_graphics(&pads, (-5.24, -11.66, 8.76, 1.5), (-5.6, 2.2));
    FootprintTemplate {
        name: "XT30PW(2+2)".into(),
        pads,
        graphics,
    }
}

/// AMASS XT30PB(2+2)-M (vertical, 2 power + 2 signal). AMASS drawing
/// 3099990210 "XT30(2+2)PB-M.G.B" V00 (2018-11-28), PCB layout: 2 × Ø1.90
/// power holes on 5.00 mm; two 0.60 mm square signal tails 3.85 mm beyond the
/// "−" hole, 2.00 mm apart across the power axis. Housing 13.90 × 5.20.
///
/// Pin order (from the mating-face view's moulded marks "+", "−", "1", "2"):
/// pad 1 = "+", pad 2 = "−" (the power contact next to the signal pair),
/// pad 3 = signal "1", pad 4 = signal "2". Seen from the component side with
/// the signals at +X, signal "1" is at −Y and "2" at +Y. The square tails get a
/// Ø1.0 hole (0.85 mm tail diagonal + clearance).
fn xt30pb_2p2() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -2.5, 0.0, 1.9),
        power_tht("2", 2.5, 0.0, 1.9),
        circle_tht("3", 6.35, -1.0, 1.6, 1.0),
        circle_tht("4", 6.35, 1.0, 1.6, 1.0),
    ];
    // Housing x: 3.90 outboard of the "+" hole, 13.90 long; 5.20 wide.
    let graphics = outlined_graphics(&pads, (-6.4, -2.6, 7.5, 2.6), (-6.8, -3.0));
    FootprintTemplate {
        name: "XT30PB(2+2)".into(),
        pads,
        graphics,
    }
}

/// AMASS XT60PW-M (horizontal). AMASS product spec XT60PW-M30.G.Y.T, version
/// 2025V0, PCB layout: 2 × Ø2.90 on 7.20 mm, 2 × 0.80 × 1.90 peg slots
/// 13.50 mm apart, 6.00 mm behind the power row; housing 15.50 wide, from
/// 1.80 in front of the row to 16.35 behind it.
fn xt60pw() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -3.6, 0.0, 2.9),
        power_tht("2", 3.6, 0.0, 2.9),
        npth(-6.75, -6.0, 0.8, 1.9),
        npth(6.75, -6.0, 0.8, 1.9),
    ];
    let graphics = outlined_graphics(&pads, (-7.75, -16.35, 7.75, 1.8), (-5.8, 2.6));
    FootprintTemplate {
        name: "XT60PW".into(),
        pads,
        graphics,
    }
}

/// AMASS XT60PB-M (vertical). AMASS product spec XT60PB30-M.G.*, §9 mounting
/// hole diagram: 2 × Ø4.00 on 7.20 mm; housing 16.00 × 8.10 (4.15 outboard of
/// the holes, 4.05 from the row to the housing edge).
fn xt60pb() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -3.6, 0.0, 4.0),
        power_tht("2", 3.6, 0.0, 4.0),
    ];
    let graphics = outlined_graphics(&pads, (-7.75, -4.05, 7.75, 4.05), (-8.2, -4.4));
    FootprintTemplate {
        name: "XT60PB".into(),
        pads,
        graphics,
    }
}

/// AMASS XT90PW-M (horizontal). AMASS product spec XT90PW30-M.G.*, §9
/// mounting hole dimensions: 2 × Ø3.80 on 10.90 mm, 2 × 0.60 × 1.40 peg slots
/// 18.90 mm apart, 9.40 mm behind the power row; housing 20.80 wide, from
/// 2.51 in front of the row to 24.60 behind it.
fn xt90pw() -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -5.45, 0.0, 3.8),
        power_tht("2", 5.45, 0.0, 3.8),
        npth(-9.45, -9.4, 0.6, 1.4),
        npth(9.45, -9.4, 0.6, 1.4),
    ];
    let graphics = outlined_graphics(&pads, (-10.4, -24.6, 10.4, 2.51), (-8.0, 3.2));
    FootprintTemplate {
        name: "XT90PW".into(),
        pads,
        graphics,
    }
}

/// AMASS LCB-series vertical (board-mount, 180°) 2-pin power plugs — the
/// blind-mate dock candidate. AMASS 产品规格书 2023V2 PCB layouts:
/// - LCB30PB-M: 2 × Ø3.20 on 7.60 mm, Ø1.70 peg 3.50 off the row; 17.10 × 12.20;
/// - LCB40PB-M: 2 × Ø4.20 on 8.80 mm, Ø1.70 peg 4.05 off the row; 19.65 × 13.55.
///
/// The pad 1/2 ↔ "+"/"−" assignment is not dimensioned on the drawings —
/// check the moulded marks before committing a polarity.
fn amass_lcb(
    name: &str,
    pitch: f64,
    drill: f64,
    peg_off: f64,
    body: (f64, f64),
) -> FootprintTemplate {
    let pads = vec![
        power_tht("1", -pitch / 2.0, 0.0, drill),
        power_tht("2", pitch / 2.0, 0.0, drill),
        npth(0.0, peg_off, 1.7, 1.7),
    ];
    let (bl, bw) = body;
    let graphics = outlined_graphics(
        &pads,
        (-bl / 2.0, -bw / 2.0, bl / 2.0, bw / 2.0),
        (-bl / 2.0 - 0.4, -bw / 2.0 - 0.4),
    );
    FootprintTemplate {
        name: name.into(),
        pads,
        graphics,
    }
}

/// TI DGX0019A (VSSOP-19, 3.0 × 5.1 mm body, 0.5 mm pitch) — TPS4811x-Q1.
/// TI drawing 4226944/A (07/2021), "EXAMPLE BOARD LAYOUT" in SLUSEE5E: a
/// 20-position land pattern, 20× 1.45 × 0.30 pads on 0.50 mm, rows 4.40 mm
/// apart (pad centres). Position 16 is depopulated (the HV creepage gap next
/// to VS/ISCP), so the 19 leads keep their 20-pin numbers: 1–10 down the left
/// row, 11–20 up the right row, 16 absent. No exposed pad.
fn ti_dgx19() -> FootprintTemplate {
    let (xc, pitch, w, h) = (2.2, 0.5, 1.45, 0.3);
    let mut pads = Vec::new();
    for n in 1..=20u32 {
        if n == 16 {
            continue;
        }
        let (x, i) = if n <= 10 { (-xc, n - 1) } else { (xc, 20 - n) };
        pads.push(rect_smd(&n.to_string(), x, -2.25 + pitch * i as f64, w, h));
    }
    let graphics = outlined_graphics(&pads, (-1.5, -2.55, 1.5, 2.55), (-2.2, -2.9));
    FootprintTemplate {
        name: "VSSOP-19_TI_DGX".into(),
        pads,
        graphics,
    }
}

/// SOT-563 (1.6 × 1.6 mm, 0.5 mm pitch, 6 leads), JEDEC MO-293B, IPC-7351
/// nominal as captured in KiCad 9's `SOT-563.kicad_mod`: 0.675 × 0.35 lands at
/// x = ±0.7125; pins 1–3 down the left side, 4–6 up the right.
fn sot563() -> FootprintTemplate {
    let mut pads = Vec::new();
    for n in 1..=6u32 {
        let (x, i) = if n <= 3 {
            (-0.7125, n - 1)
        } else {
            (0.7125, 6 - n)
        };
        pads.push(rect_smd(
            &n.to_string(),
            x,
            -0.5 + 0.5 * i as f64,
            0.675,
            0.35,
        ));
    }
    let graphics = outlined_graphics(&pads, (-0.8, -0.8, 0.8, 0.8), (-1.25, -0.9));
    FootprintTemplate {
        name: "SOT-563".into(),
        pads,
        graphics,
    }
}

/// Two-terminal SMD power inductor with rectangular lands at ±`cx`.
fn smd_inductor(name: &str, cx: f64, pad: (f64, f64), body: (f64, f64)) -> FootprintTemplate {
    let pads = vec![
        rect_smd("1", -cx, 0.0, pad.0, pad.1),
        rect_smd("2", cx, 0.0, pad.0, pad.1),
    ];
    let (bw, bh) = body;
    let graphics = outlined_graphics(
        &pads,
        (-bw / 2.0, -bh / 2.0, bw / 2.0, bh / 2.0),
        (-bw / 2.0 - 0.4, -bh / 2.0 - 0.4),
    );
    FootprintTemplate {
        name: name.into(),
        pads,
        graphics,
    }
}

/// Panasonic-style SMD aluminium electrolytic `CP_Elec_<D>x<L>`: land
/// (centre x, pad w × h) per can, from KiCad 9's `Capacitor_SMD/CP_Elec_*`
/// (Panasonic/Nichicon recommended land patterns). Pad 1 = +.
fn cp_elec_land(d: &str, l: &str) -> Option<(f64, f64, f64)> {
    Some(match (d, l) {
        ("3", _) => (1.5, 2.2, 1.6),
        ("4", _) => (1.8, 2.6, 1.6),
        ("5", _) => (2.2, 3.0, 1.6),
        ("6.3", "4.9" | "5.2" | "5.3" | "5.4" | "5.9" | "9.9") => (2.8, 3.5, 1.6),
        ("6.3", _) => (2.7, 3.5, 1.6),
        ("8", "5.4" | "6.2" | "6.5" | "6.7") => (3.05, 4.0, 2.5),
        ("8", "6.9" | "11.9") => (3.475, 4.15, 1.9),
        ("8", "10") => (3.25, 3.5, 2.5),
        ("8", "10.5") => (3.7, 4.4, 2.5),
        ("10", "10") => (4.0, 4.0, 2.5),
        ("10", "7.7" | "7.9" | "12.6") => (4.35, 4.4, 1.9),
        ("10", _) => (4.2, 4.4, 2.5),
        ("16", _) => (6.25, 7.8, 9.6),
        ("18", _) => (6.75, 8.8, 9.6),
        _ => return None,
    })
}

fn cp_elec(base: &str) -> Option<FootprintTemplate> {
    let rest = base.split("CP_Elec_").nth(1)?;
    let (d, tail) = rest.split_once('x')?;
    let l: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let (cx, w, h) = cp_elec_land(d, &l)?;
    let dia: f64 = d.parse().ok()?;
    let pads = vec![rect_smd("1", -cx, 0.0, w, h), rect_smd("2", cx, 0.0, w, h)];
    let r = dia / 2.0 + 0.1;
    let graphics = outlined_graphics(&pads, (-r, -r, r, r), (-r - 0.4, -r));
    Some(FootprintTemplate {
        name: format!("CP_Elec_{d}x{l}"),
        pads,
        graphics,
    })
}

/// Molex Micro-Fit 3.0 dual-row headers, 3.00 mm pitch, Ø1.02 holes.
/// - Right-angle 43045-xx00/01/02: Molex SD-43045-001 rev G — Ø3.00 locating
///   pegs 4.32 mm in front of the circuit-1 row (one centred peg up to 6
///   circuits; two at ±(B − 4.30)/2 from 8 up, B = (cols − 1)·3).
/// - Vertical 43045-xx12/13/14: Molex SD-43045-009 rev H4 — two Ø1.02
///   polarization pegs 0.94 mm beyond the far row, B + 6.00 apart.
///
/// Numbering per both drawings (component side, rotated 180° so circuit 1 is
/// top-left): circuits 1…n/2 along the first row, n/2+1…n along the second,
/// each below its partner. Pin 1 is the square pad.
fn microfit_dual(cols: u32, vertical: bool) -> FootprintTemplate {
    let mut pads = Vec::new();
    let span = (cols as f64 - 1.0) * 3.0; // Molex "B"
    let x0 = -span / 2.0;
    for row in 0..2u32 {
        for c in 0..cols {
            let num = (row * cols + c + 1).to_string();
            let (x, y) = (x0 + c as f64 * 3.0, row as f64 * 3.0);
            pads.push(if num == "1" {
                rect_tht(&num, x, y, 1.9, 1.9, 1.02)
            } else {
                circle_tht(&num, x, y, 1.9, 1.02)
            });
        }
    }
    if vertical {
        let c = span + 6.0;
        pads.push(npth(-c / 2.0, 3.94, 1.02, 1.02));
        pads.push(npth(c / 2.0, 3.94, 1.02, 1.02));
    } else if cols <= 3 {
        pads.push(npth(0.0, -4.32, 3.0, 3.0));
    } else {
        let c = span - 4.30;
        pads.push(npth(-c / 2.0, -4.32, 3.0, 3.0));
        pads.push(npth(c / 2.0, -4.32, 3.0, 3.0));
    }
    // Housing "A" = B + 6.65 wide; 7.37 deep around the two rows.
    let a = span + 6.65;
    let graphics = outlined_graphics(
        &pads,
        (-a / 2.0, -2.19, a / 2.0, 5.19),
        (-a / 2.0 - 0.4, -2.6),
    );
    FootprintTemplate {
        name: format!(
            "Micro-Fit_2x{cols:02}_{}",
            if vertical { "Vertical" } else { "Horizontal" }
        ),
        pads,
        graphics,
    }
}

fn match_power_family(base: &str) -> Option<(FootprintTemplate, &'static str)> {
    if base.contains("HSOF-8") || base.contains("TOLL") {
        return Some((toll(), "TOLL"));
    }
    if base.contains("TSDSON-8") {
        return Some((tsdson8(), "PowerSON8"));
    }
    if base.contains("TDSON-8") || base.contains("SuperSO8") {
        return Some((tdson8(), "PowerSON8"));
    }
    if base.contains("TO-277") || base.contains("SMPC") {
        return Some((to277a(), "TO-277A"));
    }
    if base.contains("BMI088") {
        return Some((bmi088(), "LGA-IMU"));
    }
    if base.contains("ICM-42688") || base.contains("ICM42688") {
        return Some((icm42688(), "LGA-IMU"));
    }
    if base.contains("TI_DGX") || base.contains("VSSOP-19") {
        return Some((ti_dgx19(), "TI-DGX"));
    }
    if base.contains("SOT-563") {
        return Some((sot563(), "SOT-563"));
    }
    // Bourns SRR1260 (12.5 × 12.5): bourns.com SRR1260 datasheet land
    // pattern as captured in KiCad 9's `L_Bourns_SRR1260.kicad_mod`.
    if base.contains("SRR1260") {
        return Some((
            smd_inductor("L_Bourns_SRR1260", 4.85, (2.9, 5.4), (12.5, 12.5)),
            "SMD-Inductor",
        ));
    }
    // Changjiang FXL1365 (12.9 × 12.6 × 6.5): cjiang FXL datasheet land
    // pattern as captured in KiCad 9's `L_Changjiang_FXL1365.kicad_mod`.
    if base.contains("FXL1365") {
        return Some((
            smd_inductor("L_Changjiang_FXL1365", 5.625, (3.25, 5.5), (12.9, 12.6)),
            "SMD-Inductor",
        ));
    }
    if base.contains("CP_Elec_") {
        if let Some(t) = cp_elec(base) {
            return Some((t, "CP_Elec"));
        }
    }
    if base.contains("XT30PW") && base.contains("2+2") {
        return Some((xt30pw_2p2(), "AMASS-XT"));
    }
    if base.contains("XT30") && base.contains("2+2") {
        return Some((xt30pb_2p2(), "AMASS-XT"));
    }
    if base.contains("XT30PW") {
        return Some((xt30pw(), "AMASS-XT"));
    }
    if base.contains("XT60PW") {
        return Some((xt60pw(), "AMASS-XT"));
    }
    if base.contains("XT60PB") || base.contains("XT60UPB") {
        return Some((xt60pb(), "AMASS-XT"));
    }
    if base.contains("XT90PW") {
        return Some((xt90pw(), "AMASS-XT"));
    }
    if base.contains("LCB30PB") {
        return Some((
            amass_lcb("LCB30PB", 7.6, 3.2, 3.5, (17.1, 12.2)),
            "AMASS-LCB",
        ));
    }
    if base.contains("LCB40PB") {
        return Some((
            amass_lcb("LCB40PB", 8.8, 4.2, 4.05, (19.65, 13.55)),
            "AMASS-LCB",
        ));
    }
    if base.contains("Micro-Fit") || base.contains("MicroFit") {
        let cols = rows_cols(base)
            .map(|(r, c)| if r == 2 { c } else { (r * c).div_ceil(2) })
            .unwrap_or(2);
        // 43045-xx12/13/14 are the vertical headers; xx00…xx10 right-angle.
        let vertical = base.contains("Vertical")
            || base
                .split("43045-")
                .nth(1)
                .and_then(|s| s.get(2..4))
                .is_some_and(|v| matches!(v, "12" | "13" | "14"));
        return Some((microfit_dual(cols.max(1), vertical), "Micro-Fit"));
    }
    None
}

fn match_family(base: &str, pin_count: u32) -> Option<(FootprintTemplate, &'static str)> {
    if let Some(hit) = match_power_family(base) {
        return Some(hit);
    }
    // --- Chip passives (imperial + metric synonyms) ------------------------
    // Bounded match only: a bare code must be `_`/boundary-delimited so an
    // embedded run (MAX2512, BGA-1206, ...) doesn't get mistaken for a chip.
    for (token, code) in [
        ("0201", "0201"),
        ("0402", "0402"),
        ("0603", "0603"),
        ("0805", "0805"),
        ("1206", "1206"),
        ("1210", "1210"),
        ("2010", "2010"),
        ("2512", "2512"),
        ("1008", "1008"),
        ("2520Metric", "1008"),
        ("1005Metric", "0402"),
        ("1608Metric", "0603"),
        ("2012Metric", "0805"),
        ("3216Metric", "1206"),
        ("3225Metric", "1210"),
        ("5025Metric", "2010"),
        ("6332Metric", "2512"),
    ] {
        if delimited_contains(base, token) {
            return Some((chip(code), "Chip"));
        }
    }

    // --- SMD crystals -------------------------------------------------------
    // Keyed on the `Crystal` token only (a bare `Xtal` in a vendor id stays in
    // the generic fallback). 2- vs 4-pad decided by an explicit `-2Pin`/`-4Pin`
    // marker, then the size code, then the declared pin count.
    if base.contains("Crystal") {
        // (code, body_w, body_h, four_pad_default)
        let by_code = [
            ("3225", 3.2, 2.5, true),
            ("2520", 2.5, 2.0, true),
            ("2016", 2.0, 1.6, true),
            ("1612", 1.6, 1.2, true),
            ("7050", 7.0, 5.0, false),
            ("6035", 6.0, 3.5, false),
            ("5032", 5.0, 3.2, false),
        ]
        .into_iter()
        .find(|(code, ..)| base.contains(code));
        let (code, mut body_w, mut body_h, four_default) = by_code.unwrap_or({
            if pin_count == 4 {
                ("3225", 3.2, 2.5, true)
            } else {
                ("5032", 5.0, 3.2, false)
            }
        });
        if let Some((w, h)) = body_mm(base) {
            body_w = w;
            body_h = h;
        }
        let four = if base.contains("-2Pin") || base.contains("_2Pin") {
            false
        } else if base.contains("-4Pin") || base.contains("_4Pin") {
            true
        } else if pin_count == 2 || pin_count == 4 {
            pin_count == 4
        } else {
            four_default
        };
        let fp = if four {
            crystal_4pad(code, body_w, body_h)
        } else {
            crystal_2pad(code, body_w, body_h)
        };
        return Some((fp, "Crystal"));
    }

    // --- Screw terminals / terminal blocks ---------------------------------
    if base.contains("TerminalBlock") || base.contains("Screw_Terminal") {
        let positions = rows_cols(base)
            .map(|(r, c)| r * c)
            .filter(|&n| n > 0)
            .unwrap_or(pin_count.max(2));
        let pitch = float_after(base, "_P").unwrap_or(5.08);
        return Some((screw_terminal(positions, pitch), "ScrewTerminal"));
    }

    // --- Pin headers / sockets ---------------------------------------------
    if base.contains("PinHeader")
        || base.contains("PinSocket")
        || base.contains("Socket_Strip")
        || base.contains("IDC-Header")
    {
        if let Some((rows, cols)) = rows_cols(base) {
            if rows >= 1 && cols >= 1 {
                let pitch = float_after(base, "_P").unwrap_or(2.54);
                return Some((pin_header(rows, cols, pitch), "PinHeader"));
            }
        }
    }

    // --- Radial electrolytic capacitor -------------------------------------
    if base.contains("CP_Radial") || base.starts_with("C_Radial") {
        let pitch = float_after(base, "_P").unwrap_or(5.0);
        let body = float_after(base, "_D").unwrap_or(6.3);
        return Some((radial_electrolytic(pitch, body), "Electrolytic"));
    }

    // --- USB micro-B receptacle --------------------------------------------
    // Matched before USB-C so "Micro" wins even if a vendor id also carries a
    // generic USB token.
    if base.contains("USB_Micro")
        || base.contains("USB-Micro")
        || base.contains("Micro-B")
        || base.contains("Micro_B")
        || base.contains("MicroUSB")
        || base.contains("Micro_USB")
    {
        return Some((usb_micro_b(), "USB-Micro-B"));
    }

    // --- USB-C receptacle (simplified) -------------------------------------
    // Matched before the IC families so a "..._SO..."-ish vendor token can't
    // hijack it; geometry is approximate (see `usb_c`).
    if base.contains("USB_C")
        || base.contains("USB-C")
        || base.contains("Type-C")
        || base.contains("TYPE-C")
        || base.contains("TypeC")
    {
        // KiCad USB-C ids don't carry a clean contact count, so trust the
        // declared pin count; default to the full 24-contact receptacle.
        let signal = if pin_count >= 2 { pin_count } else { 24 };
        return Some((usb_c(signal), "USB-C"));
    }

    // --- Tag-Connect spring-pin programming pads ---------------------------
    if base.contains("Tag-Connect")
        || base.contains("TagConnect")
        || base.contains("TC2030")
        || base.contains("TC2050")
    {
        let pins = rows_cols(base)
            .map(|(r, c)| r * c)
            .filter(|&n| n >= 2)
            .or(if base.contains("TC2050") {
                Some(10)
            } else if base.contains("TC2030") {
                Some(6)
            } else {
                None
            })
            .unwrap_or(pin_count.max(6));
        return Some((tag_connect(pins), "Tag-Connect"));
    }

    // --- Single-row wire-to-board: JST PH/XH/EH (THT), SH/GH (SMD) ----------
    for (marker, count_marker, pitch_default, pad, label) in [
        // JST PH posts are ~0.5mm round, XH/EH ~0.6mm — drill the IPC gap over
        // the lead (0.7 / 0.8mm) instead of a flat oversized 1.0mm, which had
        // hole edges crowding neighbours once the placer packed connectors in.
        (
            "JST_PH",
            "JST_PH_",
            2.0,
            ConnPad::Tht {
                pad: tht_pad(tht_drill(0.5), 0.3),
                drill: tht_drill(0.5),
            },
            "JST-PH",
        ),
        (
            "JST_XH",
            "JST_XH_",
            2.5,
            ConnPad::Tht {
                pad: tht_pad(tht_drill(0.6), 0.3),
                drill: tht_drill(0.6),
            },
            "JST-XH",
        ),
        (
            "JST_EH",
            "JST_EH_",
            2.5,
            ConnPad::Tht {
                pad: tht_pad(tht_drill(0.6), 0.3),
                drill: tht_drill(0.6),
            },
            "JST-EH",
        ),
        (
            "JST_SH",
            "JST_SH_",
            1.0,
            ConnPad::Smd { w: 0.6, h: 1.55 },
            "JST-SH",
        ),
        (
            "JST_GH",
            "JST_GH_",
            1.25,
            ConnPad::Smd { w: 0.7, h: 1.55 },
            "JST-GH",
        ),
    ] {
        if base.contains(marker) {
            let pins = connector_pins(base, count_marker, pin_count).max(2);
            let pitch = float_after(base, "_P").unwrap_or(pitch_default);
            return Some((
                linear_connector(format!("{label}-{pins}"), pins, pitch, pad),
                label,
            ));
        }
    }

    // --- Molex Pico-Blade (53048 / 53261), 1.25 mm SMD ---------------------
    // No bare-count marker — the part number after the family token is digits,
    // so size off the `RxC` token or the declared count (never `uint_after`).
    if base.contains("PicoBlade")
        || base.contains("Pico-Blade")
        || base.contains("53048")
        || base.contains("53261")
    {
        let pins = rows_cols(base)
            .map(|(r, c)| r * c)
            .filter(|&n| n >= 2)
            .unwrap_or(pin_count.max(2));
        let pitch = float_after(base, "_P").unwrap_or(1.25);
        return Some((
            linear_connector(
                format!("Molex-PicoBlade-{pins}"),
                pins,
                pitch,
                ConnPad::Smd { w: 0.8, h: 1.6 },
            ),
            "Molex-PicoBlade",
        ));
    }

    // --- SOT family (SOT-223 before SOT-23 due to substring overlap) -------
    if base.contains("SOT-223") {
        // N-1 signal leads + 1 tab pad. The classic 3-lead part is 4 pads
        // (leads 1-3 + tab 4); SOT-223-5 / -8 variants exist, so size off the
        // name's "-N" (KiCad counts leads there) or the declared pin count
        // rather than hardcoding 4 — otherwise extra pins are silently dropped.
        let named = family_pins(base, &["SOT-223-"], 0);
        let total = if named >= 4 {
            named // e.g. SOT-223-5 → 5 pads, -8 → 8 pads
        } else {
            pin_count.max(4) // classic 4-pad, or honor a larger declared count
        };
        let leads: Vec<u32> = (1..total).collect();
        return Some((
            tab_package(
                "SOT-223".to_string(),
                &leads,
                total,
                2.3,
                (1.0, 1.5),
                (3.5, 1.5),
                false,
            ),
            "SOT-223",
        ));
    }
    if base.contains("SOT-23") || base.contains("TSOT-23") {
        let pins = family_pins(base, &["SOT-23-", "TSOT-23-"], 3).max(3);
        let name = if pins == 3 {
            "SOT-23".to_string()
        } else {
            format!("SOT-23-{pins}")
        };
        return Some((sot_dual(name, pins, 0.95, 1.1, 0.6, 0.9), "SOT-23"));
    }
    if base.contains("SC-70") || base.contains("SOT-353") || base.contains("SOT-363") {
        let pins = family_pins(base, &["SOT-353-", "SOT-363-", "SC-70-"], 5).max(3);
        return Some((
            sot_dual(format!("SC-70-{pins}"), pins, 0.65, 0.95, 0.4, 0.7),
            "SC-70",
        ));
    }
    if base.contains("SOT-89") {
        return Some((
            tab_package(
                "SOT-89".to_string(),
                &[1, 2, 3],
                2,
                1.5,
                (0.6, 1.0),
                (1.8, 1.2),
                false,
            ),
            "SOT-89",
        ));
    }

    // --- Two-terminal SMD diodes / DO-214 ----------------------------------
    if base.contains("SOD-123") {
        return Some((two_pad("SOD-123".to_string(), 3.6, 0.9, 1.2), "SOD"));
    }
    if base.contains("SOD-323") || base.contains("SOD-523") {
        return Some((two_pad("SOD-323".to_string(), 2.3, 0.9, 0.6), "SOD"));
    }
    if base.contains("SOD-882") {
        return Some((two_pad("SOD-882".to_string(), 1.0, 0.6, 0.6), "SOD"));
    }
    if base.contains("D_SMA") || base.contains("DO-214AC") {
        return Some((two_pad("SMA".to_string(), 4.6, 1.5, 1.6), "DO-214"));
    }
    if base.contains("D_SMB") || base.contains("DO-214AA") {
        return Some((two_pad("SMB".to_string(), 5.0, 2.1, 2.3), "DO-214"));
    }
    if base.contains("D_SMC") || base.contains("DO-214AB") {
        return Some((two_pad("SMC".to_string(), 7.0, 2.2, 3.2), "DO-214"));
    }

    // --- QFN / DFN / SON (no-lead, with thermal pad) -----------------------
    if base.contains("QFN") || base.contains("DHVQFN") {
        let pins = family_pins(
            base,
            &["QFN-", "VQFN-", "UQFN-", "WQFN-", "TQFN-", "DHVQFN-"],
            pin_count,
        );
        if pins >= 4 {
            // Round down to a multiple of 4 so all four sides match.
            let pins = pins - (pins % 4);
            let pitch = float_after(base, "_P").unwrap_or(0.5);
            let body = body_mm(base)
                .map(|(w, _)| w)
                .unwrap_or((pins / 4) as f64 * pitch + 1.0);
            let ep = (body * 0.55).max(1.0);
            // Prefer the unified parametric generator (one PackageClass →
            // footprint + symbol + 3D body in one pass); fall back to the
            // legacy table only if derivation ever fails.
            let template = qfn_via_derive(pins, pitch, body, ep).unwrap_or_else(|| {
                let row_center = body / 2.0 - 0.25;
                quad(
                    format!("QFN-{pins}"),
                    pins,
                    pitch,
                    row_center,
                    0.5,
                    0.3,
                    Some(ep),
                )
            });
            return Some((template, "QFN"));
        }
    }
    if base.contains("DFN-") || base.contains("SON-") || base.contains("WSON") {
        let pins = family_pins(base, &["DFN-", "SON-", "WSON-"], pin_count).max(2);
        let pins = if pins.is_multiple_of(2) {
            pins
        } else {
            pins + 1
        };
        let pitch = float_after(base, "_P").unwrap_or(0.5);
        let body = body_mm(base).map(|(w, _)| w).unwrap_or(2.0);
        // Dual no-lead: pads on left & right, tight.
        let mut fp = dual_lr(format!("DFN-{pins}"), pins, pitch, body, 0.45, 0.25);
        fp.pads.push(rect_smd(
            "EP",
            0.0,
            0.0,
            (body * 0.5).max(0.8),
            pins as f64 / 2.0 * pitch * 0.6,
        ));
        return Some((fp, "DFN"));
    }

    // --- QFP / LQFP / TQFP / PQFP ------------------------------------------
    if base.contains("QFP") {
        let pins = family_pins(
            base,
            &["QFP-", "LQFP-", "TQFP-", "PQFP-", "CQFP-"],
            pin_count,
        );
        if pins >= 8 {
            let pins = pins - (pins % 4);
            let pitch = float_after(base, "_P").unwrap_or(0.5);
            let body = body_mm(base)
                .map(|(w, _)| w)
                .unwrap_or((pins / 4) as f64 * pitch + 2.0);
            let row_center = body / 2.0 + 0.75;
            return Some((
                quad(
                    format!("QFP-{pins}"),
                    pins,
                    pitch,
                    row_center,
                    1.5,
                    0.3,
                    None,
                ),
                "QFP",
            ));
        }
    }

    // --- HTSSOP / TSSOP-EP (PowerPad: exposed thermal pad) ------------------
    // Must run before the plain SOIC/TSSOP loop, which would otherwise match
    // the HTSSOP token and silently drop the exposed pad — the pad a motor
    // driver like the DRV8833PWP dumps its heat into. The EP size is parsed
    // from KiCad's `_EP<w>x<h>mm` token when present, else defaulted off the
    // body. The board still owes the EP a thermal via field (board-level, not
    // part of the land pattern).
    let has_ep_marker = base.contains("-1EP") || base.contains("_EP");
    if base.contains("HTSSOP")
        || base.contains("PowerPAD")
        || base.contains("PowerPad")
        || (base.contains("TSSOP") && has_ep_marker)
    {
        let pins = family_pins(base, &["HTSSOP-", "TSSOP-"], pin_count).max(4);
        let pins = if pins.is_multiple_of(2) {
            pins
        } else {
            pins + 1
        };
        let pitch = float_after(base, "_P").unwrap_or(0.65);
        let body_w = body_mm(base).map(|(w, _)| w).unwrap_or(4.4);
        let mut fp = dual_lr(format!("HTSSOP-{pins}"), pins, pitch, body_w, 1.0, 0.3);
        let (ep_w, ep_h) = base
            .find("_EP")
            .and_then(|i| body_mm(&base[i + 3..]))
            .unwrap_or((
                (body_w * 0.55).max(1.5),
                (pins as f64 / 2.0 * pitch * 0.55).max(1.5),
            ));
        fp.pads.push(rect_smd("EP", 0.0, 0.0, ep_w, ep_h));
        return Some((fp, "HTSSOP"));
    }

    // --- SOIC / SO / SOP / SSOP / TSSOP / MSOP / VSSOP ----------------------
    // Longest markers first so TSSOP isn't shadowed by SOP/SO.
    for (marker, pitch, body_w, lead_len, lead_wid, label) in [
        ("TSSOP-", 0.65, 4.4, 1.0, 0.3, "TSSOP"),
        ("VSSOP-", 0.5, 3.0, 0.9, 0.3, "VSSOP"),
        ("MSOP-", 0.65, 3.0, 0.9, 0.3, "MSOP"),
        ("QSOP-", 0.635, 3.9, 1.2, 0.4, "QSOP"),
        ("SSOP-", 0.65, 5.3, 1.3, 0.4, "SSOP"),
        ("SOIC-", 1.27, 3.9, 1.55, 0.6, "SOIC"),
        ("SOP-", 1.27, 3.9, 1.55, 0.6, "SOP"),
        ("SO-", 1.27, 3.9, 1.55, 0.6, "SOIC"),
    ] {
        if let Some(pins) = uint_after(base, marker) {
            if pins >= 4 && pins.is_multiple_of(2) {
                let mut fp = dual_lr(
                    format!("SOIC-{pins}"),
                    pins,
                    pitch,
                    body_w,
                    lead_len,
                    lead_wid,
                );
                // KiCad `-1EP` packages (MSOP-8-1EP, TI_SO-8-1EP, ...): the
                // exposed pad is numbered pins + 1 (KiCad's convention) and
                // sized from the `_EP<w>x<h>mm` token. Dropping it silently
                // lost the thermal/GND pad of LDOs and buck regulators.
                if base.contains("-1EP") {
                    if let Some((ep_w, ep_h)) =
                        base.find("_EP").and_then(|i| body_mm(&base[i + 3..]))
                    {
                        fp.pads
                            .push(rect_smd(&(pins + 1).to_string(), 0.0, 0.0, ep_w, ep_h));
                    }
                }
                return Some((fp, label));
            }
        }
    }

    // --- DIP / PDIP --------------------------------------------------------
    for marker in ["DIP-", "PDIP-"] {
        if let Some(pins) = uint_after(base, marker) {
            if pins >= 4 && pins.is_multiple_of(2) {
                return Some((dip(pins), "DIP"));
            }
        }
    }

    // --- Power tab packages: DPAK / D2PAK (TO-252 / TO-263), TO-220/247 -----
    if base.contains("TO-252") || base.contains("DPAK") {
        let pins = family_pins(base, &["TO-252-", "DPAK-"], 3).max(2);
        let tab = uint_after(base, "TabPin").unwrap_or(2).min(pins);
        let leads: Vec<u32> = (1..=pins).filter(|&n| n != tab).collect();
        return Some((
            tab_package(
                "TO-252".to_string(),
                &leads,
                tab,
                2.28,
                (0.9, 1.6),
                (5.5, 6.1),
                false,
            ),
            "DPAK",
        ));
    }
    if base.contains("TO-263") || base.contains("D2PAK") || base.contains("DDPAK") {
        let pins = family_pins(base, &["TO-263-", "D2PAK-", "DDPAK-"], 3).max(2);
        let tab = uint_after(base, "TabPin").unwrap_or(2).min(pins);
        let leads: Vec<u32> = (1..=pins).filter(|&n| n != tab).collect();
        return Some((
            tab_package(
                "TO-263".to_string(),
                &leads,
                tab,
                2.54,
                (1.4, 2.0),
                (9.0, 9.0),
                false,
            ),
            "D2PAK",
        ));
    }
    if base.contains("TO-220") || base.contains("TO-247") {
        let pins = family_pins(base, &["TO-220-", "TO-247-"], 3).max(2);
        let tab = uint_after(base, "TabPin").unwrap_or(pins).min(pins);
        let leads: Vec<u32> = (1..=pins).filter(|&n| n != tab).collect();
        return Some((
            tab_package(
                "TO-220".to_string(),
                &leads,
                tab,
                2.54,
                (1.2, 2.0),
                (4.0, 2.0),
                true,
            ),
            "TO-220",
        ));
    }

    None
}

// ============================================================================
// Public API
// ============================================================================

/// Resolve a KiCad-style footprint id into a land pattern + resolution status.
///
/// Parses the package family, pin count, pitch, and body size out of `name`
/// and synthesizes the matching pads. `pin_count` (the caller's declared pin
/// count) is used only when the id itself carries no count, and as the basis
/// for the generic fallback. See the module docs for supported families.
pub fn resolve_footprint(name: &str, pin_count: u32) -> FootprintResolution {
    let base = name.rsplit(':').next().unwrap_or(name);

    if let Some((template, family)) = match_family(base, pin_count) {
        return FootprintResolution {
            note: format!("matched {family} family ({} pads)", template.pads.len()),
            template: Some(template),
            matched: true,
            family: Some(family.to_string()),
        };
    }

    // Generic fallback — keyed off pin count so pads never stack and the part
    // stays on the board, but flagged so the caller can warn.
    match pin_count {
        0 => FootprintResolution {
            template: None,
            matched: false,
            family: None,
            note: format!("unrecognized footprint '{base}' and no pins to synthesize from"),
        },
        1 | 2 => FootprintResolution {
            template: Some(chip("0805")),
            matched: false,
            family: None,
            note: format!(
                "unrecognized footprint '{base}'; substituted a 0805 chip placeholder. \
                 Pass a recognized KiCad footprint id (chip/SOIC/QFN/QFP/HTSSOP/SOT/DPAK/crystal/\
                 JST/Molex/USB-Micro-B/USB-C/Tag-Connect/pin-header/screw-terminal) or \
                 inline `pads` geometry to fix."
            ),
        },
        n => FootprintResolution {
            template: Some(grid_fallback(n)),
            matched: false,
            family: None,
            note: format!(
                "unrecognized footprint '{base}'; substituted a compact {n}-pad grid placeholder. \
                 Pass a recognized KiCad footprint id (chip/SOIC/QFN/QFP/HTSSOP/SOT/DPAK/crystal/\
                 JST/Molex/USB-Micro-B/USB-C/Tag-Connect/pin-header/screw-terminal) or \
                 inline `pads` geometry to fix."
            ),
        },
    }
}

/// Resolve a footprint id to a land pattern, discarding the resolution status.
///
/// Back-compatible thin wrapper over [`resolve_footprint`]; returns `None` only
/// when the id is unrecognized and `pin_count` is zero.
pub fn footprint_for_name(name: &str, pin_count: u32) -> Option<FootprintTemplate> {
    resolve_footprint(name, pin_count).template
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions_unique(fp: &FootprintTemplate) -> usize {
        let mut p: Vec<(i64, i64)> = fp
            .pads
            .iter()
            .map(|pad| {
                (
                    (pad.position.x * 1000.0) as i64,
                    (pad.position.y * 1000.0) as i64,
                )
            })
            .collect();
        p.sort();
        p.dedup();
        p.len()
    }

    /// Pad bounding-box half-extents (w/2, h/2) for an overlap test.
    fn pad_half(p: &Pad) -> (f64, f64) {
        match p.shape {
            PadShape::Circle { diameter } => (diameter / 2.0, diameter / 2.0),
            PadShape::Rect { width, height }
            | PadShape::Oval { width, height }
            | PadShape::RoundRect { width, height, .. } => (width / 2.0, height / 2.0),
            PadShape::Custom { .. } => (0.5, 0.5),
        }
    }

    /// No two copper lands in a footprint may physically overlap. The
    /// `positions_unique` check only catches *coincident centres*; a pad taller
    /// than its pitch still has a distinct centre yet overlaps its neighbour and
    /// surfaces as a 0 mm clearance in DRC. AABB-disjoint (touching is allowed).
    fn assert_no_pad_overlap(fp: &FootprintTemplate) {
        for (i, a) in fp.pads.iter().enumerate() {
            for b in fp.pads.iter().skip(i + 1) {
                let (ax, ay) = pad_half(a);
                let (bx, by) = pad_half(b);
                let dx = (a.position.x - b.position.x).abs();
                let dy = (a.position.y - b.position.y).abs();
                let overlap = dx < ax + bx - 1e-6 && dy < ay + by - 1e-6;
                assert!(
                    !overlap,
                    "pads {} and {} overlap in {}",
                    a.number, b.number, fp.name
                );
            }
        }
    }

    /// No pad may sit outside a generous bounding box for its package — this is
    /// the regression guard for the off-board-rail bug.
    fn assert_on_board(fp: &FootprintTemplate, max_extent: f64) {
        for pad in &fp.pads {
            assert!(
                pad.position.x.abs() <= max_extent && pad.position.y.abs() <= max_extent,
                "pad {} at ({:.2},{:.2}) exceeds extent {max_extent} in {}",
                pad.number,
                pad.position.x,
                pad.position.y,
                fp.name
            );
        }
    }

    #[test]
    fn qfn_resolves_with_thermal_and_stays_compact() {
        let r = resolve_footprint("Package_DFN_QFN:QFN-40_5x5mm_P0.4mm", 40);
        assert!(r.matched, "QFN must be a real match, got {:?}", r);
        assert_eq!(r.family.as_deref(), Some("QFN"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 41, "40 leads + 1 thermal EP");
        // 40 lead positions + EP center are all distinct.
        assert_eq!(positions_unique(&fp), 41);
        // 5x5 body → everything well within a few mm. This is the bug fix:
        // the old fallback produced a ~74mm column for this exact part.
        assert_on_board(&fp, 4.0);
    }

    #[test]
    fn qfn_geometry_comes_from_parametric_generator() {
        // Locks the P1 wiring: the QFN land is produced by the unified
        // generator, not the legacy table. Fine-pitch lands are narrow
        // (< pitch, avoiding bridging) and project a toe beyond the body edge.
        let r = resolve_footprint("Package_DFN_QFN:QFN-40_5x5mm_P0.4mm", 40);
        let fp = r.template.unwrap();
        let lead1 = fp.pads.iter().find(|p| p.number == "1").unwrap();
        if let PadShape::Rect { width, height } = lead1.shape {
            // Left-side pad: radial (width) is the long axis, tangential
            // (height) the narrow one.
            assert!(height < 0.4, "tangential land {height} must be < 0.4 pitch");
            let outer_edge = lead1.position.x - width / 2.0;
            assert!(
                outer_edge < -2.5,
                "land outer edge {outer_edge} should project past the 5mm body edge"
            );
        } else {
            panic!("expected rect pad");
        }
        assert!(
            fp.pads.iter().any(|p| p.number == "EP"),
            "exposed pad present"
        );
    }

    #[test]
    fn qfn_with_fewer_declared_pins_uses_name_count() {
        // The ESC gate driver: id says QFN-40 but only 30 pins were declared.
        let r = resolve_footprint("Package_DFN_QFN:QFN-40_5x5mm_P0.4mm", 30);
        let fp = r.template.unwrap();
        assert_eq!(
            fp.pads.len(),
            41,
            "package count (40) wins over declared 30"
        );
        assert_on_board(&fp, 4.0);
    }

    #[test]
    fn dpak_tab_is_pin2() {
        // TO-252-3_TabPin2 = MOSFET: gate(1)/source(3) small leads, drain tab=2.
        let r = resolve_footprint("Package_TO_SOT_SMD:TO-252-3_TabPin2", 3);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("DPAK"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 3);
        let nums: Vec<&str> = fp.pads.iter().map(|p| p.number.as_str()).collect();
        assert!(nums.contains(&"1") && nums.contains(&"2") && nums.contains(&"3"));
        // Tab (pad 2) is the largest pad.
        let tab = fp.pads.iter().find(|p| p.number == "2").unwrap();
        let area = |p: &Pad| match p.shape {
            PadShape::Rect { width, height } => width * height,
            PadShape::Circle { diameter } => diameter * diameter,
            _ => 0.0,
        };
        let max = fp.pads.iter().map(area).fold(0.0, f64::max);
        assert_eq!(area(tab), max, "tab must be the largest pad");
        assert_on_board(&fp, 8.0);
    }

    #[test]
    fn lqfp48_quad_resolves() {
        let r = resolve_footprint("Package_QFP:LQFP-48_7x7mm_P0.5mm", 48);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("QFP"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 48);
        assert_eq!(positions_unique(&fp), 48);
        assert_on_board(&fp, 8.0);
    }

    #[test]
    fn screw_terminal_resolves() {
        let r = resolve_footprint("Connector:Screw_Terminal_01x02_P5.08mm", 2);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("ScrewTerminal"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 2);
        assert!(fp.pads.iter().all(|p| p.pad_type == PadType::THT));
    }

    #[test]
    fn radial_cap_resolves() {
        let r = resolve_footprint("Capacitor_THT:CP_Radial_D10.0mm_P5.00mm", 2);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("Electrolytic"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 2);
        // Pads 5mm apart.
        let dx = (fp.pads[0].position.x - fp.pads[1].position.x).abs();
        assert!((dx - 5.0).abs() < 0.01, "pitch should be ~5mm, got {dx}");
    }

    #[test]
    fn sot23_variants() {
        for (id, n) in [
            ("Package_TO_SOT_SMD:SOT-23", 3),
            ("Package_TO_SOT_SMD:SOT-23-5", 5),
            ("Package_TO_SOT_SMD:SOT-23-6", 6),
        ] {
            let r = resolve_footprint(id, n);
            assert!(r.matched, "{id} should match");
            let fp = r.template.unwrap();
            assert_eq!(fp.pads.len(), n as usize, "{id}");
            assert_eq!(
                positions_unique(&fp),
                n as usize,
                "{id} pads must not stack"
            );
            assert_on_board(&fp, 3.0);
        }
    }

    #[test]
    fn tssop_not_shadowed_by_sop() {
        let r = resolve_footprint("Package_SO:TSSOP-20_4.4x6.5mm_P0.65mm", 20);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("TSSOP"));
        assert_eq!(r.template.unwrap().pads.len(), 20);
    }

    #[test]
    fn sod_diode_resolves() {
        let r = resolve_footprint("Diode_SMD:D_SOD-123", 2);
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("SOD"));
        assert_eq!(r.template.unwrap().pads.len(), 2);
    }

    #[test]
    fn unknown_part_compact_grid_not_off_board() {
        // The regression: a 30-pin unknown id must NOT become a 74mm column.
        let r = resolve_footprint("Mystery:WeirdPart", 30);
        assert!(!r.matched, "unknown id must be flagged unmatched");
        assert!(r.family.is_none());
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 30);
        assert_eq!(positions_unique(&fp), 30);
        // sqrt(30)≈6 cols × 5 rows × 2.54 ≈ 12.7mm wide, 10.2mm tall.
        assert_on_board(&fp, 10.0);
    }

    #[test]
    fn fallback_contract_preserved() {
        // Back-compat with the old footprint_for_name fallback behavior.
        assert_eq!(footprint_for_name("Mystery:Part", 2).unwrap().pads.len(), 2);
        assert_eq!(
            footprint_for_name("Mystery:Part5", 5).unwrap().pads.len(),
            5
        );
        assert!(footprint_for_name("Mystery:Unknowable", 0).is_none());
    }

    #[test]
    fn legacy_kicad_names_still_resolve() {
        assert_eq!(
            footprint_for_name("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm", 8)
                .unwrap()
                .name,
            "SOIC-8"
        );
        assert_eq!(
            footprint_for_name("Resistor_SMD:R_0805_2012Metric", 2)
                .unwrap()
                .name,
            "0805"
        );
        assert_eq!(
            footprint_for_name(
                "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
                2
            )
            .unwrap()
            .name,
            "PinHeader_1x2"
        );
        assert_eq!(
            footprint_for_name("Package_DIP:DIP-14_W7.62mm", 14)
                .unwrap()
                .name,
            "DIP-14"
        );
        assert_eq!(
            footprint_for_name("Package_QFP:LQFP-32_7x7mm_P0.8mm", 32)
                .unwrap()
                .name,
            "QFP-32"
        );
        assert_eq!(
            footprint_for_name("Package_TO_SOT_SMD:SOT-23", 3)
                .unwrap()
                .name,
            "SOT-23"
        );
        assert_eq!(
            footprint_for_name("Package_TO_SOT_SMD:SOT-223-3_TabPin2", 4)
                .unwrap()
                .name,
            "SOT-223"
        );
    }

    #[test]
    fn header_pitch_parsed() {
        let r = resolve_footprint("Connector:PinHeader_1x05_P2.54mm", 5);
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 5);
        assert_eq!(positions_unique(&fp), 5);
    }

    #[test]
    fn chip_token_does_not_hijack_other_families() {
        // A part number embedding "2512" must NOT be read as a 2512 chip — the
        // SOT-23 marker wins (regression: this used to return a 2-pad chip).
        let r = resolve_footprint("Custom:MAX2512_SOT-23-6", 6);
        assert_eq!(r.family.as_deref(), Some("SOT-23"));
        assert_eq!(r.template.unwrap().pads.len(), 6);

        // "BGA-1206" must not become a 1206 chip; BGA is unsupported → fallback.
        let bga = resolve_footprint("Package_BGA:BGA-1206", 1206);
        assert!(!bga.matched, "BGA-1206 must not be hijacked into a chip");

        // But a genuinely-delimited chip code still resolves.
        assert_eq!(
            resolve_footprint("Resistor_SMD:R_0805_2012Metric", 2)
                .template
                .unwrap()
                .name,
            "0805"
        );
        assert_eq!(
            resolve_footprint("Capacitor_SMD:C_1206_3216Metric", 2)
                .template
                .unwrap()
                .name,
            "1206"
        );
    }

    #[test]
    fn sot223_variants_keep_all_pads() {
        // Classic 3-lead SOT-223 = 4 pads (leads 1-3 + tab 4).
        let classic = resolve_footprint("Package_TO_SOT_SMD:SOT-223-3_TabPin2", 4);
        assert_eq!(classic.family.as_deref(), Some("SOT-223"));
        assert_eq!(classic.template.unwrap().pads.len(), 4);
        // Multi-pin variants must NOT silently drop pins.
        assert_eq!(
            resolve_footprint("Package_TO_SOT_SMD:SOT-223-5", 5)
                .template
                .unwrap()
                .pads
                .len(),
            5
        );
        assert_eq!(
            resolve_footprint("Package_TO_SOT_SMD:SOT-223-8", 8)
                .template
                .unwrap()
                .pads
                .len(),
            8
        );
    }

    // ------------------------------------------------------------------
    // Connectors
    // ------------------------------------------------------------------

    /// The exact reproduction from the issue: a 2-pin JST-PH used to fall back
    /// to a 0805 chip placeholder. It must now resolve to a real 2-contact THT
    /// land at 2.0 mm pitch.
    #[test]
    fn jst_ph_repro_resolves_not_placeholder() {
        let r = resolve_footprint("JST_PH_2", 2);
        assert!(r.matched, "JST_PH_2 must resolve, got {:?}", r);
        assert_eq!(r.family.as_deref(), Some("JST-PH"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 2, "2 contacts");
        assert!(
            fp.pads.iter().all(|p| p.pad_type == PadType::THT),
            "JST-PH is through-hole"
        );
        let dx = (fp.pads[0].position.x - fp.pads[1].position.x).abs();
        assert!((dx - 2.0).abs() < 1e-6, "pitch must be 2.0mm, got {dx}");
        assert_on_board(&fp, 4.0);
    }

    #[test]
    fn jst_pitch_and_tech_by_family() {
        // (id, declared, family, pitch, through-hole?)
        for (id, n, fam, pitch, tht) in [
            (
                "Connector_JST:JST_PH_B2B-PH-K_1x04_P2.00mm_Vertical",
                4,
                "JST-PH",
                2.0,
                true,
            ),
            (
                "Connector_JST:JST_XH_B5B-XH-A_1x05_P2.50mm_Vertical",
                5,
                "JST-XH",
                2.5,
                true,
            ),
            (
                "Connector_JST:JST_EH_B3B-EH-A_1x03_P2.50mm_Vertical",
                3,
                "JST-EH",
                2.5,
                true,
            ),
            (
                "Connector_JST:JST_SH_BM06B-SRSS-TB_1x06-1MP_P1.00mm_Horizontal",
                6,
                "JST-SH",
                1.0,
                false,
            ),
            (
                "Connector_JST:JST_GH_SM04B-GHS-TB_1x04-1MP_P1.25mm_Horizontal",
                4,
                "JST-GH",
                1.25,
                false,
            ),
        ] {
            let r = resolve_footprint(id, n);
            assert!(r.matched, "{id} should match");
            assert_eq!(r.family.as_deref(), Some(fam), "{id}");
            let fp = r.template.unwrap();
            assert_eq!(fp.pads.len(), n as usize, "{id}: one pad per contact");
            assert_eq!(
                positions_unique(&fp),
                n as usize,
                "{id}: pads must not stack"
            );
            let want = if tht { PadType::THT } else { PadType::SMD };
            assert!(fp.pads.iter().all(|p| p.pad_type == want), "{id}: pad tech");
            // Adjacent contacts sit one pitch apart along the row.
            let mut xs: Vec<f64> = fp.pads.iter().map(|p| p.position.x).collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert!((xs[1] - xs[0] - pitch).abs() < 1e-6, "{id}: pitch {pitch}");
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 10.0);
        }
    }

    #[test]
    fn jst_pin1_is_squared_for_polarity() {
        // THT connectors square off pad 1 (Rect) and round the rest (Circle).
        let fp = resolve_footprint("JST_PH_4", 4).template.unwrap();
        let p1 = fp.pads.iter().find(|p| p.number == "1").unwrap();
        assert!(matches!(p1.shape, PadShape::Rect { .. }), "pad 1 squared");
        for p in fp.pads.iter().filter(|p| p.number != "1") {
            assert!(
                matches!(p.shape, PadShape::Circle { .. }),
                "pad {} round",
                p.number
            );
        }
    }

    #[test]
    fn jst_kicad_mounting_pad_suffix_counts_contacts() {
        // `1x02-1MP` (1 mounting pad) must still parse to 2 *contacts*, not the
        // mounting tab and not the part-number digits.
        let fp = resolve_footprint(
            "Connector_JST:JST_PH_S2B-PH-SM4-TB_1x02-1MP_P2.00mm_Horizontal",
            2,
        )
        .template
        .unwrap();
        assert_eq!(fp.pads.len(), 2);
    }

    #[test]
    fn molex_picoblade_smd_125() {
        let r = resolve_footprint(
            "Connector_Molex:Molex_PicoBlade_53261-0271_1x02-1MP_P1.25mm_Vertical",
            2,
        );
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("Molex-PicoBlade"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 2);
        assert!(fp.pads.iter().all(|p| p.pad_type == PadType::SMD));
        let dx = (fp.pads[0].position.x - fp.pads[1].position.x).abs();
        assert!((dx - 1.25).abs() < 1e-6, "1.25mm pitch, got {dx}");
        // A Molex part number must not be read as a chip code.
        assert_ne!(fp.name, "0805");
    }

    #[test]
    fn tag_connect_two_columns_no_paste() {
        for (id, declared, want) in [
            ("Tag-Connect:TC2050-IDC-NL_2x05_P1.27mm", 10, 10),
            ("Tag-Connect:TC2030-IDC-NL_2x03_P1.27mm", 6, 6),
            ("TC2050", 0, 10), // count derived from the name, not declared
        ] {
            let r = resolve_footprint(id, declared);
            assert!(r.matched, "{id}");
            assert_eq!(r.family.as_deref(), Some("Tag-Connect"), "{id}");
            let fp = r.template.unwrap();
            assert_eq!(fp.pads.len(), want, "{id}: pad count");
            assert_eq!(positions_unique(&fp), want, "{id}: pads must not stack");
            // Two columns at ±0.635mm (1.27mm pitch).
            let cols: std::collections::BTreeSet<i64> = fp
                .pads
                .iter()
                .map(|p| (p.position.x * 1000.0) as i64)
                .collect();
            assert_eq!(cols.len(), 2, "{id}: exactly two columns");
            // Spring-pin pads get a mask opening but never solder paste.
            assert!(
                fp.pads
                    .iter()
                    .all(|p| !p.layers.contains(&PcbLayer::FPaste)),
                "{id}: pogo pads must not carry paste"
            );
            // The dense 1.27mm pad field must not self-overlap (regression: pads
            // taller than the pitch overlapped and tripped DRC at 0mm).
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 6.0);
        }
    }

    #[test]
    fn usb_c_signal_rows_plus_shield_posts() {
        for (signal, pitch) in [(6u32, 1.0), (16, 0.8), (24, 0.5)] {
            let r = resolve_footprint("Connector_USB:USB_C_Receptacle_Simplified", signal);
            assert!(r.matched, "USB-C {signal}");
            assert_eq!(r.family.as_deref(), Some("USB-C"));
            let fp = r.template.unwrap();
            // Exactly `signal` numeric contacts (1..=signal) + 4 shield posts.
            let numeric: Vec<u32> = fp
                .pads
                .iter()
                .filter_map(|p| p.number.parse::<u32>().ok())
                .collect();
            assert_eq!(numeric.len(), signal as usize, "{signal}: signal contacts");
            assert_eq!(
                (1..=signal).filter(|n| numeric.contains(n)).count(),
                signal as usize,
                "{signal}: contacts numbered 1..={signal}"
            );
            let shields = fp
                .pads
                .iter()
                .filter(|p| p.number.starts_with("SH"))
                .count();
            assert_eq!(shields, 4, "{signal}: four shield/mount posts");
            // Signal contacts are SMD in two rows; posts are through-hole.
            let rows: std::collections::BTreeSet<i64> = fp
                .pads
                .iter()
                .filter(|p| p.number.parse::<u32>().is_ok())
                .map(|p| (p.position.y * 100.0) as i64)
                .collect();
            assert_eq!(rows.len(), 2, "{signal}: two contact rows");
            // Contact pitch tightens with the count.
            let mut xs: Vec<f64> = fp
                .pads
                .iter()
                .filter(|p| p.position.y < 0.0 && p.number.parse::<u32>().is_ok())
                .map(|p| p.position.x)
                .collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if xs.len() >= 2 {
                assert!(
                    (xs[1] - xs[0] - pitch).abs() < 1e-6,
                    "{signal}: pitch {pitch}"
                );
            }
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 8.0);
        }
    }

    #[test]
    fn pin_header_1xn_resolves() {
        // The issue's `PinHeader_1x{n}_P2.54mm` form (already supported — lock it).
        let r = resolve_footprint(
            "Connector_PinHeader_2.54mm:PinHeader_1x06_P2.54mm_Vertical",
            6,
        );
        assert!(r.matched);
        assert_eq!(r.family.as_deref(), Some("PinHeader"));
        let fp = r.template.unwrap();
        assert_eq!(fp.pads.len(), 6);
        assert!(fp.pads.iter().all(|p| p.pad_type == PadType::THT));
    }

    #[test]
    fn unknown_footprint_note_points_to_escape_hatch() {
        // Discoverability: the fallback must tell the caller how to fix it.
        let note = resolve_footprint("Vendor:WhoKnows", 3).note;
        assert!(
            note.contains("pads"),
            "note should mention the `pads` hatch: {note}"
        );
        let note2 = resolve_footprint("Vendor:TwoPin", 2).note;
        assert!(note2.contains("pads"), "2-pin note should too: {note2}");
    }

    // ------------------------------------------------------------------------
    // THT drill sizing (drill = lead + ~0.2mm, never oversized)
    // ------------------------------------------------------------------------

    /// Worst (smallest) intra-footprint hole-to-hole edge distance, mm — the
    /// metric DRC's HoleToHole rule flags. `f64::MAX` when there are <2 holes.
    fn worst_hole_to_hole(fp: &FootprintTemplate) -> f64 {
        let holes: Vec<(Vec2, f64)> = fp
            .pads
            .iter()
            .filter_map(|p| p.drill.as_ref().map(|d| (p.position, d.diameter / 2.0)))
            .collect();
        let mut worst = f64::MAX;
        for i in 0..holes.len() {
            for j in (i + 1)..holes.len() {
                let dx = holes[i].0.x - holes[j].0.x;
                let dy = holes[i].0.y - holes[j].0.y;
                let edge = (dx * dx + dy * dy).sqrt() - holes[i].1 - holes[j].1;
                worst = worst.min(edge);
            }
        }
        worst
    }

    #[test]
    fn tht_drill_follows_ipc_gap() {
        // drill = lead + 0.2, floored at a fab-practical 0.3mm.
        assert!((tht_drill(0.5) - 0.7).abs() < 1e-9);
        assert!((tht_drill(0.6) - 0.8).abs() < 1e-9);
        assert!((tht_drill(0.8) - 1.0).abs() < 1e-9);
        assert_eq!(tht_drill(0.0), 0.3, "sub-fab leads floor at 0.3mm");
    }

    /// The fix from the issue: standard THT parts must drill fab-buildable holes
    /// (≈ lead + 0.2mm) rather than the old pitch-scaled / flat oversized values.
    #[test]
    fn standard_tht_parts_are_not_oversized() {
        // (id, declared pins, max acceptable drill mm) — each upper bound is the
        // pre-fix value, so a regression to the old oversized drill trips this.
        let cases = [
            ("PinHeader_1x04_P2.54mm", 4, 1.0),   // was 1.016
            ("JST_PH_2", 2, 0.7),                 // was 1.0
            ("JST_XH_3", 3, 0.8),                 // was 1.0
            ("CP_Radial_D6.3mm_P2.50mm", 2, 0.8), // was 1.0
            ("Vendor:Mystery4PinXtal", 4, 0.8),   // grid fallback, was 0.9
        ];
        for (id, pins, max_drill) in cases {
            let fp = resolve_footprint(id, pins).template.unwrap();
            for pad in &fp.pads {
                if let Some(d) = &pad.drill {
                    assert!(
                        d.diameter <= max_drill + 1e-9,
                        "{id} pad {} drill {:.3}mm exceeds {max_drill}mm — oversized",
                        pad.number,
                        d.diameter
                    );
                    assert!(
                        d.diameter >= 0.3,
                        "{id} pad {} drill {:.3}mm below fab floor",
                        pad.number,
                        d.diameter
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Crystals, USB micro-B, HTSSOP (exposed pad)
    // ------------------------------------------------------------------

    /// The RP2040 repro: a 4-pad 3225 crystal used to fall back to an R_1206
    /// stand-in. It must now resolve to a real 4-corner-pad land.
    #[test]
    fn crystal_3225_four_pads() {
        for (id, n) in [
            ("Crystal:Crystal_SMD_3225-4Pin_3.2x2.5mm", 4u32),
            ("Crystal_SMD_2520-4Pin_2.5x2.0mm", 4),
        ] {
            let r = resolve_footprint(id, n);
            assert!(r.matched, "{id} should match, got {:?}", r);
            assert_eq!(r.family.as_deref(), Some("Crystal"), "{id}");
            let fp = r.template.unwrap();
            assert_eq!(fp.pads.len(), 4, "{id}: four corner pads");
            assert_eq!(positions_unique(&fp), 4, "{id}");
            assert!(fp.pads.iter().all(|p| p.pad_type == PadType::SMD));
            // Pads occupy all four quadrants (corner layout, not a row).
            let quads: std::collections::BTreeSet<(bool, bool)> = fp
                .pads
                .iter()
                .map(|p| (p.position.x > 0.0, p.position.y > 0.0))
                .collect();
            assert_eq!(quads.len(), 4, "{id}: one pad per corner");
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 3.0);
        }
    }

    #[test]
    fn crystal_2pad_variants() {
        for (id, body_w) in [
            ("Crystal:Crystal_SMD_5032-2Pin_5.0x3.2mm", 5.0),
            ("Crystal_SMD_6035", 6.0),
        ] {
            let r = resolve_footprint(id, 2);
            assert!(r.matched, "{id}");
            assert_eq!(r.family.as_deref(), Some("Crystal"), "{id}");
            let fp = r.template.unwrap();
            assert_eq!(fp.pads.len(), 2, "{id}");
            // End pads straddle the body along x.
            let dx = (fp.pads[0].position.x - fp.pads[1].position.x).abs();
            assert!(
                dx > body_w * 0.5 && dx < body_w + 1.0,
                "{id}: pad span {dx} vs body {body_w}"
            );
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, body_w);
        }
    }

    #[test]
    fn crystal_pin_count_disambiguates_pad_style() {
        // No size code, no -NPin marker: the declared pin count decides.
        assert_eq!(
            resolve_footprint("Crystal_SMD_Generic", 4)
                .template
                .unwrap()
                .pads
                .len(),
            4
        );
        assert_eq!(
            resolve_footprint("Crystal_SMD_Generic", 2)
                .template
                .unwrap()
                .pads
                .len(),
            2
        );
        // A bare "Xtal" vendor id stays in the generic fallback (locked by
        // standard_tht_parts_are_not_oversized's Mystery4PinXtal case too).
        assert!(!resolve_footprint("Vendor:Mystery4PinXtal", 4).matched);
    }

    #[test]
    fn usb_micro_b_contacts_plus_shield() {
        for id in [
            "Connector_USB:USB_Micro-B_Molex-105017-0001",
            "USB_Micro-B_Amphenol_10103594-0001LF_Horizontal",
        ] {
            let r = resolve_footprint(id, 5);
            assert!(r.matched, "{id}");
            assert_eq!(r.family.as_deref(), Some("USB-Micro-B"), "{id}");
            let fp = r.template.unwrap();
            // Five SMD signal contacts numbered 1..=5 at 0.65mm pitch.
            let mut xs: Vec<f64> = fp
                .pads
                .iter()
                .filter(|p| p.number.parse::<u32>().is_ok())
                .map(|p| p.position.x)
                .collect();
            assert_eq!(xs.len(), 5, "{id}: five signal contacts");
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert!((xs[1] - xs[0] - 0.65).abs() < 1e-6, "{id}: 0.65mm pitch");
            // Four THT shield posts, mechanical numbering.
            let shields: Vec<&Pad> = fp
                .pads
                .iter()
                .filter(|p| p.number.starts_with("SH"))
                .collect();
            assert_eq!(shields.len(), 4, "{id}: four shield posts");
            assert!(shields.iter().all(|p| p.pad_type == PadType::THT));
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 5.0);
            assert!(worst_hole_to_hole(&fp) >= 0.5 - 1e-9, "{id}: hole-to-hole");
        }
    }

    /// The DRV8833PWP repro: HTSSOP must keep its exposed thermal pad instead
    /// of degrading to a plain TSSOP land (which starves the motor driver of
    /// its heat path).
    #[test]
    fn htssop_keeps_exposed_pad() {
        for (id, pins) in [
            ("Package_SO:HTSSOP-16-1EP_4.4x5mm_P0.65mm_EP3.4x5mm", 16u32),
            ("HTSSOP-16", 16),
            ("Package_SO:TSSOP-20-1EP_4.4x6.5mm_P0.65mm", 20),
        ] {
            let r = resolve_footprint(id, pins);
            assert!(r.matched, "{id}");
            assert_eq!(r.family.as_deref(), Some("HTSSOP"), "{id}");
            let fp = r.template.unwrap();
            assert_eq!(
                fp.pads.len(),
                pins as usize + 1,
                "{id}: leads + exposed pad"
            );
            let ep = fp
                .pads
                .iter()
                .find(|p| p.number == "EP")
                .unwrap_or_else(|| panic!("{id}: EP pad missing"));
            assert_eq!(ep.pad_type, PadType::SMD);
            assert!(
                (ep.position.x.abs() + ep.position.y.abs()) < 1e-9,
                "{id}: EP centered"
            );
            assert_no_pad_overlap(&fp);
            assert_on_board(&fp, 5.0);
        }
        // The KiCad `_EP<w>x<h>mm` token sizes the pad exactly.
        let fp = resolve_footprint("Package_SO:HTSSOP-16-1EP_4.4x5mm_P0.65mm_EP3.4x5mm", 16)
            .template
            .unwrap();
        let ep = fp.pads.iter().find(|p| p.number == "EP").unwrap();
        match ep.shape {
            PadShape::Rect { width, height } => {
                assert!((width - 3.4).abs() < 1e-9 && (height - 5.0).abs() < 1e-9);
            }
            _ => panic!("EP must be rect"),
        }
    }

    #[test]
    fn plain_tssop_still_has_no_ep() {
        let fp = resolve_footprint("Package_SO:TSSOP-20_4.4x6.5mm_P0.65mm", 20)
            .template
            .unwrap();
        assert!(fp.pads.iter().all(|p| p.number != "EP"));
        assert_eq!(fp.pads.len(), 20);
    }

    /// Across every THT family the generator emits, drilled holes must clear the
    /// default 0.5mm hole-to-hole rule and keep a >=0.15mm annular ring — so a
    /// single standard part never self-flags a manufacturing DRC violation.
    #[test]
    fn generated_tht_footprints_pass_hole_and_ring_rules() {
        let ids = [
            ("PinHeader_1x04_P2.54mm", 4u32),
            ("PinHeader_2x05_P2.54mm", 10),
            ("PinHeader_1x40_P2.54mm", 40),
            ("DIP-8", 8),
            ("JST_PH_2", 2),
            ("JST_PH_6", 6),
            ("JST_XH_4", 4),
            ("JST_EH_2", 2),
            ("TerminalBlock_1x03_P5.08mm", 3),
            ("CP_Radial_D6.3mm_P2.50mm", 2),
            ("TO-220-3", 3),
            ("Vendor:Mystery4PinXtal", 4), // generic grid fallback (THT)
        ];
        for (id, pins) in ids {
            let fp = resolve_footprint(id, pins).template.unwrap();
            assert!(
                worst_hole_to_hole(&fp) >= 0.5 - 1e-9,
                "{id}: intra-footprint hole-to-hole {:.3}mm < 0.5mm",
                worst_hole_to_hole(&fp)
            );
            for pad in &fp.pads {
                if let (PadShape::Circle { diameter }, Some(d)) = (&pad.shape, &pad.drill) {
                    let ring = (diameter - d.diameter) / 2.0;
                    assert!(
                        ring >= 0.15 - 1e-9,
                        "{id} pad {} annular ring {:.3}mm < 0.15mm",
                        pad.number,
                        ring
                    );
                }
            }
        }
    }

    /// Resolve `name` into its template, asserting the family and that no two
    /// pads with DIFFERENT numbers overlap (same-number pads may abut: they
    /// are one terminal split into rectangles).
    fn power_fp(name: &str, family: &str) -> FootprintTemplate {
        let r = resolve_footprint(name, 0);
        assert!(r.matched, "{name}: {}", r.note);
        assert_eq!(r.family.as_deref(), Some(family), "{name}");
        let t = r.template.unwrap();
        for (i, a) in t.pads.iter().enumerate() {
            for b in &t.pads[i + 1..] {
                if a.number == b.number && !a.number.is_empty() {
                    continue;
                }
                let (a0, a1, a2, a3) = pad_extent(a);
                let (b0, b1, b2, b3) = pad_extent(b);
                let overlap = a0 < b2 && b0 < a2 && a1 < b3 && b1 < a3;
                assert!(
                    !overlap,
                    "{name}: pads {:?} and {:?} overlap",
                    a.number, b.number
                );
            }
        }
        // Courtyard encloses every pad.
        let cy = t
            .graphics
            .iter()
            .find_map(|g| match g {
                FootprintGraphic::Rect {
                    start, end, layer, ..
                } if *layer == PcbLayer::FCrtYd => Some((*start, *end)),
                _ => None,
            })
            .expect("courtyard");
        for p in &t.pads {
            let (x0, y0, x1, y1) = pad_extent(p);
            assert!(
                x0 >= cy.0.x && y0 >= cy.0.y && x1 <= cy.1.x && y1 <= cy.1.y,
                "{name}: pad {:?} outside courtyard",
                p.number
            );
        }
        t
    }

    fn numbers(t: &FootprintTemplate) -> Vec<&str> {
        let mut v: Vec<&str> = t
            .pads
            .iter()
            .map(|p| p.number.as_str())
            .filter(|n| !n.is_empty())
            .collect();
        v.dedup();
        v
    }

    fn pads_of<'a>(t: &'a FootprintTemplate, n: &str) -> Vec<&'a Pad> {
        t.pads.iter().filter(|p| p.number == n).collect()
    }

    fn size(p: &Pad) -> (f64, f64) {
        match p.shape {
            PadShape::Rect { width, height } => (width, height),
            PadShape::Circle { diameter } => (diameter, diameter),
            _ => (0.0, 0.0),
        }
    }

    fn drill(p: &Pad) -> f64 {
        p.drill.as_ref().map(|d| d.diameter).unwrap_or(0.0)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn npths(t: &FootprintTemplate) -> Vec<&Pad> {
        t.pads
            .iter()
            .filter(|p| p.pad_type == PadType::NPTH)
            .collect()
    }

    #[test]
    fn toll_matches_infineon_hsof8_land() {
        let t = power_fp("Infineon_PG-HSOF-8-1_TOLL", "TOLL");
        assert_eq!(numbers(&t), ["1", "2", "3"]);
        // Gate = lead 1 at the −X end of the 1.20 mm lead row.
        let g = pads_of(&t, "1")[0];
        assert!(close(g.position.x, -4.2) && close(size(g).0, 0.8) && close(size(g).1, 2.8));
        // 7 source fingers on the 1.20 pitch + bridging bar.
        let fingers: Vec<f64> = pads_of(&t, "2")
            .iter()
            .filter(|p| close(size(p).0, 0.8))
            .map(|p| p.position.x)
            .collect();
        assert_eq!(fingers.len(), 7);
        for (i, x) in fingers.iter().enumerate() {
            assert!(close(*x, -3.0 + 1.2 * i as f64), "finger {i} at {x}");
        }
        // 2.5 mm source-to-drain gap (source bar inner edge 3.85, drain 1.35).
        let drain_low = pads_of(&t, "3")
            .iter()
            .map(|p| pad_extent(p).3)
            .fold(f64::MIN, f64::max);
        let src_high = pads_of(&t, "2")
            .iter()
            .map(|p| pad_extent(p).1)
            .fold(f64::MAX, f64::min);
        assert!(close(src_high - drain_low, 2.5));
    }

    #[test]
    fn tdson8_matches_infineon_boardpads() {
        let t = power_fp("Infineon_PG-TDSON-8", "PowerSON8");
        assert_eq!(numbers(&t), ["1", "2", "3"]);
        let g = pads_of(&t, "1")[0];
        assert!(close(g.position.x, 1.905) && close(size(g).0, 0.5) && close(size(g).1, 1.1));
        // Drain body 4.41 wide; source land 3.2 wide, 0.77 from the gate.
        assert!(pads_of(&t, "3").iter().any(|p| close(size(p).0, 4.41)));
        let bar = pads_of(&t, "2")
            .into_iter()
            .find(|p| close(size(p).0, 3.2))
            .unwrap();
        assert!(close(pad_extent(g).0 - pad_extent(bar).2, 0.77));
        // Land spans ±3.325 (3.325 + 3.325 in the drawing).
        let all: Vec<_> = t.pads.iter().map(pad_extent).collect();
        let ymin = all.iter().map(|e| e.1).fold(f64::MAX, f64::min);
        let ymax = all.iter().map(|e| e.3).fold(f64::MIN, f64::max);
        assert!(close(ymin, -3.325) && close(ymax, 3.325));
    }

    #[test]
    fn tsdson8_matches_infineon_f_dimensions() {
        let t = power_fp("Infineon_PG-TSDSON-8", "PowerSON8");
        assert_eq!(numbers(&t), ["1", "2", "3"]);
        let all: Vec<_> = t.pads.iter().map(pad_extent).collect();
        let ymin = all.iter().map(|e| e.1).fold(f64::MAX, f64::min);
        let ymax = all.iter().map(|e| e.3).fold(f64::MIN, f64::max);
        assert!(close(ymax - ymin, 3.90), "F1"); // F1
        let drain_w = pads_of(&t, "3")
            .iter()
            .map(|p| size(p).0)
            .fold(0.0, f64::max);
        assert!(close(drain_w, 2.29), "F2");
        assert!(close(pads_of(&t, "2")[0].position.x, -0.325));
        assert!(close(size(pads_of(&t, "2")[0]).0, 1.64), "F8");
        // Gate = lead 4 at +1.5·pitch, F3 = 0.31 from the source land.
        let g = pads_of(&t, "1")[0];
        assert!(close(g.position.x, 0.975));
        assert!(close(
            pad_extent(g).0 - pad_extent(pads_of(&t, "2")[0]).2,
            0.31
        ));
    }

    #[test]
    fn to277a_matches_vishay_mounting_pads() {
        let t = power_fp("Diode_SMD:D_TO-277A", "TO-277A");
        assert_eq!(numbers(&t), ["1", "2"]);
        let k = pads_of(&t, "1")[0];
        assert_eq!(size(k), (4.80, 4.72));
        let a = pads_of(&t, "2");
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|p| size(p) == (1.40, 1.27)));
        assert!(close(a[1].position.x - a[0].position.x, 2.08));
        let all: Vec<_> = t.pads.iter().map(pad_extent).collect();
        let ymin = all.iter().map(|e| e.1).fold(f64::MAX, f64::min);
        let ymax = all.iter().map(|e| e.3).fold(f64::MIN, f64::max);
        assert!(close(ymax - ymin, 6.80));
        // Cathode-to-anode gap = 6.80 − 4.72 − 1.27.
        assert!(close(pad_extent(a[0]).1 - pad_extent(k).3, 0.81));
    }

    #[test]
    fn bmi088_matches_bosch_landing_pattern() {
        let t = power_fp("BMI088", "LGA-IMU");
        let nums: Vec<String> = (1..=16).map(|n| n.to_string()).collect();
        assert_eq!(
            numbers(&t),
            nums.iter().map(String::as_str).collect::<Vec<_>>()
        );
        let p = |n: &str| pads_of(&t, n)[0];
        // Pin 1 top-right, 1..7 down the right column at 0.5 mm, pin 4 on the
        // centre line; 9..15 up the left; 8 bottom, 16 top.
        assert!(p("1").position.x > 0.0 && close(p("1").position.y, -1.5));
        assert!(close(p("4").position.y, 0.0) && close(p("7").position.y, 1.5));
        assert!(close(p("9").position.y, 1.5) && close(p("15").position.y, -1.5));
        assert!(p("9").position.x < 0.0);
        assert!(p("8").position.y > 2.0 && p("16").position.y < -2.0);
        assert_eq!(size(p("1")), (0.675, 0.25));
        assert_eq!(size(p("8")), (0.25, 0.675));
        // Inner edges: 0.925 (side rows) and 1.675 (pins 8/16) from centre.
        assert!(close(pad_extent(p("9")).2, -0.925));
        assert!(close(pad_extent(p("8")).1, 1.675));
    }

    #[test]
    fn icm42688_matches_tdk_pinout() {
        let t = power_fp("ICM-42688-P", "LGA-IMU");
        assert_eq!(numbers(&t).len(), 14);
        let p = |n: &str| pads_of(&t, n)[0];
        // Pin 1 top-left; 1-4 down the left, 5-7 bottom L→R, 8-11 up the
        // right, 12-14 top R→L (DS-000347 fig. 5, top view).
        assert!(p("1").position.x < -1.0 && close(p("1").position.y, -0.75));
        assert!(close(p("4").position.y, 0.75));
        assert!(close(p("5").position.x, -0.5) && p("5").position.y > 0.9);
        assert!(close(p("8").position.y, 0.75) && p("8").position.x > 1.0);
        assert!(close(p("11").position.y, -0.75));
        assert!(close(p("12").position.x, 0.5) && p("12").position.y < -0.9);
        assert!(close(p("14").position.x, -0.5));
        // Lands cover the 0.25 × 0.475 lead (inset 0.10 from the 3.0 × 2.5 body).
        let (x0, _, x1, _) = pad_extent(p("1"));
        assert!(x0 <= -1.5 && x1 >= -1.5 + 0.10 + 0.475);
        let (_, y0, _, y1) = pad_extent(p("6"));
        assert!(y1 >= 1.25 && y0 <= 1.25 - 0.10 - 0.475);
        assert!(close(size(p("1")).1, 0.30));
    }

    #[test]
    fn xt30pw_2p2_matches_easyeda_land() {
        let t = power_fp("AMASS_XT30PW_2+2_-M.G.B_Horizontal", "AMASS-XT");
        assert_eq!(numbers(&t), ["1", "2", "3", "4"]);
        let p = |n: &str| pads_of(&t, n)[0];
        assert!(close(p("2").position.x - p("1").position.x, 5.0));
        assert!(close(drill(p("1")), 1.9));
        assert!(close(p("3").position.x - p("2").position.x, 3.85));
        assert!(close(p("3").position.y - p("4").position.y, 2.0));
        // The vertical PB part must not capture the RA name.
        let v = power_fp("AMASS_XT30PB_2+2_-M", "AMASS-XT");
        assert!(close(pads_of(&v, "3")[0].position.y, -1.0));
    }

    #[test]
    fn xt30pb_2p2_matches_amass_drawing() {
        let t = power_fp("AMASS_XT30PB_2+2_-M", "AMASS-XT");
        assert_eq!(numbers(&t), ["1", "2", "3", "4"]);
        let p = |n: &str| pads_of(&t, n)[0];
        assert!(close(p("2").position.x - p("1").position.x, 5.0));
        assert!(close(drill(p("1")), 1.9) && close(drill(p("2")), 1.9));
        // Signals 3.85 beyond the "−" (pad 2) hole, 2.00 apart; "1" at −Y.
        assert!(close(p("3").position.x - p("2").position.x, 3.85));
        assert!(close(p("4").position.y - p("3").position.y, 2.0));
        assert!(p("3").position.y < 0.0);
        assert!(
            matches!(p("1").shape, PadShape::Rect { .. }),
            "pin 1 square"
        );
    }

    #[test]
    fn xt30pw_xt60pw_xt90pw_match_amass_hole_diagrams() {
        // (name, pitch, drill, peg (dx, dy, w, h))
        let cases = [
            (
                "AMASS_XT30PW-M_Horizontal",
                5.0,
                1.7,
                (11.0, -10.0, 1.0, 1.0),
            ),
            (
                "AMASS_XT60PW-M_Horizontal",
                7.2,
                2.9,
                (13.5, -6.0, 0.8, 1.9),
            ),
            (
                "AMASS_XT90PW-M_Horizontal",
                10.9,
                3.8,
                (18.9, -9.4, 0.6, 1.4),
            ),
        ];
        for (name, pitch, d, (dx, dy, w, h)) in cases {
            let t = power_fp(name, "AMASS-XT");
            assert_eq!(numbers(&t), ["1", "2"], "{name}");
            let p1 = pads_of(&t, "1")[0];
            let p2 = pads_of(&t, "2")[0];
            assert!(close(p2.position.x - p1.position.x, pitch), "{name} pitch");
            assert!(close(drill(p1), d) && close(drill(p2), d), "{name} drill");
            let pegs = npths(&t);
            assert_eq!(pegs.len(), 2, "{name}");
            assert!(close(pegs[1].position.x - pegs[0].position.x, dx), "{name}");
            assert!(pegs.iter().all(|p| close(p.position.y, dy)), "{name}");
            assert!(pegs.iter().all(|p| size(p) == (w, h)), "{name}");
        }
        let t = power_fp("AMASS_XT60PB-M_Vertical", "AMASS-XT");
        assert!(close(drill(pads_of(&t, "1")[0]), 4.0));
        assert!(npths(&t).is_empty());
    }

    #[test]
    fn dual_row_1ep_keeps_its_exposed_pad() {
        for (name, ep) in [
            ("MSOP-8-1EP_3x3mm_P0.65mm_EP1.68x1.88mm", (1.68, 1.88)),
            ("TI_SO-8-1EP_3.9x4.9mm_P1.27mm_EP2.41x3.1mm", (2.41, 3.1)),
        ] {
            let r = resolve_footprint(name, 9);
            assert!(r.matched, "{name}");
            let t = r.template.unwrap();
            assert_eq!(t.pads.len(), 9, "{name}");
            let p9 = pads_of(&t, "9")[0];
            assert_eq!(size(p9), ep, "{name}");
            assert!(close(p9.position.x, 0.0) && close(p9.position.y, 0.0));
        }
    }

    #[test]
    fn ti_dgx19_matches_ti_land_pattern() {
        let t = power_fp("VSSOP-19_3x4.9mm_P0.5mm_TI_DGX", "TI-DGX");
        let nums = numbers(&t);
        assert_eq!(nums.len(), 19);
        assert!(!nums.contains(&"16"), "position 16 depopulated");
        let p = |n: &str| pads_of(&t, n)[0];
        // 4.40 between rows, 0.50 pitch, 1.45 × 0.30 lands.
        assert!(close(p("20").position.x - p("1").position.x, 4.4));
        assert!(close(p("1").position.y, p("20").position.y));
        assert!(close(p("10").position.y, p("11").position.y));
        assert!(close(p("2").position.y - p("1").position.y, 0.5));
        assert!(
            close(p("15").position.y, p("6").position.y),
            "15 faces 6 (16 faces 5)"
        );
        assert_eq!(size(p("7")), (1.45, 0.3));
    }

    #[test]
    fn sot563_srr1260_fxl1365_cp_elec_1008_resolve() {
        let t = power_fp("SOT-563", "SOT-563");
        assert_eq!(numbers(&t), ["1", "2", "3", "4", "5", "6"]);
        let p = |t: &FootprintTemplate, n: &str| pads_of(t, n)[0].position;
        assert!(close(p(&t, "6").x - p(&t, "1").x, 1.425) && close(p(&t, "6").y, p(&t, "1").y));
        let t = power_fp("L_Bourns_SRR1260", "SMD-Inductor");
        assert!(close(p(&t, "2").x - p(&t, "1").x, 9.7));
        assert_eq!(size(pads_of(&t, "1")[0]), (2.9, 5.4));
        let t = power_fp("L_Changjiang_FXL1365", "SMD-Inductor");
        assert!(close(p(&t, "2").x - p(&t, "1").x, 11.25));
        let t = power_fp("CP_Elec_8x6.9", "CP_Elec");
        assert!(close(p(&t, "2").x - p(&t, "1").x, 6.95));
        assert_eq!(size(pads_of(&t, "1")[0]), (4.15, 1.9));
        let t = power_fp("Capacitor_SMD:CP_Elec_10x10.5", "CP_Elec");
        assert!(close(p(&t, "2").x - p(&t, "1").x, 8.4));
        assert!(
            !resolve_footprint("CP_Elec_7x7", 2).matched,
            "unknown can stays a placeholder"
        );
        let r = resolve_footprint("L_1008_2520Metric", 2);
        assert!(r.matched);
        let t = r.template.unwrap();
        assert!(close(p(&t, "2").x - p(&t, "1").x, 2.15));
    }

    #[test]
    fn amass_lcb_matches_drawings() {
        for (name, pitch, d, off) in [
            ("AMASS_LCB30PB-M", 7.6, 3.2, 3.5),
            ("AMASS_LCB40PB-M", 8.8, 4.2, 4.05),
        ] {
            let t = power_fp(name, "AMASS-LCB");
            assert_eq!(numbers(&t), ["1", "2"]);
            let p1 = pads_of(&t, "1")[0];
            let p2 = pads_of(&t, "2")[0];
            assert!(close(p2.position.x - p1.position.x, pitch));
            assert!(close(drill(p1), d));
            let peg = npths(&t)[0];
            assert!(close(peg.position.y, off) && close(size(peg).0, 1.7));
        }
    }

    #[test]
    fn microfit_matches_molex_sd_43045() {
        // Right-angle 2x02: one Ø3.00 peg, centred, 4.32 in front of row 1.
        let t = power_fp(
            "Molex_Micro-Fit_3.0_43045-0400_2x02_P3.00mm_Horizontal",
            "Micro-Fit",
        );
        assert_eq!(numbers(&t), ["1", "2", "3", "4"]);
        let p = |t: &FootprintTemplate, n: &str| pads_of(t, n)[0].position;
        assert!(close(p(&t, "2").x - p(&t, "1").x, 3.0));
        assert!(close(p(&t, "3").y - p(&t, "1").y, 3.0) && close(p(&t, "3").x, p(&t, "1").x));
        let pegs = npths(&t);
        assert_eq!(pegs.len(), 1);
        assert!(close(pegs[0].position.x, 0.0) && close(pegs[0].position.y, -4.32));
        assert!(close(size(pegs[0]).0, 3.0));
        assert!(close(drill(pads_of(&t, "1")[0]), 1.02));
        // Right-angle 2x04: two pegs, C = 4.70 apart (SD-43045-001 table).
        let t = power_fp(
            "Molex_Micro-Fit_3.0_43045-0800_2x04_P3.00mm_Horizontal",
            "Micro-Fit",
        );
        let pegs = npths(&t);
        assert!(close(pegs[1].position.x - pegs[0].position.x, 4.70));
        // Vertical 2x04 (xx12): Ø1.02 pegs C = 15.00 apart, 0.94 past row 2.
        let t = power_fp("Molex_Micro-Fit_3.0_43045-0812_2x04_P3.00mm", "Micro-Fit");
        assert_eq!(numbers(&t).len(), 8);
        let pegs = npths(&t);
        assert!(close(pegs[1].position.x - pegs[0].position.x, 15.0));
        assert!(pegs
            .iter()
            .all(|q| close(q.position.y, 3.94) && close(size(q).0, 1.02)));
        assert!(matches!(pads_of(&t, "1")[0].shape, PadShape::Rect { .. }));
    }
}
