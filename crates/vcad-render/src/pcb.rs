//! Flat, top-down, per-layer 2D PCB renderer.
//!
//! Where [`crate::render_svg_str`] projects a 3D CAD document to isometric
//! line art, this module renders a [`vcad_ir::ecad::Pcb`] straight down the
//! board normal (looking down −Z, Y up) as a flat fabrication-style preview —
//! the "copper/silk" view an EDA tool shows, not the 3D enclosure shot.
//!
//! Only the requested layers are drawn, composited bottom-up in a sensible
//! z-order (board fill → copper zones → copper traces/arcs → pads → vias →
//! silkscreen → edge cuts on top). Each layer gets a distinct colour so a
//! reader (human or agent) can tell front copper from back copper from silk.
//!
//! The renderer is deliberately simple and additive: it draws exactly the
//! layers handed to it, in the colours below, with no front/back mirroring.
//! A caller that wants a bottom view supplies the back layers; the geometry
//! is the same, only the colour differs.
//!
//! Coordinates are board-local millimetres (the same frame the [`Pcb`]
//! geometry lives in). The viewBox is computed from the board outline bounds
//! plus a small margin, and the whole image is flipped vertically once on
//! output so that PCB +Y (up) maps to SVG up.
//!
//! [`Pcb`]: vcad_ir::ecad::Pcb

use std::fmt::Write as _;

use vcad_ir::ecad::{
    BoardOutline, FootprintGraphic, Pad, PadShape, Pcb, PcbLayer, Trace, TraceArc, Via,
};
use vcad_ir::{stroke_font, Vec2};

/// Margin around the board outline, in millimetres, added to every side of
/// the computed viewBox.
const MARGIN_MM: f64 = 2.0;

/// Edge-cut stroke width in millimetres (thin yellow outline).
const EDGE_WIDTH_MM: f64 = 0.15;

/// Fallback board extent when the outline is empty/degenerate (mm).
const FALLBACK_EXTENT_MM: f64 = 10.0;

// ─── colours ────────────────────────────────────────────────────────────────

/// Dark board substrate fill (FR4-ish), drawn behind every layer.
const BOARD_FILL: &str = "#1b3a26";
/// Board edge-cut outline colour.
const EDGE_COLOR: &str = "#d9c200";

/// Resolve the display colour for a layer's copper/graphic geometry.
fn layer_color(layer: PcbLayer) -> &'static str {
    match layer {
        // Copper
        PcbLayer::FCu => "#c8602a",   // warm red/copper (front)
        PcbLayer::BCu => "#2a8fb0",   // blue/teal (back)
        PcbLayer::In1Cu => "#8a7a3a", // intermediate olives/golds
        PcbLayer::In2Cu => "#7a8a3a",
        PcbLayer::In3Cu => "#3a8a6a",
        PcbLayer::In4Cu => "#6a3a8a",
        PcbLayer::In5Cu => "#8a3a6a",
        PcbLayer::In6Cu => "#3a6a8a",
        // Silkscreen
        PcbLayer::FSilkS => "#f2f2ec", // white (front)
        PcbLayer::BSilkS => "#c9c9c0", // off-white (back)
        // Solder mask
        PcbLayer::FMask => "#2f9e44",
        PcbLayer::BMask => "#1f7a33",
        // Solder paste
        PcbLayer::FPaste => "#9aa0a6",
        PcbLayer::BPaste => "#6a7075",
        // Fabrication / documentation
        PcbLayer::FFab => "#b08968",
        PcbLayer::BFab => "#7a5c44",
        PcbLayer::FCrtYd => "#b06aa0",
        PcbLayer::BCrtYd => "#7a4a70",
        // Mechanical
        PcbLayer::EdgeCuts => EDGE_COLOR,
        PcbLayer::UserDrawings => "#9aa0a6",
        PcbLayer::UserComments => "#7a8088",
    }
}

/// Z-order rank for a layer — lower draws first (further back). Copper sits
/// above the board fill; silk sits above copper; edge cuts on top.
fn layer_rank(layer: PcbLayer) -> u8 {
    match layer {
        PcbLayer::EdgeCuts => 9,
        PcbLayer::FSilkS | PcbLayer::BSilkS => 7,
        PcbLayer::FFab
        | PcbLayer::BFab
        | PcbLayer::FCrtYd
        | PcbLayer::BCrtYd
        | PcbLayer::UserDrawings
        | PcbLayer::UserComments => 6,
        PcbLayer::FMask | PcbLayer::BMask | PcbLayer::FPaste | PcbLayer::BPaste => 5,
        // Copper
        _ => 3,
    }
}

