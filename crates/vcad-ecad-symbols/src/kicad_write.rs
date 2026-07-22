//! Writers for native, **editable** KiCad files.
//!
//! These are the inverse of [`crate::kicad_pcb`] (and a companion for the
//! schematic side): they serialize a [`vcad_ir::ecad::Pcb`] back into a
//! `.kicad_pcb` S-expression board, and a [`vcad_ir::ecad::SchematicSheet`]
//! into a `.kicad_sch` schematic.  The point is a real round trip — a board an
//! agent authored (or imported and edited) can be handed to a human to finish
//! in KiCad 9, then re-imported.
//!
//! The output targets the KiCad 9 file format (board `version 20241229`,
//! schematic `version 20250114`).  Coordinates pass through verbatim: vcad PCB
//! space is already KiCad space (millimetres, Y-down), so no transform is
//! applied.  Net references in the IR are net *names* (see
//! [`crate::kicad_pcb`]); the writer assigns each a numeric KiCad net index and
//! emits the `(net N "name")` table the format requires.

use std::collections::BTreeMap;

use vcad_ir::ecad::{
    BoardOutline, Footprint, FootprintGraphic, LabelScope, LayerStackup, Pad, PadShape, PadType,
    Pcb, PcbLayer, PinType, SchematicComponent, SchematicLabel, SchematicPin, SchematicSheet,
    Trace, TraceArc, Via, Zone,
};
use vcad_ir::Vec2;

// ---------------------------------------------------------------------------
// Low-level S-expression emitter
// ---------------------------------------------------------------------------

/// Accumulates indented S-expression text and hands out deterministic UUIDs.
struct Emitter {
    buf: String,
    next_uuid: u64,
}

impl Emitter {
    fn new() -> Self {
        Emitter {
            buf: String::with_capacity(4096),
            next_uuid: 0,
        }
    }

    /// Write one tab-indented line.
    fn line(&mut self, depth: usize, content: &str) {
        for _ in 0..depth {
            self.buf.push('\t');
        }
        self.buf.push_str(content);
        self.buf.push('\n');
    }

    /// A stable, valid UUID derived from an incrementing counter, so the same
    /// document always serializes byte-identically (golden tests, diffs).
    fn uuid(&mut self) -> String {
        self.next_uuid += 1;
        format!("00000000-0000-0000-0000-{:012x}", self.next_uuid)
    }
}

/// Format a float the way KiCad does: fixed up to 6 decimals, trailing zeros
/// trimmed, and `-0` normalized to `0`.
fn num(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{:.6}", v);
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Escape a string for a double-quoted KiCad token.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Quote-and-escape.
fn q(s: &str) -> String {
    format!("\"{}\"", esc(s))
}

// ---------------------------------------------------------------------------
// Layer naming
// ---------------------------------------------------------------------------

/// Canonical KiCad layer name for a [`PcbLayer`].
fn layer_name(layer: PcbLayer) -> &'static str {
    match layer {
        PcbLayer::FCu => "F.Cu",
        PcbLayer::BCu => "B.Cu",
        PcbLayer::In1Cu => "In1.Cu",
        PcbLayer::In2Cu => "In2.Cu",
        PcbLayer::In3Cu => "In3.Cu",
        PcbLayer::In4Cu => "In4.Cu",
        PcbLayer::In5Cu => "In5.Cu",
        PcbLayer::In6Cu => "In6.Cu",
        PcbLayer::In7Cu => "In7.Cu",
        PcbLayer::In8Cu => "In8.Cu",
        PcbLayer::FSilkS => "F.SilkS",
        PcbLayer::BSilkS => "B.SilkS",
        PcbLayer::FMask => "F.Mask",
        PcbLayer::BMask => "B.Mask",
        PcbLayer::FPaste => "F.Paste",
        PcbLayer::BPaste => "B.Paste",
        PcbLayer::FCrtYd => "F.CrtYd",
        PcbLayer::BCrtYd => "B.CrtYd",
        PcbLayer::FFab => "F.Fab",
        PcbLayer::BFab => "B.Fab",
        PcbLayer::EdgeCuts => "Edge.Cuts",
        PcbLayer::UserDrawings => "Dwgs.User",
        PcbLayer::UserComments => "Cmts.User",
    }
}

/// KiCad 9 canonical numeric id for a copper layer (the v9 renumbering, where
/// copper layers take even ids and technical layers take odd ones).
fn copper_layer_id(layer: PcbLayer) -> u32 {
    match layer {
        PcbLayer::FCu => 0,
        PcbLayer::BCu => 2,
        PcbLayer::In1Cu => 4,
        PcbLayer::In2Cu => 6,
        PcbLayer::In3Cu => 8,
        PcbLayer::In4Cu => 10,
        PcbLayer::In5Cu => 12,
        PcbLayer::In6Cu => 14,
        PcbLayer::In7Cu => 16,
        PcbLayer::In8Cu => 18,
        _ => 0,
    }
}

/// Copper layers present on the board, ordered F.Cu → inner → B.Cu.
fn copper_layers(stackup: &LayerStackup) -> Vec<PcbLayer> {
    let mut copper: Vec<PcbLayer> = stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    if copper.is_empty() {
        copper = vec![PcbLayer::FCu, PcbLayer::BCu];
    }
    copper.sort_by_key(|l| match l {
        PcbLayer::FCu => 0,
        PcbLayer::In1Cu => 1,
        PcbLayer::In2Cu => 2,
        PcbLayer::In3Cu => 3,
        PcbLayer::In4Cu => 4,
        PcbLayer::In5Cu => 5,
        PcbLayer::In6Cu => 6,
        PcbLayer::BCu => 7,
        _ => 8,
    });
    copper.dedup();
    copper
}

// ---------------------------------------------------------------------------
// Net index table
// ---------------------------------------------------------------------------

/// Maps net *names* (the IR's net references) to KiCad numeric net ids.
struct NetTable {
    /// name → index (index 0 is the implicit "no net").
    index: BTreeMap<String, u32>,
    /// Ordered (index, name) for emitting the `(net …)` table.
    ordered: Vec<(u32, String)>,
}

impl NetTable {
    /// Build the table from the board's declared nets plus any net names
    /// referenced by copper that were never declared (added defensively so the
    /// file is always self-consistent).
    fn build(pcb: &Pcb) -> Self {
        let mut index = BTreeMap::new();
        let mut ordered = vec![(0u32, String::new())];
        index.insert(String::new(), 0u32);
        let mut next = 1u32;

        let add = |name: &str,
                   index: &mut BTreeMap<String, u32>,
                   ordered: &mut Vec<(u32, String)>,
                   next: &mut u32| {
            if name.is_empty() || index.contains_key(name) {
                return;
            }
            index.insert(name.to_string(), *next);
            ordered.push((*next, name.to_string()));
            *next += 1;
        };

        for net in &pcb.nets {
            add(&net.name, &mut index, &mut ordered, &mut next);
        }
        for fp in &pcb.footprints {
            for pad in &fp.pads {
                if let Some(n) = &pad.net {
                    add(n, &mut index, &mut ordered, &mut next);
                }
            }
        }
        for t in &pcb.traces {
            add(&t.net, &mut index, &mut ordered, &mut next);
        }
        for a in &pcb.trace_arcs {
            add(&a.net, &mut index, &mut ordered, &mut next);
        }
        for v in &pcb.vias {
            add(&v.net, &mut index, &mut ordered, &mut next);
        }
        for z in &pcb.zones {
            add(&z.net, &mut index, &mut ordered, &mut next);
        }

        NetTable { index, ordered }
    }

