//! Flat, top-down, per-layer 2D PCB renderer — "Studio Graphite".
//!
//! Where [`crate::render_svg_str`] projects a 3D CAD document to isometric
//! line art, this module renders a [`vcad_ir::ecad::Pcb`] straight down the
//! board normal (looking down −Z, Y up) as a flat, editor-style preview — the
//! "copper/silk" view an EDA tool shows, not the 3D enclosure shot.
//!
//! The default look is a dark, high-contrast theme ([`Theme::Dark`]) where the
//! conductor is the only saturated colour: front copper glows warm, back copper
//! is a cool counterpoint, and everything that is not copper (board, grid, silk,
//! edge, ratsnest) recedes. A board therefore gets visually *quieter* as it gets
//! more routed — unrouted work shows as recessive ratsnest air-wires that vanish
//! the instant copper covers them. The legacy white/green fabrication look is
//! retained as [`Theme::Light`].
//!
//! Only the requested layers are drawn, composited bottom-up in a sensible
//! z-order (background → grid → board → ratsnest → copper zones → traces → pads
//! → vias → silk/values → edge cuts on top). No front/back mirroring is applied:
//! a caller that wants a bottom view supplies the back layers.
//!
//! Coordinates are board-local millimetres. The viewBox is computed from the
//! board outline bounds plus a margin, and the image is flipped vertically on
//! output so PCB +Y (up) maps to SVG up.
//!
//! Text is drawn with the shared single-stroke font ([`vcad_ir::stroke_font`])
//! and never with `<text>` — the WASM/resvg rasterization path ships no fonts,
//! so `<text>` would silently vanish. Every label is rendered twice (a fat
//! background-colour halo pass, then the colour pass) so it stays legible over
//! copper without a font dependency.
//!
//! [`Pcb`]: vcad_ir::ecad::Pcb

use std::fmt::Write as _;

use vcad_ecad_pcb::ratsnest::{compute_ratsnest, NetConnection, Netlist, NetlistNet};
use vcad_ir::ecad::{
    BoardOutline, Footprint, FootprintGraphic, Pad, PadShape, Pcb, PcbLayer, Trace, TraceArc, Via,
};
use vcad_ir::{stroke_font, Vec2};

/// Margin around the board outline, in millimetres, added to every side of
/// the computed viewBox. Gives the board room to sit on the gridded canvas.
const MARGIN_MM: f64 = 4.0;

/// Edge-cut stroke width in millimetres (thin scribe line).
const EDGE_WIDTH_MM: f64 = 0.15;

/// Fallback board extent when the outline is empty/degenerate (mm).
const FALLBACK_EXTENT_MM: f64 = 10.0;

/// Minimum stroke width in device pixels, so hairline traces never sub-pixel to
/// nothing through the rasterizer.
const MIN_STROKE_PX: f64 = 0.35;

/// Below this rendered width (px) the board is "zoomed out" and value labels are
/// suppressed to avoid a wall of unreadable text.
const VALUE_LABEL_MIN_WIDTH_PX: f64 = 700.0;

/// A net's annotation is skipped unless its longest routed segment is at least
/// this long (mm) — keeps short fan-out stubs from drowning a dense board in
/// overlapping net names.
const NETLABEL_MIN_TRACE_MM: f64 = 4.0;

/// Opacity applied to the non-primary (back) copper when both front and back
/// copper layers are shown, so the active side reads on top.
const BACK_COPPER_OPACITY: f64 = 0.55;

/// Opacity applied to everything *not* highlighted, when a highlight is active.
const HIGHLIGHT_DIM: f64 = 0.32;

// ─── palette ─────────────────────────────────────────────────────────────────