// ─── KiCad-style layer name parsing ──────────────────────────────────────────

/// Parse a layer name. Accepts the serde variant names (`"FCu"`, `"FSilkS"`,
/// `"EdgeCuts"`), KiCad dotted names (`"F.Cu"`, `"Edge.Cuts"`, `"Dwgs.User"`),
/// and the Gerber-filename underscored names (`"F_Cu"`, `"Edge_Cuts"`).
/// Separators (`.` and `_`) are normalized away, so all three spellings of a
/// given layer resolve identically; the canonical letters are case-sensitive.
fn parse_layer(name: &str) -> Option<PcbLayer> {
    // Collapse `F.Cu` / `F_Cu` / `FCu` (and `Dwgs.User` / `DwgsUser`) to one key.
    let key: String = name
        .trim()
        .chars()
        .filter(|c| *c != '.' && *c != '_')
        .collect();
    let layer = match key.as_str() {
        "FCu" => PcbLayer::FCu,
        "BCu" => PcbLayer::BCu,
        "In1Cu" => PcbLayer::In1Cu,
        "In2Cu" => PcbLayer::In2Cu,
        "In3Cu" => PcbLayer::In3Cu,
        "In4Cu" => PcbLayer::In4Cu,
        "In5Cu" => PcbLayer::In5Cu,
        "In6Cu" => PcbLayer::In6Cu,
        "FSilkS" => PcbLayer::FSilkS,
        "BSilkS" => PcbLayer::BSilkS,
        "FMask" => PcbLayer::FMask,
        "BMask" => PcbLayer::BMask,
        "FPaste" => PcbLayer::FPaste,
        "BPaste" => PcbLayer::BPaste,
        "FFab" => PcbLayer::FFab,
        "BFab" => PcbLayer::BFab,
        "FCrtYd" => PcbLayer::FCrtYd,
        "BCrtYd" => PcbLayer::BCrtYd,
        "EdgeCuts" => PcbLayer::EdgeCuts,
        "UserDrawings" | "DwgsUser" => PcbLayer::UserDrawings,
        "UserComments" | "CmtsUser" => PcbLayer::UserComments,
        _ => return None,
    };
    Some(layer)
}

/// Push a non-empty SVG fragment onto the z-ordered draw list at `rank`,
/// stamping it with its insertion index so the later stable sort preserves
/// emission order within a rank.
fn push_item(items: &mut Vec<(u8, usize, String)>, rank: u8, frag: String) {
    if frag.is_empty() {
        return;
    }
    let n = items.len();
    items.push((rank, n, frag));
}

// ─── bounds ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Bounds {
    fn empty() -> Self {
        Bounds {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    fn add(&mut self, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn is_valid(&self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x >= self.min_x
            && self.max_y >= self.min_y
    }
}

/// Compute board bounds from the outline (fall back to all geometry if the
/// outline is empty, then to a fixed extent so the SVG is never degenerate).
fn board_bounds(pcb: &Pcb) -> Bounds {
    let mut b = Bounds::empty();
    for v in &pcb.outline.vertices {
        b.add(v.x, v.y);
    }
    if b.is_valid() {
        return b;
    }
    // No usable outline — span all geometry so the preview still frames.
    for t in &pcb.traces {
        b.add(t.start.x, t.start.y);
        b.add(t.end.x, t.end.y);
    }
    for via in &pcb.vias {
        b.add(via.position.x, via.position.y);
    }
    for fp in &pcb.footprints {
        b.add(fp.position.x, fp.position.y);
    }
    if b.is_valid() {
        return b;
    }
    Bounds {
        min_x: 0.0,
        min_y: 0.0,
        max_x: FALLBACK_EXTENT_MM,
        max_y: FALLBACK_EXTENT_MM,
    }
}

// ─── coordinate transform ────────────────────────────────────────────────────

/// Maps board-local mm (Y up) into SVG pixel space (Y down), flipping
/// vertically and applying `scale` (pixels per mm) plus the margin offset.
struct Xf {
    min_x: f64,
    max_y: f64,
    scale: f64,
    margin_px: f64,
}

impl Xf {
    fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.min_x) * self.scale + self.margin_px,
            // Flip Y: board top (max_y) maps to SVG y = margin.
            (self.max_y - y) * self.scale + self.margin_px,
        )
    }

    fn px(&self, mm: f64) -> f64 {
        mm * self.scale
    }
}

// ─── small SVG geometry helpers ──────────────────────────────────────────────