    fn id(&self, name: &str) -> u32 {
        self.index.get(name).copied().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// PCB writer
// ---------------------------------------------------------------------------

/// Serialize a [`Pcb`] to a KiCad 9 `.kicad_pcb` board file.
pub fn write_kicad_pcb(pcb: &Pcb) -> String {
    let mut e = Emitter::new();
    let nets = NetTable::build(pcb);

    e.line(0, "(kicad_pcb");
    e.line(1, "(version 20241229)");
    e.line(1, "(generator \"vcad\")");
    e.line(1, "(generator_version \"9.0\")");

    // General
    e.line(1, "(general");
    e.line(2, &format!("(thickness {})", num(pcb.outline.thickness)));
    e.line(2, "(legacy_teardrops no)");
    e.line(1, ")");
    e.line(1, "(paper \"A4\")");

    write_layers(&mut e, pcb);
    write_setup(&mut e);

    // Nets
    for (idx, name) in &nets.ordered {
        e.line(1, &format!("(net {} {})", idx, q(name)));
    }

    // Footprints
    for fp in &pcb.footprints {
        write_footprint(&mut e, fp, &nets);
    }

    // Board outline (Edge.Cuts)
    write_outline(&mut e, &pcb.outline);

    // Routed copper
    for t in &pcb.traces {
        write_segment(&mut e, t, &nets);
    }
    for a in &pcb.trace_arcs {
        write_arc(&mut e, a, &nets);
    }
    for v in &pcb.vias {
        write_via(&mut e, v, &nets);
    }
    for z in &pcb.zones {
        write_zone(&mut e, z, &nets);
    }

    e.line(1, "(embedded_fonts no)");
    e.line(0, ")");
    e.buf
}

fn write_layers(e: &mut Emitter, pcb: &Pcb) {
    let copper = copper_layers(&pcb.stackup);
    e.line(1, "(layers");
    // F.Cu, inner copper, B.Cu — in physical stack order, KiCad lists F.Cu
    // first, then inner, then B.Cu.
    let f = copper.iter().find(|l| **l == PcbLayer::FCu);
    let b = copper.iter().find(|l| **l == PcbLayer::BCu);
    if let Some(l) = f {
        e.line(
            2,
            &format!("({} {} signal)", copper_layer_id(*l), q("F.Cu")),
        );
    }
    for l in &copper {
        if l.is_copper() && *l != PcbLayer::FCu && *l != PcbLayer::BCu {
            e.line(
                2,
                &format!("({} {} signal)", copper_layer_id(*l), q(layer_name(*l))),
            );
        }
    }
    if let Some(l) = b {
        e.line(
            2,
            &format!("({} {} signal)", copper_layer_id(*l), q("B.Cu")),
        );
    }
    // Technical layers — fixed KiCad 9 ids.
    for (id, name, kind, user) in [
        (9u32, "F.Adhes", "user", Some("F.Adhesive")),
        (11, "B.Adhes", "user", Some("B.Adhesive")),
        (13, "F.Paste", "user", None),
        (15, "B.Paste", "user", None),
        (5, "F.SilkS", "user", Some("F.Silkscreen")),
        (7, "B.SilkS", "user", Some("B.Silkscreen")),
        (1, "F.Mask", "user", None),
        (3, "B.Mask", "user", None),
        (17, "Dwgs.User", "user", Some("User.Drawings")),
        (19, "Cmts.User", "user", Some("User.Comments")),
        (21, "Eco1.User", "user", Some("User.Eco1")),
        (23, "Eco2.User", "user", Some("User.Eco2")),
        (25, "Edge.Cuts", "user", None),
        (27, "Margin", "user", None),
        (31, "F.CrtYd", "user", Some("F.Courtyard")),
        (29, "B.CrtYd", "user", Some("B.Courtyard")),
        (35, "F.Fab", "user", None),
        (33, "B.Fab", "user", None),
    ] {
        match user {
            Some(u) => e.line(2, &format!("({} {} {} {})", id, q(name), kind, q(u))),
            None => e.line(2, &format!("({} {} {})", id, q(name), kind)),
        }
    }
    e.line(1, ")");
}

fn write_setup(e: &mut Emitter) {
    e.line(1, "(setup");
    e.line(2, "(pad_to_mask_clearance 0)");
    e.line(2, "(allow_soldermask_bridges_in_footprints no)");
    e.line(1, ")");
}

fn write_outline(e: &mut Emitter, outline: &BoardOutline) {
    let edge = |e: &mut Emitter, a: Vec2, b: Vec2| {
        let uuid = e.uuid();
        e.line(1, "(gr_line");
        e.line(2, &format!("(start {} {})", num(a.x), num(a.y)));
        e.line(2, &format!("(end {} {})", num(b.x), num(b.y)));
        e.line(2, "(stroke");
        e.line(3, "(width 0.1)");
        e.line(3, "(type solid)");
        e.line(2, ")");
        e.line(2, "(layer \"Edge.Cuts\")");
        e.line(2, &format!("(uuid {})", q(&uuid)));
        e.line(1, ")");
    };

    write_closed_loop(e, &outline.vertices, &edge);
    for cutout in &outline.cutouts {
        write_closed_loop(e, cutout, &edge);
    }
}

/// Emit `gr_line` edges around a closed vertex loop (last connects to first).
fn write_closed_loop(e: &mut Emitter, verts: &[Vec2], edge: &dyn Fn(&mut Emitter, Vec2, Vec2)) {
    let n = verts.len();
    if n < 2 {
        return;
    }
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        edge(e, a, b);
    }
}

fn write_footprint(e: &mut Emitter, fp: &Footprint, nets: &NetTable) {
    let layer = if fp.front { "F.Cu" } else { "B.Cu" };
    let uuid = e.uuid();
    let name = if fp.footprint_name.is_empty() {
        fp.reference.clone()
    } else {
        fp.footprint_name.clone()
    };
    e.line(1, &format!("(footprint {}", q(&name)));
    e.line(2, &format!("(layer {})", q(layer)));
    e.line(2, &format!("(uuid {})", q(&uuid)));
    let at = if fp.rotation != 0.0 {
        format!(
            "(at {} {} {})",
            num(fp.position.x),
            num(fp.position.y),
            num(fp.rotation)
        )
    } else {
        format!("(at {} {})", num(fp.position.x), num(fp.position.y))
    };
    e.line(2, &at);

    write_fp_property(e, "Reference", &fp.reference, "F.SilkS");
    write_fp_property(e, "Value", &fp.value, "F.Fab");

    for g in &fp.graphics {
        write_fp_graphic(e, g);
    }
    for pad in &fp.pads {
        write_pad(e, pad, fp, nets);
    }

    e.line(1, ")");
}

fn write_fp_property(e: &mut Emitter, key: &str, value: &str, layer: &str) {
    let uuid = e.uuid();
    e.line(2, &format!("(property {} {}", q(key), q(value)));
    e.line(3, "(at 0 0 0)");
    e.line(3, &format!("(layer {})", q(layer)));
    e.line(3, &format!("(uuid {})", q(&uuid)));
    e.line(3, "(effects");
    e.line(4, "(font");
    e.line(5, "(size 1 1)");
    e.line(5, "(thickness 0.15)");
    e.line(4, ")");
    e.line(3, ")");
    e.line(2, ")");
}

fn write_fp_graphic(e: &mut Emitter, g: &FootprintGraphic) {
    match g {
        FootprintGraphic::Line {
            start,
            end,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            e.line(2, "(fp_line");
            e.line(3, &format!("(start {} {})", num(start.x), num(start.y)));
            e.line(3, &format!("(end {} {})", num(end.x), num(end.y)));
            write_stroke(e, 3, *width);
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(2, ")");
        }
        FootprintGraphic::Circle {
            center,
            radius,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            e.line(2, "(fp_circle");
            e.line(3, &format!("(center {} {})", num(center.x), num(center.y)));
            e.line(
                3,
                &format!("(end {} {})", num(center.x + radius), num(center.y)),
            );
            write_stroke(e, 3, *width);
            e.line(3, "(fill no)");
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(2, ")");
        }
        FootprintGraphic::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            let (s, m, end) = arc_points(*center, *radius, *start_angle, *end_angle);
            e.line(2, "(fp_arc");
            e.line(3, &format!("(start {} {})", num(s.x), num(s.y)));
            e.line(3, &format!("(mid {} {})", num(m.x), num(m.y)));
            e.line(3, &format!("(end {} {})", num(end.x), num(end.y)));
            write_stroke(e, 3, *width);
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(2, ")");
        }
        FootprintGraphic::Rect {
            start,
            end,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            e.line(2, "(fp_rect");
            e.line(3, &format!("(start {} {})", num(start.x), num(start.y)));
            e.line(3, &format!("(end {} {})", num(end.x), num(end.y)));
            write_stroke(e, 3, *width);
            e.line(3, "(fill no)");
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(2, ")");
        }
        FootprintGraphic::Polygon {
            vertices,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            e.line(2, "(fp_poly");
            write_pts(e, 3, vertices);
            write_stroke(e, 3, *width);
            e.line(3, "(fill no)");
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(2, ")");
        }
        FootprintGraphic::Text {
            text,
            position,
            rotation,
            height,
            width,
            layer,
        } => {
            let uuid = e.uuid();
            e.line(2, &format!("(fp_text user {}", q(text)));
            e.line(
                3,
                &format!(
                    "(at {} {} {})",
                    num(position.x),
                    num(position.y),
                    num(*rotation)
                ),
            );
            e.line(3, &format!("(layer {})", q(layer_name(*layer))));
            e.line(3, &format!("(uuid {})", q(&uuid)));
            e.line(3, "(effects");
            e.line(4, "(font");
            e.line(5, &format!("(size {} {})", num(*height), num(*height)));
            e.line(5, &format!("(thickness {})", num(*width)));
            e.line(4, ")");
            e.line(3, ")");
            e.line(2, ")");
        }
    }
}

fn write_stroke(e: &mut Emitter, depth: usize, width: f64) {
    e.line(depth, "(stroke");
    e.line(depth + 1, &format!("(width {})", num(width)));
    e.line(depth + 1, "(type solid)");
    e.line(depth, ")");
}

fn write_pts(e: &mut Emitter, depth: usize, verts: &[Vec2]) {
    e.line(depth, "(pts");
    for v in verts {
        e.line(depth + 1, &format!("(xy {} {})", num(v.x), num(v.y)));
    }
    e.line(depth, ")");
}

fn write_pad(e: &mut Emitter, pad: &Pad, fp: &Footprint, nets: &NetTable) {
    let type_str = match pad.pad_type {
        PadType::SMD => "smd",
        PadType::THT => "thru_hole",
        PadType::NPTH => "np_thru_hole",
    };
    let (shape_str, w, h, rratio) = pad_shape_fields(&pad.shape);

    e.line(
        2,
        &format!("(pad {} {} {}", q(&pad.number), type_str, shape_str),
    );
    let at = if pad.rotation != 0.0 {
        format!(
            "(at {} {} {})",
            num(pad.position.x),
            num(pad.position.y),
            num(pad.rotation)
        )
    } else {
        format!("(at {} {})", num(pad.position.x), num(pad.position.y))
    };
    e.line(3, &at);
    e.line(3, &format!("(size {} {})", num(w), num(h)));

    if let Some(d) = &pad.drill {
        if d.oval {
            let oh = d.oval_height.unwrap_or(d.diameter);
            e.line(3, &format!("(drill oval {} {})", num(d.diameter), num(oh)));
        } else {
            e.line(3, &format!("(drill {})", num(d.diameter)));
        }
    }

    write_pad_layers(e, pad, fp);

    if let PadShape::RoundRect { .. } = pad.shape {
        e.line(3, &format!("(roundrect_rratio {})", num(rratio)));
    }

    if let Some(n) = &pad.net {
        if !n.is_empty() {
            e.line(3, &format!("(net {} {})", nets.id(n), q(n)));
        }
    }
    let uuid = e.uuid();
    e.line(3, &format!("(uuid {})", q(&uuid)));
    e.line(2, ")");
}

/// Return `(shape_token, width, height, roundrect_ratio)` for a pad shape.
fn pad_shape_fields(shape: &PadShape) -> (&'static str, f64, f64, f64) {
    match shape {
        PadShape::Circle { diameter } => ("circle", *diameter, *diameter, 0.0),
        PadShape::Rect { width, height } => ("rect", *width, *height, 0.0),
        PadShape::Oval { width, height } => ("oval", *width, *height, 0.0),
        PadShape::RoundRect {
            width,
            height,
            corner_ratio,
        } => ("roundrect", *width, *height, *corner_ratio),
        PadShape::Custom { vertices } => {
            // Approximate a custom polygon by its axis-aligned bounding box —
            // valid and visible; full primitive export is not yet supported.
            let (mut minx, mut miny, mut maxx, mut maxy) = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for v in vertices {
                minx = minx.min(v.x);
                miny = miny.min(v.y);
                maxx = maxx.max(v.x);
                maxy = maxy.max(v.y);
            }
            let w = if maxx > minx { maxx - minx } else { 1.0 };
            let h = if maxy > miny { maxy - miny } else { 1.0 };
            ("rect", w, h, 0.0)
        }
    }
}