/// A full colour palette for the 2D board render. Every field is an SVG colour
/// string. Two presets exist: [`DARK`] (default — "Studio Graphite") and
/// [`LIGHT`] (the legacy fabrication-preview look).
#[derive(Clone, Copy)]
pub struct Palette {
    /// Page/canvas background (behind everything).
    pub background: &'static str,
    /// Minor grid lines (50 mil).
    pub grid_minor: &'static str,
    /// Major grid lines (500 mil).
    pub grid_major: &'static str,
    /// Board substrate fill.
    pub board_fill: &'static str,
    /// Thin inner stroke seating the board a hair above the page.
    pub board_edge_seat: &'static str,
    /// Front copper.
    pub f_cu: &'static str,
    /// Back copper.
    pub b_cu: &'static str,
    /// Intermediate copper layers In1..In6.
    pub inner_cu: [&'static str; 6],
    /// Front silkscreen.
    pub f_silk: &'static str,
    /// Back silkscreen.
    pub b_silk: &'static str,
    /// Front solder mask.
    pub f_mask: &'static str,
    /// Back solder mask.
    pub b_mask: &'static str,
    /// Front solder paste.
    pub f_paste: &'static str,
    /// Back solder paste.
    pub b_paste: &'static str,
    /// Front fabrication layer.
    pub f_fab: &'static str,
    /// Back fabrication layer.
    pub b_fab: &'static str,
    /// Front courtyard.
    pub f_crtyd: &'static str,
    /// Back courtyard.
    pub b_crtyd: &'static str,
    /// User drawings layer.
    pub user_dwg: &'static str,
    /// User comments layer.
    pub user_cmt: &'static str,
    /// Board edge cuts (outline).
    pub edge_cuts: &'static str,
    /// Ratsnest air-wires.
    pub ratsnest: &'static str,
    /// Pad fill (a value-step off the copper hue so pads read distinct).
    pub pad: &'static str,
    /// Via copper ring.
    pub via_ring: &'static str,
    /// Drilled bore (the dark hole).
    pub via_bore: &'static str,
    /// Inner shadow ring just inside the copper edge of a drilled hole.
    pub via_inner_ring: &'static str,
    /// Highlight / selection colour (vcad brand pink).
    pub highlight: &'static str,
    /// Component value text.
    pub value_text: &'static str,
    /// Net-name annotation on front copper.
    pub netname_f: &'static str,
    /// Net-name annotation on back copper.
    pub netname_b: &'static str,
    /// Halo colour drawn behind every glyph for legibility.
    pub text_halo: &'static str,
}

/// Default dark theme — "NERV neon". Hardcore-ECAD, Evangelion-coded: acid-lime
/// front copper and EVA-01 violet back copper glowing on a deep phosphor-black
/// canvas, a hazard-orange board edge, and hot-magenta selection. Copper still
/// carries the most saturation; the rest is neon-tinted but recessive.
pub const DARK: Palette = Palette {
    background: "#06080A",
    grid_minor: "#0E1712",
    grid_major: "#173024",
    board_fill: "#0A0F0C",
    board_edge_seat: "#040605",
    f_cu: "#9DFF34",
    b_cu: "#A24BFF",
    inner_cu: [
        "#00E5D0", "#FF3DAE", "#FFB000", "#3CA8FF", "#FF5E3A", "#B6FF00",
    ],
    f_silk: "#E8FFF2",
    b_silk: "#7FA890",
    f_mask: "#1FE06A",
    b_mask: "#13A04E",
    f_paste: "#9AA6A0",
    b_paste: "#5F7A72",
    f_fab: "#C08A4A",
    b_fab: "#7A5C44",
    f_crtyd: "#FF3DAE",
    b_crtyd: "#A02E78",
    user_dwg: "#7FE0C0",
    user_cmt: "#5A8A80",
    edge_cuts: "#FF6A1A",
    ratsnest: "#3C6E66",
    pad: "#C6FF5C",
    via_ring: "#CBFF3C",
    via_bore: "#040605",
    via_inner_ring: "#173024",
    highlight: "#FF2D95",
    value_text: "#74B894",
    netname_f: "#7CE03C",
    netname_b: "#A878F0",
    text_halo: "#06080A",
};

/// Legacy light/fabrication theme: white page, green board, the historical
/// per-layer colours. Retained as an option, not the default.
pub const LIGHT: Palette = Palette {
    background: "#ffffff",
    grid_minor: "#eef0f2",
    grid_major: "#e2e5e9",
    board_fill: "#1b3a26",
    board_edge_seat: "#0f2417",
    f_cu: "#c8602a",
    b_cu: "#2a8fb0",
    inner_cu: [
        "#8a7a3a", "#7a8a3a", "#3a8a6a", "#6a3a8a", "#8a3a6a", "#3a6a8a",
    ],
    f_silk: "#f2f2ec",
    b_silk: "#c9c9c0",
    f_mask: "#2f9e44",
    b_mask: "#1f7a33",
    f_paste: "#9aa0a6",
    b_paste: "#6a7075",
    f_fab: "#b08968",
    b_fab: "#7a5c44",
    f_crtyd: "#b06aa0",
    b_crtyd: "#7a4a70",
    user_dwg: "#9aa0a6",
    user_cmt: "#7a8088",
    edge_cuts: "#d9c200",
    ratsnest: "#8a93a0",
    pad: "#c8602a",
    via_ring: "#c8602a",
    via_bore: "#1b3a26",
    via_inner_ring: "#0f2417",
    highlight: "#d6336c",
    value_text: "#6b6b66",
    netname_f: "#a04a26",
    netname_b: "#1f6a86",
    text_halo: "#1b3a26",
};

/// Render theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Dark "Studio Graphite" (default).
    Dark,
    /// Legacy light/fabrication look.
    Light,
}

impl Theme {
    fn palette(self) -> Palette {
        match self {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }
    /// Whether the gridded canvas is drawn by default for this theme.
    fn grid_default(self) -> bool {
        matches!(self, Theme::Dark)
    }
}

/// A set of nets and/or component references to highlight (focus everything
/// else dims; highlighted copper recolours to the brand pink with a glow).
#[derive(Clone, Default)]
pub struct Highlight {
    /// Net names to highlight.
    pub nets: Vec<String>,
    /// Footprint references to highlight.
    pub refs: Vec<String>,
}

impl Highlight {
    fn active(&self) -> bool {
        !self.nets.is_empty() || !self.refs.is_empty()
    }
    fn net(&self, n: &str) -> bool {
        self.nets.iter().any(|x| x == n)
    }
    fn reference(&self, r: &str) -> bool {
        self.refs.iter().any(|x| x == r)
    }
}

/// Options controlling the board render. [`Default`] is the dark editor view
/// with values + ratsnest on and net-labels/hero off.
#[derive(Clone)]
pub struct PcbRenderOpts {
    /// Colour theme.
    pub theme: Theme,
    /// Draw the gridded canvas (defaults to the theme's preference).
    pub grid: Option<bool>,
    /// Draw component value labels (zoom-gated).
    pub show_values: bool,
    /// Draw net-name annotations on routed copper.
    pub show_net_labels: bool,
    /// Draw ratsnest air-wires for unrouted nets.
    pub show_ratsnest: bool,
    /// Hero/marketing still: copper bloom + vignette. Never for agent-eyes
    /// renders (a glow fattens copper and lies about its extent).
    pub hero: bool,
    /// Nets/refs to highlight.
    pub highlight: Highlight,
}

impl Default for PcbRenderOpts {
    fn default() -> Self {
        PcbRenderOpts {
            theme: Theme::Dark,
            grid: None,
            show_values: true,
            show_net_labels: false,
            show_ratsnest: true,
            hero: false,
            highlight: Highlight::default(),
        }
    }
}

// ─── layer colour / z-order ────────────────────────────────────────────────

/// Resolve the display colour for a layer's copper/graphic geometry.
fn layer_color(layer: PcbLayer, p: &Palette) -> &'static str {
    match layer {
        PcbLayer::FCu => p.f_cu,
        PcbLayer::BCu => p.b_cu,
        PcbLayer::In1Cu => p.inner_cu[0],
        PcbLayer::In2Cu => p.inner_cu[1],
        PcbLayer::In3Cu => p.inner_cu[2],
        PcbLayer::In4Cu => p.inner_cu[3],
        PcbLayer::In5Cu => p.inner_cu[4],
        PcbLayer::In6Cu => p.inner_cu[5],
        PcbLayer::FSilkS => p.f_silk,
        PcbLayer::BSilkS => p.b_silk,
        PcbLayer::FMask => p.f_mask,
        PcbLayer::BMask => p.b_mask,
        PcbLayer::FPaste => p.f_paste,
        PcbLayer::BPaste => p.b_paste,
        PcbLayer::FFab => p.f_fab,
        PcbLayer::BFab => p.b_fab,
        PcbLayer::FCrtYd => p.f_crtyd,
        PcbLayer::BCrtYd => p.b_crtyd,
        PcbLayer::EdgeCuts => p.edge_cuts,
        PcbLayer::UserDrawings => p.user_dwg,
        PcbLayer::UserComments => p.user_cmt,
    }
}