/// Format a `points="..."` attribute body from board-space vertices.
fn points_attr(xf: &Xf, verts: &[Vec2]) -> String {
    let mut s = String::new();
    for (i, v) in verts.iter().enumerate() {
        let (px, py) = xf.pt(v.x, v.y);
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{px:.3},{py:.3}");
    }
    s
}

/// Rotate a local pad/text offset `(dx, dy)` (board-space mm) by `deg`
/// counter-clockwise and translate by `origin`.
fn place(origin: Vec2, deg: f64, dx: f64, dy: f64) -> Vec2 {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    Vec2::new(origin.x + dx * c - dy * s, origin.y + dx * s + dy * c)
}

/// Approximate an arc (degrees) as a polyline of board-space points.
fn arc_points(center: Vec2, radius: f64, start_deg: f64, end_deg: f64) -> Vec<Vec2> {
    let sweep = (end_deg - start_deg).abs();
    // ~2° per segment, min 2 points.
    let segs = (sweep / 2.0).ceil().max(1.0) as usize;
    let mut pts = Vec::with_capacity(segs + 1);
    for i in 0..=segs {
        let t = i as f64 / segs as f64;
        let a = (start_deg + (end_deg - start_deg) * t).to_radians();
        pts.push(Vec2::new(
            center.x + radius * a.cos(),
            center.y + radius * a.sin(),
        ));
    }
    pts
}

// ─── per-layer drawing ───────────────────────────────────────────────────────

/// Append an open polyline path with round caps/joins.
fn push_polyline(out: &mut String, xf: &Xf, pts: &[Vec2], color: &str, width_px: f64) {
    if pts.len() < 2 {
        return;
    }
    out.push_str("<polyline points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(
        out,
        "\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{width_px:.3}\" \
         stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
    );
}

/// Append a filled polygon.
fn push_polygon(out: &mut String, xf: &Xf, pts: &[Vec2], fill: &str, opacity: f64) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<polygon points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(out, "\" fill=\"{fill}\" fill-opacity=\"{opacity:.3}\"/>");
}

/// Draw a trace segment on `layer` if it is requested.
fn draw_trace(out: &mut String, xf: &Xf, t: &Trace) {
    let color = layer_color(t.layer);
    let w = xf.px(t.width).max(0.05);
    let (x1, y1) = xf.pt(t.start.x, t.start.y);
    let (x2, y2) = xf.pt(t.end.x, t.end.y);
    let _ = write!(
        out,
        "<line x1=\"{x1:.3}\" y1=\"{y1:.3}\" x2=\"{x2:.3}\" y2=\"{y2:.3}\" \
         stroke=\"{color}\" stroke-width=\"{w:.3}\" stroke-linecap=\"round\"/>"
    );
}

/// Draw an arc-routed trace as a stroked path.
fn draw_trace_arc(out: &mut String, xf: &Xf, a: &TraceArc) {
    let color = layer_color(a.layer);
    let w = xf.px(a.width).max(0.05);
    let pts = arc_points(a.center, a.radius, a.start_angle, a.end_angle);
    push_polyline(out, xf, &pts, color, w);
}

/// Draw a pad as a filled shape, honouring rotation. `origin` is the pad's
/// board-space centre, `rot` the total rotation (footprint + pad) in degrees.
fn draw_pad(out: &mut String, xf: &Xf, pad: &Pad, origin: Vec2, rot: f64, color: &str) {
    // Build the pad outline in board space.
    let poly: Vec<Vec2> = match &pad.shape {
        PadShape::Circle { diameter } => {
            // Emit a real <circle>; rotation is irrelevant.
            let (cx, cy) = xf.pt(origin.x, origin.y);
            let r = xf.px(diameter / 2.0).max(0.05);
            let _ = write!(
                out,
                "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{r:.3}\" fill=\"{color}\"/>"
            );
            return;
        }
        PadShape::Rect { width, height } => rect_poly(origin, rot, *width, *height),
        PadShape::Oval { width, height } => {
            // Stadium: round-capped thick line along the long axis.
            oval_path(out, xf, origin, rot, *width, *height, color);
            return;
        }
        PadShape::RoundRect {
            width,
            height,
            corner_ratio,
        } => {
            // Approximate: filled rect; the corner radius is a visual nicety
            // we skip for the flat preview (kept simple, still reads as a pad).
            let _ = corner_ratio;
            rect_poly(origin, rot, *width, *height)
        }
        PadShape::Custom { vertices } => vertices
            .iter()
            .map(|v| place(origin, rot, v.x, v.y))
            .collect(),
    };
    push_filled_polygon(out, xf, &poly, color);
}