fn write_pad_layers(e: &mut Emitter, pad: &Pad, fp: &Footprint) {
    if !pad.layers.is_empty() {
        let names: Vec<String> = pad.layers.iter().map(|l| q(layer_name(*l))).collect();
        e.line(3, &format!("(layers {})", names.join(" ")));
        return;
    }
    // No explicit layers (e.g. a THT pad whose "*.Cu" set the parser dropped).
    // Fall back to a sensible default for the pad type.
    let default = match pad.pad_type {
        PadType::SMD => {
            if fp.front {
                "\"F.Cu\" \"F.Paste\" \"F.Mask\""
            } else {
                "\"B.Cu\" \"B.Paste\" \"B.Mask\""
            }
        }
        PadType::THT => "\"*.Cu\" \"*.Mask\"",
        PadType::NPTH => "\"*.Cu\" \"*.Mask\"",
    };
    e.line(3, &format!("(layers {})", default));
}

fn write_segment(e: &mut Emitter, t: &Trace, nets: &NetTable) {
    let uuid = e.uuid();
    e.line(1, "(segment");
    e.line(2, &format!("(start {} {})", num(t.start.x), num(t.start.y)));
    e.line(2, &format!("(end {} {})", num(t.end.x), num(t.end.y)));
    e.line(2, &format!("(width {})", num(t.width)));
    e.line(2, &format!("(layer {})", q(layer_name(t.layer))));
    e.line(2, &format!("(net {})", nets.id(&t.net)));
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

fn write_arc(e: &mut Emitter, a: &TraceArc, nets: &NetTable) {
    let uuid = e.uuid();
    let (s, m, end) = arc_points(a.center, a.radius, a.start_angle, a.end_angle);
    e.line(1, "(arc");
    e.line(2, &format!("(start {} {})", num(s.x), num(s.y)));
    e.line(2, &format!("(mid {} {})", num(m.x), num(m.y)));
    e.line(2, &format!("(end {} {})", num(end.x), num(end.y)));
    e.line(2, &format!("(width {})", num(a.width)));
    e.line(2, &format!("(layer {})", q(layer_name(a.layer))));
    e.line(2, &format!("(net {})", nets.id(&a.net)));
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

fn write_via(e: &mut Emitter, v: &Via, nets: &NetTable) {
    let uuid = e.uuid();
    e.line(1, "(via");
    e.line(
        2,
        &format!("(at {} {})", num(v.position.x), num(v.position.y)),
    );
    e.line(2, &format!("(size {})", num(v.diameter)));
    e.line(2, &format!("(drill {})", num(v.drill)));
    e.line(
        2,
        &format!(
            "(layers {} {})",
            q(layer_name(v.start_layer)),
            q(layer_name(v.end_layer))
        ),
    );
    e.line(2, &format!("(net {})", nets.id(&v.net)));
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

fn write_zone(e: &mut Emitter, z: &Zone, nets: &NetTable) {
    let uuid = e.uuid();
    e.line(1, "(zone");
    e.line(2, &format!("(net {})", nets.id(&z.net)));
    e.line(2, &format!("(net_name {})", q(&z.net)));
    e.line(2, &format!("(layer {})", q(layer_name(z.layer))));
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(2, "(hatch edge 0.508)");
    e.line(2, "(connect_pads");
    e.line(3, &format!("(clearance {})", num(z.clearance)));
    e.line(2, ")");
    e.line(2, &format!("(min_thickness {})", num(z.clearance.max(0.0))));
    e.line(2, "(filled_areas_thickness no)");
    e.line(2, "(fill yes");
    e.line(3, "(thermal_gap 0.508)");
    e.line(3, "(thermal_bridge_width 0.508)");
    e.line(2, ")");
    e.line(2, "(polygon");
    write_pts(e, 3, &z.outline);
    e.line(2, ")");
    e.line(1, ")");
}

// ---- arc helper ----

/// Start, midpoint, and end of an arc, in KiCad's (start, mid, end) form.
fn arc_points(center: Vec2, radius: f64, start_deg: f64, end_deg: f64) -> (Vec2, Vec2, Vec2) {
    let mid_deg = (start_deg + end_deg) / 2.0;
    let pt = |deg: f64| {
        let r = deg.to_radians();
        Vec2::new(center.x + radius * r.cos(), center.y + radius * r.sin())
    };
    (pt(start_deg), pt(mid_deg), pt(end_deg))
}

// ---------------------------------------------------------------------------
// Schematic writer
// ---------------------------------------------------------------------------

/// Serialize a [`SchematicSheet`] to a KiCad 9 `.kicad_sch` schematic file.
///
/// Each component is emitted with a self-contained `lib_symbol` derived from
/// its pins (a generic rectangular body sized to bound the pins), so the file
/// opens and is editable in KiCad 9 with references, values, and connectivity
/// (wires / labels / junctions) preserved.  It is a faithful editable starting
/// point rather than a pixel-match of KiCad's built-in symbol artwork.
///
/// When the stored component positions are degenerate (all at one point, or
/// packed tighter than the symbol bodies allow), a deterministic auto-layout
/// pass replaces them with a readable left-to-right signal-flow arrangement
/// derived from `sheet.nets`; otherwise stored positions pass through
/// untouched.  Declared nets are additionally emitted as global-label stubs on
/// every referenced pin so connectivity survives into KiCad even without
/// drawn wires.
pub fn write_kicad_sch(sheet: &SchematicSheet) -> String {
    let mut e = Emitter::new();
    let root_uuid = e.uuid();
    let placements = sheet_placements(sheet);

    e.line(0, "(kicad_sch");
    e.line(1, "(version 20250114)");
    e.line(1, "(generator \"vcad\")");
    e.line(1, "(generator_version \"9.0\")");
    e.line(1, &format!("(uuid {})", q(&root_uuid)));
    e.line(1, "(paper \"A4\")");

    // lib_symbols — one generated definition per component (keyed by ref).
    e.line(1, "(lib_symbols");
    for comp in &sheet.components {
        write_lib_symbol(&mut e, comp);
    }
    e.line(1, ")");

    // Component instances.
    for (comp, pos) in sheet.components.iter().zip(&placements) {
        write_sch_symbol(&mut e, comp, *pos, &root_uuid);
    }

    // Wires.
    for w in &sheet.wires {
        write_wire(&mut e, w.start, w.end);
    }

    // Junctions.
    for j in &sheet.junctions {
        let uuid = e.uuid();
        e.line(1, "(junction");
        e.line(
            2,
            &format!("(at {} {})", num(j.position.x), num(j.position.y)),
        );
        e.line(2, "(diameter 0)");
        e.line(2, "(color 0 0 0 0)");
        e.line(2, &format!("(uuid {})", q(&uuid)));
        e.line(1, ")");
    }

    // Labels.
    for l in &sheet.labels {
        write_label(&mut e, l);
    }

    // Netlist declared as data (the MCP `nets` flow): a wire stub plus a
    // global label at every connected pin, on the placed positions, so
    // connectivity reaches KiCad even when the sheet has no drawn wires.
    write_net_stubs(&mut e, sheet, &placements);

    e.line(1, "(sheet_instances");
    e.line(2, "(path \"/\"");
    e.line(3, "(page \"1\")");
    e.line(2, ")");
    e.line(1, ")");
    e.line(1, "(embedded_fonts no)");
    e.line(0, ")");
    e.buf
}

// ---------------------------------------------------------------------------
// Placement pass
// ---------------------------------------------------------------------------

/// KiCad schematic grid pitch (mm). Pin offsets in generated symbols are
/// multiples of this, so snapping component origins keeps pin ends on-grid.
const SCH_GRID: f64 = 1.27;

/// Snap a coordinate to the schematic grid.
fn snap_grid(v: f64) -> f64 {
    (v / SCH_GRID).round() * SCH_GRID
}

/// Half-extents (x, y) of a component's pin bounding box — the space the
/// symbol itself needs on the sheet.
fn comp_half_extents(comp: &SchematicComponent) -> (f64, f64) {
    let mut hx: f64 = 2.54;
    let mut hy: f64 = 2.54;
    // Measure the pins the symbol will actually be drawn with: a component
    // whose stored pins are degenerate gets a synthesized layout that is far
    // wider than the stored (0,0) stack, and spacing must account for it.
    let (pins, _) = symbol_layout(comp);
    for p in &pins {
        hx = hx.max(p.position.x.abs());
        hy = hy.max(p.position.y.abs());
    }
    (hx, hy)
}

/// One position per component: the stored positions when they are usable, or
/// a deterministic auto-layout when they are degenerate.
fn sheet_placements(sheet: &SchematicSheet) -> Vec<Vec2> {
    if placement_is_degenerate(sheet) {
        return auto_layout(sheet);
    }
    // Stored positions pass through — but snap them to the schematic grid when
    // nothing on the sheet is drawn at fixed coordinates. KiCad's connection
    // grid is 1.27 mm, and a pin landing off it (`endpoint_off_grid`) can't be
    // wired to without dropping the grid size, which defeats the editable
    // handoff even though our synthesized connectivity is internally correct.
    //
    // Only safe when connectivity is fully synthesized: snapping moves symbols
    // by up to half a grid step, which would pull pins off any hand-drawn wire
    // or label placed at explicit coordinates. Those sheets keep raw positions.
    let synthesized_only = sheet.wires.is_empty() && sheet.labels.is_empty();
    sheet
        .components
        .iter()
        .map(|c| {
            if synthesized_only {
                Vec2::new(snap_grid(c.position.x), snap_grid(c.position.y))
            } else {
                c.position
            }
        })
        .collect()
}

/// Stored positions are degenerate when two or more symbols overlap — which
/// covers everything-at-origin and any spacing tighter than the symbol bodies
/// allow.
fn placement_is_degenerate(sheet: &SchematicSheet) -> bool {
    let comps = &sheet.components;
    for i in 0..comps.len() {
        let (hxi, hyi) = comp_half_extents(&comps[i]);
        for j in (i + 1)..comps.len() {
            let (hxj, hyj) = comp_half_extents(&comps[j]);
            let dx = (comps[i].position.x - comps[j].position.x).abs();
            let dy = (comps[i].position.y - comps[j].position.y).abs();
            if dx < hxi + hxj && dy < hyi + hyj {
                return true;
            }
        }
    }
    false
}

/// Component index for a `"R1.2"`-style pin reference, or `None` if the
/// reference names no component on the sheet.
fn pin_ref_comp<'a>(sheet: &SchematicSheet, pin_ref: &'a str) -> Option<(usize, &'a str)> {
    let (comp_ref, pin_no) = pin_ref.rsplit_once('.')?;
    let idx = sheet
        .components
        .iter()
        .position(|c| c.reference == comp_ref)?;
    Some((idx, pin_no))
}

/// Deterministic readable layout from declared connectivity: BFS rank from
/// signal sources into columns (sources left, sinks right), each column
/// ordered top-to-bottom by shared-net count, spaced by symbol extents plus
/// label clearance, all origins snapped to the schematic grid.
fn auto_layout(sheet: &SchematicSheet) -> Vec<Vec2> {
    let n = sheet.components.len();

    // Per-component net degree and adjacency, from the declared netlist.
    let mut degree = vec![0u32; n];
    let mut adjacency: Vec<std::collections::BTreeSet<usize>> = vec![Default::default(); n];
    if let Some(nets) = &sheet.nets {
        for pins in nets.values() {
            let mut members = std::collections::BTreeSet::new();
            for pin_ref in pins {
                if let Some((idx, _)) = pin_ref_comp(sheet, pin_ref) {
                    members.insert(idx);
                }
            }
            for &i in &members {
                degree[i] += 1;
                for &j in &members {
                    if i != j {
                        adjacency[i].insert(j);
                    }
                }
            }
        }
    }

    // BFS roots: components that drive signal (output/power-output pins);
    // fall back to the first component so the walk always starts somewhere.
    let mut roots: Vec<usize> = (0..n)
        .filter(|&i| {
            sheet.components[i]
                .pins
                .iter()
                .any(|p| matches!(p.pin_type, PinType::Output | PinType::PowerOutput))
        })
        .collect();
    if roots.is_empty() && n > 0 {
        roots.push(0);
    }

    // Multi-source BFS rank → column index.
    let mut column = vec![usize::MAX; n];
    let mut queue = std::collections::VecDeque::new();
    for &r in &roots {
        column[r] = 0;
        queue.push_back(r);
    }
    let mut max_col = 0;
    while let Some(i) = queue.pop_front() {
        for &j in &adjacency[i] {
            if column[j] == usize::MAX {
                column[j] = column[i] + 1;
                max_col = max_col.max(column[j]);
                queue.push_back(j);
            }
        }
    }
    // Components the netlist never reached: park them in a trailing column,
    // wrapped so an unconnected sheet still lays out as a compact grid.
    let unreached: Vec<usize> = (0..n).filter(|&i| column[i] == usize::MAX).collect();
    const WRAP: usize = 4;
    for (k, &i) in unreached.iter().enumerate() {
        column[i] = max_col + 1 + k / WRAP;
    }

    // Group by column; order each column by shared-net count (heaviest at the
    // top), tying back to the stable component index.
    let n_cols = column.iter().map(|c| c + 1).max().unwrap_or(0);
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n_cols];
    for i in 0..n {
        cols[column[i]].push(i);
    }
    for col in &mut cols {
        col.sort_by_key(|&i| (std::cmp::Reverse(degree[i]), i));
    }

    // Clearance around each symbol for reference/value and net-stub labels.
    const LABEL_CLEARANCE_X: f64 = 10.16;
    const LABEL_CLEARANCE_Y: f64 = 6.35;
    const ORIGIN: f64 = 25.4;

    let mut positions = vec![Vec2::new(0.0, 0.0); n];
    let mut x_cursor = ORIGIN;
    for col in &cols {
        let col_half_w = col
            .iter()
            .map(|&i| comp_half_extents(&sheet.components[i]).0 + LABEL_CLEARANCE_X)
            .fold(0.0f64, f64::max);
        let cx = snap_grid(x_cursor + col_half_w);
        let mut y_cursor = ORIGIN;
        for &i in col {
            let (_, hy) = comp_half_extents(&sheet.components[i]);
            let cy = snap_grid(y_cursor + hy + LABEL_CLEARANCE_Y);
            positions[i] = Vec2::new(cx, cy);
            y_cursor = cy + hy + LABEL_CLEARANCE_Y;
        }
        x_cursor = cx + col_half_w;
    }
    positions
}

/// World position of a pin's connection end, given the component's placed
/// origin. Symbol-local coordinates are Y-up; the sheet is Y-down.
fn pin_world(comp: &SchematicComponent, comp_pos: Vec2, pin_pos: Vec2) -> Vec2 {
    let px = if comp.mirror { -pin_pos.x } else { pin_pos.x };
    let py = pin_pos.y;
    let th = comp.rotation.to_radians();
    let (s, c) = (th.sin(), th.cos());
    Vec2::new(comp_pos.x + px * c - py * s, comp_pos.y - (px * s + py * c))
}

/// Emit connectivity for `sheet.nets` (net name → `"R1.2"` pin refs): a short
/// wire stub extending outward from each connected pin, capped with a global
/// label carrying the net name. This is how a data-declared netlist reaches
/// KiCad's ERC/netlister when the sheet has no coordinate-drawn wires.
///
/// Pins resolve through [`symbol_layout`], so components whose stored pin
/// positions are degenerate get stubs on their *synthesized* pin ends rather
/// than every stub collapsing onto the component origin. Positions come from
/// the placement pass, never the stored ones.
fn write_net_stubs(e: &mut Emitter, sheet: &SchematicSheet, placements: &[Vec2]) {
    let Some(nets) = &sheet.nets else {
        return;
    };
    // Points where the sheet already carries drawn connectivity. A pin sitting
    // on one of these is wired up in KiCad's eyes, so adding a stub there would
    // duplicate the net — and because a re-import reconstructs `nets` from that
    // same drawn geometry, the duplicates would compound on every export cycle.
    let mut drawn: Vec<Vec2> = Vec::new();
    for w in &sheet.wires {
        drawn.push(w.start);
        drawn.push(w.end);
    }
    for l in &sheet.labels {
        drawn.push(l.position);
    }
    let already_drawn = |p: Vec2| {
        drawn
            .iter()
            .any(|d| (d.x - p.x).abs() < 1e-6 && (d.y - p.y).abs() < 1e-6)
    };

    for (net, pin_refs) in nets {
        if net.is_empty() {
            continue;
        }
        for pin_ref in pin_refs {
            let Some((idx, pin_no)) = pin_ref_comp(sheet, pin_ref) else {
                continue;
            };
            let comp = &sheet.components[idx];
            let (pins, body) = symbol_layout(comp);
            let Some(pin) = pins.iter().find(|p| p.number == pin_no) else {
                continue;
            };
            if already_drawn(pin_world(comp, placements[idx], pin.position)) {
                continue;
            }
            // Outward direction in symbol space (Y-up): opposite the pin's
            // stub direction, which points toward the body.
            let (ox, oy) = match pin_angle(comp, pin.position, body) as i64 {
                0 => (-1.0, 0.0),
                180 => (1.0, 0.0),
                90 => (0.0, -1.0),
                _ => (0.0, 1.0),
            };
            let start = pin_world(comp, placements[idx], pin.position);
            let end = pin_world(
                comp,
                placements[idx],
                Vec2::new(
                    pin.position.x + ox * PIN_PITCH,
                    pin.position.y + oy * PIN_PITCH,
                ),
            );
            write_wire(e, start, end);

            // Label faces along the stub, reading away from the body.
            let (dx, dy) = (end.x - start.x, end.y - start.y);
            let rotation = if dx.abs() >= dy.abs() {
                if dx >= 0.0 {
                    0.0
                } else {
                    180.0
                }
            } else if dy >= 0.0 {
                270.0
            } else {
                90.0
            };
            write_label(
                e,
                &SchematicLabel {
                    name: net.clone(),
                    position: end,
                    rotation,
                    scope: LabelScope::Global,
                },
            );
        }
    }
}

/// KiCad pin electrical-type token for a [`PinType`].
fn pin_type_token(t: PinType) -> &'static str {
    match t {
        PinType::Input => "input",
        PinType::Output => "output",
        PinType::Bidirectional => "bidirectional",
        PinType::TriState => "tri_state",
        PinType::Passive => "passive",
        PinType::PowerInput => "power_in",
        PinType::PowerOutput => "power_out",
        PinType::OpenCollector => "open_collector",
        PinType::OpenEmitter => "open_emitter",
        PinType::NotConnected => "no_connect",
        PinType::Free => "unspecified",
    }
}

