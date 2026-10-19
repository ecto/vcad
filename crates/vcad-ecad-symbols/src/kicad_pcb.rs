//! Parser for KiCad `.kicad_pcb` board files.
//!
//! Converts a `.kicad_pcb` S-expression file into a [`vcad_ir::ecad::Pcb`]
//! struct.  Reuses the `sexpr` S-expression parser.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use vcad_ir::ecad::{
    BoardOutline, DesignRules, DrillSpec, Footprint, FootprintGraphic, LayerStackup, Net,
    NetClassRules, Pad, PadShape, PadType, Pcb, PcbLayer, StackupLayer, ThermalReliefStyle, Trace,
    TraceArc, Via, Zone, ZoneFillType,
};
use vcad_ir::Vec2;

use crate::sexpr::{parse_sexpr, SExpr};
use crate::ParseError;

/// Parse a `.kicad_pcb` file content into a [`Pcb`].
///
/// Anything the importer could not represent faithfully (a layer with no IR
/// equivalent, an unknown pad type/shape, a via without parseable layers) is
/// reported on stderr. Use [`parse_kicad_pcb_with_warnings`] to get those
/// warnings as data instead.
pub fn parse_kicad_pcb(input: &str) -> Result<Pcb, ParseError> {
    let (pcb, warnings) = parse_kicad_pcb_with_warnings(input)?;
    for w in &warnings {
        eprintln!("kicad_pcb import warning: {w}");
    }
    Ok(pcb)
}

/// Parse a `.kicad_pcb` file, returning the board plus every import warning
/// (dropped/approximated items). Never silently drops copper: each item that
/// did not make it into the IR unchanged is accounted for here.
pub fn parse_kicad_pcb_with_warnings(input: &str) -> Result<(Pcb, Vec<String>), ParseError> {
    let (rest, root) = parse_sexpr(input)?;
    if !rest.trim().is_empty() {
        return Err(ParseError::TrailingInput);
    }
    if root.tag_name() != Some("kicad_pcb") {
        return Err(ParseError::Nom("expected (kicad_pcb ...)".into()));
    }
    convert_pcb(&root)
}

/// Import context: the board's real copper stack (for wildcard expansion) and
/// the warning sink.
struct Ctx<'a> {
    net_map: &'a HashMap<u32, String>,
    /// The board's copper layers, top → bottom, from its layer table.
    copper: Vec<PcbLayer>,
    /// Layer token → number of items dropped because it has no IR layer.
    dropped: RefCell<BTreeMap<String, usize>>,
    warnings: RefCell<Vec<String>>,
}

impl Ctx<'_> {
    fn warn(&self, msg: String) {
        self.warnings.borrow_mut().push(msg);
    }

    /// Resolve one layer token, expanding KiCad wildcards against the board's
    /// stack: `*.Cu` → every copper layer the board has, `F&B.Cu` → outer
    /// copper, `*.Mask`/`F&B.Mask` (and Paste/SilkS/…) → front + back.
    /// Unknown tokens resolve to nothing and are counted for a warning.
    fn resolve(&self, tok: &str) -> Vec<PcbLayer> {
        if let Some(l) = parse_layer(tok) {
            return vec![l];
        }
        let wild = tok
            .strip_prefix("*.")
            .map(|s| (s, true))
            .or_else(|| tok.strip_prefix("F&B.").map(|s| (s, false)));
        if let Some((suffix, all)) = wild {
            if suffix == "Cu" {
                return if all {
                    self.copper.clone()
                } else {
                    vec![PcbLayer::FCu, PcbLayer::BCu]
                };
            }
            let pair: Vec<PcbLayer> = [format!("F.{suffix}"), format!("B.{suffix}")]
                .iter()
                .filter_map(|n| parse_layer(n))
                .collect();
            if !pair.is_empty() {
                return pair;
            }
        }
        *self
            .dropped
            .borrow_mut()
            .entry(tok.to_string())
            .or_default() += 1;
        Vec::new()
    }

    /// Read a `(layer "F.Cu")` child. A missing child is `None` silently (not
    /// every node has one); a present but unrepresentable one is counted.
    fn layer(&self, node: &SExpr<'_>) -> Option<PcbLayer> {
        let tok = node
            .find("layer")
            .and_then(|n| n.children())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str())?;
        let layers = self.resolve(tok);
        if layers.len() > 1 {
            self.warn(format!(
                "single-layer item on wildcard layer {tok:?}; kept on {:?} only",
                layers[0]
            ));
        }
        layers.first().copied()
    }

    /// Read a `(layers ...)` child with wildcards expanded, de-duplicated,
    /// in file order.
    fn layers(&self, node: &SExpr<'_>) -> Vec<PcbLayer> {
        let mut out: Vec<PcbLayer> = Vec::new();
        if let Some(c) = node.find("layers").and_then(|n| n.children()) {
            for tok in c.iter().skip(1).filter_map(|v| v.as_str()) {
                for l in self.resolve(tok) {
                    if !out.contains(&l) {
                        out.push(l);
                    }
                }
            }
        }
        out
    }

    fn finish(self) -> Vec<String> {
        let mut w = self.warnings.into_inner();
        for (tok, n) in self.dropped.into_inner() {
            w.push(format!(
                "layer {tok:?} has no vcad equivalent: {n} item(s) dropped from it"
            ));
        }
        w
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a child float value: `(tag value)` → `value`.
fn child_f64(node: &SExpr<'_>, tag: &str) -> Option<f64> {
    node.find(tag)
        .and_then(|n| n.children())
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_f64())
}

/// Read an `(at X Y [angle])` position.
fn read_at(node: &SExpr<'_>) -> (Vec2, f64) {
    match node.find("at").and_then(|n| n.children()) {
        Some(c) if c.len() >= 3 => {
            let x = c[1].as_f64().unwrap_or(0.0);
            let y = c[2].as_f64().unwrap_or(0.0);
            let angle = c.get(3).and_then(|v| v.as_f64()).unwrap_or(0.0);
            (Vec2::new(x, y), angle)
        }
        _ => (Vec2::new(0.0, 0.0), 0.0),
    }
}

/// Read `(xy X Y)` from a child.
fn read_xy(node: &SExpr<'_>, tag: &str) -> Vec2 {
    match node.find(tag).and_then(|c| c.children()) {
        Some(c) if c.len() >= 3 => {
            Vec2::new(c[1].as_f64().unwrap_or(0.0), c[2].as_f64().unwrap_or(0.0))
        }
        _ => Vec2::new(0.0, 0.0),
    }
}

/// Read `(start X Y)`.
fn read_start(node: &SExpr<'_>) -> Vec2 {
    read_xy(node, "start")
}

/// Read `(end X Y)`.
fn read_end(node: &SExpr<'_>) -> Vec2 {
    read_xy(node, "end")
}

