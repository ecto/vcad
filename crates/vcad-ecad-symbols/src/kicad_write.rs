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
// Schematic ↔ board linkage
// ---------------------------------------------------------------------------

/// UUIDs captured while writing a schematic, used to link the board file back
/// to it so KiCad can cross-probe (click a symbol → highlight its footprint).
struct SchematicLinks {
    /// The schematic's root sheet uuid (also recorded in the `.kicad_pro`).
    root_uuid: String,
    /// Reference designator (e.g. `R1`) → symbol instance uuid.
    symbol_uuids: BTreeMap<String, String>,
    /// The schematic filename, emitted as each footprint's `sheetfile`.
    sch_filename: String,
}

// ---------------------------------------------------------------------------
// PCB writer
// ---------------------------------------------------------------------------

/// Serialize a [`Pcb`] to a KiCad 9 `.kicad_pcb` board file.
pub fn write_kicad_pcb(pcb: &Pcb) -> String {
    write_kicad_pcb_impl(pcb, None)
}

fn write_kicad_pcb_impl(pcb: &Pcb, links: Option<&SchematicLinks>) -> String {
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
    write_setup(&mut e, pcb);

    // Nets
    for (idx, name) in &nets.ordered {
        e.line(1, &format!("(net {} {})", idx, q(name)));
    }

    // Net classes — the rules the board was DRC'd against, so KiCad checks it
    // against those and not its own defaults.
    write_net_classes(&mut e, pcb);

    // Footprints
    for fp in &pcb.footprints {
        write_footprint(&mut e, fp, &nets, links);
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

/// Emit `(setup …)` including the board-level *design constraints*.
///
/// Without these, KiCad checks the board against its own factory defaults
/// (0.1 mm annular ring, 0.3 mm hole, 0.5 mm via) rather than the rules the
/// board was routed and DRC'd under — a 0.12/0.21 mm via class then reads as
/// thousands of spurious violations, and any rule calibration applied during
/// fab-prep is invisible in the file.
///
/// Only tokens KiCad 9's board parser actually accepts are emitted; the set
/// below was verified empirically against `kicad-cli pcb drc` (a `clearance`
/// or `edge_clearance` token in `(setup)`, for instance, makes KiCad 9 refuse
/// to load the board at all). Clearance and per-net widths travel in the
/// `(net_class …)` blocks instead; edge clearance, which has no board token,
/// travels in the project file (see `write_kicad_pro`).
fn write_setup(e: &mut Emitter, pcb: &Pcb) {
    let r = &pcb.rules;
    let d = &r.default_rules;
    e.line(1, "(setup");
    e.line(2, "(pad_to_mask_clearance 0)");
    e.line(2, &format!("(trace_min {})", num(d.trace_width)));
    e.line(2, &format!("(via_size {})", num(d.via_diameter)));
    e.line(2, &format!("(via_drill {})", num(d.via_drill)));
    e.line(2, &format!("(via_min_size {})", num(min_via_diameter(pcb))));
    e.line(2, &format!("(via_min_drill {})", num(r.min_drill)));
    e.line(2, &format!("(via_min_annulus {})", num(r.min_annular_ring)));
    e.line(2, &format!("(hole_to_hole_min {})", num(r.hole_to_hole)));
    e.line(2, "(allow_soldermask_bridges_in_footprints no)");
    e.line(1, ")");
}

/// The smallest via diameter any class on the board declares — the board-wide
/// `via_min_size` floor. Taking the *minimum* keeps a board whose fine class
/// uses 0.21 mm vias legal even when the default class is 0.8 mm.
fn min_via_diameter(pcb: &Pcb) -> f64 {
    std::iter::once(&pcb.rules.default_rules)
        .chain(pcb.rules.class_rules.iter())
        .map(|c| c.via_diameter)
        .fold(f64::INFINITY, f64::min)
}

/// The smallest via drill any class declares, floored by the board's declared
/// `minDrill` (a class may legitimately be looser than the fab's floor).
fn min_via_drill(pcb: &Pcb) -> f64 {
    std::iter::once(&pcb.rules.default_rules)
        .chain(pcb.rules.class_rules.iter())
        .map(|c| c.via_drill)
        .fold(pcb.rules.min_drill, f64::min)
}

/// Emit one `(net_class …)` block per declared class, with the class's nets
/// listed as `(add_net …)`. KiCad 9 still reads these from the board file, so
/// clearance and per-class trace/via geometry travel with the board even when
/// no project file accompanies it.
fn write_net_classes(e: &mut Emitter, pcb: &Pcb) {
    // net id → net name, so an assignment recorded by id resolves to the name
    // KiCad indexes nets by. Entries that are already names pass through.
    let by_id: BTreeMap<&str, &str> = pcb
        .nets
        .iter()
        .map(|n| (n.id.as_str(), n.name.as_str()))
        .collect();

    for class in std::iter::once(&pcb.rules.default_rules).chain(pcb.rules.class_rules.iter()) {
        let is_default = std::ptr::eq(class, &pcb.rules.default_rules);
        e.line(1, &format!("(net_class {} \"\"", q(&class.name)));
        e.line(2, &format!("(clearance {})", num(class.clearance)));
        e.line(2, &format!("(trace_width {})", num(class.trace_width)));
        e.line(2, &format!("(via_dia {})", num(class.via_diameter)));
        e.line(2, &format!("(via_drill {})", num(class.via_drill)));
        e.line(2, &format!("(uvia_dia {})", num(class.via_diameter)));
        e.line(2, &format!("(uvia_drill {})", num(class.via_drill)));
        if let Some(w) = class.diff_pair_width {
            e.line(2, &format!("(diff_pair_width {})", num(w)));
        }
        if let Some(g) = class.diff_pair_gap {
            e.line(2, &format!("(diff_pair_gap {})", num(g)));
        }
        if !is_default {
            let mut names: Vec<&str> = pcb
                .rules
                .net_class_assignments
                .get(&class.name)
                .into_iter()
                .flatten()
                .map(|n| *by_id.get(n.as_str()).unwrap_or(&n.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            names.sort_unstable();
            names.dedup();
            for n in names {
                e.line(2, &format!("(add_net {})", q(n)));
            }
        }
        e.line(1, ")");
    }
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

fn write_footprint(
    e: &mut Emitter,
    fp: &Footprint,
    nets: &NetTable,
    links: Option<&SchematicLinks>,
) {
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

    // Cross-probe linkage: point the footprint at its schematic symbol
    // instance (KiCad 9 root-sheet paths are "/<symbol-uuid>").
    if let Some(links) = links {
        if let Some(sym_uuid) = links.symbol_uuids.get(&fp.reference) {
            e.line(2, &format!("(path {})", q(&format!("/{}", sym_uuid))));
            e.line(2, "(sheetname \"/\")");
            e.line(2, &format!("(sheetfile {})", q(&links.sch_filename)));
        }
    }

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
    // KiCad's pad angle is ABSOLUTE (it includes the footprint's orientation);
    // vcad's IR keeps it relative to the footprint, so compose on the way out.
    // The reader performs the inverse, and `pad_rotation_round_trips` pins the
    // pair together.
    let abs_rot = {
        let r = (fp.rotation + pad.rotation) % 360.0;
        if r < 0.0 {
            r + 360.0
        } else {
            r
        }
    };
    let at = if abs_rot != 0.0 {
        format!(
            "(at {} {} {})",
            num(pad.position.x),
            num(pad.position.y),
            num(abs_rot)
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
/// Recognizable part classes (resistors, capacitors, diodes/LEDs,
/// inductors, transistors — see [`symbol_artwork`]) get real schematic
/// artwork; anything else keeps the generic rectangular body.
///
/// When the stored component positions are degenerate (all at one point, or
/// packed tighter than the symbol bodies allow), a deterministic auto-layout
/// pass replaces them with a readable left-to-right signal-flow arrangement
/// derived from `sheet.nets`; otherwise stored positions pass through
/// untouched.  Declared nets are additionally emitted as global-label stubs on
/// every referenced pin so connectivity survives into KiCad even without
/// drawn wires.
pub fn write_kicad_sch(sheet: &SchematicSheet) -> String {
    write_kicad_sch_impl(sheet, "", "").0
}

/// Writer core shared by [`write_kicad_sch`] and [`write_kicad_project`].
///
/// `project_name` is embedded in each symbol's `(instances (project …))`
/// block and `sch_filename` is recorded in the returned links; both are empty
/// for a standalone (projectless) export, which keeps that output unchanged.
fn write_kicad_sch_impl(
    sheet: &SchematicSheet,
    project_name: &str,
    sch_filename: &str,
) -> (String, SchematicLinks) {
    let mut e = Emitter::new();
    let root_uuid = e.uuid();
    let mut symbol_uuids = BTreeMap::new();
    let placements = sheet_placements(sheet);

    e.line(0, "(kicad_sch");
    e.line(1, "(version 20250114)");
    e.line(1, "(generator \"vcad\")");
    e.line(1, "(generator_version \"9.0\")");
    e.line(1, &format!("(uuid {})", q(&root_uuid)));
    e.line(1, "(paper \"A4\")");

    // Title block. No date — output must be byte-stable (golden tests, diffs).
    if let Some(title) = &sheet.title {
        e.line(1, "(title_block");
        e.line(2, &format!("(title {})", q(title)));
        e.line(2, "(rev \"vcad\")");
        e.line(2, "(company \"generated by vcad\")");
        e.line(1, ")");
    }

    // lib_symbols — one generated definition per component (keyed by ref).
    e.line(1, "(lib_symbols");
    for comp in &sheet.components {
        write_lib_symbol(&mut e, comp);
    }
    e.line(1, ")");

    // Component instances.
    for (comp, pos) in sheet.components.iter().zip(&placements) {
        let uuid = write_sch_symbol(&mut e, comp, *pos, &root_uuid, project_name);
        symbol_uuids.insert(comp.reference.clone(), uuid);
    }

    // Wires.
    for w in &sheet.wires {
        write_wire(&mut e, w.start, w.end);
    }

    // Junctions.
    for j in &sheet.junctions {
        write_junction(&mut e, j.position);
    }

    // Labels.
    for l in &sheet.labels {
        write_label(&mut e, l);
    }

    // Netlist declared as data (the MCP `nets` flow): a wire stub plus a
    // global label at every connected pin, on the placed positions, so
    // connectivity reaches KiCad even when the sheet has no drawn wires.
    let stubs = net_stubs(sheet, &placements);
    for st in &stubs {
        write_wire(&mut e, st.start, st.end);
        write_label(
            &mut e,
            &SchematicLabel {
                name: st.net.clone(),
                position: st.end,
                rotation: st.label_rotation,
                scope: LabelScope::Global,
            },
        );
    }

    // Junction dots where ≥3 wire endpoints (drawn wires + generated stubs)
    // coincide, so KiCad doesn't render tees as crossing-without-connection.
    let mut endpoints: BTreeMap<String, (Vec2, usize)> = BTreeMap::new();
    {
        let mut touch = |p: Vec2| {
            endpoints
                .entry(point_key(p))
                .and_modify(|(_, n)| *n += 1)
                .or_insert((p, 1));
        };
        for w in &sheet.wires {
            touch(w.start);
            touch(w.end);
        }
        for st in &stubs {
            touch(st.start);
            touch(st.end);
        }
    }
    let declared: std::collections::BTreeSet<String> = sheet
        .junctions
        .iter()
        .map(|j| point_key(j.position))
        .collect();
    for (key, (p, n)) in &endpoints {
        if *n >= 3 && !declared.contains(key) {
            write_junction(&mut e, *p);
        }
    }

    // No-connect flags on pins that are in no net and touch no wire endpoint,
    // so KiCad ERC doesn't warn "pin not connected" on intentionally open pins.
    write_no_connects(&mut e, sheet, &placements, &endpoints);

    e.line(1, "(sheet_instances");
    e.line(2, "(path \"/\"");
    e.line(3, "(page \"1\")");
    e.line(2, ")");
    e.line(1, ")");
    e.line(1, "(embedded_fonts no)");
    e.line(0, ")");
    (
        e.buf,
        SchematicLinks {
            root_uuid,
            symbol_uuids,
            sch_filename: sch_filename.to_string(),
        },
    )
}

// ---------------------------------------------------------------------------
// Class-specific symbol artwork
// ---------------------------------------------------------------------------
//
// Recognizable part classes (R/C/D/LED/L/Q, by reference prefix plus value /
// footprint hints) get real KiCad symbol artwork — zigzag resistors, plate
// capacitors, diode triangles — instead of the generic bounding rectangle, so
// exported schematics read like hand-drawn ones. Artwork is generated in a
// canonical frame (pin 1 at (-d, 0), pin 2 at (d, 0)) and rotated onto the
// component's actual pin axis, so pin positions — and therefore wire / label
// connectivity — are untouched.

/// A graphic primitive inside a lib_symbol body.
enum SymPrim {
    /// Open polyline through the given points.
    Polyline(Vec<Vec2>),
    /// Arc through start → mid → end.
    Arc { start: Vec2, mid: Vec2, end: Vec2 },
    /// Circle outline.
    Circle { center: Vec2, radius: f64 },
}

/// Class-specific body artwork plus per-pin (angle, length) overrides,
/// parallel to `comp.pins`.
struct SymbolArtwork {
    prims: Vec<SymPrim>,
    pins: Vec<(f64, f64)>,
}

/// Two-pin part classes with dedicated artwork.
enum TwoPinClass {
    Resistor,
    Capacitor { polarized: bool },
    Diode { led: bool },
    Inductor,
}

/// Leading alphabetic run of a reference designator, uppercased ("R12" → "R",
/// "LED3" → "LED").
fn ref_prefix(reference: &str) -> String {
    reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Case-insensitive "any needle appears in value or footprint id".
fn hints_contain(comp: &SchematicComponent, needles: &[&str]) -> bool {
    let hay = format!(
        "{} {}",
        comp.value.to_ascii_lowercase(),
        comp.footprint_id.to_ascii_lowercase()
    );
    needles.iter().any(|n| hay.contains(n))
}

/// Classify a component into a two-pin artwork class, if it matches one.
fn classify_two_pin(comp: &SchematicComponent) -> Option<TwoPinClass> {
    match ref_prefix(&comp.reference).as_str() {
        "R" => Some(TwoPinClass::Resistor),
        "C" => Some(TwoPinClass::Capacitor {
            polarized: hints_contain(comp, &["elec", "tant", "polar", "cp_"]),
        }),
        "LED" => Some(TwoPinClass::Diode { led: true }),
        "D" => Some(TwoPinClass::Diode {
            led: hints_contain(comp, &["led"]),
        }),
        "L" => Some(TwoPinClass::Inductor),
        _ => None,
    }
}

/// Grid-snapped pin angle pointing from the pin position toward the origin.
fn axis_pin_angle(p: Vec2) -> f64 {
    if p.x.abs() >= p.y.abs() {
        if p.x > 0.0 {
            180.0
        } else {
            0.0
        }
    } else if p.y > 0.0 {
        270.0
    } else {
        90.0
    }
}

/// Build class-specific artwork for a component, or `None` to keep the
/// generic rectangle body.
fn symbol_artwork(comp: &SchematicComponent, pins: &[SchematicPin]) -> Option<SymbolArtwork> {
    if ref_prefix(&comp.reference) == "Q" {
        return transistor_artwork(pins);
    }
    let class = classify_two_pin(comp)?;
    two_pin_artwork(pins, &class)
}

/// Artwork for symmetric two-pin parts. Requires exactly two pins placed
/// opposite each other through the origin (the layout `create_schematic`
/// produces: ±2.54 or ±3.81 on an axis).
fn two_pin_artwork(pins: &[SchematicPin], class: &TwoPinClass) -> Option<SymbolArtwork> {
    if pins.len() != 2 {
        return None;
    }
    let p1 = pins[0].position;
    let p2 = pins[1].position;
    if (p1.x + p2.x).abs() > 1e-6 || (p1.y + p2.y).abs() > 1e-6 {
        return None;
    }
    let d = (p1.x * p1.x + p1.y * p1.y).sqrt();
    if d < 2.0 {
        return None;
    }
    // Rotation from the canonical frame (pin 1 at (-d, 0)) onto the pin axis.
    let u = Vec2::new((p2.x - p1.x) / (2.0 * d), (p2.y - p1.y) / (2.0 * d));
    let map = |x: f64, y: f64| Vec2::new(u.x * x - u.y * y, u.y * x + u.x * y);
    let poly =
        |pts: &[(f64, f64)]| SymPrim::Polyline(pts.iter().map(|&(x, y)| map(x, y)).collect());
    let arc = |s: (f64, f64), m: (f64, f64), e: (f64, f64)| SymPrim::Arc {
        start: map(s.0, s.1),
        mid: map(m.0, m.1),
        end: map(e.0, e.1),
    };

    let (prims, len1, len2) = match class {
        TwoPinClass::Resistor => {
            // IEEE zigzag spanning the body, three peaks up / two down.
            let b = (0.6 * d).min(2.286);
            let a = 1.016;
            let zigzag = poly(&[
                (-b, 0.0),
                (-2.0 * b / 3.0, a),
                (-b / 3.0, -a),
                (0.0, a),
                (b / 3.0, -a),
                (2.0 * b / 3.0, a),
                (b, 0.0),
            ]);
            (vec![zigzag], d - b, d - b)
        }
        TwoPinClass::Capacitor { polarized } => {
            let g = 0.508;
            let h = 1.27;
            let mut prims = vec![poly(&[(-g, -h), (-g, h)])];
            let len2 = if *polarized {
                // Curved negative plate bowing away from the flat plate,
                // plus a "+" marker beside pin 1.
                prims.push(arc((g + 0.55, -h), (g, 0.0), (g + 0.55, h)));
                prims.push(poly(&[(-g - 1.9, h), (-g - 1.1, h)]));
                prims.push(poly(&[(-g - 1.5, h - 0.4), (-g - 1.5, h + 0.4)]));
                d - g - 0.55
            } else {
                prims.push(poly(&[(g, -h), (g, h)]));
                d - g
            };
            (prims, d - g, len2)
        }
        TwoPinClass::Diode { led } => {
            // Triangle pointing anode (pin 1) → cathode (pin 2), bar at the
            // cathode; LEDs add two light-emission arrows.
            let b = (0.5 * d).min(1.27);
            let mut prims = vec![
                poly(&[(-b, b), (-b, -b), (b, 0.0), (-b, b)]),
                poly(&[(b, -b), (b, b)]),
            ];
            if *led {
                prims.push(poly(&[(-0.3, b + 0.3), (0.6, b + 1.2), (0.25, b + 1.1)]));
                prims.push(poly(&[(0.5, b + 0.1), (1.4, b + 1.0), (1.05, b + 0.9)]));
            }
            (prims, d - b, d - b)
        }
        TwoPinClass::Inductor => {
            // Four humps across the body.
            let b = (0.75 * d).min(2.54);
            let r = b / 4.0;
            let prims = (0..4)
                .map(|k| {
                    let c = -b + r + 2.0 * r * k as f64;
                    arc((c - r, 0.0), (c, r), (c + r, 0.0))
                })
                .collect();
            (prims, d - b, d - b)
        }
    };

    if len1 < 0.4 || len2 < 0.4 {
        return None;
    }
    Some(SymbolArtwork {
        prims,
        pins: vec![(axis_pin_angle(p1), len1), (axis_pin_angle(p2), len2)],
    })
}

/// BJT artwork for the common layout: base alone on the left at y≈0,
/// collector and emitter on the right at ±y.
fn transistor_artwork(pins: &[SchematicPin]) -> Option<SymbolArtwork> {
    if pins.len() != 3 {
        return None;
    }
    let mut base_idx = None;
    let mut right: Vec<usize> = Vec::new();
    for (i, p) in pins.iter().enumerate() {
        if p.position.x < 0.0 {
            if base_idx.is_some() {
                return None;
            }
            base_idx = Some(i);
        } else {
            right.push(i);
        }
    }
    let base_idx = base_idx?;
    let base = pins[base_idx].position;
    if right.len() != 2 || base.y.abs() > 0.5 {
        return None;
    }
    let (a, b) = (pins[right[0]].position, pins[right[1]].position);
    if a.y * b.y >= 0.0 || a.y.abs() < 1.9 || b.y.abs() < 1.9 {
        return None;
    }
    let base_len = -base.x - 0.635;
    if base_len < 0.4 {
        return None;
    }
    let top = if a.y > 0.0 { a } else { b };
    let bot = if a.y > 0.0 { b } else { a };

    let prims = vec![
        // Vertical base bar, branch lines to collector/emitter pin stubs,
        // and the envelope circle.
        SymPrim::Polyline(vec![Vec2::new(-0.635, -1.397), Vec2::new(-0.635, 1.397)]),
        SymPrim::Polyline(vec![Vec2::new(-0.635, 0.508), Vec2::new(top.x, 1.27)]),
        SymPrim::Polyline(vec![Vec2::new(-0.635, -0.508), Vec2::new(bot.x, -1.27)]),
        SymPrim::Circle {
            center: Vec2::new((top.x - 0.635) / 2.0, 0.0),
            radius: 2.54,
        },
    ];

    let mut out = vec![(0.0, 0.0); 3];
    out[base_idx] = (0.0, base_len);
    for &i in &right {
        let p = pins[i].position;
        out[i] = if p.y > 0.0 {
            (270.0, p.y - 1.27)
        } else {
            (90.0, -p.y - 1.27)
        };
    }
    Some(SymbolArtwork { prims, pins: out })
}

/// Emit lib_symbol graphic primitives at body depth (inside the `_0_1`
/// sub-symbol).
fn write_sym_prims(e: &mut Emitter, prims: &[SymPrim]) {
    let stroke_fill = |e: &mut Emitter| {
        e.line(5, "(stroke");
        e.line(6, "(width 0.254)");
        e.line(6, "(type default)");
        e.line(5, ")");
        e.line(5, "(fill");
        e.line(6, "(type none)");
        e.line(5, ")");
    };
    for prim in prims {
        match prim {
            SymPrim::Polyline(pts) => {
                e.line(4, "(polyline");
                e.line(5, "(pts");
                for v in pts {
                    e.line(6, &format!("(xy {} {})", num(v.x), num(v.y)));
                }
                e.line(5, ")");
                stroke_fill(e);
                e.line(4, ")");
            }
            SymPrim::Arc { start, mid, end } => {
                e.line(4, "(arc");
                e.line(5, &format!("(start {} {})", num(start.x), num(start.y)));
                e.line(5, &format!("(mid {} {})", num(mid.x), num(mid.y)));
                e.line(5, &format!("(end {} {})", num(end.x), num(end.y)));
                stroke_fill(e);
                e.line(4, ")");
            }
            SymPrim::Circle { center, radius } => {
                e.line(4, "(circle");
                e.line(5, &format!("(center {} {})", num(center.x), num(center.y)));
                e.line(5, &format!("(radius {})", num(*radius)));
                stroke_fill(e);
                e.line(4, ")");
            }
        }
    }
}

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
    for p in &symbol_layout(comp).pins {
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
    // Snap the *rotated offset*, not the absolute result: component origins are
    // not always grid multiples, and snapping absolutely would shift a stub off
    // the pin it anchors to. Offsets from 90°-multiple rotations are already
    // grid multiples, so those pass through bit-identical; only odd angles move.
    // Endpoints sharing a coordinate before the snap still share it after, so
    // stubs stay axis-aligned wherever the rotation permits.
    Vec2::new(
        comp_pos.x + snap_to_grid(px * c - py * s),
        comp_pos.y + snap_to_grid(-(px * s + py * c)),
    )
}

/// Schematic wire grid (mm). Generated connection points land on it so KiCad
/// treats them as on-grid, connectable endpoints.
const WIRE_GRID: f64 = 1.27;

/// Round to the nearest [`WIRE_GRID`] multiple.
fn snap_to_grid(v: f64) -> f64 {
    (v / WIRE_GRID).round() * WIRE_GRID
}

/// Stable text key for endpoint-coincidence tests, using the same 6-decimal
/// rounding the emitter prints with — so "same point" means "same printed
/// point", with no float-epsilon disagreement against the emitted file.
fn point_key(p: Vec2) -> String {
    format!("{},{}", num(p.x), num(p.y))
}

fn write_junction(e: &mut Emitter, at: Vec2) {
    let uuid = e.uuid();
    e.line(1, "(junction");
    e.line(2, &format!("(at {} {})", num(at.x), num(at.y)));
    e.line(2, "(diameter 0)");
    e.line(2, "(color 0 0 0 0)");
    e.line(2, &format!("(uuid {})", q(&uuid)));
    e.line(1, ")");
}

/// Emit a `(no_connect)` at every pin that carries no connectivity: absent from
/// every declared net, and not sitting on any wire endpoint. Without these,
/// KiCad ERC reports "pin not connected" for pins left open on purpose.
fn write_no_connects(
    e: &mut Emitter,
    sheet: &SchematicSheet,
    placements: &[Vec2],
    endpoints: &BTreeMap<String, (Vec2, usize)>,
) {
    let netted: std::collections::BTreeSet<&str> = sheet
        .nets
        .iter()
        .flat_map(|nets| nets.values())
        .flatten()
        .map(String::as_str)
        .collect();
    for (idx, comp) in sheet.components.iter().enumerate() {
        for pin in &symbol_layout(comp).pins {
            let pin_ref = format!("{}.{}", comp.reference, pin.number);
            if netted.contains(pin_ref.as_str()) {
                continue;
            }
            let at = pin_world(comp, placements[idx], pin.position);
            if endpoints.contains_key(&point_key(at)) {
                continue;
            }
            let uuid = e.uuid();
            e.line(1, "(no_connect");
            e.line(2, &format!("(at {} {})", num(at.x), num(at.y)));
            e.line(2, &format!("(uuid {})", q(&uuid)));
            e.line(1, ")");
        }
    }
}

/// A generated net stub: a short wire from a pin outward, labeled with its net.
struct NetStub {
    start: Vec2,
    end: Vec2,
    net: String,
    label_rotation: f64,
}

/// Connectivity for `sheet.nets` (net name → `"R1.2"` pin refs): a short wire
/// stub extending outward from each connected pin, capped with a global label
/// carrying the net name. This is how a data-declared netlist reaches KiCad's
/// ERC/netlister when the sheet has no coordinate-drawn wires.
///
/// Pins resolve through [`symbol_layout`], so components whose stored pin
/// positions are degenerate get stubs on their *synthesized* pin ends rather
/// than every stub collapsing onto the component origin. Positions come from
/// the placement pass, never the stored ones.
///
/// Returned as data rather than emitted directly: the junction and no-connect
/// passes both need these endpoints before anything is written.
fn net_stubs(sheet: &SchematicSheet, placements: &[Vec2]) -> Vec<NetStub> {
    let mut stubs = Vec::new();
    let Some(nets) = &sheet.nets else {
        return stubs;
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
            let layout = symbol_layout(comp);
            let Some(pi) = layout.pins.iter().position(|p| p.number == pin_no) else {
                continue;
            };
            let pin = &layout.pins[pi];
            if already_drawn(pin_world(comp, placements[idx], pin.position)) {
                continue;
            }
            // Outward direction in symbol space (Y-up): opposite the pin's
            // stub direction, which points toward the body.
            let (ox, oy) = match layout.angles[pi] as i64 {
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
            // Label faces along the stub, reading away from the body.
            let (dx, dy) = (end.x - start.x, end.y - start.y);
            let label_rotation = if dx.abs() >= dy.abs() {
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
            stubs.push(NetStub {
                start,
                end,
                net: net.clone(),
                label_rotation,
            });
        }
    }
    stubs
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
/// Synthesized layout minimum body half-width (pin x minus the 2.54 pin
/// length); wider when the top/bottom rails need the room.
const SYNTH_BODY_X: f64 = 5.08;

/// Effective symbol geometry: laid-out pins, matching per-pin angles (same
/// order as `pins`), and the body rectangle.
struct SymbolLayout {
    pins: Vec<SchematicPin>,
    angles: Vec<f64>,
    body: (Vec2, Vec2),
}

/// Which symbol edge a laid-out pin sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// KiCad pin angle for this edge. The angle points from the pin's
    /// connection end toward the body (see [`pin_angle`]), so a right-edge pin
    /// is 180 and a top-edge pin points down at 270 — matching what
    /// [`pin_angle`] already yields for side pins.
    fn angle(self) -> f64 {
        match self {
            Edge::Left => 0.0,
            Edge::Right => 180.0,
            Edge::Top => 270.0,
            Edge::Bottom => 90.0,
        }
    }
}

/// True when the pin name carries no information ("~" is KiCad's "no name").
fn pin_name_empty(name: &str) -> bool {
    matches!(name.trim(), "" | "~")
}

/// True for conventional ground-rail names (GND, VSS, VEE, V-, AGND, …).
fn is_ground_name(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    n == "V-"
        || n.starts_with("GND")
        || n.starts_with("VSS")
        || n.starts_with("VEE")
        || n.starts_with("AGND")
        || n.starts_with("DGND")
        || n.starts_with("PGND")
}

/// True for conventional supply-rail names (VCC, VDD, V+, VBAT, VBUS, VIN).
fn is_supply_name(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    n == "V+"
        || n.starts_with("VCC")
        || n.starts_with("VDD")
        || n.starts_with("VBAT")
        || n.starts_with("VBUS")
        || n.starts_with("VIN")
}

/// Conventional edge for a pin with real metadata, or `None` for pins that
/// just fill remaining left/right slots (passives and the like).
fn pin_edge(pin: &SchematicPin) -> Option<Edge> {
    match pin.pin_type {
        PinType::PowerInput | PinType::PowerOutput => {
            if is_ground_name(&pin.name) {
                Some(Edge::Bottom)
            } else {
                Some(Edge::Top)
            }
        }
        PinType::Input => Some(Edge::Left),
        PinType::Output
        | PinType::Bidirectional
        | PinType::TriState
        | PinType::OpenCollector
        | PinType::OpenEmitter => Some(Edge::Right),
        _ => {
            // Passive/NC/Free pins with rail names still go to the rails.
            if is_ground_name(&pin.name) {
                Some(Edge::Bottom)
            } else if is_supply_name(&pin.name) {
                Some(Edge::Top)
            } else {
                None
            }
        }
    }
}

/// Edge assignment for a degenerate-pin component, in declaration order.
///
/// Pins carrying real names/types get conventional placement — power rails on
/// the top edge, grounds on the bottom, inputs on the left, outputs and
/// bidirectionals on the right — with unclassified pins filling the shorter of
/// the two side columns (left wins ties) so the body stays balanced. When no
/// pin carries a usable name the layout falls back to the index-based
/// left-then-right split.
///
/// Two-pin parts stay symmetric through the origin under every branch, which
/// is what keeps the class artwork in [`symbol_artwork`] matching.
fn synth_edges(comp: &SchematicComponent) -> Vec<Edge> {
    if !comp.pins.iter().any(|p| !pin_name_empty(&p.name)) {
        let n_left = comp.pins.len().div_ceil(2);
        return (0..comp.pins.len())
            .map(|i| if i < n_left { Edge::Left } else { Edge::Right })
            .collect();
    }
    let mut edges: Vec<Option<Edge>> = comp.pins.iter().map(pin_edge).collect();
    let count = |es: &[Option<Edge>], e: Edge| es.iter().filter(|x| **x == Some(e)).count();
    for i in 0..edges.len() {
        if edges[i].is_none() {
            let fill = if count(&edges, Edge::Left) <= count(&edges, Edge::Right) {
                Edge::Left
            } else {
                Edge::Right
            };
            edges[i] = Some(fill);
        }
    }
    edges
        .into_iter()
        .map(|e| e.expect("every pin assigned an edge"))
        .collect()
}

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

/// Effective pin layout, per-pin angles, and body rectangle for a symbol.
///
/// When pin positions are usable they pass through untouched (body from
/// [`symbol_body`], angles from [`pin_angle`]). When they are missing or
/// degenerate, an IC-style layout is synthesized on [`synth_edges`]: pins
/// distributed along their conventional edges at 2.54mm pitch, on-grid, with
/// the body rectangle derived from the resulting pin extents.
///
/// Angles come from the edge assignment rather than from pin geometry, so a
/// top or bottom rail pin is never mistaken for a side pin on a tall symbol.
fn symbol_layout(comp: &SchematicComponent) -> SymbolLayout {
    if !pins_degenerate(comp) {
        let body = symbol_body(comp);
        let angles = comp
            .pins
            .iter()
            .map(|p| pin_angle(comp, p.position, body))
            .collect();
        return SymbolLayout {
            pins: comp.pins.clone(),
            angles,
            body,
        };
    }
    let edges = synth_edges(comp);
    let count = |e: Edge| edges.iter().filter(|x| **x == e).count();
    let (n_left, n_right) = (count(Edge::Left), count(Edge::Right));
    let (n_top, n_bottom) = (count(Edge::Top), count(Edge::Bottom));

    // Rows on the side edges, columns on the top/bottom edges, everything
    // centered on the 2.54mm grid.
    let rows = n_left.max(n_right).max(1);
    let top = (rows as f64 - 1.0) * PIN_PITCH / 2.0;
    let body_y = top + PIN_PITCH;
    let cols = n_top.max(n_bottom);
    let body_x = if cols == 0 {
        SYNTH_BODY_X
    } else {
        SYNTH_BODY_X.max((cols as f64 - 1.0) * PIN_PITCH / 2.0 + PIN_PITCH)
    };
    let (pin_x, pin_y) = (body_x + PIN_PITCH, body_y + PIN_PITCH);

    // Slot counters per edge; pins keep their declared order along each edge.
    let (mut i_left, mut i_right, mut i_top, mut i_bottom) = (0usize, 0, 0, 0);
    let mut slot = |edge: Edge| -> Vec2 {
        match edge {
            Edge::Left => {
                let row = i_left;
                i_left += 1;
                Vec2::new(-pin_x, top - row as f64 * PIN_PITCH)
            }
            Edge::Right => {
                let row = i_right;
                i_right += 1;
                Vec2::new(pin_x, top - row as f64 * PIN_PITCH)
            }
            Edge::Top => {
                let col = i_top;
                i_top += 1;
                let x0 = -(n_top as f64 - 1.0) * PIN_PITCH / 2.0;
                Vec2::new(x0 + col as f64 * PIN_PITCH, pin_y)
            }
            Edge::Bottom => {
                let col = i_bottom;
                i_bottom += 1;
                let x0 = -(n_bottom as f64 - 1.0) * PIN_PITCH / 2.0;
                Vec2::new(x0 + col as f64 * PIN_PITCH, -pin_y)
            }
        }
    };
    let pins: Vec<SchematicPin> = comp
        .pins
        .iter()
        .zip(&edges)
        .map(|(p, &edge)| {
            let mut np = p.clone();
            np.position = slot(edge);
            np
        })
        .collect();
    SymbolLayout {
        pins,
        angles: edges.iter().map(|e| e.angle()).collect(),
        body: (Vec2::new(-body_x, body_y), Vec2::new(body_x, -body_y)),
    }
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

    // Body — real artwork for recognized part classes, otherwise a generic
    // rectangle bounding the pins. Classify against the *laid-out* pins so a
    // component whose stored positions are degenerate (the nets flow) still
    // gets artwork from its synthesized layout.
    let SymbolLayout { pins, angles, body } = symbol_layout(comp);
    let art = symbol_artwork(comp, &pins);
    e.line(
        3,
        &format!(
            "(symbol {}",
            q(&format!("{}_0_1", sanitize_lib_name(&comp.reference)))
        ),
    );
    if let Some(a) = &art {
        write_sym_prims(e, &a.prims);
    } else {
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
    }
    e.line(3, ")");

    e.line(
        3,
        &format!(
            "(symbol {}",
            q(&format!("{}_1_1", sanitize_lib_name(&comp.reference)))
        ),
    );
    for (i, pin) in pins.iter().enumerate() {
        let (angle, length) = match &art {
            Some(a) => a.pins[i],
            // No class artwork: use the layout's own edge-derived angle, which
            // is correct for the top/bottom rail pins conventional placement
            // produces (pin_angle infers from geometry and can read a side pin
            // on a tall symbol as a top pin).
            None => (angles[i], 2.54),
        };
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
        e.line(5, &format!("(length {})", num(length)));
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

/// Emit one symbol instance and return its uuid (the board's cross-probe key).
fn write_sch_symbol(
    e: &mut Emitter,
    comp: &SchematicComponent,
    pos: Vec2,
    root_uuid: &str,
    project_name: &str,
) -> String {
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
    e.line(3, &format!("(project {}", q(project_name)));
    e.line(4, &format!("(path {}", q(&format!("/{}", root_uuid))));
    e.line(5, &format!("(reference {})", q(&comp.reference)));
    e.line(5, "(unit 1)");
    e.line(4, ")");
    e.line(3, ")");
    e.line(2, ")");
    e.line(1, ")");
    uuid
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

// ---------------------------------------------------------------------------
// Project bundle writer
// ---------------------------------------------------------------------------

/// Serialize a linked KiCad 9 project bundle: `<name>.kicad_pro`,
/// `<name>.kicad_sch`, and `<name>.kicad_pcb` as `(filename, contents)` pairs.
///
/// Unlike exporting the two files separately, the bundle is *linked*: each
/// board footprint carries a `(path "/<symbol-uuid>")` pointing at the
/// schematic symbol instance with the same reference designator, and the
/// project file records the schematic's root sheet uuid — so KiCad can
/// cross-probe (click a symbol → highlight its footprint, and vice versa).
/// Output is deterministic: the same inputs always produce identical bytes.
pub fn write_kicad_project(sheet: &SchematicSheet, pcb: &Pcb, name: &str) -> Vec<(String, String)> {
    let sch_filename = format!("{}.kicad_sch", name);
    let (sch, links) = write_kicad_sch_impl(sheet, name, &sch_filename);
    let board = write_kicad_pcb_impl(pcb, Some(&links));
    let pro = write_kicad_pro(name, &links.root_uuid, pcb);
    vec![
        (format!("{}.kicad_pro", name), pro),
        (sch_filename, sch),
        (format!("{}.kicad_pcb", name), board),
    ]
}

/// Minimal valid KiCad 9 project JSON (`meta.version` 3), recording the
/// schematic's root sheet uuid in `sheets` the way KiCad itself does.
///
/// The board's design constraints and net classes are written here too. KiCad's
/// GUI reads design settings from the project (the board's `(setup …)` /
/// `(net_class …)` blocks are the standalone-board path), so a bundle whose
/// project carried factory defaults would silently override the rules the board
/// was checked against. `min_copper_edge_clearance` has no board token at all
/// and only travels here.
fn write_kicad_pro(name: &str, root_uuid: &str, pcb: &Pcb) -> String {
    let r = &pcb.rules;
    let rules = serde_json::json!({
        "min_clearance": r.default_rules.clearance,
        "min_track_width": r.default_rules.trace_width,
        "min_via_diameter": min_via_diameter(pcb),
        "min_through_hole_diameter": min_via_drill(pcb),
        "min_via_annular_width": r.min_annular_ring,
        "min_hole_to_hole": r.hole_to_hole,
        "min_copper_edge_clearance": r.edge_clearance,
    });
    let by_id: BTreeMap<&str, &str> = pcb
        .nets
        .iter()
        .map(|n| (n.id.as_str(), n.name.as_str()))
        .collect();
    let mut classes = Vec::new();
    let mut patterns = Vec::new();
    for class in std::iter::once(&r.default_rules).chain(r.class_rules.iter()) {
        let mut c = serde_json::json!({
            "name": class.name,
            "clearance": class.clearance,
            "track_width": class.trace_width,
            "via_diameter": class.via_diameter,
            "via_drill": class.via_drill,
            "microvia_diameter": class.via_diameter,
            "microvia_drill": class.via_drill,
            "line_style": 0,
            "pcb_color": "rgba(0, 0, 0, 0.000)",
            "schematic_color": "rgba(0, 0, 0, 0.000)",
            "wire_width": 6,
            "bus_width": 12,
        });
        if let Some(w) = class.diff_pair_width {
            c["diff_pair_width"] = serde_json::json!(w);
        }
        if let Some(g) = class.diff_pair_gap {
            c["diff_pair_gap"] = serde_json::json!(g);
        }
        if std::ptr::eq(class, &r.default_rules) {
            c["priority"] = serde_json::json!(2147483647u32);
        } else {
            let mut names: Vec<&str> = r
                .net_class_assignments
                .get(&class.name)
                .into_iter()
                .flatten()
                .map(|n| *by_id.get(n.as_str()).unwrap_or(&n.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            names.sort_unstable();
            names.dedup();
            for n in names {
                patterns.push(serde_json::json!({ "netclass": class.name, "pattern": n }));
            }
        }
        classes.push(c);
    }
    write_kicad_pro_json(name, root_uuid, rules, classes, patterns)
}

fn write_kicad_pro_json(
    name: &str,
    root_uuid: &str,
    rules: serde_json::Value,
    classes: Vec<serde_json::Value>,
    netclass_patterns: Vec<serde_json::Value>,
) -> String {
    let pro = serde_json::json!({
        "board": {
            "3dviewports": [],
            "design_settings": {
                "defaults": {},
                "diff_pair_dimensions": [],
                "drc_exclusions": [],
                "meta": { "version": 2 },
                "rule_severities": {},
                "rules": rules,
                "track_widths": [],
                "via_dimensions": []
            },
            "layer_presets": [],
            "viewports": []
        },
        "boards": [],
        "cvpcb": { "equivalence_files": [] },
        "libraries": {
            "pinned_footprint_libs": [],
            "pinned_symbol_libs": []
        },
        "meta": {
            "filename": format!("{}.kicad_pro", name),
            "version": 3
        },
        "net_settings": {
            "classes": classes,
            "meta": { "version": 4 },
            "net_colors": null,
            "netclass_assignments": null,
            "netclass_patterns": netclass_patterns
        },
        "pcbnew": {
            "last_paths": {
                "gencad": "",
                "idf": "",
                "netlist": "",
                "plot": "",
                "pos_files": "",
                "specctra_dsn": "",
                "step": "",
                "svg": "",
                "vrml": ""
            },
            "page_layout_descr_file": ""
        },
        "schematic": {
            "legacy_lib_dir": "",
            "legacy_lib_list": []
        },
        "sheets": [[root_uuid, "Root"]],
        "text_variables": {}
    });
    let mut s = serde_json::to_string_pretty(&pro).expect("static project JSON serializes");
    s.push('\n');
    s
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
                    target_impedance: None,
                    target_diff_impedance: None,
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

    /// A board whose fab floors are *tighter* than KiCad's factory defaults —
    /// the CM5 shape: 0.12 mm drills, 0.21 mm vias, 0.045 mm annular ring,
    /// 0.08 mm clearance — plus a second net class, so class rules and their
    /// net membership are exercised too.
    fn fine_pitch_pcb() -> Pcb {
        use vcad_ir::ecad::NetClassRules;
        let mut pcb = sample_pcb();
        let r = &mut pcb.rules;
        r.default_rules.trace_width = 0.1;
        r.default_rules.clearance = 0.08;
        r.default_rules.via_diameter = 0.21;
        r.default_rules.via_drill = 0.12;
        r.min_drill = 0.12;
        r.min_annular_ring = 0.045;
        r.hole_to_hole = 0.2;
        r.edge_clearance = 0.2;
        r.class_rules = vec![NetClassRules {
            name: "Power".into(),
            trace_width: 0.4,
            clearance: 0.15,
            via_diameter: 0.6,
            via_drill: 0.3,
            diff_pair_gap: Some(0.13),
            diff_pair_width: Some(0.09),
            target_impedance: None,
            target_diff_impedance: None,
        }];
        r.net_class_assignments
            .insert("Power".into(), vec!["GND".into()]);
        for v in &mut pcb.vias {
            v.diameter = 0.21;
            v.drill = 0.12;
        }
        pcb
    }

    /// The rules a board was DRC'd against must travel *with* the board.
    ///
    /// Before this, `(setup …)` carried no design constraints and the file no
    /// net classes at all, so KiCad checked every exported board against its
    /// own factory defaults (0.1 mm annulus, 0.3 mm hole, 0.5 mm via, 0.2 mm
    /// netclass clearance) — and any calibration applied during fab-prep was
    /// invisible in the file. Re-importing shows the same, since our reader and
    /// KiCad's read the same tokens.
    #[test]
    fn design_rules_round_trip_through_kicad_board() {
        let pcb = fine_pitch_pcb();
        let text = write_kicad_pcb(&pcb);
        let back = parse_kicad_pcb(&text).expect("re-parse exported board");

        let (a, b) = (&pcb.rules, &back.rules);
        assert_eq!(b.default_rules.trace_width, a.default_rules.trace_width);
        assert_eq!(b.default_rules.clearance, a.default_rules.clearance);
        assert_eq!(b.default_rules.via_diameter, a.default_rules.via_diameter);
        assert_eq!(b.default_rules.via_drill, a.default_rules.via_drill);
        assert_eq!(b.min_drill, a.min_drill);
        assert_eq!(b.min_annular_ring, a.min_annular_ring);
        assert_eq!(b.hole_to_hole, a.hole_to_hole);

        // Class overrides and their net membership survive too.
        assert_eq!(b.class_rules.len(), 1);
        let p = &b.class_rules[0];
        assert_eq!(p.name, "Power");
        assert_eq!(p.trace_width, 0.4);
        assert_eq!(p.clearance, 0.15);
        assert_eq!(p.via_diameter, 0.6);
        assert_eq!(p.via_drill, 0.3);
        assert_eq!(p.diff_pair_gap, Some(0.13));
        assert_eq!(p.diff_pair_width, Some(0.09));
        assert_eq!(
            b.net_class_assignments.get("Power").map(|v| v.as_slice()),
            Some(&["GND".to_string()][..])
        );

        // The rule text itself is a fixpoint: re-exporting the re-imported
        // board emits the same (setup …) and (net_class …) blocks.
        let text2 = write_kicad_pcb(&back);
        let rules_text = |s: &str| {
            s.lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with('(')
                        && (t.contains("min")
                            || t.contains("clearance")
                            || t.contains("net_class")
                            || t.contains("trace_")
                            || t.contains("via_")
                            || t.contains("uvia_")
                            || t.contains("diff_pair")
                            || t.contains("add_net")
                            || t.contains("hole_to_hole"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(rules_text(&text), rules_text(&text2));
    }

    /// The project file in a bundle must carry the same constraints: KiCad's
    /// GUI reads design settings from the project, so a project holding factory
    /// defaults would override the board's own blocks.
    #[test]
    fn project_bundle_carries_board_design_rules() {
        let files = write_kicad_project(&sample_sheet(), &fine_pitch_pcb(), "brd");
        let pro = &files
            .iter()
            .find(|(n, _)| n.ends_with(".kicad_pro"))
            .expect("project file")
            .1;
        let j: serde_json::Value = serde_json::from_str(pro).expect("project is JSON");
        let rules = &j["board"]["design_settings"]["rules"];
        assert_eq!(rules["min_clearance"], 0.08);
        assert_eq!(rules["min_track_width"], 0.1);
        assert_eq!(rules["min_via_diameter"], 0.21);
        assert_eq!(rules["min_through_hole_diameter"], 0.12);
        assert_eq!(rules["min_via_annular_width"], 0.045);
        assert_eq!(rules["min_hole_to_hole"], 0.2);
        assert_eq!(rules["min_copper_edge_clearance"], 0.2);

        let classes = j["net_settings"]["classes"].as_array().expect("classes");
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0]["name"], "Default");
        assert_eq!(classes[1]["name"], "Power");
        assert_eq!(classes[1]["clearance"], 0.15);
        assert_eq!(
            j["net_settings"]["netclass_patterns"],
            serde_json::json!([{ "netclass": "Power", "pattern": "GND" }])
        );
    }

    /// KiCad's pad angle is absolute (it includes the footprint's rotation);
    /// vcad's IR keeps it relative, because every geometry consumer composes
    /// `fp.rotation + pad.rotation`. Writer and reader must be exact inverses,
    /// or a rotated footprint's non-square pads come out turned by the
    /// footprint's own angle — which on a 0.5mm-pitch QFN makes neighbouring
    /// pads OVERLAP (this was real: it produced hundreds of phantom DRC shorts
    /// on the CM5 fixture and a pin field the router could not escape).
    #[test]
    fn pad_rotation_round_trips_through_footprint_rotation() {
        let mut pcb = sample_pcb();
        pcb.footprints[0].rotation = 90.0;
        pcb.footprints[0].pads[0].rotation = 0.0;
        pcb.footprints[0].pads[1].rotation = 45.0;

        let text = write_kicad_pcb(&pcb);
        // On the wire the angles are absolute: 90 and 135.
        assert!(
            text.contains(" 90)"),
            "absolute pad angle 90 must be emitted"
        );
        assert!(
            text.contains(" 135)"),
            "absolute pad angle 45+90=135 must be emitted"
        );

        let back = crate::parse_kicad_pcb(&text).expect("re-parse");
        let fp = &back.footprints[0];
        assert_eq!(fp.rotation, 90.0);
        // ...and relative again in the IR.
        assert!((fp.pads[0].rotation - 0.0).abs() < 1e-6);
        assert!((fp.pads[1].rotation - 45.0).abs() < 1e-6);
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

        // Linked project bundle in its own subdirectory.
        let proj_dir = dir.join("project");
        std::fs::create_dir_all(&proj_dir).unwrap();
        for (name, contents) in write_kicad_project(&sheet, &sample_pcb(), "vcad_demo") {
            let path = proj_dir.join(&name);
            std::fs::write(&path, contents).unwrap();
            println!("wrote {}", path.display());
        }
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
    fn project_bundle_links_symbols_to_footprints() {
        let sheet = sample_sheet();
        let pcb = sample_pcb();
        let files = write_kicad_project(&sheet, &pcb, "demo");

        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["demo.kicad_pro", "demo.kicad_sch", "demo.kicad_pcb"]
        );
        let get = |n: &str| &files.iter().find(|(f, _)| f == n).unwrap().1;
        let pro = get("demo.kicad_pro");
        let sch = get("demo.kicad_sch");
        let board = get("demo.kicad_pcb");

        // The project file parses as JSON and records the sch root sheet uuid.
        let pro_json: serde_json::Value = serde_json::from_str(pro).expect("valid project JSON");
        assert_eq!(pro_json["meta"]["filename"], "demo.kicad_pro");
        let root_uuid = pro_json["sheets"][0][0].as_str().expect("root sheet uuid");
        assert!(sch.contains(&format!("(uuid \"{}\")", root_uuid)));

        // R1's schematic symbol uuid appears as R1's footprint path in the pcb.
        // The symbol instance uuid is the `(uuid …)` line inside the `(symbol`
        // block whose reference is R1.
        let sym_block = sch
            .split("\n\t(symbol\n")
            .find(|b| b.contains("(property \"Reference\" \"R1\""))
            .expect("R1 symbol instance in sch");
        let sym_uuid = sym_block
            .lines()
            .find_map(|l| l.trim().strip_prefix("(uuid \""))
            .and_then(|s| s.strip_suffix("\")"))
            .expect("R1 symbol uuid");
        assert!(
            board.contains(&format!("(path \"/{}\")", sym_uuid)),
            "footprint path should reference R1's symbol uuid {}",
            sym_uuid
        );
        assert!(board.contains("(sheetfile \"demo.kicad_sch\")"));

        // Instances carry the project name; sch symbols use it for cross-probe.
        assert!(sch.contains("(project \"demo\""));

        // The bundled schematic is still a valid, re-importable .kicad_sch.
        let reparsed = crate::kicad_sch::parse_kicad_sch(sch).expect("re-parse bundled sch");
        let refs: Vec<String> = reparsed
            .components
            .iter()
            .map(|c| c.reference.clone())
            .collect();
        assert!(refs.contains(&"R1".to_string()));

        // Deterministic: same inputs → identical bytes.
        assert_eq!(files, write_kicad_project(&sheet, &pcb, "demo"));
    }

    #[test]
    fn standalone_exports_unchanged_by_bundle_refactor() {
        // Standalone sch/pcb writers must not gain project linkage artifacts.
        let sch = write_kicad_sch(&sample_sheet());
        assert!(sch.contains("(project \"\""));
        let board = write_kicad_pcb(&sample_pcb());
        assert!(!board.contains("(sheetfile"));
        assert!(!board.contains("(path \"/"));
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

    /// A two-pin component with pins at (-d, 0) and (d, 0).
    fn two_pin_comp(reference: &str, value: &str, footprint: &str, d: f64) -> SchematicComponent {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin};
        SchematicComponent {
            reference: reference.into(),
            value: value.into(),
            footprint_id: footprint.into(),
            position: Vec2::new(100.0, 50.0),
            rotation: 0.0,
            mirror: false,
            pins: vec![
                SchematicPin {
                    number: "1".into(),
                    name: "~".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(-d, 0.0),
                },
                SchematicPin {
                    number: "2".into(),
                    name: "~".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(d, 0.0),
                },
            ],
            pads_override: None,
            properties: std::collections::HashMap::new(),
        }
    }

    fn sheet_of(components: Vec<SchematicComponent>) -> SchematicSheet {
        SchematicSheet {
            title: None,
            components,
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: None,
        }
    }

    #[test]
    fn resistor_gets_zigzag_not_rectangle() {
        let sheet = sheet_of(vec![two_pin_comp("R1", "10k", "Resistor_SMD:R_0805", 3.81)]);
        let text = write_kicad_sch(&sheet);
        assert!(text.contains("(polyline"), "resistor body is a polyline");
        assert!(!text.contains("(rectangle"), "no generic rectangle for R");
        // Pin connection points are untouched.
        assert!(text.contains("(at -3.81 0 0)"));
        assert!(text.contains("(at 3.81 0 180)"));
    }

    #[test]
    fn capacitor_gets_plates_polarized_gets_arc() {
        let plain = sheet_of(vec![two_pin_comp(
            "C1",
            "100nF",
            "Capacitor_SMD:C_0603",
            2.54,
        )]);
        let text = write_kicad_sch(&plain);
        assert!(text.contains("(polyline"));
        assert!(!text.contains("(rectangle"));
        assert!(!text.contains("(arc"), "plain cap has two straight plates");

        let polar = sheet_of(vec![two_pin_comp(
            "C2",
            "100uF",
            "Capacitor_THT:CP_Radial_D5.0mm",
            3.81,
        )]);
        let text = write_kicad_sch(&polar);
        assert!(text.contains("(arc"), "polarized cap has a curved plate");
    }

    #[test]
    fn diode_led_and_inductor_get_artwork() {
        let text = write_kicad_sch(&sheet_of(vec![
            two_pin_comp("D1", "1N4148", "Diode_SMD:D_SOD-323", 2.54),
            two_pin_comp("LED1", "Red", "LED_SMD:LED_0805", 2.54),
            two_pin_comp("L1", "10uH", "Inductor_SMD:L_0805", 3.81),
        ]));
        assert!(text.contains("(polyline"), "diode triangle present");
        assert!(text.contains("(arc"), "inductor humps present");
        assert!(!text.contains("(rectangle"), "no generic rectangles");
        // The LED symbol has more polylines than the plain diode (two arrows).
        let led = write_kicad_sch(&sheet_of(vec![two_pin_comp(
            "LED1",
            "Red",
            "LED_SMD:LED_0805",
            2.54,
        )]));
        let plain = write_kicad_sch(&sheet_of(vec![two_pin_comp(
            "D1",
            "1N4148",
            "Diode_SMD:D_SOD-323",
            2.54,
        )]));
        assert_eq!(
            led.matches("(polyline").count(),
            plain.matches("(polyline").count() + 2
        );
    }

    #[test]
    fn transistor_gets_envelope_circle() {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin};
        let q = SchematicComponent {
            reference: "Q1".into(),
            value: "2N2222".into(),
            footprint_id: "Package_TO_SOT_SMD:SOT-23".into(),
            position: Vec2::new(100.0, 50.0),
            rotation: 0.0,
            mirror: false,
            pins: vec![
                SchematicPin {
                    number: "1".into(),
                    name: "B".into(),
                    pin_type: PinType::Input,
                    position: Vec2::new(-2.54, 0.0),
                },
                SchematicPin {
                    number: "2".into(),
                    name: "C".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(2.54, 2.54),
                },
                SchematicPin {
                    number: "3".into(),
                    name: "E".into(),
                    pin_type: PinType::Passive,
                    position: Vec2::new(2.54, -2.54),
                },
            ],
            pads_override: None,
            properties: std::collections::HashMap::new(),
        };
        let text = write_kicad_sch(&sheet_of(vec![q]));
        assert!(text.contains("(circle"), "BJT envelope circle present");
        assert!(!text.contains("(rectangle"));
        // Connection points untouched.
        assert!(text.contains("(at -2.54 0 0)"));
        assert!(text.contains("(at 2.54 2.54 270)"));
        assert!(text.contains("(at 2.54 -2.54 90)"));
    }

    #[test]
    fn unmatched_parts_keep_rectangle_body() {
        // A U-prefixed IC and an R with asymmetric pins both fall back.
        let mut ic = two_pin_comp("U1", "LM358", "Package_SO:SOIC-8", 2.54);
        ic.pins[0].pin_type = PinType::Input;
        let mut odd_r = two_pin_comp("R9", "10k", "Resistor_SMD:R_0805", 2.54);
        odd_r.pins[1].position = Vec2::new(5.08, 2.54); // not symmetric
        let text = write_kicad_sch(&sheet_of(vec![ic, odd_r]));
        assert_eq!(text.matches("(rectangle").count(), 2);
        assert!(!text.contains("(polyline"));
    }

    #[test]
    fn vertical_two_pin_artwork_is_rotated_onto_axis() {
        let mut c = two_pin_comp("C1", "100nF", "Capacitor_SMD:C_0603", 2.54);
        c.pins[0].position = Vec2::new(0.0, 2.54);
        c.pins[1].position = Vec2::new(0.0, -2.54);
        let text = write_kicad_sch(&sheet_of(vec![c]));
        assert!(text.contains("(polyline"));
        assert!(!text.contains("(rectangle"));
        assert!(text.contains("(at 0 2.54 270)"));
        assert!(text.contains("(at 0 -2.54 90)"));
    }

    #[test]
    fn schematic_is_deterministic() {
        let sheet = sample_sheet();
        assert_eq!(write_kicad_sch(&sheet), write_kicad_sch(&sheet));
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

        // Nondegenerate body. R1/C1 are recognized part classes, so the body is
        // real artwork rather than the synthesized rectangle — but the invariant
        // this test exists for still holds: nothing collapses onto the origin.
        assert!(text.contains("(polyline"));
        assert!(!text.contains("(start 0 0)"));
        assert!(!text.contains("(xy 0 0)"));

        // Pins land on the synthesized positions, not stacked at the origin.
        assert!(text.contains("(at -7.62 0 0)"));
        assert!(text.contains("(at 7.62 0 180)"));

        // Deterministic.
        assert_eq!(text, write_kicad_sch(&sheet));
    }

    /// The synthesized-layout body rectangle, which the nets-flow test above
    /// used to assert before recognized part classes gained artwork. An
    /// unrecognized reference still gets the generic rect at the layout extent,
    /// so a regression in `symbol_layout` cannot hide behind the artwork path.
    #[test]
    fn degenerate_unrecognized_part_keeps_synthesized_rectangle() {
        use vcad_ir::ecad::{SchematicComponent, SchematicPin};
        let degenerate_pin = |number: &str| SchematicPin {
            number: number.into(),
            name: "~".into(),
            pin_type: PinType::Passive,
            position: Vec2::new(0.0, 0.0),
        };
        let sheet = sheet_of(vec![SchematicComponent {
            reference: "U1".into(),
            value: "NE555".into(),
            footprint_id: "Package_SO:SOIC-8".into(),
            position: Vec2::new(100.0, 50.0),
            rotation: 0.0,
            mirror: false,
            pins: vec![degenerate_pin("1"), degenerate_pin("2")],
            pads_override: None,
            properties: std::collections::HashMap::new(),
        }]);
        let text = write_kicad_sch(&sheet);
        // 2 pins → left/right at ±7.62, body x∈[-5.08, 5.08], y∈[-2.54, 2.54].
        assert!(text.contains("(start -5.08 2.54)"));
        assert!(text.contains("(end 5.08 -2.54)"));
        assert!(text.contains("(at -7.62 0 0)"));
        assert!(text.contains("(at 7.62 0 180)"));
    }

    /// Each pin of a degenerate-pin component must get its *own* stub anchor:
    /// the bug was every net of a component collapsing onto one point, which
    /// shorts them together in KiCad.
    #[test]
    fn degenerate_pins_get_distinct_stub_anchors() {
        let sheet = degenerate_pins_sheet();
        let placements = sheet_placements(&sheet);
        let pins = symbol_layout(&sheet.components[0]).pins;
        let a = pin_world(&sheet.components[0], placements[0], pins[0].position);
        let b = pin_world(&sheet.components[0], placements[0], pins[1].position);
        assert!(a != b, "R1 pin anchors collapsed to one point: {a:?}");
    }

    /// Minimal two-pin component for the schematic-polish tests.
    fn test_comp(reference: &str, pos: Vec2, rotation: f64) -> SchematicComponent {
        use vcad_ir::ecad::SchematicPin;
        SchematicComponent {
            reference: reference.into(),
            value: "10k".into(),
            footprint_id: "Resistor_SMD:R_0805".into(),
            position: pos,
            rotation,
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
        }
    }

    fn empty_sheet() -> SchematicSheet {
        SchematicSheet {
            title: None,
            components: vec![],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: None,
        }
    }

    #[test]
    fn title_block_present_when_titled_absent_otherwise() {
        let mut sheet = empty_sheet();
        let text = write_kicad_sch(&sheet);
        assert!(!text.contains("(title_block"));

        sheet.title = Some("Amp Board".into());
        let text = write_kicad_sch(&sheet);
        assert!(text.contains("(title_block"));
        assert!(text.contains("(title \"Amp Board\")"));
        assert!(text.contains("(rev \"vcad\")"));
        assert!(text.contains("(company \"generated by vcad\")"));
        // No wall-clock date — output must be byte-stable.
        assert!(!text.contains("(date"));
        assert_eq!(text, write_kicad_sch(&sheet));
    }

    #[test]
    fn three_way_tee_gets_exactly_one_junction() {
        use vcad_ir::ecad::SchematicWire;
        let mut sheet = empty_sheet();
        sheet.wires = vec![
            SchematicWire {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(12.7, 0.0),
            },
            SchematicWire {
                start: Vec2::new(12.7, 0.0),
                end: Vec2::new(25.4, 0.0),
            },
            SchematicWire {
                start: Vec2::new(12.7, 0.0),
                end: Vec2::new(12.7, 12.7),
            },
        ];
        let text = write_kicad_sch(&sheet);
        assert_eq!(text.matches("(junction").count(), 1);
        assert!(text.contains("(at 12.7 0)"));

        // Dedupe: a caller-declared junction at the tee is not emitted twice.
        sheet.junctions = vec![vcad_ir::ecad::SchematicJunction {
            position: Vec2::new(12.7, 0.0),
        }];
        let text = write_kicad_sch(&sheet);
        assert_eq!(text.matches("(junction").count(), 1);
    }

    #[test]
    fn unconnected_pin_gets_no_connect_at_placed_position() {
        let mut sheet = empty_sheet();
        sheet.components = vec![test_comp("R1", Vec2::new(100.0, 50.0), 0.0)];
        let mut nets = std::collections::BTreeMap::new();
        nets.insert("VCC".to_string(), vec!["R1.1".to_string()]);
        sheet.nets = Some(nets);
        let text = write_kicad_sch(&sheet);

        // Pin 1 is netted (stub + label); pin 2 is open → exactly one
        // no_connect, at pin 2's placed position.
        assert_eq!(text.matches("(no_connect").count(), 1);
        let placements = sheet_placements(&sheet);
        let pins = symbol_layout(&sheet.components[0]).pins;
        let p2 = pin_world(&sheet.components[0], placements[0], pins[1].position);
        assert!(text.contains(&format!(
            "(no_connect\n\t\t(at {} {})",
            num(p2.x),
            num(p2.y)
        )));

        // A fully netted sheet emits none.
        let mut nets = std::collections::BTreeMap::new();
        nets.insert(
            "VCC".to_string(),
            vec!["R1.1".to_string(), "R1.2".to_string()],
        );
        sheet.nets = Some(nets);
        let text = write_kicad_sch(&sheet);
        assert_eq!(text.matches("(no_connect").count(), 0);
    }

    #[test]
    fn rotated_stub_endpoints_snap_to_wire_grid() {
        // 45° rotation: the rotated pin offsets would land on irrational
        // coordinates without the grid snap.
        let mut sheet = empty_sheet();
        sheet.components = vec![test_comp("R1", Vec2::new(127.0, 63.5), 45.0)];
        let mut nets = std::collections::BTreeMap::new();
        nets.insert(
            "VCC".to_string(),
            vec!["R1.1".to_string(), "R1.2".to_string()],
        );
        sheet.nets = Some(nets);
        let text = write_kicad_sch(&sheet);

        // Every stub endpoint is an exact 1.27mm multiple. Read the wire
        // nodes rather than every `(xy …)` line in the file: symbol artwork
        // carries its own `(xy …)` points in lib_symbols, and those are
        // body-relative shapes, not connection points on the sheet grid.
        let (_, root) = crate::sexpr::parse_sexpr(&text).expect("parse exported sheet");
        let mut checked = 0;
        for wire in root.find_all("wire") {
            let pts = wire.find("pts").expect("wire has (pts …)");
            for xy in pts.find_all("xy") {
                let c = xy.children().expect("(xy x y)");
                for v in [c[1].as_f64().expect("xy x"), c[2].as_f64().expect("xy y")] {
                    let steps = v / 1.27;
                    assert!(
                        (steps - steps.round()).abs() < 1e-9,
                        "off-grid stub coordinate {v}"
                    );
                    checked += 1;
                }
            }
        }
        // 2 stubs × 2 endpoints × 2 coords.
        assert_eq!(checked, 8);
        assert_eq!(text, write_kicad_sch(&sheet));
    }

    /// 90°-multiple rotations must be untouched by the grid snap — their
    /// offsets are already grid multiples, so the common case stays exact.
    #[test]
    fn right_angle_rotations_are_unaffected_by_snap() {
        for rot in [0.0, 90.0, 180.0, 270.0] {
            let comp = test_comp("R1", Vec2::new(100.0, 50.0), rot);
            let pos = Vec2::new(100.0, 50.0);
            for pin in &comp.pins {
                let got = pin_world(&comp, pos, pin.position);
                let th: f64 = rot.to_radians();
                let (s, c) = (th.sin(), th.cos());
                let (px, py) = (pin.position.x, pin.position.y);
                let want = Vec2::new(pos.x + px * c - py * s, pos.y - (px * s + py * c));
                assert!(
                    (got.x - want.x).abs() < 1e-9 && (got.y - want.y).abs() < 1e-9,
                    "rot {rot}: snap moved an on-grid point {want:?} → {got:?}"
                );
            }
        }
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
        export_sheet(subdir, &degenerate_pins_sheet())
    }

    /// Write `sheet` to a scratch `.kicad_sch` under `subdir`.
    fn export_sheet(subdir: &str, sheet: &SchematicSheet) -> std::path::PathBuf {
        let dir = std::env::var("VCAD_KICAD_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("vcad_kicad"))
            .join(subdir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nets_flow.kicad_sch");
        std::fs::write(&path, write_kicad_sch(sheet)).unwrap();
        path
    }

    /// Run `kicad-cli sch export netlist` on `sch` and return the net -> pin
    /// refs map KiCad itself derives.
    fn kicad_derived_nets(
        cli: &std::path::Path,
        sch: &std::path::Path,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        use std::collections::{BTreeMap, BTreeSet};
        let netfile = sch.with_file_name("derived.net");
        let out = std::process::Command::new(cli)
            .args([
                "sch",
                "export",
                "netlist",
                "--format",
                "kicadsexpr",
                "--output",
            ])
            .arg(&netfile)
            .arg(sch)
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
        extracted
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

    /// Mixed-pin IC: degenerate stored positions, but real pin metadata.
    fn sample_ic() -> SchematicComponent {
        let pin = |number: &str, name: &str, pin_type: PinType| SchematicPin {
            number: number.into(),
            name: name.into(),
            pin_type,
            position: Vec2::new(0.0, 0.0),
        };
        SchematicComponent {
            reference: "U1".into(),
            value: "OPAMP".into(),
            footprint_id: "Package_SO:SOIC-8".into(),
            position: Vec2::new(100.33, 100.33),
            rotation: 0.0,
            mirror: false,
            pins: vec![
                pin("1", "VCC", PinType::PowerInput),
                pin("2", "GND", PinType::PowerInput),
                pin("3", "IN_A", PinType::Input),
                pin("4", "IN_B", PinType::Input),
                pin("5", "OUT", PinType::Output),
            ],
            pads_override: None,
            properties: std::collections::HashMap::new(),
        }
    }

    /// Real pin metadata drives conventional edge assignment, with per-edge
    /// angles and a nondegenerate body.
    #[test]
    fn metadata_pins_get_conventional_edges() {
        let comp = sample_ic();
        let layout = symbol_layout(&comp);
        let (tl, br) = layout.body;
        assert!(br.x > tl.x + 1.0, "body must have width");
        assert!(tl.y > br.y + 1.0, "body must have height");

        let find = |number: &str| {
            let i = layout.pins.iter().position(|p| p.number == number).unwrap();
            (layout.pins[i].position, layout.angles[i])
        };
        // VCC on the top edge, pointing down into the body.
        let (vcc, a_vcc) = find("1");
        assert!(vcc.y > tl.y, "VCC should sit above the body");
        assert_eq!(a_vcc, 270.0);
        // GND on the bottom edge, pointing up.
        let (gnd, a_gnd) = find("2");
        assert!(gnd.y < br.y, "GND should sit below the body");
        assert_eq!(a_gnd, 90.0);
        // Inputs on the left.
        for n in ["3", "4"] {
            let (p, a) = find(n);
            assert!(p.x < tl.x, "input {n} should sit left of the body");
            assert_eq!(a, 0.0);
        }
        // Output on the right.
        let (out, a_out) = find("5");
        assert!(out.x > br.x, "output should sit right of the body");
        assert_eq!(a_out, 180.0);

        // Every synthesized coordinate stays on the schematic grid.
        for p in &layout.pins {
            for v in [p.position.x, p.position.y] {
                assert!((snap_grid(v) - v).abs() < 1e-9, "off-grid coordinate {v}");
            }
        }
        // Distinct anchors: no two pins share a point.
        for i in 0..layout.pins.len() {
            for j in (i + 1)..layout.pins.len() {
                assert!(
                    layout.pins[i].position != layout.pins[j].position,
                    "pins {i} and {j} collapsed onto one point"
                );
            }
        }
    }

    /// Anonymous pins ("~"/empty names) keep the index-based left/right split,
    /// geometry unchanged from before conventional placement existed.
    #[test]
    fn anonymous_pins_keep_index_split() {
        let mut comp = sample_ic();
        for p in &mut comp.pins {
            p.name = "~".into();
        }
        let layout = symbol_layout(&comp);
        // 5 anonymous pins -> 3 left, 2 right; nothing on the top/bottom rails.
        let left = layout.pins.iter().filter(|p| p.position.x < 0.0).count();
        let right = layout.pins.iter().filter(|p| p.position.x > 0.0).count();
        assert_eq!((left, right), (3, 2));
        assert!(layout.angles.iter().all(|a| *a == 0.0 || *a == 180.0));
        // The historical +/-7.62 pin column.
        assert!(layout.pins.iter().all(|p| p.position.x.abs() == 7.62));
    }

    /// A degenerate two-pin passive stays symmetric through the origin under
    /// every branch, including when its pins carry rail names — that symmetry
    /// is the gate the class artwork matches on.
    #[test]
    fn two_pin_parts_stay_symmetric_for_artwork() {
        let mut comp = sample_ic();
        comp.reference = "C1".into();
        comp.value = "100nF".into();
        comp.footprint_id = "Capacitor_SMD:C_0603".into();
        comp.pins.truncate(2);
        comp.pins[0].name = "VCC".into();
        comp.pins[1].name = "GND".into();
        comp.pins[0].pin_type = PinType::Passive;
        comp.pins[1].pin_type = PinType::Passive;

        let layout = symbol_layout(&comp);
        let (a, b) = (layout.pins[0].position, layout.pins[1].position);
        assert!(
            (a.x + b.x).abs() < 1e-9 && (a.y + b.y).abs() < 1e-9,
            "rail-named two-pin part lost origin symmetry: {a:?} {b:?}"
        );
    }

    #[test]
    fn metadata_layout_is_deterministic() {
        let sheet = SchematicSheet {
            title: None,
            components: vec![sample_ic()],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: None,
        };
        assert_eq!(write_kicad_sch(&sheet), write_kicad_sch(&sheet));
    }

    /// A sheet whose IC carries real pin metadata, so the layout puts pins on
    /// all four edges — including the top/bottom rails that only conventional
    /// placement produces.
    fn metadata_ic_sheet() -> SchematicSheet {
        let mut nets = std::collections::BTreeMap::new();
        nets.insert("VCC".to_string(), vec!["U1.1".to_string()]);
        nets.insert("GND".to_string(), vec!["U1.2".to_string()]);
        nets.insert("SIG_A".to_string(), vec!["U1.3".to_string()]);
        nets.insert("SIG_B".to_string(), vec!["U1.4".to_string()]);
        nets.insert("OUT".to_string(), vec!["U1.5".to_string()]);
        SchematicSheet {
            title: Some("metadata ic".into()),
            components: vec![sample_ic()],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: Some(nets),
        }
    }

    /// A top-edge power pin's net stub must run upward out of the symbol and
    /// carry its global label at the far end.
    #[test]
    fn top_edge_power_pin_stub_goes_up() {
        let sheet = metadata_ic_sheet();
        let comp = &sheet.components[0];
        let layout = symbol_layout(comp);
        let i = layout.pins.iter().position(|p| p.number == "1").unwrap();
        assert_eq!(layout.angles[i], 270.0, "VCC must be a top-edge pin");

        let placements = sheet_placements(&sheet);
        let start = pin_world(comp, placements[0], layout.pins[i].position);
        // The stub runs outward from a top-edge pin: +y in the symbol's Y-up
        // frame, which is a smaller y once mapped into the Y-down sheet.
        let end = pin_world(
            comp,
            placements[0],
            Vec2::new(
                layout.pins[i].position.x,
                layout.pins[i].position.y + PIN_PITCH,
            ),
        );
        assert!(end.y < start.y, "top-edge stub should run up the sheet");

        let text = write_kicad_sch(&sheet);
        assert!(
            text.contains(&format!("(xy {} {})", num(end.x), num(end.y))),
            "stub should extend up from the pin"
        );
        assert!(text.contains("(global_label \"VCC\""));
    }

    /// The convention check that matters for conventional placement: KiCad
    /// itself must derive the declared netlist from a symbol with pins on all
    /// four edges. Top- and bottom-rail pins are new here — a wrong angle or a
    /// stub drawn in the wrong direction would leave the label off the pin and
    /// drop that net, which this catches and a self-consistency test cannot.
    #[test]
    #[ignore = "requires kicad-cli (KiCad 9) — run with --ignored"]
    fn kicad_netlist_matches_for_four_edge_symbol() {
        use std::collections::{BTreeMap, BTreeSet};

        let Some(cli) = kicad_cli() else {
            eprintln!("SKIP: kicad-cli not found (PATH, $KICAD_CLI, or KiCad.app)");
            return;
        };
        let sheet = metadata_ic_sheet();

        // Guard the premise: this sheet must actually exercise all four edges.
        let angles = symbol_layout(&sheet.components[0]).angles;
        for a in [0.0, 90.0, 180.0, 270.0] {
            assert!(angles.contains(&a), "sheet does not exercise angle {a}");
        }

        let sch = export_sheet("netlist_ic", &sheet);
        let extracted = kicad_derived_nets(&cli, &sch);
        let declared: BTreeMap<String, BTreeSet<String>> = sheet
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

    /// A four-edge symbol must keep every connection point on KiCad's 1.27 mm
    /// connection grid — the geometry claim conventional placement makes.
    ///
    /// This deliberately does *not* assert zero ERC errors. A sheet holding a
    /// single IC always raises `power_pin_not_driven` and `pin_not_driven`:
    /// nothing on it drives the power or signal inputs, which is a statement
    /// about the fixture, not about pin placement. (The passives-only sheet in
    /// `kicad_erc_reports_zero_errors` avoids them because passive pins are
    /// exempt from the driven-ness rules.) The connectivity claim is covered by
    /// `kicad_netlist_matches_for_four_edge_symbol`.
    #[test]
    #[ignore = "requires kicad-cli (KiCad 9) — run with --ignored"]
    fn kicad_four_edge_symbol_stays_on_grid() {
        let Some(cli) = kicad_cli() else {
            eprintln!("SKIP: kicad-cli not found (PATH, $KICAD_CLI, or KiCad.app)");
            return;
        };
        let sch = export_sheet("erc_ic", &metadata_ic_sheet());
        let report = sch.with_file_name("erc.json");
        let out = std::process::Command::new(&cli)
            .args(["sch", "erc", "--format", "json", "--output"])
            .arg(&report)
            .arg(&sch)
            .output()
            .expect("run kicad-cli sch erc");
        let text = std::fs::read_to_string(&report).unwrap_or_default();
        assert!(
            !text.is_empty(),
            "no ERC report produced: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !text.contains("endpoint_off_grid"),
            "four-edge symbol put a connection point off the KiCad grid:\n{text}"
        );
    }

    /// Run `kicad-cli pcb drc` on `text` and return the violation type counts
    /// KiCad itself reports.
    fn kicad_drc_types(
        cli: &std::path::Path,
        subdir: &str,
        text: &str,
    ) -> std::collections::BTreeMap<String, usize> {
        let dir = std::env::var("VCAD_KICAD_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("vcad_kicad"))
            .join(subdir);
        std::fs::create_dir_all(&dir).unwrap();
        let board = dir.join("board.kicad_pcb");
        std::fs::write(&board, text).unwrap();
        let report = dir.join("drc.json");
        let _ = std::fs::remove_file(&report);
        let out = std::process::Command::new(cli)
            .args(["pcb", "drc", "--format", "json", "--severity-all", "-o"])
            .arg(&report)
            .arg(&board)
            .output()
            .expect("run kicad-cli pcb drc");
        let json = std::fs::read_to_string(&report).unwrap_or_else(|_| {
            panic!(
                "no DRC report produced (KiCad refused the board?): {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let v: serde_json::Value = serde_json::from_str(&json).expect("DRC report is JSON");
        let mut counts = std::collections::BTreeMap::new();
        for viol in v["violations"].as_array().into_iter().flatten() {
            let t = viol["type"].as_str().unwrap_or("?").to_string();
            *counts.entry(t).or_insert(0) += 1;
        }
        counts
    }

    /// The claim that matters: KiCad must check an exported board against *the
    /// board's own* rules, not its factory defaults.
    ///
    /// `fine_pitch_pcb` is legal under its declared rules (0.12 mm drill,
    /// 0.21 mm via, 0.045 mm annulus, 0.08 mm clearance) and illegal under
    /// KiCad's defaults (0.3 / 0.5 / 0.1 / 0.2). So this is a differential test:
    /// as exported, KiCad reports none of those four violation types; with the
    /// emitted constraints and net classes stripped out — exactly what the file
    /// looked like before — the same board and the same KiCad report them.
    #[test]
    #[ignore = "requires kicad-cli (KiCad 9) — run with --ignored"]
    fn kicad_checks_board_against_its_own_rules() {
        let Some(cli) = kicad_cli() else {
            eprintln!("SKIP: kicad-cli not found (PATH, $KICAD_CLI, or KiCad.app)");
            return;
        };
        let rule_types = [
            "annular_width",
            "drill_out_of_range",
            "via_diameter",
            "clearance",
        ];
        let text = write_kicad_pcb(&fine_pitch_pcb());

        let with_rules = kicad_drc_types(&cli, "drc_rules", &text);
        for t in rule_types {
            assert_eq!(
                with_rules.get(t).copied().unwrap_or(0),
                0,
                "KiCad flagged {t} on a board legal under its own rules: {with_rules:?}"
            );
        }

        // Negative control: strip the rules we emit and the violations return.
        let stripped: String = {
            let mut out = String::new();
            let mut skip_class = false;
            for line in text.lines() {
                let t = line.trim();
                if skip_class {
                    if t == ")" {
                        skip_class = false;
                    }
                    continue;
                }
                if t.starts_with("(net_class ") {
                    skip_class = true;
                    continue;
                }
                if t.starts_with("(via_min_") || t.starts_with("(hole_to_hole_min") {
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        };
        let without = kicad_drc_types(&cli, "drc_norules", &stripped);
        assert!(
            rule_types
                .iter()
                .any(|t| without.get(*t).copied().unwrap_or(0) > 0),
            "premise broken: the fixture is legal under KiCad defaults too, so \
             this test proves nothing: {without:?}"
        );
    }
}