/// A stable lib_id for a component's generated symbol.
fn comp_lib_id(comp: &SchematicComponent) -> String {
    format!("vcad:{}", sanitize_lib_name(&comp.reference))
}

/// KiCad lib names can't contain `{}` or whitespace; replace the unsafe bits.
fn sanitize_lib_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() || "{}()\"".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "SYM".to_string()
    } else {
        cleaned
    }
}

/// Schematic grid pitch (mm) — pins, stubs, and synthesized layouts snap to it.
const PIN_PITCH: f64 = 2.54;
/// Synthesized IC-style layout: pin connection points sit at ±this x.
const SYNTH_PIN_X: f64 = 7.62;
/// Synthesized layout body half-width (pin x minus the 2.54 pin length).
const SYNTH_BODY_X: f64 = 5.08;

/// True when the component's pin positions carry no usable geometry — every
/// pin stacked at a single point. The MCP `create_schematic` `nets` flow
/// leaves them all at the (0,0) default, which would collapse the symbol body
/// to a zero-area rectangle.
fn pins_degenerate(comp: &SchematicComponent) -> bool {
    match comp.pins.first() {
        None => false,
        Some(first) => comp.pins.iter().all(|p| p.position == first.position),
    }
}

/// Effective pin layout and body rectangle for a component's symbol.
///
/// When pin positions are usable they pass through untouched (body from
/// [`symbol_body`]). When they are missing/degenerate, an IC-style layout is
/// synthesized: pins distributed down the left then right edges at 2.54mm
/// pitch, body rectangle derived from the pin extents.
fn symbol_layout(comp: &SchematicComponent) -> (Vec<SchematicPin>, (Vec2, Vec2)) {
    if !pins_degenerate(comp) {
        return (comp.pins.clone(), symbol_body(comp));
    }
    let n = comp.pins.len();
    let n_left = n.div_ceil(2);
    let rows = n_left.max(n - n_left).max(1);
    let top = (rows as f64 - 1.0) * PIN_PITCH / 2.0;
    let pins: Vec<SchematicPin> = comp
        .pins
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, row) = if i < n_left {
                (-SYNTH_PIN_X, i)
            } else {
                (SYNTH_PIN_X, i - n_left)
            };
            let mut np = p.clone();
            np.position = Vec2::new(x, top - row as f64 * PIN_PITCH);
            np
        })
        .collect();
    let body = (
        Vec2::new(-SYNTH_BODY_X, top + PIN_PITCH),
        Vec2::new(SYNTH_BODY_X, -top - PIN_PITCH),
    );
    (pins, body)
}