/// Parse a KiCad layer name string into a [`PcbLayer`].
fn parse_layer(s: &str) -> Option<PcbLayer> {
    if let Some(n) = s.strip_prefix("In").and_then(|r| r.strip_suffix(".Cu")) {
        return n.parse::<usize>().ok().and_then(PcbLayer::inner);
    }
    match s {
        "F.Cu" => Some(PcbLayer::FCu),
        "B.Cu" => Some(PcbLayer::BCu),
        "F.SilkS" => Some(PcbLayer::FSilkS),
        "B.SilkS" => Some(PcbLayer::BSilkS),
        "F.Mask" => Some(PcbLayer::FMask),
        "B.Mask" => Some(PcbLayer::BMask),
        "F.Paste" => Some(PcbLayer::FPaste),
        "B.Paste" => Some(PcbLayer::BPaste),
        "F.CrtYd" => Some(PcbLayer::FCrtYd),
        "B.CrtYd" => Some(PcbLayer::BCrtYd),
        "F.Fab" => Some(PcbLayer::FFab),
        "B.Fab" => Some(PcbLayer::BFab),
        "Edge.Cuts" => Some(PcbLayer::EdgeCuts),
        "Dwgs.User" => Some(PcbLayer::UserDrawings),
        "Cmts.User" => Some(PcbLayer::UserComments),
        _ => None,
    }
}

/// Read `(width W)` child.
fn read_width(node: &SExpr<'_>) -> f64 {
    child_f64(node, "width").unwrap_or(0.15)
}

/// Read `(net N)` — get net name from index using net map.
fn read_net_str(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> String {
    let idx = node
        .find("net")
        .and_then(|n| n.children())
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    net_map.get(&idx).cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Main converter
// ---------------------------------------------------------------------------

fn convert_pcb(root: &SExpr<'_>) -> Result<(Pcb, Vec<String>), ParseError> {
    // 1. Parse nets
    let mut net_map: HashMap<u32, String> = HashMap::new();
    let mut nets = Vec::new();
    for net_node in root.find_all("net") {
        if let Some(c) = net_node.children() {
            if c.len() >= 3 {
                let idx = c[1]
                    .as_str()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let name = c[2].as_str().unwrap_or("").to_string();
                if idx > 0 {
                    net_map.insert(idx, name.clone());
                    nets.push(Net {
                        id: idx.to_string(),
                        name,
                    });
                }
            }
        }
    }

    // 2. Parse general section for thickness
    let thickness = root
        .find("general")
        .and_then(|g| child_f64(g, "thickness"))
        .unwrap_or(1.6);

    // 3. Parse board outline from Edge.Cuts lines
    let outline = parse_board_outline(root, thickness);

    // 4. Parse layers for stackup
    let mut early_warnings = Vec::new();
    let stackup = parse_stackup(root, &mut early_warnings);
    let ctx = Ctx {
        net_map: &net_map,
        copper: stackup.layers.iter().map(|l| l.layer).collect(),
        dropped: RefCell::new(BTreeMap::new()),
        warnings: RefCell::new(early_warnings),
    };

    // 5. Parse design rules
    let rules = parse_design_rules(root);
    let has_micro = root
        .find_all("via")
        .iter()
        .any(|v| v.children().and_then(|c| c.get(1)).and_then(|x| x.as_str()) == Some("micro"));
    for (key, v) in [
        ("uvia_min_drill", rules.min_microvia_drill),
        ("uvia_min_size", rules.min_microvia_diameter),
    ] {
        if has_micro && root.find("setup").and_then(|s| child_f64(s, key)).is_none() {
            ctx.warn(format!(
                "board setup has no ({key}); using KiCad's default {v} mm for microvias"
            ));
        }
    }

    // 6. Parse footprints. Legacy boards (KiCad 4/5, e.g. the VESC 4.12)
    // call them `module`; the inner shape (pads, at, layer) is compatible.
    let footprints: Vec<Footprint> = root
        .find_all("footprint")
        .iter()
        .chain(root.find_all("module").iter())
        .filter_map(|n| parse_footprint(n, &ctx))
        .collect();

    // 7. Parse traces (segment)
    let traces: Vec<Trace> = root
        .find_all("segment")
        .iter()
        .filter_map(|n| parse_trace(n, &ctx))
        .collect();

    // 7b. Parse arc tracks (KiCad 6+ `(arc (start) (mid) (end) ...)`)
    let trace_arcs: Vec<TraceArc> = root
        .find_all("arc")
        .iter()
        .filter_map(|n| parse_trace_arc(n, &ctx))
        .collect();

    // 8. Parse vias
    let vias: Vec<Via> = root
        .find_all("via")
        .iter()
        .filter_map(|n| parse_via(n, &ctx))
        .collect();

    // 9. Parse zones
    let zones: Vec<Zone> = root
        .find_all("zone")
        .iter()
        .flat_map(|n| parse_zone(n, &ctx))
        .collect();

    // The .kicad_pcb setup block rarely carries usable rules (net classes
    // live in the .kicad_pro project file), so a routed board would otherwise
    // import with the fat 0.25/0.2/0.8 defaults — hopeless on an HDI design
    // whose own copper is 0.1 mm tracks and 0.2 mm vias. The board's existing
    // copper is the best available evidence of its fab capability: calibrate
    // the default rules against it.
    let mut rules = calibrate_rules_from_copper(rules, &traces, &vias);
    infer_via_floors(root, &mut rules, &vias, &ctx);
    infer_neck(&mut rules, &traces, &ctx);

    let pcb = Pcb {
        outline,
        stackup,
        nets,
        rules,
        footprints,
        traces,
        trace_arcs,
        vias,
        zones,
        keepouts: vec![],
        net_ties: vec![],
    };
    // KiCad's layer type is "signal" by default and carries no plane intent
    // on boards like the CM5, so roles are inferred from the copper itself.
    let mut pcb = pcb;
    for line in pcb.infer_layer_roles() {
        ctx.warn(line);
    }
    // Stacked microvias: a .kicad_pcb carries no fab stacking capability, so
    // a board that already stacks them is the evidence it is allowed.
    let stacks = pcb.stacked_microvia_pairs().len();
    if stacks > 0 && !pcb.rules.allow_stacked_microvias {
        ctx.warn(format!(
            "INFERRED fab capability: allowStackedMicrovias = true (the board stacks {stacks} \
             microvia pair(s) of its own; fallback was false)"
        ));
        pcb.rules.allow_stacked_microvias = true;
        pcb.rules
            .inferred_floors
            .push("allowStackedMicrovias".into());
    }
    Ok((pcb, ctx.finish()))
}

/// Via floors when the file authors none: the board's own non-micro vias are
/// the only evidence of its fab capability, so the floor is the smallest drill
/// / annular ring among them (through and blind alike — KiCad keeps one via
/// floor for both). Logged loudly and recorded in `rules.inferred_floors` so
/// every DRC finding against it says the floor was inferred, not authored.
fn infer_via_floors(root: &SExpr<'_>, rules: &mut DesignRules, vias: &[Via], ctx: &Ctx<'_>) {
    let setup = root.find("setup");
    let authored = |k: &str| setup.and_then(|s| child_f64(s, k)).is_some();
    let normal: Vec<&Via> = vias.iter().filter(|v| !v.is_micro()).collect();
    if normal.is_empty() {
        return;
    }
    if !authored("via_min_drill") {
        let min = normal.iter().map(|v| v.drill).fold(f64::INFINITY, f64::min);
        ctx.warn(format!(
            "INFERRED via floor: board authors no (via_min_drill); using the smallest drill \
             of its own {} non-micro vias, {min} mm (fallback was {} mm)",
            normal.len(),
            rules.min_drill
        ));
        rules.min_drill = min;
        rules.inferred_floors.push("minDrill".into());
    }
    if !authored("via_min_annulus") {
        let min = normal
            .iter()
            .map(|v| (v.diameter - v.drill) / 2.0)
            .fold(f64::INFINITY, f64::min);
        ctx.warn(format!(
            "INFERRED via floor: board authors no (via_min_annulus); using the smallest annular \
             ring of its own {} non-micro vias, {min} mm (fallback was {} mm)",
            normal.len(),
            rules.min_annular_ring
        ));
        rules.min_annular_ring = min;
        rules.inferred_floors.push("minAnnularRing".into());
    }
}

/// Neck rule when the board authors none: a routed board whose tracks run
/// narrower than its trace width is necking down at pad entries (BGA and
/// fine-pitch escapes). The neck width is the most common such narrower
/// width among the board's own tracks (at least 10 of them); the neck length
/// stays the default region from the land. Logged loudly and recorded in
/// `rules.inferred_floors` ("neckWidth") so the DRC says it was inferred.
/// An authored neck (default or any class) wins.
fn infer_neck(rules: &mut DesignRules, traces: &[Trace], ctx: &Ctx<'_>) {
    if rules.default_rules.neck_width.is_some()
        || rules.class_rules.iter().any(|c| c.neck_width.is_some())
    {
        return;
    }
    let w = rules.default_rules.trace_width;
    let mut counts: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for t in traces
        .iter()
        .filter(|t| t.width < w - 1e-9 && t.width > 0.0)
    {
        *counts.entry((t.width * 1e4).round() as i64).or_default() += 1;
    }
    let Some((&k, &n)) = counts.iter().max_by_key(|(k, n)| (**n, -**k)) else {
        return;
    };
    if n < 10 {
        return;
    }
    let neck = k as f64 / 1e4;
    let narrow: usize = counts.values().sum();
    ctx.warn(format!(
        "INFERRED neck rule: board authors none; {narrow} of its tracks run narrower than the \
         {w} mm trace width, most commonly {neck} mm ({n} tracks) — neckWidth = {neck} mm within \
         the default neck length of a same-net pad"
    ));
    rules.default_rules.neck_width = Some(neck);
    rules.inferred_floors.push("neckWidth".into());
}

/// Calibrate imported design rules against the board's existing copper.
///
/// Only tightens, never loosens: a routed board whose median track is thinner
/// than the parsed default width adopts the median (and caps clearance at
/// that width — a fab that draws 0.1 mm tracks spaces them comparably); a via
/// population smaller than the parsed default adopts the modal via. A board
/// with no copper (fresh layout) keeps the parsed/default rules untouched.
fn calibrate_rules_from_copper(
    mut rules: DesignRules,
    traces: &[Trace],
    vias: &[Via],
) -> DesignRules {
    if !traces.is_empty() {
        let mut widths: Vec<f64> = traces
            .iter()
            .map(|t| t.width)
            .filter(|w| *w > 0.0)
            .collect();
        widths.sort_by(f64::total_cmp);
        if let Some(&median) = widths.get(widths.len() / 2) {
            if median < rules.default_rules.trace_width {
                rules.default_rules.trace_width = median;
            }
            if median < rules.default_rules.clearance {
                rules.default_rules.clearance = median;
            }
        }
    }
    if !vias.is_empty() {
        // Modal (diameter, drill) pair — the via the design actually uses most.
        let mut counts: HashMap<(u64, u64), usize> = HashMap::new();
        for v in vias {
            *counts
                .entry((v.diameter.to_bits(), v.drill.to_bits()))
                .or_default() += 1;
        }
        if let Some(((d, dr), _)) = counts.into_iter().max_by_key(|(_, c)| *c) {
            let (d, dr) = (f64::from_bits(d), f64::from_bits(dr));
            if d > 0.0 && d < rules.default_rules.via_diameter {
                rules.default_rules.via_diameter = d;
                rules.default_rules.via_drill = dr.min(d);
            }
        }
    }
    rules
}

// ---------------------------------------------------------------------------
// Board outline from Edge.Cuts
// ---------------------------------------------------------------------------

fn parse_board_outline(root: &SExpr<'_>, thickness: f64) -> BoardOutline {
    let mut edge_points: Vec<Vec2> = Vec::new();

    // Collect gr_line on Edge.Cuts
    for line in root.find_all("gr_line") {
        let layer = line
            .find("layer")
            .and_then(|n| n.children())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str());
        if layer == Some("Edge.Cuts") {
            edge_points.push(read_start(line));
            edge_points.push(read_end(line));
        }
    }

    // Collect gr_arc on Edge.Cuts (approximate as line between start/end)
    for arc in root.find_all("gr_arc") {
        let layer = arc
            .find("layer")
            .and_then(|n| n.children())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str());
        if layer == Some("Edge.Cuts") {
            edge_points.push(read_start(arc));
            edge_points.push(read_end(arc));
        }
    }

    // Deduplicate and order points into a polygon
    let vertices = order_outline_points(edge_points);

    if vertices.is_empty() {
        // Fallback: 50x30 default
        BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(50.0, 0.0),
                Vec2::new(50.0, 30.0),
                Vec2::new(0.0, 30.0),
            ],
            cutouts: vec![],
            thickness,
        }
    } else {
        BoardOutline {
            vertices,
            cutouts: vec![],
            thickness,
        }
    }
}