/// Z-order rank for a layer — lower draws first (further back). Ratsnest sits
/// at rank 2 (above board, below copper); copper above that; silk above copper;
/// edge cuts on top.
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

/// Z-rank for ratsnest air-wires: above board+grid, below all copper.
const RATSNEST_RANK: u8 = 2;

// ─── KiCad-style layer name parsing ──────────────────────────────────────────

/// Parse a layer name. Accepts the serde variant names (`"FCu"`, `"FSilkS"`,
/// `"EdgeCuts"`), KiCad dotted names (`"F.Cu"`, `"Edge.Cuts"`, `"Dwgs.User"`),
/// and the Gerber-filename underscored names (`"F_Cu"`, `"Edge_Cuts"`).
fn parse_layer(name: &str) -> Option<PcbLayer> {
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

/// Wrap a fragment in a `<g>` carrying opacity and/or a filter, when needed.
fn wrap(frag: String, opacity: f64, filter: Option<&str>) -> String {
    if frag.is_empty() {
        return frag;
    }
    let needs = opacity < 0.999 || filter.is_some();
    if !needs {
        return frag;
    }
    let mut s = String::from("<g");
    if opacity < 0.999 {
        let _ = write!(s, " opacity=\"{opacity:.3}\"");
    }
    if let Some(f) = filter {
        let _ = write!(s, " filter=\"url(#{f})\"");
    }
    s.push('>');
    s.push_str(&frag);
    s.push_str("</g>");
    s
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

/// Maps board-local mm (Y up) into SVG pixel space (Y down).
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

/// Rotate a local offset `(dx, dy)` (board-space mm) by `deg` counter-clockwise
/// and translate by `origin`.
fn place(origin: Vec2, deg: f64, dx: f64, dy: f64) -> Vec2 {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    Vec2::new(origin.x + dx * c - dy * s, origin.y + dx * s + dy * c)
}

/// Approximate an arc (degrees) as a polyline of board-space points.
fn arc_points(center: Vec2, radius: f64, start_deg: f64, end_deg: f64) -> Vec<Vec2> {
    let sweep = (end_deg - start_deg).abs();
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

/// Append a filled polygon with an explicit opacity.
fn push_polygon(out: &mut String, xf: &Xf, pts: &[Vec2], fill: &str, opacity: f64) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<polygon points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(out, "\" fill=\"{fill}\" fill-opacity=\"{opacity:.3}\"/>");
}

/// Append a filled polygon (fully opaque).
fn push_filled_polygon(out: &mut String, xf: &Xf, pts: &[Vec2], fill: &str) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<polygon points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(out, "\" fill=\"{fill}\"/>");
}

/// Append a stroked (outline-only) polygon ring with opacity.
fn push_ring_outline(
    out: &mut String,
    xf: &Xf,
    pts: &[Vec2],
    color: &str,
    width_px: f64,
    opacity: f64,
) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<polygon points=\"");
    out.push_str(&points_attr(xf, pts));
    let _ = write!(
        out,
        "\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{width_px:.3}\" \
         stroke-opacity=\"{opacity:.3}\" stroke-linejoin=\"round\"/>"
    );
}

/// Draw a straight trace segment in `color`.
fn draw_trace(out: &mut String, xf: &Xf, t: &Trace, color: &str) {
    let w = xf.px(t.width).max(MIN_STROKE_PX);
    let (x1, y1) = xf.pt(t.start.x, t.start.y);
    let (x2, y2) = xf.pt(t.end.x, t.end.y);
    let _ = write!(
        out,
        "<line x1=\"{x1:.3}\" y1=\"{y1:.3}\" x2=\"{x2:.3}\" y2=\"{y2:.3}\" \
         stroke=\"{color}\" stroke-width=\"{w:.3}\" stroke-linecap=\"round\"/>"
    );
}

/// Draw an arc-routed trace as a stroked path in `color`.
fn draw_trace_arc(out: &mut String, xf: &Xf, a: &TraceArc, color: &str) {
    let w = xf.px(a.width).max(MIN_STROKE_PX);
    let pts = arc_points(a.center, a.radius, a.start_angle, a.end_angle);
    push_polyline(out, xf, &pts, color, w);
}

/// Draw a pad as a filled shape, honouring rotation. `origin` is the pad's
/// board-space centre, `rot` the total rotation in degrees. If the pad is
/// through-hole (`pad.drill`), a drilled bore + inner shadow ring is punched
/// using palette `p`.
fn draw_pad(
    out: &mut String,
    xf: &Xf,
    pad: &Pad,
    origin: Vec2,
    rot: f64,
    color: &str,
    p: &Palette,
) {
    match &pad.shape {
        PadShape::Circle { diameter } => {
            let (cx, cy) = xf.pt(origin.x, origin.y);
            let r = xf.px(diameter / 2.0).max(MIN_STROKE_PX);
            let _ = write!(
                out,
                "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{r:.3}\" fill=\"{color}\"/>"
            );
        }
        PadShape::Rect { width, height } => {
            push_filled_polygon(out, xf, &rect_poly(origin, rot, *width, *height), color);
        }
        PadShape::Oval { width, height } => {
            oval_path(out, xf, origin, rot, *width, *height, color);
        }
        PadShape::RoundRect {
            width,
            height,
            corner_ratio,
        } => {
            let _ = corner_ratio;
            push_filled_polygon(out, xf, &rect_poly(origin, rot, *width, *height), color);
        }
        PadShape::Custom { vertices } => {
            let poly: Vec<Vec2> = vertices
                .iter()
                .map(|v| place(origin, rot, v.x, v.y))
                .collect();
            push_filled_polygon(out, xf, &poly, color);
        }
    }
    // Through-hole bore + inner shadow ring.
    if let Some(drill) = &pad.drill {
        draw_bore(out, xf, origin, drill.diameter, p);
    }
}