/// Build a rotated rectangle polygon (4 corners) centred at `origin`.
fn rect_poly(origin: Vec2, rot: f64, w: f64, h: f64) -> Vec<Vec2> {
    let hw = w / 2.0;
    let hh = h / 2.0;
    vec![
        place(origin, rot, -hw, -hh),
        place(origin, rot, hw, -hh),
        place(origin, rot, hw, hh),
        place(origin, rot, -hw, hh),
    ]
}

/// Render an oval pad as a round-capped stroke along its long axis.
fn oval_path(out: &mut String, xf: &Xf, origin: Vec2, rot: f64, w: f64, h: f64, color: &str) {
    // Thickness = short axis; the capsule spans the long axis.
    let (long, short, axis_deg) = if w >= h {
        (w, h, rot)
    } else {
        (h, w, rot + 90.0)
    };
    let half = (long - short).max(0.0) / 2.0;
    let r = axis_deg.to_radians();
    let (s, c) = r.sin_cos();
    let a = Vec2::new(origin.x - half * c, origin.y - half * s);
    let b = Vec2::new(origin.x + half * c, origin.y + half * s);
    let (ax, ay) = xf.pt(a.x, a.y);
    let (bx, by) = xf.pt(b.x, b.y);
    let stroke = xf.px(short).max(0.05);
    let _ = write!(
        out,
        "<line x1=\"{ax:.3}\" y1=\"{ay:.3}\" x2=\"{bx:.3}\" y2=\"{by:.3}\" \
         stroke=\"{color}\" stroke-width=\"{stroke:.3}\" stroke-linecap=\"round\"/>"
    );
}

/// Append a filled polygon (no opacity attr — fully opaque copper).
fn push_filled_polygon(out: &mut String, xf: &Xf, pts: &[Vec2], fill: &str) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<polygon points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(out, "\" fill=\"{fill}\"/>");
}

/// Draw a via as an annulus (outer pad ring with a drilled centre).
fn draw_via(out: &mut String, xf: &Xf, v: &Via, color: &str) {
    let (cx, cy) = xf.pt(v.position.x, v.position.y);
    let outer = xf.px(v.diameter / 2.0).max(0.05);
    let inner = xf.px(v.drill / 2.0).clamp(0.0, outer);
    // Outer copper ring.
    let _ = write!(
        out,
        "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{outer:.3}\" fill=\"{color}\"/>"
    );
    // Drilled hole (board substrate colour shows through).
    if inner > 0.0 {
        let _ = write!(
            out,
            "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{inner:.3}\" fill=\"{BOARD_FILL}\"/>"
        );
    }
}

/// Draw a poured copper zone from its *filled* rings — the outline minus the
/// clearance voids around other-net copper. CCW outer rings are translucent
/// copper; CW void rings are punched back to the board fill, so the plane shows
/// its real cut-outs and thermal reliefs instead of a solid flood.
fn draw_zone(out: &mut String, xf: &Xf, layer: PcbLayer, rings: &[Vec<Vec2>]) {
    let color = layer_color(layer);
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        if ring_signed_area(ring) >= 0.0 {
            push_polygon(out, xf, ring, color, 0.4);
        } else {
            push_polygon(out, xf, ring, BOARD_FILL, 1.0);
        }
    }
}

/// Signed area of a ring (shoelace). Positive = CCW (copper outer), negative =
/// CW (a punched void).
fn ring_signed_area(ring: &[Vec2]) -> f64 {
    let n = ring.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    0.5 * s
}

/// Parameters for a stroke-font text run, centred on `position`.
struct TextRun<'a> {
    text: &'a str,
    position: Vec2,
    /// Rotation in degrees (counter-clockwise).
    rotation: f64,
    /// Cap height in mm.
    height: f64,
    /// Stroke width in mm.
    width: f64,
    color: &'a str,
}

/// Draw single-stroke text via the shared stroke font, centred on
/// `run.position`.
fn draw_text(out: &mut String, xf: &Xf, run: &TextRun) {
    let strokes = stroke_font::text_strokes(run.text, run.height);
    if strokes.is_empty() {
        return;
    }
    let w = xf.px(run.width).max(0.05);
    // Centre the laid-out text on `position` (font origin is left baseline).
    let tw = stroke_font::text_width(run.text, run.height);
    let dx0 = -tw / 2.0;
    let dy0 = -run.height / 2.0;
    for stroke in &strokes {
        let pts: Vec<Vec2> = stroke
            .iter()
            // Local font coords: x right, y up, baseline y=0.
            .map(|&(lx, ly)| place(run.position, run.rotation, dx0 + lx, dy0 + ly))
            .collect();
        push_polyline(out, xf, &pts, run.color, w);
    }
}