/// Order outline points by following connected segments into a closed polygon.
fn order_outline_points(points: Vec<Vec2>) -> Vec<Vec2> {
    if points.len() < 4 {
        return points;
    }

    // Build edge segments (pairs of points)
    let mut segments: Vec<(Vec2, Vec2)> = Vec::new();
    let mut i = 0;
    while i + 1 < points.len() {
        segments.push((points[i], points[i + 1]));
        i += 2;
    }

    if segments.is_empty() {
        return vec![];
    }

    // Walk the chain
    let mut result = vec![segments[0].0];
    let mut current = segments[0].1;
    result.push(current);
    let mut used = vec![false; segments.len()];
    used[0] = true;

    let eps = 0.01;
    for _ in 0..segments.len() {
        let mut found = false;
        for (j, seg) in segments.iter().enumerate() {
            if used[j] {
                continue;
            }
            if (seg.0.x - current.x).abs() < eps && (seg.0.y - current.y).abs() < eps {
                current = seg.1;
                result.push(current);
                used[j] = true;
                found = true;
                break;
            }
            if (seg.1.x - current.x).abs() < eps && (seg.1.y - current.y).abs() < eps {
                current = seg.0;
                result.push(current);
                used[j] = true;
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }

    // Remove the last point if it closes the loop (same as first)
    if result.len() > 2 {
        let first = result[0];
        let last = result[result.len() - 1];
        if (first.x - last.x).abs() < eps && (first.y - last.y).abs() < eps {
            result.pop();
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Stackup
// ---------------------------------------------------------------------------

fn parse_stackup(root: &SExpr<'_>, warnings: &mut Vec<String>) -> LayerStackup {
    // Try to read from the layers section which copper layers are defined
    let mut copper_layers: Vec<PcbLayer> = Vec::new();
    if let Some(layers_node) = root.find("layers") {
        if let Some(children) = layers_node.children() {
            for child in children.iter().skip(1) {
                if let Some(cc) = child.children() {
                    // (N "F.Cu" signal)
                    if let Some(name) = cc.get(1).and_then(|v| v.as_str()) {
                        if let Some(layer) = parse_layer(name) {
                            if layer.is_copper() {
                                copper_layers.push(layer);
                            }
                        }
                    }
                }
            }
        }
    }

    if copper_layers.is_empty() {
        warnings.push(
            "board has no copper layers in its (layers ...) table; assuming 2-layer F.Cu/B.Cu"
                .into(),
        );
        copper_layers = vec![PcbLayer::FCu, PcbLayer::BCu];
    }
    copper_layers.dedup();

    // Sort: FCu first, then inner, then BCu
    copper_layers.sort_by_key(|l| l.copper_position().unwrap_or(u8::MAX));

    // The real thing, when the board carries it: `(setup (stackup ...))` lists
    // every physical layer top to bottom — copper with its thickness, and the
    // prepreg/core between them with thickness, material and epsilon_r.
    // Impedance work needs those actual numbers, so prefer them.
    if let Some(real) = parse_declared_stackup(root, &copper_layers) {
        return real;
    }

    // No declared stackup. Copper thickness stays at the 1oz default every
    // downstream consumer of copper weight already assumed; the dielectric is
    // left EMPTY rather than filled with board-thickness/(n-1) at er 4.5. That
    // synthetic dielectric read like measured data to anything downstream —
    // and the impedance solver must fail closed on a board that does not
    // declare its stackup, not solve against an invented one.
    let layers: Vec<StackupLayer> = copper_layers
        .iter()
        .map(|&layer| StackupLayer {
            role: None,
            role_inferred: false,
            layer,
            copper_thickness: Some(0.035),
            dielectric_thickness: None,
            dielectric_er: None,
            material: None,
        })
        .collect();

    LayerStackup { layers }
}

/// Read `(setup (stackup ...))` into per-copper-layer physical data.
///
/// Each [`StackupLayer`] carries the dielectric *below* it (toward the next
/// copper layer down); the bottom copper layer carries none. Where KiCad splits
/// the space between two copper layers into several prepreg/core sub-layers,
/// thicknesses add and `er` is thickness-weighted.
///
/// Returns `None` when there is no stackup section, or when it does not cover
/// the board's copper layers — a partially-invented stackup is worse than none.
fn parse_declared_stackup(root: &SExpr<'_>, copper: &[PcbLayer]) -> Option<LayerStackup> {
    let stackup = root.find("setup")?.find("stackup")?;
    let entries = stackup.children()?;

    let mut out: Vec<StackupLayer> = Vec::new();
    let mut pending: Option<(PcbLayer, Option<f64>)> = None; // (layer, copper thickness)
    let mut diel_t = 0.0_f64;
    let mut diel_er_t = 0.0_f64; // thickness-weighted er accumulator
    let mut saw_diel = false;
    let mut saw_er = false;

    for e in entries.iter().skip(1) {
        if e.tag_name() != Some("layer") {
            continue;
        }
        let name = e.children().and_then(|c| c.get(1)).and_then(|v| v.as_str());
        let ty = e
            .find("type")
            .and_then(|n| n.children())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let thickness = child_f64(e, "thickness");

        if ty == "copper" {
            let Some(layer) = name.and_then(parse_layer).filter(|l| l.is_copper()) else {
                continue;
            };
            // Close out the previous copper layer with the dielectric that ran
            // between it and this one.
            if let Some((prev, cu_t)) = pending.take() {
                out.push(StackupLayer {
                    role: None,
                    role_inferred: false,
                    layer: prev,
                    copper_thickness: cu_t,
                    dielectric_thickness: (saw_diel && diel_t > 0.0).then_some(diel_t),
                    dielectric_er: (saw_er && diel_t > 0.0).then(|| diel_er_t / diel_t),
                    material: saw_diel.then(|| "dielectric".to_string()),
                });
            }
            pending = Some((layer, thickness));
            diel_t = 0.0;
            diel_er_t = 0.0;
            saw_diel = false;
            saw_er = false;
        } else if matches!(ty, "prepreg" | "core" | "dielectric") {
            // Only dielectric *between* two copper layers counts.
            let Some(t) = thickness else { continue };
            if pending.is_some() {
                diel_t += t;
                saw_diel = true;
                // Thickness without an epsilon_r leaves the stack only
                // partially characterised: the geometry is recorded, the `er`
                // is not invented, and the impedance layer fails closed on it.
                if let Some(er) = child_f64(e, "epsilon_r") {
                    diel_er_t += t * er;
                    saw_er = true;
                }
            }
        }
    }
    if let Some((last, cu_t)) = pending {
        out.push(StackupLayer {
            role: None,
            role_inferred: false,
            layer: last,
            copper_thickness: cu_t,
            dielectric_thickness: None,
            dielectric_er: None,
            material: None,
        });
    }

    if out.len() < 2 || out.len() != copper.len() {
        return None;
    }
    Some(LayerStackup { layers: out })
}

// ---------------------------------------------------------------------------
// Design Rules
// ---------------------------------------------------------------------------

fn parse_design_rules(root: &SExpr<'_>) -> DesignRules {
    let mut trace_width = 0.25;
    let mut clearance_val = 0.2;
    let mut via_diameter = 0.8;
    let mut via_drill = 0.4;
    let mut hole_to_hole = 0.25;
    let mut min_annular_ring = 0.13;
    let mut min_drill = 0.2;

    if let Some(setup) = root.find("setup") {
        // KiCad 6+ stores rules in (setup ...)
        if let Some(v) = child_f64(setup, "trace_min") {
            trace_width = v;
        }
        if let Some(v) = child_f64(setup, "clearance") {
            clearance_val = v;
        }
        if let Some(v) = child_f64(setup, "via_size") {
            via_diameter = v;
        }
        if let Some(v) = child_f64(setup, "via_drill") {
            via_drill = v;
        }
        // Board-level design constraints (the fab floors), as written by
        // `crate::kicad_write::write_setup`.
        if let Some(v) = child_f64(setup, "via_min_annulus") {
            min_annular_ring = v;
        }
        if let Some(v) = child_f64(setup, "via_min_drill") {
            min_drill = v;
        }
        if let Some(v) = child_f64(setup, "hole_to_hole_min") {
            hole_to_hole = v;
        }
    }

    // Net classes. The first `(net_class …)` named "Default" (or, absent one,
    // the first block) supplies the default rules; the rest become class
    // overrides with their `(add_net …)` members as assignments.
    let blocks = root.find_all("net_class");
    let mut class_rules: Vec<NetClassRules> = Vec::new();
    let mut net_class_assignments: HashMap<String, Vec<String>> = HashMap::new();
    let mut default_from_class: Option<NetClassRules> = None;
    for (i, block) in blocks.iter().enumerate() {
        let name = block
            .children()
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rules = NetClassRules {
            neck_width: None,
            neck_length: None,
            name: name.clone(),
            trace_width: child_f64(block, "trace_width").unwrap_or(trace_width),
            clearance: child_f64(block, "clearance").unwrap_or(clearance_val),
            via_diameter: child_f64(block, "via_dia").unwrap_or(via_diameter),
            via_drill: child_f64(block, "via_drill").unwrap_or(via_drill),
            diff_pair_gap: child_f64(block, "diff_pair_gap"),
            diff_pair_width: child_f64(block, "diff_pair_width"),
            target_impedance: None,
            target_diff_impedance: None,
        };
        let is_default = name == "Default" || (i == 0 && blocks.len() == 1);
        if is_default && default_from_class.is_none() {
            default_from_class = Some(rules);
            continue;
        }
        let nets: Vec<String> = block
            .find_all("add_net")
            .into_iter()
            .filter_map(|n| n.children()?.get(1)?.as_str().map(str::to_string))
            .collect();
        if !nets.is_empty() {
            net_class_assignments.insert(name, nets);
        }
        class_rules.push(rules);
    }

    let default_rules = default_from_class.unwrap_or(NetClassRules {
        neck_width: None,
        neck_length: None,
        name: "Default".to_string(),
        trace_width,
        clearance: clearance_val,
        via_diameter,
        via_drill,
        diff_pair_gap: None,
        diff_pair_width: None,
        target_impedance: None,
        target_diff_impedance: None,
    });

    let setup = root.find("setup");
    let uvia = |key: &str, dflt: f64| setup.and_then(|s| child_f64(s, key)).unwrap_or(dflt);
    DesignRules {
        allow_stacked_microvias: false,
        inferred_floors: Vec::new(),
        // KiCad's own defaults when the board omits them (they normally live
        // in the .kicad_pro); `parse_kicad_pcb` logs which were defaulted.
        min_microvia_drill: uvia("uvia_min_drill", vcad_ir::ecad::DEFAULT_MIN_MICROVIA_DRILL),
        min_microvia_diameter: uvia(
            "uvia_min_size",
            vcad_ir::ecad::DEFAULT_MIN_MICROVIA_DIAMETER,
        ),
        default_rules,
        class_rules,
        net_class_assignments,
        // KiCad 9's board format has no edge-clearance token (`edge_clearance`
        // in `(setup …)` makes it refuse the file), so this one constraint
        // travels only in the project file and cannot round-trip through a
        // board on its own.
        edge_clearance: 0.25,
        hole_to_hole,
        min_annular_ring,
        min_drill,
    }
}

// ---------------------------------------------------------------------------
// Footprint
// ---------------------------------------------------------------------------

fn parse_footprint(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Option<Footprint> {
    let children = node.children()?;

    // Footprint name is second element
    let footprint_name = children
        .get(1)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // KiCad's footprint angle turns counter-clockwise on screen; vcad's turns
    // the other way (math rotation in a y-down frame). Negate on the way in,
    // as the writer negates on the way out (`kicad_write::kicad_angle`).
    let (position, kicad_rotation) = read_at(node);
    let rotation = normalize_deg(-kicad_rotation);

    // Determine front/back from layer
    let layer = node
        .find("layer")
        .and_then(|n| n.children())
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str());
    let front = layer != Some("B.Cu");

    // Reference and value from fp_text
    let mut reference = String::new();
    let mut value = String::new();
    for txt in node.find_all("fp_text") {
        if let Some(c) = txt.children() {
            let kind = c.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let text = c.get(2).and_then(|v| v.as_str()).unwrap_or("").to_string();
            match kind {
                "reference" => reference = text,
                "value" => value = text,
                _ => {}
            }
        }
    }
    // KiCad 8+ uses (property "Reference" "R1")
    for prop in node.find_all("property") {
        if let Some(c) = prop.children() {
            let key = c.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let val = c.get(2).and_then(|v| v.as_str()).unwrap_or("").to_string();
            match key {
                "Reference" => reference = val,
                "Value" => value = val,
                _ => {}
            }
        }
    }

    if reference.is_empty() {
        reference = footprint_name.clone();
    }

    // Parse pads.
    //
    // KiCad stores a pad's angle ABSOLUTELY — it already includes the
    // footprint's orientation (rotate a footprint 90 degrees and every pad's
    // `(at x y a)` becomes `a + 90`; measured across this repo's CM5 fixture:
    // fp_rot -90 -> pad_rot 270 on 798 pads, 45 -> 45, 90 -> 90, 135 -> 135).
    // vcad's IR stores it RELATIVE to the footprint, because every consumer
    // (DRC, the router's spatial index, DFM, pour synthesis, render, the 3D
    // mesh) composes `fp.rotation + pad.rotation`. Storing the absolute angle
    // double-counted the footprint rotation on every rotated part: a QFN's
    // 0.25 x 0.875mm pads at 0.5mm pitch came out rotated 90 degrees off, so
    // neighbouring pads OVERLAPPED — phantom shorts in DRC and a pin field the
    // router could not escape.
    let pads: Vec<Pad> = node
        .find_all("pad")
        .iter()
        .filter_map(|p| parse_pad(p, ctx, &footprint_name))
        .map(|mut pad| {
            // KiCad pad angles are absolute and counter-clockwise; vcad's are
            // relative to the footprint and clockwise: -(abs - fp).
            pad.rotation = normalize_deg(kicad_rotation - pad.rotation);
            pad
        })
        .collect();

    // Parse graphics (fp_line, fp_circle, fp_rect, fp_arc)
    let mut graphics = Vec::new();
    for line in node.find_all("fp_line") {
        if let Some(layer) = ctx.layer(line) {
            graphics.push(FootprintGraphic::Line {
                start: read_start(line),
                end: read_end(line),
                width: read_width(line),
                layer,
            });
        }
    }
    for circ in node.find_all("fp_circle") {
        if let Some(layer) = ctx.layer(circ) {
            let center = read_xy(circ, "center");
            let end = read_end(circ);
            let dx = end.x - center.x;
            let dy = end.y - center.y;
            let radius = (dx * dx + dy * dy).sqrt();
            graphics.push(FootprintGraphic::Circle {
                center,
                radius,
                width: read_width(circ),
                layer,
            });
        }
    }
    for rect in node.find_all("fp_rect") {
        if let Some(layer) = ctx.layer(rect) {
            graphics.push(FootprintGraphic::Rect {
                start: read_start(rect),
                end: read_end(rect),
                width: read_width(rect),
                layer,
            });
        }
    }

    Some(Footprint {
        reference,
        value,
        footprint_name,
        position,
        rotation,
        front,
        pads,
        graphics,
        model_3d: None,
        properties: HashMap::new(),
    })
}

// ---------------------------------------------------------------------------
// Pad
// ---------------------------------------------------------------------------

/// Fold an angle in degrees into `[0, 360)`.
fn normalize_deg(a: f64) -> f64 {
    let r = a % 360.0;
    if r < 0.0 {
        r + 360.0
    } else {
        r
    }
}

fn parse_pad(node: &SExpr<'_>, ctx: &Ctx<'_>, fp_name: &str) -> Option<Pad> {
    let net_map = ctx.net_map;
    let children = node.children()?;
    // (pad "1" smd rect (at X Y) (size W H) (layers ...) (net N "name"))
    let number = children
        .get(1)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pad_type_str = children.get(2).and_then(|v| v.as_str()).unwrap_or("");
    let shape_str = children.get(3).and_then(|v| v.as_str()).unwrap_or("");

    let pad_type = match pad_type_str {
        "smd" => PadType::SMD,
        "thru_hole" => PadType::THT,
        "np_thru_hole" => PadType::NPTH,
        // Edge-connector style copper with no paste: surface copper.
        "connect" => PadType::SMD,
        other => {
            ctx.warn(format!(
                "{fp_name} pad {number:?}: unknown pad type {other:?}, imported as SMD"
            ));
            PadType::SMD
        }
    };

    let (position, rotation) = read_at(node);

    // Size
    let (sw, sh) = match node.find("size").and_then(|n| n.children()) {
        Some(c) if c.len() >= 3 => (c[1].as_f64().unwrap_or(1.0), c[2].as_f64().unwrap_or(1.0)),
        _ => (1.0, 1.0),
    };

    let shape = match shape_str {
        "circle" => PadShape::Circle { diameter: sw },
        "oval" => PadShape::Oval {
            width: sw,
            height: sh,
        },
        "roundrect" => {
            let ratio = child_f64(node, "roundrect_rratio").unwrap_or(0.25);
            PadShape::RoundRect {
                width: sw,
                height: sh,
                corner_ratio: ratio,
            }
        }
        "rect" => PadShape::Rect {
            width: sw,
            height: sh,
        },
        other => {
            // trapezoid / custom (and anything newer) have no exact IR form
            // here; keep the anchor rectangle but say so.
            ctx.warn(format!(
                "{fp_name} pad {number:?}: pad shape {other:?} approximated as a {sw}x{sh} rect"
            ));
            PadShape::Rect {
                width: sw,
                height: sh,
            }
        }
    };

    let layers = ctx.layers(node);
    if layers.is_empty() {
        ctx.warn(format!(
            "{fp_name} pad {number:?}: no representable layers; pad has no copper"
        ));
    }

    // Drill
    let drill = node.find("drill").and_then(|d| {
        let dc = d.children()?;
        let diameter = dc.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        if diameter <= 0.0 {
            return None;
        }
        // Check for oval drill
        let oval_height = dc.get(2).and_then(|v| v.as_f64());
        Some(DrillSpec {
            diameter,
            oval: oval_height.is_some(),
            oval_height,
        })
    });

    // Net
    let net = {
        let net_node = node.find("net");
        net_node
            .and_then(|n| n.children())
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u32>().ok())
            .and_then(|idx| net_map.get(&idx).cloned())
    };

    Some(Pad {
        number,
        pad_type,
        shape,
        position,
        rotation,
        drill,
        net,
        layers,
    })
}

// ---------------------------------------------------------------------------
// Trace (segment)
// ---------------------------------------------------------------------------

fn parse_trace(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Option<Trace> {
    let start = read_start(node);
    let end = read_end(node);
    let width = read_width(node);
    let layer = ctx.layer(node)?;
    let net = read_net_str(node, ctx.net_map);

    Some(Trace {
        start,
        end,
        width,
        layer,
        net,
        source: None,
    })
}

/// `(arc (start) (mid) (end) (width) (layer) (net))` → [`TraceArc`]. The IR
/// sweeps linearly from `start_angle` to `end_angle` (either sign), so the end
/// angle is unwrapped to pass through the file's mid point.
fn parse_trace_arc(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Option<TraceArc> {
    let (a, m, b) = (read_start(node), read_xy(node, "mid"), read_end(node));
    let layer = ctx.layer(node)?;
    let Some((center, radius)) = circle_through(a, m, b) else {
        ctx.warn(format!(
            "arc track at ({}, {}) is degenerate (collinear points); dropped",
            a.x, a.y
        ));
        return None;
    };
    let ang = |p: Vec2| (p.y - center.y).atan2(p.x - center.x).to_degrees();
    let (sa, ma, ea) = (ang(a), ang(m), ang(b));
    let ccw = (ea - sa).rem_euclid(360.0);
    let mid_off = (ma - sa).rem_euclid(360.0);
    let end_angle = if mid_off < ccw {
        sa + ccw
    } else {
        sa - (360.0 - ccw)
    };
    Some(TraceArc {
        center,
        radius,
        start_angle: sa,
        end_angle,
        width: read_width(node),
        layer,
        net: read_net_str(node, ctx.net_map),
    })
}

/// Circumscribed circle of three points, or `None` if collinear.
fn circle_through(a: Vec2, b: Vec2, c: Vec2) -> Option<(Vec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (
        a.x * a.x + a.y * a.y,
        b.x * b.x + b.y * b.y,
        c.x * c.x + c.y * c.y,
    );
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Vec2::new(ux, uy);
    let r = ((a.x - ux).powi(2) + (a.y - uy).powi(2)).sqrt();
    Some((center, r))
}

// ---------------------------------------------------------------------------
// Via
// ---------------------------------------------------------------------------

fn parse_via(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Option<Via> {
    let (position, _) = read_at(node);
    let diameter = child_f64(node, "size").unwrap_or(0.8);
    let drill = child_f64(node, "drill").unwrap_or(0.4);
    let net = read_net_str(node, ctx.net_map);

    // Span = the outermost copper layers listed, in stack order (file order
    // is not guaranteed top-first).
    let copper: Vec<PcbLayer> = ctx
        .layers(node)
        .into_iter()
        .filter(|l| l.is_copper())
        .collect();
    let pos = |l: &&PcbLayer| l.copper_position();
    let (start_layer, end_layer) =
        match (copper.iter().min_by_key(pos), copper.iter().max_by_key(pos)) {
            (Some(&a), Some(&b)) => (a, b),
            _ => {
                ctx.warn(format!(
                    "via at ({}, {}) has no parseable copper layers; imported as a through via",
                    position.x, position.y
                ));
                (PcbLayer::FCu, PcbLayer::BCu)
            }
        };

    // KiCad's type token sits right after `via`: `micro`, `blind`, or none
    // (through).
    let token = node
        .children()
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let via_type = Some(match token {
        "micro" => vcad_ir::ecad::ViaType::Micro,
        "blind" => vcad_ir::ecad::ViaType::Blind,
        _ => vcad_ir::ecad::ViaType::Through,
    });
    Some(Via {
        via_type,
        position,
        diameter,
        drill,
        start_layer,
        end_layer,
        net,
        source: None,
    })
}

// ---------------------------------------------------------------------------
// Zone
// ---------------------------------------------------------------------------

/// One IR zone per copper layer: KiCad 7+ writes a multi-layer zone as
/// `(layers "F.Cu" "B.Cu")` (or `"*.Cu"`), older files as `(layer "F.Cu")`.
fn parse_zone(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Vec<Zone> {
    if node.find("keepout").is_some() {
        // A rule area, not copper. vcad has no keepout import yet; never let
        // one masquerade as a pour (a `*.Cu` keepout would flood every layer).
        ctx.warn("keepout rule area not imported".into());
        return Vec::new();
    }
    let mut layers = ctx.layers(node);
    if let Some(l) = ctx.layer(node) {
        if !layers.contains(&l) {
            layers.push(l);
        }
    }
    let zones: Vec<Zone> = match parse_zone_on(node, ctx) {
        Some(z) => layers
            .into_iter()
            .map(|layer| Zone { layer, ..z.clone() })
            .collect(),
        None => Vec::new(),
    };
    if zones.is_empty() && node.find("polygon").is_some() {
        ctx.warn("zone with an outline but no representable layer; dropped".into());
    }
    zones
}

fn parse_zone_on(node: &SExpr<'_>, ctx: &Ctx<'_>) -> Option<Zone> {
    let net = read_net_str(node, ctx.net_map);
    let layer = PcbLayer::FCu; // placeholder; parse_zone stamps each real layer
    let clearance_val = child_f64(node, "min_thickness").unwrap_or(0.2);

    // Parse polygon outline
    let mut outline = Vec::new();
    if let Some(poly) = node.find("polygon") {
        if let Some(pts) = poly.find("pts") {
            for xy in pts.find_all("xy") {
                if let Some(c) = xy.children() {
                    if c.len() >= 3 {
                        outline.push(Vec2::new(
                            c[1].as_f64().unwrap_or(0.0),
                            c[2].as_f64().unwrap_or(0.0),
                        ));
                    }
                }
            }
        }
    }

    if outline.is_empty() {
        return None;
    }

    Some(Zone {
        outline,
        holes: vec![],
        net,
        layer,
        clearance: clearance_val,
        min_area: 0.0,
        fill_type: ZoneFillType::Solid,
        thermal_relief: ThermalReliefStyle::Relief,
        thermal_gap: None,
        thermal_spoke_width: None,
        priority: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4-layer board whose copper stack is F.Cu/In1.Cu/In2.Cu/B.Cu.
    const FOUR_LAYER_THT: &str = r#"(kicad_pcb (version 20240108) (generator test)
  (general (thickness 1.6))
  (layers
    (0 "F.Cu" signal)
    (1 "In1.Cu" signal)
    (2 "In2.Cu" signal)
    (31 "B.Cu" signal)
    (38 "B.Mask" user)
    (39 "F.Mask" user)
    (44 "Edge.Cuts" user)
  )
  (net 0 "")
  (net 1 "SIG")
  (net 2 "GND")
  (gr_line (start 0 0) (end 40 0) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 40 0) (end 40 20) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 40 20) (end 0 20) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 0 20) (end 0 0) (layer "Edge.Cuts") (width 0.05))
  (footprint "Conn_1x01" (layer "F.Cu") (at 10 10)
    (property "Reference" "J1")
    (pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SIG"))
  )
  (footprint "Conn_1x01" (layer "F.Cu") (at 30 10)
    (property "Reference" "J2")
    (pad "1" thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "SIG"))
  )
  (segment (start 10 10) (end 30 10) (width 0.25) (layer "In2.Cu") (net 1))
)"#;

    #[test]
    fn tht_wildcard_pad_lands_on_every_board_copper_layer() {
        let (pcb, warnings) = parse_kicad_pcb_with_warnings(FOUR_LAYER_THT).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let pad = &pcb.footprints[0].pads[0];
        assert_eq!(
            pad.layers,
            vec![
                PcbLayer::FCu,
                PcbLayer::In1Cu,
                PcbLayer::In2Cu,
                PcbLayer::BCu,
                PcbLayer::FMask,
                PcbLayer::BMask
            ]
        );
    }

    #[test]
    fn f_and_b_cu_pad_and_two_layer_wildcard() {
        let input = FOUR_LAYER_THT.replace(
            r#"(layers "*.Cu" "*.Mask") (net 1 "SIG"))
  )
  (footprint "Conn_1x01" (layer "F.Cu") (at 30 10)"#,
            r#"(layers "F&B.Cu" "*.Mask") (net 1 "SIG"))
  )
  (footprint "Conn_1x01" (layer "F.Cu") (at 30 10)"#,
        );
        let pcb = parse_kicad_pcb(&input).unwrap();
        assert_eq!(
            pcb.footprints[0].pads[0].layers,
            vec![
                PcbLayer::FCu,
                PcbLayer::BCu,
                PcbLayer::FMask,
                PcbLayer::BMask
            ]
        );
    }

    #[test]
    fn inner_layers_past_in8_import_and_unknown_layers_warn() {
        // 32-layer board: F.Cu, In1..In30, B.Cu.
        let mut table = String::from("(0 \"F.Cu\" signal)\n");
        for n in 1..=30 {
            table += &format!("({n} \"In{n}.Cu\" signal)\n");
        }
        table += "(31 \"B.Cu\" signal)\n";
        let input = format!(
            r#"(kicad_pcb (version 20240108) (generator test)
  (layers {table})
  (net 0 "") (net 1 "A")
  (segment (start 0 0) (end 5 0) (width 0.2) (layer "In22.Cu") (net 1))
  (arc (start 0 0) (mid 1 1) (end 2 0) (width 0.2) (layer "In30.Cu") (net 1))
  (via blind (at 5 0) (size 0.4) (drill 0.2) (layers "In20.Cu" "In9.Cu") (net 1))
  (segment (start 0 0) (end 5 0) (width 0.2) (layer "User.1") (net 1))
  (zone (net 1) (net_name "A") (layers "*.Cu")
    (polygon (pts (xy 0 0) (xy 5 0) (xy 5 5) (xy 0 5))))
)"#
        );
        let (pcb, warnings) = parse_kicad_pcb_with_warnings(&input).unwrap();
        assert_eq!(pcb.stackup.layers.len(), 32);
        assert_eq!(pcb.traces.len(), 1);
        assert_eq!(pcb.traces[0].layer, PcbLayer::In22Cu);
        assert_eq!(pcb.vias[0].start_layer, PcbLayer::In9Cu);
        assert_eq!(pcb.vias[0].end_layer, PcbLayer::In20Cu);
        assert_eq!(pcb.trace_arcs.len(), 1);
        let arc = &pcb.trace_arcs[0];
        assert_eq!(arc.layer, PcbLayer::In30Cu);
        assert!((arc.radius - 1.0).abs() < 1e-9);
        assert!((arc.center.x - 1.0).abs() < 1e-9 && arc.center.y.abs() < 1e-9);
        // start (0,0) is 180°, passes through 90° (the mid), ends at 0°.
        assert!((arc.start_angle.abs() - 180.0).abs() < 1e-9);
        let mid = (arc.start_angle + arc.end_angle) / 2.0;
        assert!(((mid.rem_euclid(360.0)) - 90.0).abs() < 1e-9, "{arc:?}");
        // *.Cu zone → one zone per copper layer of THIS board.
        assert_eq!(pcb.zones.len(), 32);
        assert!(
            warnings.iter().any(|w| w.contains("User.1")),
            "unknown layer must warn: {warnings:?}"
        );
    }

    #[test]
    fn multi_layer_zone_imports_one_zone_per_layer() {
        let input = FOUR_LAYER_THT.replace(
            "  (segment (start 10 10)",
            r#"  (zone (net 2) (net_name "GND") (layers "F.Cu" "B.Cu")
    (polygon (pts (xy 0 0) (xy 40 0) (xy 40 20) (xy 0 20))))
  (zone (net 2) (net_name "GND") (layer "In1.Cu")
    (polygon (pts (xy 0 0) (xy 40 0) (xy 40 20) (xy 0 20))))
  (zone (net 0) (net_name "") (layers "*.Cu") (keepout (tracks not_allowed))
    (polygon (pts (xy 1 1) (xy 2 1) (xy 2 2))))
  (segment (start 10 10)"#,
        );
        let (pcb, warnings) = parse_kicad_pcb_with_warnings(&input).unwrap();
        let layers: Vec<_> = pcb.zones.iter().map(|z| z.layer).collect();
        assert_eq!(layers, vec![PcbLayer::FCu, PcbLayer::BCu, PcbLayer::In1Cu]);
        assert!(pcb.zones.iter().all(|z| z.net == "GND"));
        assert!(warnings.iter().any(|w| w.contains("keepout")));
    }

    #[test]
    fn unknown_pad_type_and_shape_warn() {
        let input = FOUR_LAYER_THT.replacen("thru_hole circle", "thru_hole trapezoid", 1);
        let (_, warnings) = parse_kicad_pcb_with_warnings(&input).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("trapezoid")),
            "{warnings:?}"
        );
        let input = FOUR_LAYER_THT.replacen("thru_hole circle", "weird circle", 1);
        let (_, warnings) = parse_kicad_pcb_with_warnings(&input).unwrap();
        assert!(warnings.iter().any(|w| w.contains("weird")), "{warnings:?}");
    }

    #[test]
    fn via_without_layers_warns() {
        let input = FOUR_LAYER_THT.replace(
            "  (segment (start 10 10)",
            "  (via (at 5 5) (size 0.6) (drill 0.3) (net 1))\n  (segment (start 10 10)",
        );
        let (pcb, warnings) = parse_kicad_pcb_with_warnings(&input).unwrap();
        assert_eq!(pcb.vias.len(), 1);
        assert!(warnings.iter().any(|w| w.contains("via")), "{warnings:?}");
    }

    #[test]
    fn parse_minimal_kicad_pcb() {
        let input = r#"(kicad_pcb (version 20221018) (generator test)
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
        let pcb = parse_kicad_pcb(input).unwrap();

        assert_eq!(pcb.nets.len(), 2);
        assert_eq!(pcb.nets[0].name, "VCC");
        assert_eq!(pcb.outline.thickness, 1.6);
        assert_eq!(pcb.outline.vertices.len(), 4);
        assert_eq!(pcb.footprints.len(), 1);
        assert_eq!(pcb.footprints[0].reference, "R1");
        assert_eq!(pcb.footprints[0].pads.len(), 2);
        assert_eq!(pcb.traces.len(), 1);
        assert_eq!(pcb.vias.len(), 1);
    }

    #[test]
    fn parse_4_layer_board() {
        let input = r#"(kicad_pcb (version 20221018)
  (general (thickness 1.6))
  (layers
    (0 "F.Cu" signal)
    (1 "In1.Cu" signal)
    (2 "In2.Cu" signal)
    (31 "B.Cu" signal)
  )
)"#;
        let pcb = parse_kicad_pcb(input).unwrap();
        assert_eq!(pcb.stackup.layers.len(), 4);
        assert_eq!(pcb.stackup.layers[0].layer, PcbLayer::FCu);
        assert_eq!(pcb.stackup.layers[1].layer, PcbLayer::In1Cu);
        assert_eq!(pcb.stackup.layers[2].layer, PcbLayer::In2Cu);
        assert_eq!(pcb.stackup.layers[3].layer, PcbLayer::BCu);
    }

    /// A board that authors no neck rule but runs many tracks narrower than
    /// its trace width gets a neck inferred from them (recorded as inferred);
    /// too few narrow tracks infer nothing.
    #[test]
    fn neck_inferred_from_narrow_tracks() {
        let seg = |w: f64, n: usize, y: f64| {
            (0..n)
                .map(|i| {
                    format!(
                        "(segment (start {i} {y}) (end {} {y}) (width {w}) (layer \"F.Cu\") (net 1))\n",
                        i + 1
                    )
                })
                .collect::<String>()
        };
        let board = |narrow: usize| {
            format!(
                "(kicad_pcb (version 20221018) (general (thickness 1.0)) \
                 (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal)) (net 1 \"SIG\")\n{}{}{})",
                seg(0.1, 30, 0.0),
                seg(0.06, narrow, 1.0),
                seg(0.08, 3, 2.0)
            )
        };
        let pcb = parse_kicad_pcb(&board(20)).unwrap();
        assert_eq!(pcb.rules.default_rules.trace_width, 0.1);
        assert_eq!(pcb.rules.default_rules.neck_width, Some(0.06));
        assert!(pcb.rules.inferred_floors.iter().any(|f| f == "neckWidth"));
        let pcb = parse_kicad_pcb(&board(5)).unwrap();
        assert_eq!(pcb.rules.default_rules.neck_width, None);
        assert!(!pcb.rules.inferred_floors.iter().any(|f| f == "neckWidth"));
    }

    /// A routed board's own copper calibrates the imported rules: an HDI
    /// design with 0.1 mm tracks and 0.2 mm microvias must not import with
    /// the 0.25/0.2/0.8 fallback rules (which make it unroutable), and the
    /// calibration only ever tightens.
    #[test]
    fn rules_calibrate_to_existing_copper() {
        let input = r#"(kicad_pcb (version 20221018)
  (general (thickness 1.0))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (net 1 "SIG")
  (segment (start 0 0) (end 5 0) (width 0.1) (layer "F.Cu") (net 1))
  (segment (start 5 0) (end 9 0) (width 0.1) (layer "F.Cu") (net 1))
  (segment (start 9 0) (end 9 5) (width 0.3) (layer "F.Cu") (net 1))
  (via (at 9 5) (size 0.2) (drill 0.1) (layers "F.Cu" "B.Cu") (net 1))
  (via (at 2 0) (size 0.2) (drill 0.1) (layers "F.Cu" "B.Cu") (net 1))
  (via (at 4 0) (size 0.4) (drill 0.2) (layers "F.Cu" "B.Cu") (net 1))
)"#;
        let pcb = parse_kicad_pcb(input).unwrap();
        let r = &pcb.rules.default_rules;
        assert_eq!(r.trace_width, 0.1, "median existing width");
        assert_eq!(r.clearance, 0.1, "clearance capped at median width");
        assert_eq!(r.via_diameter, 0.2, "modal via diameter");
        assert_eq!(r.via_drill, 0.1, "modal via drill");

        // A bare board keeps the defaults — nothing to calibrate against.
        let bare = parse_kicad_pcb(
            r#"(kicad_pcb (version 20221018)
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
)"#,
        )
        .unwrap();
        assert_eq!(bare.rules.default_rules.trace_width, 0.25);
        assert_eq!(bare.rules.default_rules.via_diameter, 0.8);
    }
}