fn write_lib_symbol(e: &mut Emitter, comp: &SchematicComponent) {
    let lib_id = comp_lib_id(comp);
    e.line(2, &format!("(symbol {}", q(&lib_id)));
    e.line(3, "(pin_numbers");
    e.line(4, "(hide no)");
    e.line(3, ")");
    e.line(3, "(pin_names");
    e.line(4, "(offset 1.016)");
    e.line(3, ")");
    e.line(3, "(exclude_from_sim no)");
    e.line(3, "(in_bom yes)");
    e.line(3, "(on_board yes)");

    // Property labels inside the symbol definition.
    write_lib_property(e, "Reference", "REF", 0);
    write_lib_property(e, "Value", &comp.value, 1);
    write_lib_property(e, "Footprint", &comp.footprint_id, 2);
    write_lib_property(e, "Datasheet", "", 3);

    // Body rectangle bounding the pins, plus the pins themselves.
    let (pins, body) = symbol_layout(comp);
    e.line(
        3,
        &format!(
            "(symbol {}",
            q(&format!("{}_0_1", sanitize_lib_name(&comp.reference)))
        ),
    );
    e.line(4, "(rectangle");
    e.line(5, &format!("(start {} {})", num(body.0.x), num(body.0.y)));
    e.line(5, &format!("(end {} {})", num(body.1.x), num(body.1.y)));
    e.line(5, "(stroke");
    e.line(6, "(width 0.254)");
    e.line(6, "(type solid)");
    e.line(5, ")");
    e.line(5, "(fill");
    e.line(6, "(type background)");
    e.line(5, ")");
    e.line(4, ")");
    e.line(3, ")");

    e.line(
        3,
        &format!(
            "(symbol {}",
            q(&format!("{}_1_1", sanitize_lib_name(&comp.reference)))
        ),
    );
    for pin in &pins {
        let angle = pin_angle(comp, pin.position, body);
        e.line(4, &format!("(pin {} line", pin_type_token(pin.pin_type)));
        e.line(
            5,
            &format!(
                "(at {} {} {})",
                num(pin.position.x),
                num(pin.position.y),
                num(angle)
            ),
        );
        e.line(5, "(length 2.54)");
        e.line(5, &format!("(name {}", q(&pin.name)));
        e.line(6, "(effects");
        e.line(7, "(font");
        e.line(8, "(size 1.27 1.27)");
        e.line(7, ")");
        e.line(6, ")");
        e.line(5, ")");
        e.line(5, &format!("(number {}", q(&pin.number)));
        e.line(6, "(effects");
        e.line(7, "(font");
        e.line(8, "(size 1.27 1.27)");
        e.line(7, ")");
        e.line(6, ")");
        e.line(5, ")");
        e.line(4, ")");
    }
    e.line(3, ")");

    e.line(2, ")");
}

fn write_lib_property(e: &mut Emitter, key: &str, value: &str, idx: usize) {
    e.line(3, &format!("(property {} {}", q(key), q(value)));
    let y = -(idx as f64) * 2.54;
    e.line(4, &format!("(at 0 {} 0)", num(y)));
    e.line(4, "(effects");
    e.line(5, "(font");
    e.line(6, "(size 1.27 1.27)");
    e.line(5, ")");
    if key == "Footprint" || key == "Datasheet" {
        e.line(5, "(hide yes)");
    }
    e.line(4, ")");
    e.line(3, ")");
}

/// Bounding rectangle for the symbol body, derived from pin extents so pins sit
/// just outside the box. Returns (top-left-ish, bottom-right-ish) corners.
fn symbol_body(comp: &SchematicComponent) -> (Vec2, Vec2) {
    if comp.pins.is_empty() {
        return (Vec2::new(-2.54, 2.54), Vec2::new(2.54, -2.54));
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in &comp.pins {
        minx = minx.min(p.position.x);
        miny = miny.min(p.position.y);
        maxx = maxx.max(p.position.x);
        maxy = maxy.max(p.position.y);
    }
    // Shrink the box inside the pin ring so pin stubs read as connecting in.
    let inset = 1.27;
    let (mut x0, mut y0, mut x1, mut y1) = (minx + inset, miny + inset, maxx - inset, maxy - inset);
    if x1 <= x0 {
        x0 -= 1.27;
        x1 += 1.27;
    }
    if y1 <= y0 {
        y0 -= 1.27;
        y1 += 1.27;
    }
    (Vec2::new(x0, y1), Vec2::new(x1, y0))
}

/// Pin rotation (degrees) so the stub points from the body toward the pin.
fn pin_angle(_comp: &SchematicComponent, pos: Vec2, body: (Vec2, Vec2)) -> f64 {
    let cx = (body.0.x + body.1.x) / 2.0;
    let cy = (body.0.y + body.1.y) / 2.0;
    let dx = pos.x - cx;
    let dy = pos.y - cy;
    // KiCad pin angle points from the pin's connection end toward the body, so
    // a pin on the right of the body has angle 180.
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            180.0
        } else {
            0.0
        }
    } else if dy >= 0.0 {
        270.0
    } else {
        90.0
    }
}

fn write_sch_symbol(e: &mut Emitter, comp: &SchematicComponent, pos: Vec2, root_uuid: &str) {
    let lib_id = comp_lib_id(comp);
    let uuid = e.uuid();
    e.line(1, "(symbol");
    e.line(2, &format!("(lib_id {})", q(&lib_id)));
    let at = format!("(at {} {} {})", num(pos.x), num(pos.y), num(comp.rotation));
    e.line(2, &at);
    if comp.mirror {
        e.line(2, "(mirror y)");
    }
    e.line(2, "(unit 1)");
    e.line(2, "(exclude_from_sim no)");
    e.line(2, "(in_bom yes)");
    e.line(2, "(on_board yes)");
    e.line(2, &format!("(uuid {})", q(&uuid)));

    write_inst_property(e, "Reference", &comp.reference, 3.81, false);
    write_inst_property(e, "Value", &comp.value, -3.81, false);
    write_inst_property(e, "Footprint", &comp.footprint_id, 0.0, true);

    for pin in &comp.pins {
        let pin_uuid = e.uuid();
        e.line(2, &format!("(pin {}", q(&pin.number)));
        e.line(3, &format!("(uuid {})", q(&pin_uuid)));
        e.line(2, ")");
    }

    e.line(2, "(instances");
    e.line(3, "(project \"\"");
    e.line(4, &format!("(path {}", q(&format!("/{}", root_uuid))));
    e.line(5, &format!("(reference {})", q(&comp.reference)));
    e.line(5, "(unit 1)");
    e.line(4, ")");
    e.line(3, ")");
    e.line(2, ")");
    e.line(1, ")");
}

fn write_inst_property(e: &mut Emitter, key: &str, value: &str, y_off: f64, hide: bool) {
    e.line(2, &format!("(property {} {}", q(key), q(value)));
    e.line(3, &format!("(at 0 {} 0)", num(y_off)));
    e.line(3, "(effects");
    e.line(4, "(font");
    e.line(5, "(size 1.27 1.27)");
    e.line(4, ")");
    if hide {
        e.line(4, "(hide yes)");
    }
    e.line(3, ")");
    e.line(2, ")");
}