/// Draw a drilled bore (dark hole + inner shadow ring) of `drill` diameter.
fn draw_bore(out: &mut String, xf: &Xf, center: Vec2, drill: f64, p: &Palette) {
    let (cx, cy) = xf.pt(center.x, center.y);
    let r = xf.px(drill / 2.0).max(MIN_STROKE_PX);
    let _ = write!(
        out,
        "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{r:.3}\" fill=\"{}\"/>",
        p.via_bore
    );
    // Inner shadow ring just inside the copper edge.
    let ring_w = (r * 0.18).max(0.3);
    let _ = write!(
        out,
        "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{:.3}\" fill=\"none\" \
         stroke=\"{}\" stroke-width=\"{ring_w:.3}\"/>",
        r + ring_w * 0.5,
        p.via_inner_ring
    );
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
    let stroke = xf.px(short).max(MIN_STROKE_PX);
    let _ = write!(
        out,
        "<line x1=\"{ax:.3}\" y1=\"{ay:.3}\" x2=\"{bx:.3}\" y2=\"{by:.3}\" \
         stroke=\"{color}\" stroke-width=\"{stroke:.3}\" stroke-linecap=\"round\"/>"
    );
}

/// Draw a via as an annulus with a drilled bore + inner shadow ring.
fn draw_via(out: &mut String, xf: &Xf, v: &Via, ring_color: &str, p: &Palette) {
    let (cx, cy) = xf.pt(v.position.x, v.position.y);
    let outer = xf.px(v.diameter / 2.0).max(MIN_STROKE_PX);
    let _ = write!(
        out,
        "<circle cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{outer:.3}\" fill=\"{ring_color}\"/>"
    );
    if v.drill > 0.0 {
        draw_bore(out, xf, v.position, v.drill, p);
    }
}