/// Draw a footprint graphic (silkscreen / fabrication / courtyard).
fn draw_graphic(out: &mut String, xf: &Xf, g: &FootprintGraphic, origin: Vec2, frot: f64) {
    match g {
        FootprintGraphic::Line {
            start,
            end,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let a = place(origin, frot, start.x, start.y);
            let b = place(origin, frot, end.x, end.y);
            push_polyline(out, xf, &[a, b], color, xf.px(*width).max(0.05));
        }
        FootprintGraphic::Circle {
            center,
            radius,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let c = place(origin, frot, center.x, center.y);
            let (cx, cy) = xf.pt(c.x, c.y);
            let r = xf.px(*radius).max(0.05);
            let w = xf.px(*width).max(0.05);
            let _ = write!(
                out,
                "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{r:.3}\" fill=\"none\" \
                 stroke=\"{color}\" stroke-width=\"{w:.3}\"/>"
            );
        }
        FootprintGraphic::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let pts: Vec<Vec2> = arc_points(*center, *radius, *start_angle, *end_angle)
                .into_iter()
                .map(|p| place(origin, frot, p.x, p.y))
                .collect();
            push_polyline(out, xf, &pts, color, xf.px(*width).max(0.05));
        }
        FootprintGraphic::Rect {
            start,
            end,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let corners = [
                place(origin, frot, start.x, start.y),
                place(origin, frot, end.x, start.y),
                place(origin, frot, end.x, end.y),
                place(origin, frot, start.x, end.y),
                place(origin, frot, start.x, start.y),
            ];
            push_polyline(out, xf, &corners, color, xf.px(*width).max(0.05));
        }
        FootprintGraphic::Polygon {
            vertices,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let mut pts: Vec<Vec2> = vertices
                .iter()
                .map(|v| place(origin, frot, v.x, v.y))
                .collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first); // close
            }
            push_polyline(out, xf, &pts, color, xf.px(*width).max(0.05));
        }
        FootprintGraphic::Text {
            text,
            position,
            rotation,
            height,
            width,
            layer,
        } => {
            let color = layer_color(*layer);
            let p = place(origin, frot, position.x, position.y);
            draw_text(
                out,
                xf,
                &TextRun {
                    text,
                    position: p,
                    rotation: frot + rotation,
                    height: *height,
                    width: *width,
                    color,
                },
            );
        }
    }
}

/// Returns the layer a footprint graphic lives on.
fn graphic_layer(g: &FootprintGraphic) -> PcbLayer {
    match g {
        FootprintGraphic::Line { layer, .. }
        | FootprintGraphic::Circle { layer, .. }
        | FootprintGraphic::Arc { layer, .. }
        | FootprintGraphic::Rect { layer, .. }
        | FootprintGraphic::Polygon { layer, .. }
        | FootprintGraphic::Text { layer, .. } => *layer,
    }
}

// ─── board outline ───────────────────────────────────────────────────────────

/// Draw the board substrate fill (outline polygon minus cutouts). The thin
/// yellow edge outline is drawn separately (see [`draw_edges`]) only when the
/// EdgeCuts layer is requested.
fn draw_board(out: &mut String, xf: &Xf, outline: &BoardOutline) {
    if outline.vertices.len() < 3 {
        return;
    }
    if outline.cutouts.is_empty() {
        // Simple filled polygon.
        push_filled_polygon(out, xf, &outline.vertices, BOARD_FILL);
    } else {
        // One even-odd path so cutouts read as holes (SVG has no per-shape
        // erase; even-odd on a multi-ring path is the standard idiom).
        let mut d = String::new();
        path_ring(&mut d, xf, &outline.vertices);
        for c in &outline.cutouts {
            path_ring(&mut d, xf, c);
        }
        let _ = write!(
            out,
            "<path d=\"{d}\" fill=\"{BOARD_FILL}\" fill-rule=\"evenodd\"/>"
        );
    }
}

/// Append one closed ring (`M ... Z`) to a path data string.
fn path_ring(d: &mut String, xf: &Xf, verts: &[Vec2]) {
    for (i, v) in verts.iter().enumerate() {
        let (px, py) = xf.pt(v.x, v.y);
        let cmd = if i == 0 { 'M' } else { 'L' };
        let _ = write!(d, "{cmd}{px:.3} {py:.3} ");
    }
    d.push('Z');
    d.push(' ');
}