fn write_label(e: &mut Emitter, l: &SchematicLabel) {
    let uuid = e.uuid();
    let (tag, extra) = match l.scope {
        LabelScope::Local => ("label", None),
        LabelScope::Global => ("global_label", Some("(shape input)")),
        LabelScope::Hierarchical => ("hierarchical_label", Some("(shape input)")),
    };
    e.line(1, &format!("({} {}", tag, q(&l.name)));
    if let Some(x) = extra {
        e.line(2, x);
    }
    e.line(
        2,
        &format!(
            "(at {} {} {})",
            num(l.position.x),
            num(l.position.y),
            num(l.rotation)
        ),
    );
    e.line(2, "(effects");
    e.line(3, "(font");
    e.line(4, "(size 1.27 1.27)");
    e.line(3, ")");
    e.line(2, ")");
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

fn write_wire(e: &mut Emitter, start: Vec2, end: Vec2) {
    let uuid = e.uuid();
    e.line(1, "(wire");
    e.line(2, "(pts");
    e.line(3, &format!("(xy {} {})", num(start.x), num(start.y)));
    e.line(3, &format!("(xy {} {})", num(end.x), num(end.y)));
    e.line(2, ")");
    e.line(2, "(stroke");
    e.line(3, "(width 0)");
    e.line(3, "(type default)");
    e.line(2, ")");
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kicad_pcb::parse_kicad_pcb;

    fn sample_pcb() -> Pcb {
        use vcad_ir::ecad::{
            DesignRules, DrillSpec, Net, NetClassRules, StackupLayer, ThermalReliefStyle,
            ZoneFillType,
        };
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.51),
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
                    id: "1".into(),
                    name: "VCC".into(),
                },
                Net {
                    id: "2".into(),
                    name: "GND".into(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.25,
                hole_to_hole: 0.25,
                min_annular_ring: 0.13,
                min_drill: 0.2,
            },
            footprints: vec![Footprint {
                reference: "R1".into(),
                value: "10k".into(),
                footprint_name: "Resistor_SMD:R_0805".into(),
                position: Vec2::new(25.0, 40.0),
                rotation: 90.0,
                front: true,
                pads: vec![
                    Pad {
                        number: "1".into(),
                        pad_type: PadType::SMD,
                        shape: PadShape::RoundRect {
                            width: 1.0,
                            height: 1.2,
                            corner_ratio: 0.25,
                        },
                        position: Vec2::new(-1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("VCC".into()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                    Pad {
                        number: "2".into(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("GND".into()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                    // A through-hole pad with a drill — exercises the drill +
                    // THT layer-default paths and DrillSpec round-trip.
                    Pad {
                        number: "3".into(),
                        pad_type: PadType::THT,
                        shape: PadShape::Circle { diameter: 1.6 },
                        position: Vec2::new(0.0, 2.0),
                        rotation: 0.0,
                        drill: Some(DrillSpec {
                            diameter: 0.8,
                            oval: false,
                            oval_height: None,
                        }),
                        net: Some("GND".into()),
                        layers: vec![],
                    },
                ],
                graphics: vec![FootprintGraphic::Line {
                    start: Vec2::new(-1.5, -0.7),
                    end: Vec2::new(1.5, -0.7),
                    width: 0.12,
                    layer: PcbLayer::FSilkS,
                }],
                model_3d: None,
                properties: std::collections::HashMap::new(),
            }],
            traces: vec![Trace {
                start: Vec2::new(25.0, 40.0),
                end: Vec2::new(50.0, 40.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "VCC".into(),
                source: None,
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(50.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "VCC".into(),
                source: None,
            }],
            zones: vec![Zone {
                outline: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
                ],
                holes: vec![],
                net: "GND".into(),
                layer: PcbLayer::BCu,
                clearance: 0.3,
                min_area: 0.0,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: None,
                thermal_spoke_width: None,
                priority: 0,
            }],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn pcb_round_trips_through_kicad() {
        let pcb = sample_pcb();
        let text = write_kicad_pcb(&pcb);
        let reparsed = parse_kicad_pcb(&text).expect("re-parse exported board");

        // Structural stability — names, counts, geometry survive the round trip.
        assert_eq!(reparsed.footprints.len(), 1);
        assert_eq!(reparsed.footprints[0].reference, "R1");
        assert_eq!(reparsed.footprints[0].pads.len(), 3);
        assert_eq!(reparsed.footprints[0].position, Vec2::new(25.0, 40.0));
        assert_eq!(reparsed.footprints[0].rotation, 90.0);

        // The through-hole pad's drill survives.
        let tht = reparsed.footprints[0]
            .pads
            .iter()
            .find(|p| p.number == "3")
            .expect("THT pad present");
        assert_eq!(tht.pad_type, PadType::THT);
        assert_eq!(tht.drill.as_ref().map(|d| d.diameter), Some(0.8));

        // Pad net references (by name) survive.
        let pad_nets: Vec<Option<String>> = reparsed.footprints[0]
            .pads
            .iter()
            .map(|p| p.net.clone())
            .collect();
        assert!(pad_nets.contains(&Some("VCC".to_string())));
        assert!(pad_nets.contains(&Some("GND".to_string())));

        assert_eq!(reparsed.traces.len(), 1);
        assert_eq!(reparsed.traces[0].net, "VCC");
        assert_eq!(reparsed.traces[0].start, Vec2::new(25.0, 40.0));
        assert_eq!(reparsed.traces[0].end, Vec2::new(50.0, 40.0));

        assert_eq!(reparsed.vias.len(), 1);
        assert_eq!(reparsed.vias[0].position, Vec2::new(50.0, 40.0));
        assert_eq!(reparsed.vias[0].net, "VCC");

        assert_eq!(reparsed.zones.len(), 1);
        assert_eq!(reparsed.zones[0].net, "GND");
        assert_eq!(reparsed.zones[0].layer, PcbLayer::BCu);

        // Outline: 4 vertices recovered.
        assert_eq!(reparsed.outline.vertices.len(), 4);
        assert_eq!(reparsed.outline.thickness, 1.6);

        // Nets recovered by name.
        let names: Vec<String> = reparsed.nets.iter().map(|n| n.name.clone()).collect();
        assert!(names.contains(&"VCC".to_string()));
        assert!(names.contains(&"GND".to_string()));
    }

    /// The headline guarantee: import a real KiCad board, export it, re-import,
    /// and confirm the structure is stable across the round trip.
    #[test]
    fn import_export_reimport_is_structurally_stable() {
        let input = r#"(kicad_pcb (version 20221018) (generator pcbnew)
  (general (thickness 1.6))
  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (44 "Edge.Cuts" user)
  )
  (net 0 "")
  (net 1 "VCC")
  (net 2 "GND")

  (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 100 0) (end 100 80) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 100 80) (end 0 80) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 0 80) (end 0 0) (layer "Edge.Cuts") (width 0.05))

  (footprint "R_0805" (layer "F.Cu")
    (at 25 40)
    (fp_text reference "R1" (at 0 0) (layer "F.SilkS"))
    (fp_text value "10k" (at 0 2) (layer "F.Fab"))
    (pad "1" smd rect (at -1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "VCC"))
    (pad "2" smd rect (at 1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "GND"))
  )

  (segment (start 25 40) (end 50 40) (width 0.25) (layer "F.Cu") (net 1))
  (via (at 50 40) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net 1))
)"#;

        let pcb1 = parse_kicad_pcb(input).expect("import board");
        let exported = write_kicad_pcb(&pcb1);
        let pcb2 = parse_kicad_pcb(&exported).expect("re-import exported board");

        // Counts.
        assert_eq!(pcb1.footprints.len(), pcb2.footprints.len());
        assert_eq!(pcb1.traces.len(), pcb2.traces.len());
        assert_eq!(pcb1.vias.len(), pcb2.vias.len());
        assert_eq!(pcb1.outline.vertices.len(), pcb2.outline.vertices.len());
        assert_eq!(pcb1.outline.thickness, pcb2.outline.thickness);

        // Nets by name.
        let names1: std::collections::BTreeSet<_> =
            pcb1.nets.iter().map(|n| n.name.clone()).collect();
        let names2: std::collections::BTreeSet<_> =
            pcb2.nets.iter().map(|n| n.name.clone()).collect();
        assert_eq!(names1, names2);

        // Footprint refs + pad net assignments (by name).
        let fp1 = &pcb1.footprints[0];
        let fp2 = &pcb2.footprints[0];
        assert_eq!(fp1.reference, fp2.reference);
        assert_eq!(fp1.value, fp2.value);
        assert_eq!(fp1.position, fp2.position);
        assert_eq!(fp1.pads.len(), fp2.pads.len());
        for (a, b) in fp1.pads.iter().zip(&fp2.pads) {
            assert_eq!(a.number, b.number);
            assert_eq!(a.net, b.net);
            assert_eq!(a.position, b.position);
        }

        // Trace + via net names and geometry.
        assert_eq!(pcb1.traces[0].net, pcb2.traces[0].net);
        assert_eq!(pcb1.traces[0].start, pcb2.traces[0].start);
        assert_eq!(pcb1.traces[0].end, pcb2.traces[0].end);
        assert_eq!(pcb1.vias[0].net, pcb2.vias[0].net);
        assert_eq!(pcb1.vias[0].position, pcb2.vias[0].position);

        // Exporting the re-imported board yields byte-identical output (fixpoint).
        assert_eq!(exported, write_kicad_pcb(&pcb2));
    }

    #[test]
    fn pcb_is_deterministic() {
        let pcb = sample_pcb();
        assert_eq!(write_kicad_pcb(&pcb), write_kicad_pcb(&pcb));
    }

    #[test]
    fn pcb_has_expected_shape() {
        let pcb = sample_pcb();
        let text = write_kicad_pcb(&pcb);
        assert!(text.starts_with("(kicad_pcb"));
        assert!(text.contains("(generator \"vcad\")"));
        assert!(text.contains("(net 0 \"\")"));
        assert!(text.contains("\"VCC\""));
        assert!(text.contains("(footprint \"Resistor_SMD:R_0805\""));
        assert!(text.contains("(roundrect_rratio 0.25)"));
        assert!(text.contains("(segment"));
        assert!(text.contains("(via"));
        assert!(text.contains("(zone"));
        assert!(text.contains("(layer \"Edge.Cuts\")"));
        assert!(text.trim_end().ends_with(')'));
    }

    /// Dump a sample board + schematic to disk so they can be opened in KiCad 9
    /// (or validated with `kicad-cli pcb drc`). Ignored by default; run with:
    ///   VCAD_KICAD_OUT=/tmp/vcad_kicad cargo test -p vcad-ecad-symbols \
    ///     dump_for_kicad -- --ignored --nocapture
    #[test]
    #[ignore = "writes files for manual KiCad 9 validation"]
    fn dump_for_kicad() {
        let dir = std::env::var("VCAD_KICAD_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("vcad_kicad"));
        std::fs::create_dir_all(&dir).unwrap();
        let pcb = sample_pcb();
        let board = dir.join("board.kicad_pcb");
        std::fs::write(&board, write_kicad_pcb(&pcb)).unwrap();
        println!("wrote {}", board.display());

        let sheet = sample_sheet();
        let sch = dir.join("sheet.kicad_sch");
        std::fs::write(&sch, write_kicad_sch(&sheet)).unwrap();
        println!("wrote {}", sch.display());
    }

    fn sample_sheet() -> SchematicSheet {
        use vcad_ir::ecad::{SchematicComponent, SchematicJunction, SchematicPin, SchematicWire};
        SchematicSheet {
            title: Some("vcad export".into()),
            components: vec![
                SchematicComponent {
                    reference: "R1".into(),
                    value: "10k".into(),
                    footprint_id: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(100.0, 50.0),
                    rotation: 0.0,
                    mirror: false,
                    pins: vec![
                        SchematicPin {
                            number: "1".into(),
                            name: "~".into(),
                            pin_type: PinType::Passive,
                            position: Vec2::new(-2.54, 0.0),
                        },
                        SchematicPin {
                            number: "2".into(),
                            name: "~".into(),
                            pin_type: PinType::Passive,
                            position: Vec2::new(2.54, 0.0),
                        },
                    ],
                    pads_override: None,
                    properties: std::collections::HashMap::new(),
                },
                SchematicComponent {
                    reference: "C1".into(),
                    value: "100nF".into(),
                    footprint_id: "Capacitor_SMD:C_0603".into(),
                    position: Vec2::new(120.0, 50.0),
                    rotation: 90.0,
                    mirror: false,
                    pins: vec![
                        SchematicPin {
                            number: "1".into(),
                            name: "~".into(),
                            pin_type: PinType::Passive,
                            position: Vec2::new(0.0, 2.54),
                        },
                        SchematicPin {
                            number: "2".into(),
                            name: "~".into(),
                            pin_type: PinType::Passive,
                            position: Vec2::new(0.0, -2.54),
                        },
                    ],
                    pads_override: None,
                    properties: std::collections::HashMap::new(),
                },
            ],
            wires: vec![SchematicWire {
                start: Vec2::new(102.54, 50.0),
                end: Vec2::new(120.0, 50.0),
            }],
            junctions: vec![SchematicJunction {
                position: Vec2::new(120.0, 50.0),
            }],
            labels: vec![SchematicLabel {
                name: "VCC".into(),
                position: Vec2::new(97.46, 50.0),
                rotation: 180.0,
                scope: LabelScope::Global,
            }],
            nets: None,
        }
    }

    #[test]
    fn schematic_writes_valid_shell() {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin, SchematicWire};
        let sheet = SchematicSheet {
            title: Some("Test".into()),
            components: vec![SchematicComponent {
                reference: "R1".into(),
                value: "10k".into(),
                footprint_id: "Resistor_SMD:R_0805".into(),
                position: Vec2::new(100.0, 50.0),
                rotation: 0.0,
                mirror: false,
                pins: vec![
                    SchematicPin {
                        number: "1".into(),
                        name: "~".into(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(-2.54, 0.0),
                    },
                    SchematicPin {
                        number: "2".into(),
                        name: "~".into(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(2.54, 0.0),
                    },
                ],
                pads_override: None,
                properties: std::collections::HashMap::new(),
            }],
            wires: vec![SchematicWire {
                start: Vec2::new(97.46, 50.0),
                end: Vec2::new(90.0, 50.0),
            }],
            junctions: vec![],
            labels: vec![SchematicLabel {
                name: "VCC".into(),
                position: Vec2::new(90.0, 50.0),
                rotation: 0.0,
                scope: LabelScope::Global,
            }],
            nets: None,
        };
        let text = write_kicad_sch(&sheet);
        assert!(text.starts_with("(kicad_sch"));
        assert!(text.contains("(lib_symbols"));
        assert!(text.contains("(lib_id \"vcad:R1\")"));
        assert!(text.contains("(global_label \"VCC\""));
        assert!(text.contains("(wire"));
        assert!(text.trim_end().ends_with(')'));
    }

    /// Sheet with every component stacked at the origin (the naive
    /// create_schematic drop) plus a declared netlist.
    fn degenerate_sheet() -> SchematicSheet {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin};
        let two_pin = |number: &str, x: f64| SchematicPin {
            number: number.into(),
            name: "~".into(),
            pin_type: PinType::Passive,
            position: Vec2::new(x, 0.0),
        };
        let comp = |reference: &str, value: &str, pins: Vec<SchematicPin>| SchematicComponent {
            reference: reference.into(),
            value: value.into(),
            footprint_id: String::new(),
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            mirror: false,
            pins,
            pads_override: None,
            properties: std::collections::HashMap::new(),
        };
        let mut nets = std::collections::BTreeMap::new();
        nets.insert(
            "VCC".to_string(),
            vec!["U1.1".to_string(), "R1.1".to_string(), "C1.1".to_string()],
        );
        nets.insert(
            "OUT".to_string(),
            vec!["U1.2".to_string(), "R2.1".to_string()],
        );
        nets.insert(
            "GND".to_string(),
            vec![
                "R1.2".to_string(),
                "R2.2".to_string(),
                "C1.2".to_string(),
                "C2.2".to_string(),
            ],
        );
        SchematicSheet {
            title: Some("degenerate".into()),
            components: vec![
                comp(
                    "U1",
                    "AMP",
                    vec![
                        SchematicPin {
                            number: "1".into(),
                            name: "IN".into(),
                            pin_type: PinType::Input,
                            position: Vec2::new(-5.08, 0.0),
                        },
                        SchematicPin {
                            number: "2".into(),
                            name: "OUT".into(),
                            pin_type: PinType::Output,
                            position: Vec2::new(5.08, 0.0),
                        },
                    ],
                ),
                comp("R1", "10k", vec![two_pin("1", -2.54), two_pin("2", 2.54)]),
                comp("R2", "1k", vec![two_pin("1", -2.54), two_pin("2", 2.54)]),
                comp("C1", "100nF", vec![two_pin("1", -2.54), two_pin("2", 2.54)]),
                comp("C2", "1uF", vec![two_pin("1", -2.54), two_pin("2", 2.54)]),
            ],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: Some(nets),
        }
    }

    #[test]
    fn degenerate_placement_is_auto_laid_out() {
        let sheet = degenerate_sheet();
        assert!(placement_is_degenerate(&sheet));
        let placements = sheet_placements(&sheet);
        assert_eq!(placements.len(), sheet.components.len());

        for (i, p) in placements.iter().enumerate() {
            // On-grid origins (pin offsets are grid multiples, so pin ends
            // stay on-grid too).
            assert!(
                (p.x / SCH_GRID - (p.x / SCH_GRID).round()).abs() < 1e-9,
                "component {} x off-grid: {}",
                i,
                p.x
            );
            assert!(
                (p.y / SCH_GRID - (p.y / SCH_GRID).round()).abs() < 1e-9,
                "component {} y off-grid: {}",
                i,
                p.y
            );
            // Distinct positions.
            for (j, q2) in placements.iter().enumerate().skip(i + 1) {
                assert!(
                    p.x != q2.x || p.y != q2.y,
                    "components {} and {} share a position",
                    i,
                    j
                );
                // Non-overlapping symbol bodies.
                let (hxi, hyi) = comp_half_extents(&sheet.components[i]);
                let (hxj, hyj) = comp_half_extents(&sheet.components[j]);
                assert!(
                    (p.x - q2.x).abs() >= hxi + hxj || (p.y - q2.y).abs() >= hyi + hyj,
                    "components {} and {} overlap",
                    i,
                    j
                );
            }
        }

        // The source (U1, output pin) leads the signal flow: leftmost column.
        let u1_x = placements[0].x;
        for p in &placements[1..] {
            assert!(p.x >= u1_x, "source U1 is not leftmost");
        }

        // Byte-stable output, with net stubs on the adjusted positions.
        let text = write_kicad_sch(&sheet);
        assert_eq!(text, write_kicad_sch(&sheet));
        assert!(text.contains("(global_label \"VCC\""));
        assert!(text.contains("(global_label \"OUT\""));
        assert!(text.contains("(global_label \"GND\""));
        // Stub anchors sit on placed pin ends, not the stored origin: the
        // generated wire starts at the pin and the label caps its far end.
        let u1_out = pin_world(
            &sheet.components[0],
            placements[0],
            sheet.components[0].pins[1].position,
        );
        assert!(text.contains(&format!("(xy {} {})", num(u1_out.x), num(u1_out.y))));
    }

    #[test]
    fn healthy_placement_passes_through() {
        let sheet = sample_sheet();
        assert!(!placement_is_degenerate(&sheet));
        let placements = sheet_placements(&sheet);
        assert_eq!(placements[0], Vec2::new(100.0, 50.0));
        assert_eq!(placements[1], Vec2::new(120.0, 50.0));
        let text = write_kicad_sch(&sheet);
        assert!(text.contains("(at 100 50 0)"));
        assert!(text.contains("(at 120 50 90)"));
    }

    /// Sheet whose components carry no pin geometry at all — every pin at the
    /// (0,0) default, which is what the MCP `create_schematic` `nets` flow
    /// produces. Distinct from [`degenerate_sheet`], where the *component*
    /// positions collapse but pins are real.
    fn degenerate_pins_sheet() -> SchematicSheet {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin};
        let comp = |reference: &str, value: &str, footprint: &str, x: f64| SchematicComponent {
            reference: reference.into(),
            value: value.into(),
            footprint_id: footprint.into(),
            position: Vec2::new(x, 50.0),
            rotation: 0.0,
            mirror: false,
            pins: vec![
                SchematicPin {
                    number: "1".into(),
                    name: "~".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(0.0, 0.0),
                },
                SchematicPin {
                    number: "2".into(),
                    name: "~".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(0.0, 0.0),
                },
            ],
            pads_override: None,
            properties: std::collections::HashMap::new(),
        };
        let mut nets = std::collections::BTreeMap::new();
        nets.insert(
            "VCC".to_string(),
            vec!["R1.1".to_string(), "C1.1".to_string()],
        );
        nets.insert(
            "GND".to_string(),
            vec!["R1.2".to_string(), "C1.2".to_string()],
        );
        SchematicSheet {
            title: Some("nets flow".into()),
            components: vec![
                comp("R1", "10k", "Resistor_SMD:R_0805", 100.0),
                comp("C1", "100nF", "Capacitor_SMD:C_0603", 130.0),
            ],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: Some(nets),
        }
    }

    /// The MCP `create_schematic` `nets` flow: pins all at the (0,0) default,
    /// connectivity declared only as data. The exporter must synthesize a
    /// usable symbol layout and emit wires + global labels so the netlist
    /// round-trips into KiCad instead of collapsing to a zero-area body.
    #[test]
    fn nets_flow_synthesizes_layout_and_connectivity() {
        let sheet = degenerate_pins_sheet();
        let text = write_kicad_sch(&sheet);

        // Nonzero connectivity: one wire stub + one global label per pin ref.
        assert_eq!(text.matches("(wire").count(), 4);
        assert_eq!(text.matches("(global_label \"VCC\"").count(), 2);
        assert_eq!(text.matches("(global_label \"GND\"").count(), 2);

        // Nondegenerate synthesized body rectangle (2 pins → left/right at
        // ±7.62, body x∈[-5.08, 5.08], y∈[-2.54, 2.54]).
        assert!(text.contains("(start -5.08 2.54)"));
        assert!(text.contains("(end 5.08 -2.54)"));
        assert!(!text.contains("(start 0 0)"));

        // Pins land on the synthesized positions, not stacked at the origin.
        assert!(text.contains("(at -7.62 0 0)"));
        assert!(text.contains("(at 7.62 0 180)"));

        // Deterministic.
        assert_eq!(text, write_kicad_sch(&sheet));
    }

    /// Each pin of a degenerate-pin component must get its *own* stub anchor:
    /// the bug was every net of a component collapsing onto one point, which
    /// shorts them together in KiCad.
    #[test]
    fn degenerate_pins_get_distinct_stub_anchors() {
        let sheet = degenerate_pins_sheet();
        let placements = sheet_placements(&sheet);
        let (pins, _) = symbol_layout(&sheet.components[0]);
        let a = pin_world(&sheet.components[0], placements[0], pins[0].position);
        let b = pin_world(&sheet.components[0], placements[0], pins[1].position);
        assert!(a != b, "R1 pin anchors collapsed to one point: {a:?}");
    }

    /// Non-degenerate pin positions must pass through untouched.
    #[test]
    fn real_pin_positions_are_preserved() {
        let sheet = sample_sheet();
        let text = write_kicad_sch(&sheet);
        assert!(text.contains("(at -2.54 0 0)"));
        assert!(text.contains("(at 2.54 0 180)"));
        // sample_sheet declares no data nets — no synthesized stubs beyond the
        // one drawn wire.
        assert_eq!(text.matches("(wire").count(), 1);
    }

    /// Synthesized-connectivity sheets get their stored positions snapped to
    /// KiCad's 1.27 mm connection grid, so pins are wirable by hand after the
    /// handoff (KiCad ERC flagged `endpoint_off_grid` before this).
    #[test]
    fn synthesized_sheets_snap_stored_positions_to_grid() {
        // degenerate_pins_sheet stores y = 50.0, which is off the 1.27 grid,
        // and is spaced widely enough that auto_layout does not take over.
        let sheet = degenerate_pins_sheet();
        assert!(
            !placement_is_degenerate(&sheet),
            "would bypass the snap path"
        );
        for p in sheet_placements(&sheet) {
            // snap_grid is idempotent on grid points, so it is its own oracle
            // here — 1.27 has no exact binary representation, which makes a
            // fract()-based check unreliable.
            assert!(
                (snap_grid(p.x) - p.x).abs() < 1e-9 && (snap_grid(p.y) - p.y).abs() < 1e-9,
                "placement {p:?} is off the {SCH_GRID} mm grid"
            );
        }
    }

    /// ...but a sheet with drawn geometry keeps its raw positions: snapping
    /// would shift symbols off the wires and labels placed at fixed
    /// coordinates, silently breaking connectivity.
    #[test]
    fn drawn_sheets_keep_raw_positions() {
        let sheet = sample_sheet();
        assert!(!sheet.wires.is_empty(), "sample must carry drawn geometry");
        let placed = sheet_placements(&sheet);
        for (c, p) in sheet.components.iter().zip(&placed) {
            assert_eq!(c.position, *p);
        }
    }

    // -----------------------------------------------------------------------
    // Real-KiCad verification (requires kicad-cli; ignored by default)
    // -----------------------------------------------------------------------
    //
    // The tests above assert what *we* believe the exported file says. These
    // close the loop by handing the file to KiCad 9 itself and asking what it
    // reads back — the only check that catches a self-consistent-but-wrong
    // coordinate or connectivity convention.
    //
    // Run locally with:
    //   cargo test -p vcad-ecad-symbols -- --ignored --nocapture
    // kicad-cli is found via $KICAD_CLI, PATH, or the macOS app bundle.

    /// Locate a KiCad 9 `kicad-cli` binary, or `None` (tests skip cleanly).
    fn kicad_cli() -> Option<std::path::PathBuf> {
        use std::path::PathBuf;
        if let Ok(p) = std::env::var("KICAD_CLI") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        if let Ok(out) = std::process::Command::new("which")
            .arg("kicad-cli")
            .output()
        {
            if out.status.success() {
                let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        let mac = PathBuf::from("/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli");
        if mac.is_file() {
            return Some(mac);
        }
        None
    }

    /// Write the nets-flow sheet to a scratch dir and return its path.
    /// Honors VCAD_KICAD_OUT so the artifacts can be kept for inspection.
    fn export_nets_flow_sheet(subdir: &str) -> std::path::PathBuf {
        let dir = std::env::var("VCAD_KICAD_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("vcad_kicad"))
            .join(subdir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nets_flow.kicad_sch");
        std::fs::write(&path, write_kicad_sch(&degenerate_pins_sheet())).unwrap();
        path
    }

    /// ERC gate: KiCad 9's own electrical-rules check reports zero errors on
    /// the sheet the `nets` flow exports.  Warnings are printed but tolerated;
    /// the two KiCad 9 currently raises on our output are both artifacts of
    /// exporting a self-contained file rather than defects in it:
    ///
    /// - `lib_symbol_issues` — symbols are generated inline, so there is no
    ///   on-disk `vcad` symbol library for KiCad to resolve against.
    /// - `footprint_link_issues` — footprint ids reference the standard KiCad
    ///   libraries, which need not be installed to check the schematic.
    ///
    /// `endpoint_off_grid` used to appear here too, from stored component
    /// positions bypassing the grid snap; see `sheet_placements`.
    #[test]
    #[ignore = "requires kicad-cli (KiCad 9) — run with --ignored"]
    fn kicad_erc_reports_zero_errors() {
        let Some(cli) = kicad_cli() else {
            eprintln!("SKIP: kicad-cli not found (PATH, $KICAD_CLI, or KiCad.app)");
            return;
        };
        let sch = export_nets_flow_sheet("erc");
        let report = sch.with_file_name("erc.json");
        let out = std::process::Command::new(&cli)
            .args(["sch", "erc", "--format", "json", "--output"])
            .arg(&report)
            .arg(&sch)
            .output()
            .expect("run kicad-cli sch erc");
        assert!(
            out.status.success(),
            "kicad-cli sch erc failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap())
                .expect("parse ERC json");
        let mut errors = Vec::new();
        for sheet in json["sheets"].as_array().unwrap_or(&Vec::new()) {
            for v in sheet["violations"].as_array().unwrap_or(&Vec::new()) {
                let line = format!(
                    "[{}] {}: {}",
                    v["severity"].as_str().unwrap_or("?"),
                    v["type"].as_str().unwrap_or("?"),
                    v["description"].as_str().unwrap_or("?")
                );
                if v["severity"].as_str() == Some("error") {
                    errors.push(line);
                } else {
                    eprintln!("ERC warning (tolerated): {line}");
                }
            }
        }
        assert!(
            errors.is_empty(),
            "KiCad ERC errors:\n{}",
            errors.join("\n")
        );
    }

    /// Netlist equivalence — the real guarantee behind the `nets` flow: given
    /// only data-declared connectivity (pins all at the (0,0) default), the
    /// exporter synthesizes a layout whose geometry KiCad reads back as
    /// *exactly* the netlist we declared.  Asserting against `sheet.nets`
    /// rather than against our own expectations is what makes this a check on
    /// the convention itself and not just on the writer's self-consistency.
    #[test]
    #[ignore = "requires kicad-cli (KiCad 9) — run with --ignored"]
    fn kicad_netlist_matches_declared_nets() {
        use std::collections::{BTreeMap, BTreeSet};

        let Some(cli) = kicad_cli() else {
            eprintln!("SKIP: kicad-cli not found (PATH, $KICAD_CLI, or KiCad.app)");
            return;
        };
        let sch = export_nets_flow_sheet("netlist");
        let netfile = sch.with_file_name("nets_flow.net");
        let out = std::process::Command::new(&cli)
            .args([
                "sch",
                "export",
                "netlist",
                "--format",
                "kicadsexpr",
                "--output",
            ])
            .arg(&netfile)
            .arg(&sch)
            .output()
            .expect("run kicad-cli sch export netlist");
        assert!(
            out.status.success(),
            "kicad-cli netlist export failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let text = std::fs::read_to_string(&netfile).unwrap();
        let (_, root) = crate::sexpr::parse_sexpr(&text).expect("parse netlist s-expr");
        let nets_node = root.find("nets").expect("netlist has (nets ...)");
        let mut extracted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for net in nets_node.find_all("net") {
            let name = net
                .find("name")
                .and_then(|n| n.children().and_then(|c| c.get(1)).and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let mut pins = BTreeSet::new();
            for node in net.find_all("node") {
                let get = |key: &str| {
                    node.find(key)
                        .and_then(|n| n.children().and_then(|c| c.get(1)))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                pins.insert(format!("{}.{}", get("ref"), get("pin")));
            }
            if !pins.is_empty() {
                extracted.insert(name, pins);
            }
        }

        let declared: BTreeMap<String, BTreeSet<String>> = degenerate_pins_sheet()
            .nets
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();

        assert_eq!(
            extracted, declared,
            "KiCad-derived connectivity differs from the declared sheet.nets"
        );
    }
}