/// Draw a poured copper zone from its filled rings. CCW outer rings are a
/// translucent copper wash with a thin same-hue boundary stroke; CW void rings
/// punch back to the board fill.
fn draw_zone(out: &mut String, xf: &Xf, rings: &[Vec<Vec2>], copper: &str, board: &str) {
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        if ring_signed_area(ring) >= 0.0 {
            push_polygon(out, xf, ring, copper, 0.28);
            push_ring_outline(out, xf, ring, copper, MIN_STROKE_PX.max(0.6), 0.6);
        } else {
            push_polygon(out, xf, ring, board, 1.0);
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
    rotation: f64,
    height: f64,
    width: f64,
    color: &'a str,
}

/// Draw single-stroke text via the shared stroke font, centred on
/// `run.position`. Rendered in two passes: a fat halo pass in `halo` (so the
/// glyph stays legible over copper), then the colour pass.
fn draw_text(out: &mut String, xf: &Xf, run: &TextRun, halo: &str) {
    let strokes = stroke_font::text_strokes(run.text, run.height);
    if strokes.is_empty() {
        return;
    }
    let w = xf.px(run.width).max(MIN_STROKE_PX);
    let halo_w = w + xf.px(0.12);
    let tw = stroke_font::text_width(run.text, run.height);
    let dx0 = -tw / 2.0;
    let dy0 = -run.height / 2.0;
    let laid: Vec<Vec<Vec2>> = strokes
        .iter()
        .map(|stroke| {
            stroke
                .iter()
                .map(|&(lx, ly)| place(run.position, run.rotation, dx0 + lx, dy0 + ly))
                .collect()
        })
        .collect();
    // Halo pass (fat, background colour) under everything.
    for pts in &laid {
        push_polyline(out, xf, pts, halo, halo_w);
    }
    // Colour pass.
    for pts in &laid {
        push_polyline(out, xf, pts, run.color, w);
    }
}

/// Draw a footprint graphic (silkscreen / fabrication / courtyard). Stroke-font
/// text gets the two-pass halo via palette `p`.
fn draw_graphic(
    out: &mut String,
    xf: &Xf,
    g: &FootprintGraphic,
    origin: Vec2,
    frot: f64,
    p: &Palette,
) {
    match g {
        FootprintGraphic::Line {
            start,
            end,
            width,
            layer,
        } => {
            let color = layer_color(*layer, p);
            let a = place(origin, frot, start.x, start.y);
            let b = place(origin, frot, end.x, end.y);
            push_polyline(out, xf, &[a, b], color, xf.px(*width).max(MIN_STROKE_PX));
        }
        FootprintGraphic::Circle {
            center,
            radius,
            width,
            layer,
        } => {
            let color = layer_color(*layer, p);
            let c = place(origin, frot, center.x, center.y);
            let (cx, cy) = xf.pt(c.x, c.y);
            let r = xf.px(*radius).max(MIN_STROKE_PX);
            let w = xf.px(*width).max(MIN_STROKE_PX);
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
            let color = layer_color(*layer, p);
            let pts: Vec<Vec2> = arc_points(*center, *radius, *start_angle, *end_angle)
                .into_iter()
                .map(|pt| place(origin, frot, pt.x, pt.y))
                .collect();
            push_polyline(out, xf, &pts, color, xf.px(*width).max(MIN_STROKE_PX));
        }
        FootprintGraphic::Rect {
            start,
            end,
            width,
            layer,
        } => {
            let color = layer_color(*layer, p);
            let corners = [
                place(origin, frot, start.x, start.y),
                place(origin, frot, end.x, start.y),
                place(origin, frot, end.x, end.y),
                place(origin, frot, start.x, end.y),
                place(origin, frot, start.x, start.y),
            ];
            push_polyline(out, xf, &corners, color, xf.px(*width).max(MIN_STROKE_PX));
        }
        FootprintGraphic::Polygon {
            vertices,
            width,
            layer,
        } => {
            let color = layer_color(*layer, p);
            let mut pts: Vec<Vec2> = vertices
                .iter()
                .map(|v| place(origin, frot, v.x, v.y))
                .collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            push_polyline(out, xf, &pts, color, xf.px(*width).max(MIN_STROKE_PX));
        }
        FootprintGraphic::Text {
            text,
            position,
            rotation,
            height,
            width,
            layer,
        } => {
            let color = layer_color(*layer, p);
            let pt = place(origin, frot, position.x, position.y);
            draw_text(
                out,
                xf,
                &TextRun {
                    text,
                    position: pt,
                    rotation: frot + rotation,
                    height: *height,
                    width: *width,
                    color,
                },
                p.text_halo,
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

// ─── board outline + grid ──────────────────────────────────────────────────

/// Draw the board substrate fill (outline polygon minus cutouts).
fn draw_board(out: &mut String, xf: &Xf, outline: &BoardOutline, p: &Palette) {
    if outline.vertices.len() < 3 {
        return;
    }
    if outline.cutouts.is_empty() {
        push_filled_polygon(out, xf, &outline.vertices, p.board_fill);
    } else {
        let mut d = String::new();
        path_ring(&mut d, xf, &outline.vertices);
        for c in &outline.cutouts {
            path_ring(&mut d, xf, c);
        }
        let _ = write!(
            out,
            "<path d=\"{d}\" fill=\"{}\" fill-rule=\"evenodd\"/>",
            p.board_fill
        );
    }
    // Seat the board a hair above the page with a thin inner outline.
    let mut ring = outline.vertices.clone();
    if let Some(first) = ring.first().copied() {
        ring.push(first);
    }
    push_polyline(out, xf, &ring, p.board_edge_seat, MIN_STROKE_PX.max(0.8));
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

/// Draw the gridded canvas (50 mil minor, 500 mil major) across the full
/// viewBox so it reads as an instrument bench in the margin around the board.
fn draw_grid(out: &mut String, w: f64, h: f64, scale: f64, p: &Palette) {
    let lines = |pitch_mm: f64, color: &str, width: f64, dst: &mut String| {
        let step = (pitch_mm * scale).max(2.0);
        let mut d = String::new();
        let mut x = 0.0;
        while x <= w {
            let _ = write!(d, "M{x:.1} 0 L{x:.1} {h:.1} ");
            x += step;
        }
        let mut y = 0.0;
        while y <= h {
            let _ = write!(d, "M0 {y:.1} L{w:.1} {y:.1} ");
            y += step;
        }
        let _ = write!(
            dst,
            "<path d=\"{d}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{width}\"/>"
        );
    };
    lines(1.27, p.grid_minor, 0.4, out);
    lines(12.7, p.grid_major, 0.7, out);
}

/// Draw the edge-cut outline (and cutout rings) as thin scribe-line strokes.
fn draw_edges(out: &mut String, xf: &Xf, outline: &BoardOutline, p: &Palette) {
    let w = xf.px(EDGE_WIDTH_MM).max(MIN_STROKE_PX);
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
        push_polyline(out, xf, &pts, p.edge_cuts, w);
    }
}

// ─── ratsnest ────────────────────────────────────────────────────────────────

/// Derive a netlist from the board's own pad net assignments (one connection
/// per pad carrying a net). This is the authoritative connectivity the ratsnest
/// MST runs over; [`compute_ratsnest`] then skips any net that already has a
/// trace, so the air-wires vanish as the board gets routed.
fn derive_netlist(pcb: &Pcb) -> Netlist {
    use std::collections::BTreeMap;
    let mut by_net: BTreeMap<String, Vec<NetConnection>> = BTreeMap::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(net) = &pad.net {
                if net.is_empty() {
                    continue;
                }
                by_net.entry(net.clone()).or_default().push(NetConnection {
                    component_ref: fp.reference.clone(),
                    pin_number: pad.number.clone(),
                });
            }
        }
    }
    Netlist {
        nets: by_net
            .into_iter()
            .map(|(name, connections)| NetlistNet { name, connections })
            .collect(),
    }
}

// ─── footprint value placement ───────────────────────────────────────────────

/// World-space anchor for a footprint's value label: centred under the part's
/// world-space pad cluster, a little below it. Computed in world coords (so a
/// rotated part still gets a horizontal label below it in screen space).
/// `None` for a footprint with no pads.
fn value_anchor(fp: &Footprint) -> Option<Vec2> {
    let mut b = Bounds::empty();
    for pad in &fp.pads {
        let w = place(fp.position, fp.rotation, pad.position.x, pad.position.y);
        b.add(w.x, w.y);
    }
    if !b.is_valid() {
        return None;
    }
    let cx = 0.5 * (b.min_x + b.max_x);
    Some(Vec2::new(cx, b.min_y - 1.8))
}

// ─── public API ──────────────────────────────────────────────────────────────

/// Render a [`Pcb`] to a flat, top-down SVG showing the requested `layers` with
/// the default dark editor theme. Convenience wrapper over
/// [`render_pcb_svg_opts`].
///
/// [`Pcb`]: vcad_ir::ecad::Pcb
pub fn render_pcb_svg(pcb: &Pcb, layers: &[PcbLayer], scale: f64) -> String {
    render_pcb_svg_opts(pcb, layers, scale, &PcbRenderOpts::default())
}

/// Render a [`Pcb`] to a flat, top-down (looking down −Z, Y up) SVG showing the
/// requested `layers`, composited in z-order, styled per `opts`.
///
/// `scale` is pixels per millimetre. The viewBox is derived from the board
/// outline bounds plus a fixed margin. Never errors — a PCB with no usable
/// outline still produces a small framed SVG.
///
/// [`Pcb`]: vcad_ir::ecad::Pcb
pub fn render_pcb_svg_opts(
    pcb: &Pcb,
    layers: &[PcbLayer],
    scale: f64,
    opts: &PcbRenderOpts,
) -> String {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        crate::DEFAULT_SCALE
    };
    let pal = opts.theme.palette();
    let hl = &opts.highlight;
    let hl_on = hl.active();

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

    let want = |l: PcbLayer| layers.contains(&l);
    let show_grid = opts.grid.unwrap_or_else(|| opts.theme.grid_default());
    let show_both_copper = want(PcbLayer::FCu) && want(PcbLayer::BCu);
    let show_values = opts.show_values && w >= VALUE_LABEL_MIN_WIDTH_PX;

    // Per-element style: returns (color, opacity, filter) given a base colour
    // and whether the element matches the active highlight.
    let style = |base: &'static str,
                 matched: bool,
                 back_dim: bool,
                 copper: bool|
     -> (&'static str, f64, Option<&'static str>) {
        let mut op = 1.0;
        if back_dim {
            op *= BACK_COPPER_OPACITY;
        }
        if hl_on && !matched {
            op *= HIGHLIGHT_DIM;
        }
        let color = if hl_on && matched {
            pal.highlight
        } else {
            base
        };
        let filter = if hl_on && matched {
            Some("hl")
        } else if opts.hero && copper {
            Some("bloom")
        } else {
            None
        };
        (color, op, filter)
    };

    // ── body: background + grid + board (drawn first, behind ranked items) ──
    let mut body = String::new();
    let _ = write!(
        body,
        "<rect x=\"0\" y=\"0\" width=\"{w:.2}\" height=\"{h:.2}\" fill=\"{}\"/>",
        pal.background
    );
    if show_grid {
        draw_grid(&mut body, w, h, scale, &pal);
    }
    draw_board(&mut body, &xf, &pcb.outline, &pal);

    let mut items: Vec<(u8, usize, String)> = Vec::new();

    // ── ratsnest (rank 2, below copper) ──
    if opts.show_ratsnest {
        let netlist = derive_netlist(pcb);
        for line in compute_ratsnest(pcb, &netlist) {
            let matched = hl_on && hl.net(&line.net);
            let (color, op, filt) = style(pal.ratsnest, matched, false, false);
            let mut s = String::new();
            let (x1, y1) = xf.pt(line.from.x, line.from.y);
            let (x2, y2) = xf.pt(line.to.x, line.to.y);
            let _ = write!(
                s,
                "<line x1=\"{x1:.3}\" y1=\"{y1:.3}\" x2=\"{x2:.3}\" y2=\"{y2:.3}\" \
                 stroke=\"{color}\" stroke-width=\"{:.3}\" stroke-opacity=\"0.75\" \
                 stroke-dasharray=\"{:.2} {:.2}\" stroke-linecap=\"round\"/>",
                xf.px(0.12).max(MIN_STROKE_PX),
                xf.px(0.6),
                xf.px(0.45)
            );
            push_item(&mut items, RATSNEST_RANK, wrap(s, op, filt));
        }
    }

    // ── zones (copper pours) — lowest among copper ──
    let filled = vcad_ecad_pcb::copper_pour::fill_zones(pcb);
    for (i, z) in pcb.zones.iter().enumerate() {
        if want(z.layer) {
            let rings = filled
                .get(i)
                .map(|f| f.polygons.clone())
                .unwrap_or_else(|| vec![z.outline.clone()]);
            let matched = hl_on && hl.net(&z.net);
            let back_dim = show_both_copper && z.layer == PcbLayer::BCu;
            let (color, op, filt) = style(layer_color(z.layer, &pal), matched, back_dim, true);
            let mut s = String::new();
            draw_zone(&mut s, &xf, &rings, color, pal.board_fill);
            push_item(
                &mut items,
                layer_rank(z.layer).saturating_sub(1),
                wrap(s, op, filt),
            );
        }
    }

    // ── copper traces ──
    for t in &pcb.traces {
        if want(t.layer) {
            let matched = hl_on && hl.net(&t.net);
            let back_dim = show_both_copper && t.layer == PcbLayer::BCu;
            let (color, op, filt) = style(layer_color(t.layer, &pal), matched, back_dim, true);
            let mut s = String::new();
            draw_trace(&mut s, &xf, t, color);
            push_item(&mut items, layer_rank(t.layer), wrap(s, op, filt));
        }
    }
    // Teardrop fillets (solid copper, under pads).
    for td in &vcad_ecad_pcb::generate_teardrops(pcb) {
        if want(td.layer) && td.polygon.len() >= 3 {
            let back_dim = show_both_copper && td.layer == PcbLayer::BCu;
            let (color, op, filt) = style(layer_color(td.layer, &pal), false, back_dim, true);
            let mut s = String::new();
            push_filled_polygon(&mut s, &xf, &td.polygon, color);
            push_item(&mut items, layer_rank(td.layer), wrap(s, op, filt));
        }
    }
    for a in &pcb.trace_arcs {
        if want(a.layer) {
            let matched = hl_on && hl.net(&a.net);
            let back_dim = show_both_copper && a.layer == PcbLayer::BCu;
            let (color, op, filt) = style(layer_color(a.layer, &pal), matched, back_dim, true);
            let mut s = String::new();
            draw_trace_arc(&mut s, &xf, a, color);
            push_item(&mut items, layer_rank(a.layer), wrap(s, op, filt));
        }
    }

    // ── pads (per footprint, honouring rotation) ──
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let pad_layer = pad.layers.iter().copied().find(|l| want(*l));
            let Some(pl) = pad_layer else { continue };
            let origin = place(fp.position, fp.rotation, pad.position.x, pad.position.y);
            let total_rot = fp.rotation + pad.rotation;
            let matched = hl_on
                && (hl.reference(&fp.reference)
                    || pad.net.as_deref().map(|n| hl.net(n)).unwrap_or(false));
            let back_dim = show_both_copper && pl == PcbLayer::BCu;
            // Pads use the dedicated pad colour (value-step off copper).
            let base = if pl == PcbLayer::FCu || pl == PcbLayer::BCu {
                pal.pad
            } else {
                layer_color(pl, &pal)
            };
            let (color, op, filt) = style(base, matched, back_dim, true);
            let mut s = String::new();
            draw_pad(&mut s, &xf, pad, origin, total_rot, color, &pal);
            push_item(&mut items, layer_rank(pl) + 1, wrap(s, op, filt));
        }
    }

    // ── vias ──
    for v in &pcb.vias {
        if want(v.start_layer) || want(v.end_layer) {
            let layer = if want(v.start_layer) {
                v.start_layer
            } else {
                v.end_layer
            };
            let matched = hl_on && hl.net(&v.net);
            let (color, op, filt) = style(pal.via_ring, matched, false, true);
            let _ = layer;
            let mut s = String::new();
            draw_via(&mut s, &xf, v, color, &pal);
            push_item(&mut items, 4u8, wrap(s, op, filt));
        }
    }

    // ── footprint graphics (silk / fab / courtyard / mask / paste) ──
    for fp in &pcb.footprints {
        for g in &fp.graphics {
            let gl = graphic_layer(g);
            if want(gl) {
                let matched = hl_on && hl.reference(&fp.reference);
                let (_c, op, filt) = style(layer_color(gl, &pal), matched, false, false);
                let mut s = String::new();
                draw_graphic(&mut s, &xf, g, fp.position, fp.rotation, &pal);
                push_item(&mut items, layer_rank(gl), wrap(s, op, filt));
            }
        }
    }

    // ── value labels (silk, zoom-gated) ──
    if show_values && (want(PcbLayer::FSilkS) || want(PcbLayer::FFab)) {
        for fp in &pcb.footprints {
            if fp.value.trim().is_empty() {
                continue;
            }
            let Some(anchor) = value_anchor(fp) else {
                continue;
            };
            let matched = hl_on && hl.reference(&fp.reference);
            let op = if hl_on && !matched {
                HIGHLIGHT_DIM
            } else {
                1.0
            };
            let mut s = String::new();
            draw_text(
                &mut s,
                &xf,
                &TextRun {
                    text: &fp.value,
                    position: anchor,
                    rotation: 0.0,
                    height: 0.8,
                    width: 0.1,
                    color: pal.value_text,
                },
                pal.text_halo,
            );
            push_item(&mut items, 7, wrap(s, op, None));
        }
    }

    // ── net-name annotations on routed copper (opt-in) ──
    if opts.show_net_labels {
        use std::collections::HashMap;
        let mut longest: HashMap<&str, (f64, &Trace)> = HashMap::new();
        for t in &pcb.traces {
            if !want(t.layer) {
                continue;
            }
            let net = t.net.as_str();
            if net.is_empty() || net.eq_ignore_ascii_case("GND") {
                continue;
            }
            let len = ((t.end.x - t.start.x).powi(2) + (t.end.y - t.start.y).powi(2)).sqrt();
            let e = longest.entry(net).or_insert((0.0, t));
            if len > e.0 {
                *e = (len, t);
            }
        }
        for (net, (seg_len, t)) in longest {
            if seg_len < NETLABEL_MIN_TRACE_MM {
                continue;
            }
            let mid = Vec2::new(0.5 * (t.start.x + t.end.x), 0.5 * (t.start.y + t.end.y));
            let mut ang = (t.end.y - t.start.y)
                .atan2(t.end.x - t.start.x)
                .to_degrees();
            if ang > 90.0 {
                ang -= 180.0;
            } else if ang < -90.0 {
                ang += 180.0;
            }
            let color = if t.layer == PcbLayer::BCu {
                pal.netname_b
            } else {
                pal.netname_f
            };
            let matched = hl_on && hl.net(net);
            let op = if hl_on && !matched {
                HIGHLIGHT_DIM
            } else {
                0.9
            };
            let mut s = String::new();
            draw_text(
                &mut s,
                &xf,
                &TextRun {
                    text: net,
                    position: mid,
                    rotation: ang,
                    height: 0.55,
                    width: 0.1,
                    color,
                },
                pal.text_halo,
            );
            push_item(&mut items, 8, wrap(s, op, None));
        }
    }

    // ── edge cuts on top ──
    if want(PcbLayer::EdgeCuts) {
        let mut s = String::new();
        draw_edges(&mut s, &xf, &pcb.outline, &pal);
        push_item(&mut items, layer_rank(PcbLayer::EdgeCuts), s);
    }

    // Stable sort by (rank, insertion order) and concatenate.
    items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, _, frag) in &items {
        body.push_str(frag);
    }

    // ── assemble ──
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}\" height=\"{h:.2}\" \
         viewBox=\"0 0 {w:.2} {h:.2}\" shape-rendering=\"geometricPrecision\" \
         role=\"img\" aria-label=\"vcad pcb render\">"
    );
    // Filter defs (only when needed).
    if hl_on || opts.hero {
        out.push_str("<defs>");
        if hl_on {
            out.push_str(
                "<filter id=\"hl\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">\
                 <feGaussianBlur stdDeviation=\"1.4\" result=\"b\"/>\
                 <feMerge><feMergeNode in=\"b\"/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>",
            );
        }
        if opts.hero {
            out.push_str(
                "<filter id=\"bloom\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">\
                 <feGaussianBlur stdDeviation=\"2.0\" result=\"b\"/>\
                 <feMerge><feMergeNode in=\"b\"/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>",
            );
        }
        out.push_str("</defs>");
    }
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

