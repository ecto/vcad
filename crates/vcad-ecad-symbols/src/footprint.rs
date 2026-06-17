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
//! QFN/DFN/SON (with thermal pad), DPAK/D2PAK (TO-252/TO-263), SOD/DO-214
//! (SMA/SMB/SMC) two-terminal SMD, DIP, pin headers/sockets, screw terminals,
//! and radial electrolytic caps. Anything else falls back to a *compact grid*
//! of pads sized to stay on the board, flagged `matched: false`.

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
    let mut pads = Vec::new();
    for i in 0..half {
        let y = top - i as f64 * pitch;
        pads.push(circle_tht(
            &(i + 1).to_string(),
            -row_spacing / 2.0,
            y,
            1.6,
            0.8,
        ));
    }
    for i in 0..half {
        let y = -top + i as f64 * pitch;
        pads.push(circle_tht(
            &(half + i + 1).to_string(),
            row_spacing / 2.0,
            y,
            1.6,
            0.8,
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
    for (k, num) in leads.iter().enumerate() {
        let x = if n == 1 {
            0.0
        } else {
            (k as f64 - (n as f64 - 1.0) / 2.0) * pitch
        };
        if tht {
            pads.push(circle_tht(&num.to_string(), x, lead_y, lw.max(lh), 1.0));
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
// Connectors: pin headers / sockets, screw terminals, radial electrolytics
// ============================================================================

fn pin_header(rows: u32, cols: u32, pitch: f64) -> FootprintTemplate {
    let pad_dia = (pitch * 0.6).clamp(1.4, 2.5);
    let drill = (pitch * 0.4).clamp(0.7, 1.2);
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
    let mut pads = Vec::new();
    for i in 0..positions {
        let x = (i as f64 - (positions as f64 - 1.0) / 2.0) * pitch;
        pads.push(circle_tht(&(i + 1).to_string(), x, 0.0, 2.6, 1.3));
    }
    let half_w = (positions as f64) * pitch / 2.0 + 0.5;
    FootprintTemplate {
        name: format!("ScrewTerminal_1x{positions}"),
        pads,
        graphics: silk_rect(-half_w, -pitch / 2.0 - 1.0, half_w, pitch / 2.0 + 1.0),
    }
}

fn radial_electrolytic(pitch: f64, body_dia: f64) -> FootprintTemplate {
    FootprintTemplate {
        name: format!("CP_Radial_D{body_dia:.1}mm_P{pitch:.2}mm"),
        pads: vec![
            circle_tht("1", -pitch / 2.0, 0.0, 1.8, 1.0),
            circle_tht("2", pitch / 2.0, 0.0, 1.8, 1.0),
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
// Generic compact fallback (replaces the off-board single-column placeholder)
// ============================================================================

fn grid_fallback(pins: u32) -> FootprintTemplate {
    let cols = (pins as f64).sqrt().ceil() as u32;
    let cols = cols.max(1);
    let pitch = 2.54;
    let mut pads = Vec::new();
    let rows = pins.div_ceil(cols);
    for n in 0..pins {
        let r = n / cols;
        let c = n % cols;
        let x = (c as f64 - (cols as f64 - 1.0) / 2.0) * pitch;
        let y = (r as f64 - (rows as f64 - 1.0) / 2.0) * pitch;
        pads.push(circle_tht(&(n + 1).to_string(), x, y, 1.6, 0.9));
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
        let after_ok = run_end == s.len() || bytes[run_end] == b'_';
        if !before_ok || !after_ok {
            continue;
        }
        let rows: u32 = s[run_start..i].parse().ok()?;
        let cols: u32 = s[i + 1..run_end].parse().ok()?;
        return Some((rows, cols));
    }
    None
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

fn match_family(base: &str, pin_count: u32) -> Option<(FootprintTemplate, &'static str)> {
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

    // --- SOIC / SO / SOP / SSOP / TSSOP / MSOP / VSSOP ----------------------
    // Longest markers first so TSSOP isn't shadowed by SOP/SO.
    for (marker, pitch, body_w, lead_len, lead_wid, label) in [
        ("HTSSOP-", 0.65, 4.4, 1.0, 0.3, "TSSOP"),
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
                return Some((
                    dual_lr(
                        format!("SOIC-{pins}"),
                        pins,
                        pitch,
                        body_w,
                        lead_len,
                        lead_wid,
                    ),
                    label,
                ));
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
            note: format!("unrecognized footprint '{base}'; substituted a 0805 chip placeholder"),
        },
        n => FootprintResolution {
            template: Some(grid_fallback(n)),
            matched: false,
            family: None,
            note: format!(
                "unrecognized footprint '{base}'; substituted a compact {n}-pad grid placeholder"
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
}