/// Draw the edge-cut outline (and cutout rings) as thin yellow strokes.
fn draw_edges(out: &mut String, xf: &Xf, outline: &BoardOutline) {
    let w = xf.px(EDGE_WIDTH_MM).max(0.05);
    let mut rings: Vec<&Vec<Vec2>> = vec![&outline.vertices];
    for c in &outline.cutouts {
        rings.push(c);
    }
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        let mut pts = ring.clone();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        push_polyline(out, xf, &pts, EDGE_COLOR, w);
    }
}

// ─── public API ──────────────────────────────────────────────────────────────

/// Render a [`Pcb`] to a flat, top-down (looking down −Z, Y up) SVG showing
/// only the requested `layers`, composited in a sensible z-order.
///
/// `scale` is pixels per millimetre. The viewBox is derived from the board
/// outline bounds plus a fixed margin. The board substrate is always drawn as
/// a dark background fill so silkscreen (white) reads against it; the
/// requested layers are then drawn back-to-front by their natural z-order
/// (copper below silk below edge cuts), each in its own colour. No front/back
/// mirroring is applied — the caller picks which layers to include.
///
/// Geometry drawn per layer:
/// - **EdgeCuts**: board outline + cutout rings (thin yellow).
/// - **copper layers**: traces, trace arcs, pads, and vias on that layer,
///   plus copper-pour zones (translucent).
/// - **silk / fab / courtyard / paste / mask**: footprint graphics on that
///   layer (lines, circles, arcs, rects, polygons, and stroke-font text).
///
/// Returns a self-contained `<svg>…</svg>` string. Never errors — a PCB with
/// no usable outline still produces a small framed SVG.
///
/// [`Pcb`]: vcad_ir::ecad::Pcb
pub fn render_pcb_svg(pcb: &Pcb, layers: &[PcbLayer], scale: f64) -> String {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        crate::DEFAULT_SCALE
    };

    let bounds = board_bounds(pcb);
    let xf = Xf {
        min_x: bounds.min_x,
        max_y: bounds.max_y,
        scale,
        margin_px: MARGIN_MM * scale,
    };

    let span_x = (bounds.max_x - bounds.min_x).max(0.0);
    let span_y = (bounds.max_y - bounds.min_y).max(0.0);
    let w = span_x * scale + 2.0 * MARGIN_MM * scale;
    let h = span_y * scale + 2.0 * MARGIN_MM * scale;

    // Membership test for "is this layer requested?".
    let want = |l: PcbLayer| layers.contains(&l);
    let edge_requested = want(PcbLayer::EdgeCuts);

    let mut body = String::new();

    // 1. Board substrate fill (always — gives silk something to read against).
    draw_board(&mut body, &xf, &pcb.outline);

    // Build a draw list keyed by (rank, insertion order) so layers composite
    // back-to-front regardless of the order the caller passed them. Each entry
    // is `(rank, insertion index, svg fragment)`.
    let mut items: Vec<(u8, usize, String)> = Vec::new();

    // 2. Zones (copper pours) — drawn lowest among copper. Pour each so the
    //    render shows the real clearance voids + thermal relief, not a solid
    //    flood. `fill_zones` returns one result per zone, in order.
    let filled = vcad_ecad_pcb::copper_pour::fill_zones(pcb);
    for (i, z) in pcb.zones.iter().enumerate() {
        if want(z.layer) {
            let rings = filled
                .get(i)
                .map(|f| f.polygons.clone())
                .unwrap_or_else(|| vec![z.outline.clone()]);
            let mut s = String::new();
            draw_zone(&mut s, &xf, z.layer, &rings);
            // Force zones below traces of the same rank by nudging rank down.
            push_item(&mut items, layer_rank(z.layer).saturating_sub(1), s);
        }
    }

    // 3. Copper traces + arcs.
    for t in &pcb.traces {
        if want(t.layer) {
            let mut s = String::new();
            draw_trace(&mut s, &xf, t);
            push_item(&mut items, layer_rank(t.layer), s);
        }
    }
    for a in &pcb.trace_arcs {
        if want(a.layer) {
            let mut s = String::new();
            draw_trace_arc(&mut s, &xf, a);
            push_item(&mut items, layer_rank(a.layer), s);
        }
    }

    // 4. Pads (per footprint, honouring footprint + pad rotation).
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            // A pad is shown on a requested copper layer it belongs to.
            let pad_layer = pad.layers.iter().copied().find(|l| want(*l));
            let Some(pl) = pad_layer else { continue };
            let origin = place(fp.position, fp.rotation, pad.position.x, pad.position.y);
            let total_rot = fp.rotation + pad.rotation;
            let mut s = String::new();
            draw_pad(&mut s, &xf, pad, origin, total_rot, layer_color(pl));
            // Pads sit just above traces.
            push_item(&mut items, layer_rank(pl) + 1, s);
        }
    }

    // 5. Vias (over copper). Shown if either endpoint layer is requested.
    for v in &pcb.vias {
        if want(v.start_layer) || want(v.end_layer) {
            let color = if want(v.start_layer) {
                layer_color(v.start_layer)
            } else {
                layer_color(v.end_layer)
            };
            let mut s = String::new();
            draw_via(&mut s, &xf, v, color);
            push_item(&mut items, 4u8, s); // just above pads/copper
        }
    }

    // 6. Footprint graphics (silk / fab / courtyard / mask / paste).
    for fp in &pcb.footprints {
        for g in &fp.graphics {
            let gl = graphic_layer(g);
            if want(gl) {
                let mut s = String::new();
                draw_graphic(&mut s, &xf, g, fp.position, fp.rotation);
                push_item(&mut items, layer_rank(gl), s);
            }
        }
    }

    // 7. Edge cuts on top.
    if edge_requested {
        let mut s = String::new();
        draw_edges(&mut s, &xf, &pcb.outline);
        push_item(&mut items, layer_rank(PcbLayer::EdgeCuts), s);
    }

    // Stable sort by (rank, insertion order) and concatenate.
    items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, _, frag) in &items {
        body.push_str(frag);
    }

    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}\" height=\"{h:.2}\" \
         viewBox=\"0 0 {w:.2} {h:.2}\" role=\"img\" aria-label=\"vcad pcb render\">"
    );
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