/// JSON wrapper around [`render_pcb_svg`] for the WASM/MCP integration layer
/// (default dark theme).
///
/// `pcb_json` is a serialized [`Pcb`]; `layers_json` is a JSON array of layer
/// names — either the serde variant names (`"FCu"`, `"FSilkS"`, `"EdgeCuts"`)
/// or KiCad dotted names (`"F.Cu"`, `"F.SilkS"`, `"Edge.Cuts"`). Unknown
/// layer names are rejected with an error listing the offender.
///
/// [`Pcb`]: vcad_ir::ecad::Pcb
pub fn render_pcb_svg_json(
    pcb_json: &str,
    layers_json: &str,
    scale: f64,
) -> Result<String, String> {
    render_pcb_svg_json_opts(pcb_json, layers_json, scale, "")
}

/// JSON wrapper accepting a render-options JSON string. `opts_json` may be empty
/// (defaults) or an object like:
/// `{"theme":"dark","values":true,"netLabels":false,"ratsnest":true,"grid":true,
///   "hero":false,"highlight":{"nets":["GND"],"refs":["U1"]}}`.
///
/// Backward-compatible: existing 3-arg callers route through
/// [`render_pcb_svg_json`] with empty options.
pub fn render_pcb_svg_json_opts(
    pcb_json: &str,
    layers_json: &str,
    scale: f64,
    opts_json: &str,
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
    let opts = parse_opts(opts_json)?;
    Ok(render_pcb_svg_opts(&pcb, &layers, scale, &opts))
}

/// Parse the options JSON (empty string → defaults). Tolerant: unknown fields
/// ignored; missing fields keep their defaults.
fn parse_opts(opts_json: &str) -> Result<PcbRenderOpts, String> {
    let mut opts = PcbRenderOpts::default();
    let trimmed = opts_json.trim();
    if trimmed.is_empty() {
        return Ok(opts);
    }
    let v: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("opts json: {e}"))?;
    if let Some(t) = v.get("theme").and_then(|x| x.as_str()) {
        opts.theme = if t.eq_ignore_ascii_case("light") {
            Theme::Light
        } else {
            Theme::Dark
        };
    }
    if let Some(b) = v.get("grid").and_then(|x| x.as_bool()) {
        opts.grid = Some(b);
    }
    if let Some(b) = v.get("values").and_then(|x| x.as_bool()) {
        opts.show_values = b;
    }
    if let Some(b) = v.get("netLabels").and_then(|x| x.as_bool()) {
        opts.show_net_labels = b;
    }
    if let Some(b) = v.get("ratsnest").and_then(|x| x.as_bool()) {
        opts.show_ratsnest = b;
    }
    if let Some(b) = v.get("hero").and_then(|x| x.as_bool()) {
        opts.hero = b;
    }
    if let Some(h) = v.get("highlight") {
        let strs = |key: &str| -> Vec<String> {
            h.get(key)
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        opts.highlight.nets = strs("nets");
        opts.highlight.refs = strs("refs");
    }
    Ok(opts)
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_ir::ecad::{
        BoardOutline, DesignRules, DrillSpec, Footprint, LayerStackup, Net, NetClassRules, Pad,
        PadType, Pcb, PcbLayer, Trace, Via,
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
            nets: vec![Net {
                id: "1".to_string(),
                name: "1".to_string(),
            }],
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
                properties: HashMap::new(),
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

    /// A 2-pad net with no trace, so the ratsnest produces one air-wire.
    fn unrouted_pcb() -> Pcb {
        let mut pcb = sample_pcb();
        pcb.traces.clear();
        pcb.footprints[0].pads.push(Pad {
            number: "2".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 1.0,
                height: 1.2,
            },
            position: Vec2::new(1.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: Some("1".to_string()),
            layers: vec![PcbLayer::FCu],
        });
        pcb
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
        assert!(svg.contains("<line"), "expected a trace line");
        assert!(svg.contains("<circle"), "expected a via circle");
        assert!(svg.contains(DARK.f_cu), "front copper colour present");
        assert!(svg.contains(DARK.edge_cuts), "edge-cut colour present");
        assert!(
            svg.contains(DARK.board_fill),
            "board substrate fill present"
        );
        assert!(
            svg.contains(DARK.background),
            "dark page background present"
        );
        assert!(svg.contains("<polyline"), "expected silk text strokes");
        assert!(svg.contains(DARK.f_silk), "silk colour present");
        assert!(
            svg.contains("shape-rendering=\"geometricPrecision\""),
            "AA hint present"
        );
    }

    #[test]
    fn light_theme_restores_legacy_look() {
        let pcb = sample_pcb();
        let opts = PcbRenderOpts {
            theme: Theme::Light,
            ..Default::default()
        };
        let svg = render_pcb_svg_opts(&pcb, &[PcbLayer::FCu, PcbLayer::EdgeCuts], 8.0, &opts);
        assert!(svg.contains(LIGHT.board_fill), "legacy green board");
        assert!(svg.contains(LIGHT.edge_cuts), "legacy yellow edge");
        assert!(svg.contains("fill=\"#ffffff\""), "white page background");
    }

    #[test]
    fn fcu_only_omits_silk() {
        let pcb = sample_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        assert!(
            !svg.contains(DARK.f_silk),
            "silk colour leaked into FCu-only render"
        );
        assert!(
            !svg.contains(DARK.edge_cuts),
            "edge colour leaked into FCu-only render"
        );
        assert!(svg.contains(DARK.f_cu));
        assert!(svg.contains("<line"));
    }

    #[test]
    fn viewbox_tracks_outline_bounds() {
        let pcb = sample_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::EdgeCuts], 8.0);
        let attr = |name: &str| -> f64 {
            let pat = format!("{name}=\"");
            let start = svg.find(&pat).unwrap() + pat.len();
            let end = svg[start..].find('"').unwrap() + start;
            svg[start..end].parse().unwrap()
        };
        // 20mm × 15mm board + MARGIN_MM all sides, @ 8px/mm.
        let expect_w = (20.0 + 2.0 * MARGIN_MM) * 8.0;
        let expect_h = (15.0 + 2.0 * MARGIN_MM) * 8.0;
        assert!(
            (attr("width") - expect_w).abs() < 0.5,
            "w {}",
            attr("width")
        );
        assert!(
            (attr("height") - expect_h).abs() < 0.5,
            "h {}",
            attr("height")
        );
    }

    #[test]
    fn ratsnest_drawn_for_unrouted_net() {
        let pcb = unrouted_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        assert!(
            svg.contains("stroke-dasharray"),
            "expected a dashed ratsnest air-wire for the unrouted 2-pad net"
        );
        assert!(svg.contains(DARK.ratsnest), "ratsnest colour present");
    }

    #[test]
    fn routed_net_has_no_ratsnest() {
        // sample_pcb's net "1" has a trace, so no air-wire should be drawn.
        let pcb = sample_pcb();
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        assert!(
            !svg.contains("stroke-dasharray"),
            "routed net must not produce a ratsnest air-wire"
        );
    }

    #[test]
    fn highlight_recolors_and_adds_glow() {
        let pcb = sample_pcb();
        let opts = PcbRenderOpts {
            highlight: Highlight {
                nets: vec!["1".to_string()],
                refs: vec![],
            },
            ..Default::default()
        };
        let svg = render_pcb_svg_opts(&pcb, &[PcbLayer::FCu], 8.0, &opts);
        assert!(svg.contains(DARK.highlight), "highlight pink present");
        assert!(svg.contains("filter=\"url(#hl)\""), "glow filter applied");
        assert!(svg.contains("<filter id=\"hl\""), "glow filter defined");
    }

    #[test]
    fn through_hole_pad_has_bore() {
        let mut pcb = sample_pcb();
        pcb.footprints[0].pads[0].pad_type = PadType::THT;
        pcb.footprints[0].pads[0].drill = Some(DrillSpec {
            diameter: 0.6,
            oval: false,
            oval_height: None,
        });
        pcb.footprints[0].pads[0].shape = PadShape::Circle { diameter: 1.4 };
        let svg = render_pcb_svg(&pcb, &[PcbLayer::FCu], 8.0);
        assert!(svg.contains(DARK.via_bore), "drilled bore colour present");
    }

    #[test]
    fn json_round_trips() {
        let pcb = sample_pcb();
        let pcb_json = serde_json::to_string(&pcb).unwrap();
        let svg = render_pcb_svg_json(&pcb_json, r#"["F.Cu","F.SilkS","Edge.Cuts"]"#, 8.0).unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains(DARK.f_cu));
        assert!(svg.contains(DARK.f_silk));
        let svg2 = render_pcb_svg_json(&pcb_json, r#"["FCu"]"#, 8.0).unwrap();
        assert!(svg2.contains(DARK.f_cu));
        assert!(!svg2.contains(DARK.f_silk));
    }

    #[test]
    fn json_opts_light_theme() {
        let pcb = sample_pcb();
        let pcb_json = serde_json::to_string(&pcb).unwrap();
        let svg =
            render_pcb_svg_json_opts(&pcb_json, r#"["FCu"]"#, 8.0, r#"{"theme":"light"}"#).unwrap();
        assert!(svg.contains(LIGHT.f_cu), "light theme via json opts");
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