/// JSON wrapper around [`render_pcb_svg`] for the WASM/MCP integration layer.
///
/// `pcb_json` is a serialized [`Pcb`]; `layers_json` is a JSON array of layer
/// names — either the serde variant names (`"FCu"`, `"FSilkS"`, `"EdgeCuts"`)
/// or KiCad dotted names (`"F.Cu"`, `"F.SilkS"`, `"Edge.Cuts"`). Unknown
/// layer names are rejected with an error listing the offender.
///
/// Returns the SVG string on success, or a human-readable error
/// (`"pcb json: …"` / `"layers json: …"` / `"unknown layer: …"`).
///
/// [`Pcb`]: vcad_ir::ecad::Pcb
pub fn render_pcb_svg_json(
    pcb_json: &str,
    layers_json: &str,
    scale: f64,
) -> Result<String, String> {
    let pcb: Pcb = serde_json::from_str(pcb_json).map_err(|e| format!("pcb json: {e}"))?;
    let names: Vec<String> =
        serde_json::from_str(layers_json).map_err(|e| format!("layers json: {e}"))?;
    let mut layers = Vec::with_capacity(names.len());
    for n in &names {
        match parse_layer(n) {
            Some(l) => layers.push(l),
            None => return Err(format!("unknown layer: {n}")),
        }
    }
    Ok(render_pcb_svg(&pcb, &layers, scale))
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::{
        BoardOutline, DesignRules, Footprint, LayerStackup, NetClassRules, Pad, PadType, Pcb,
        PcbLayer, Trace, Via,
    };

    fn sample_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(20.0, 0.0),
                    Vec2::new(20.0, 15.0),
                    Vec2::new(0.0, 15.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup { layers: vec![] },
            nets: vec![],
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
            footprints: vec![Footprint {
                reference: "R1".to_string(),
                value: "10k".to_string(),
                footprint_name: "R_0805".to_string(),
                position: Vec2::new(10.0, 7.5),
                rotation: 0.0,
                front: true,
                pads: vec![Pad {
                    number: "1".to_string(),
                    pad_type: PadType::SMD,
                    shape: PadShape::Rect {
                        width: 1.0,
                        height: 1.2,
                    },
                    position: Vec2::new(-1.0, 0.0),
                    rotation: 0.0,
                    drill: None,
                    net: Some("1".to_string()),
                    layers: vec![PcbLayer::FCu],
                }],
                graphics: vec![FootprintGraphic::Text {
                    text: "R1".to_string(),
                    position: Vec2::new(0.0, 1.5),
                    rotation: 0.0,
                    height: 1.0,
                    width: 0.15,
                    layer: PcbLayer::FSilkS,
                }],
                model_3d: None,
                properties: std::collections::HashMap::new(),
            }],
            traces: vec![Trace {
                start: Vec2::new(2.0, 7.5),
                end: Vec2::new(9.0, 7.5),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".to_string(),
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(5.0, 7.5),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
            }],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn renders_all_layers() {
        let pcb = sample_pcb();
        let svg = render_pcb_svg(
            &pcb,
            &[PcbLayer::EdgeCuts, PcbLayer::FCu, PcbLayer::FSilkS],
            8.0,
        );
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        // Trace + via drawn as <line>/<circle>.
        assert!(svg.contains("<line"), "expected a trace line");
        assert!(svg.contains("<circle"), "expected a via circle");
        // Front copper colour present (trace/pad/via).
        assert!(svg.contains(layer_color(PcbLayer::FCu)));
        // Edge-cut yellow present.
        assert!(svg.contains(EDGE_COLOR));
        // Board substrate fill present.
        assert!(svg.contains(BOARD_FILL));
        // Silk text → polylines in the silk colour.
        assert!(svg.contains("<polyline"), "expected silk text strokes");
        assert!(svg.contains(layer_color(PcbLayer::FSilkS)));
    }

    #[test]
    fn fcu_only_omits_silk() {
        let pcb = sample_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        // No silkscreen colour when silk is not requested.
        assert!(
            !svg.contains(layer_color(PcbLayer::FSilkS)),
            "silk colour leaked into FCu-only render"
        );
        // No edge-cut colour either (EdgeCuts not requested).
        assert!(
            !svg.contains(EDGE_COLOR),
            "edge colour leaked into FCu-only render"
        );
        // Copper still present.
        assert!(svg.contains(layer_color(PcbLayer::FCu)));
        assert!(svg.contains("<line"));
    }

    #[test]
    fn viewbox_tracks_outline_bounds() {
        let pcb = sample_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::EdgeCuts], 8.0);
        // 20mm × 15mm board + 2mm margin all sides = 24mm × 19mm @ 8px/mm.
        let attr = |name: &str| -> f64 {
            let pat = format!("{name}=\"");
            let start = svg.find(&pat).unwrap() + pat.len();
            let end = svg[start..].find('"').unwrap() + start;
            svg[start..end].parse().unwrap()
        };
        assert!(
            (attr("width") - 24.0 * 8.0).abs() < 0.5,
            "w {}",
            attr("width")
        );
        assert!(
            (attr("height") - 19.0 * 8.0).abs() < 0.5,
            "h {}",
            attr("height")
        );
    }

    #[test]
    fn json_round_trips() {
        let pcb = sample_pcb();
        let pcb_json = serde_json::to_string(&pcb).unwrap();
        // Mix serde + KiCad-style names.
        let svg = render_pcb_svg_json(&pcb_json, r#"["F.Cu","F.SilkS","Edge.Cuts"]"#, 8.0).unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains(layer_color(PcbLayer::FCu)));
        assert!(svg.contains(layer_color(PcbLayer::FSilkS)));
        // Serde variant names work too.
        let svg2 = render_pcb_svg_json(&pcb_json, r#"["FCu"]"#, 8.0).unwrap();
        assert!(svg2.contains(layer_color(PcbLayer::FCu)));
        assert!(!svg2.contains(layer_color(PcbLayer::FSilkS)));
    }

    #[test]
    fn json_rejects_bad_input() {
        assert!(render_pcb_svg_json("not json", "[]", 8.0)
            .unwrap_err()
            .starts_with("pcb json:"));
        let pcb_json = serde_json::to_string(&sample_pcb()).unwrap();
        assert!(render_pcb_svg_json(&pcb_json, "not json", 8.0)
            .unwrap_err()
            .starts_with("layers json:"));
        assert!(render_pcb_svg_json(&pcb_json, r#"["Bogus.Layer"]"#, 8.0)
            .unwrap_err()
            .starts_with("unknown layer:"));
    }

    #[test]
    fn parse_layer_accepts_both_styles() {
        assert_eq!(parse_layer("FCu"), Some(PcbLayer::FCu));
        assert_eq!(parse_layer("F.Cu"), Some(PcbLayer::FCu));
        assert_eq!(parse_layer("Edge.Cuts"), Some(PcbLayer::EdgeCuts));
        assert_eq!(parse_layer("In1.Cu"), Some(PcbLayer::In1Cu));
        assert_eq!(parse_layer("Dwgs.User"), Some(PcbLayer::UserDrawings));
        assert_eq!(parse_layer("nope"), None);
    }

    #[test]
    fn empty_outline_still_renders() {
        let mut pcb = sample_pcb();
        pcb.outline.vertices.clear();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
    }
}
