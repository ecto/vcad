//! Parser for KiCad `.kicad_pcb` board files.
//!
//! Converts a `.kicad_pcb` S-expression file into a [`vcad_ir::ecad::Pcb`]
//! struct.  Reuses the [`crate::sexpr`] S-expression parser.

use std::collections::HashMap;

use vcad_ir::ecad::{
    BoardOutline, DesignRules, DrillSpec, Footprint, FootprintGraphic, LayerStackup, Net,
    NetClassRules, Pad, PadShape, PadType, Pcb, PcbLayer, StackupLayer, ThermalReliefStyle, Trace,
    Via, Zone, ZoneFillType,
};
use vcad_ir::Vec2;

use crate::sexpr::{parse_sexpr, SExpr};
use crate::ParseError;

/// Parse a `.kicad_pcb` file content into a [`Pcb`].
pub fn parse_kicad_pcb(input: &str) -> Result<Pcb, ParseError> {
    let (rest, root) = parse_sexpr(input)?;
    if !rest.trim().is_empty() {
        return Err(ParseError::TrailingInput);
    }
    if root.tag_name() != Some("kicad_pcb") {
        return Err(ParseError::Nom("expected (kicad_pcb ...)".into()));
    }
    convert_pcb(&root)
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
    match s {
        "F.Cu" => Some(PcbLayer::FCu),
        "B.Cu" => Some(PcbLayer::BCu),
        "In1.Cu" => Some(PcbLayer::In1Cu),
        "In2.Cu" => Some(PcbLayer::In2Cu),
        "In3.Cu" => Some(PcbLayer::In3Cu),
        "In4.Cu" => Some(PcbLayer::In4Cu),
        "In5.Cu" => Some(PcbLayer::In5Cu),
        "In6.Cu" => Some(PcbLayer::In6Cu),
        "In7.Cu" => Some(PcbLayer::In7Cu),
        "In8.Cu" => Some(PcbLayer::In8Cu),
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
        _ => None,
    }
}

/// Read `(layer "F.Cu")` child.
fn read_layer(node: &SExpr<'_>) -> Option<PcbLayer> {
    node.find("layer")
        .and_then(|n| n.children())
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str())
        .and_then(parse_layer)
}

/// Read `(layers ...)` child — list of layer names.
fn read_layers(node: &SExpr<'_>) -> Vec<PcbLayer> {
    match node.find("layers").and_then(|n| n.children()) {
        Some(c) => c
            .iter()
            .skip(1) // skip tag
            .filter_map(|v| v.as_str().and_then(parse_layer))
            .collect(),
        None => vec![],
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

fn convert_pcb(root: &SExpr<'_>) -> Result<Pcb, ParseError> {
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
    let stackup = parse_stackup(root);

    // 5. Parse design rules
    let rules = parse_design_rules(root);

    // 6. Parse footprints
    let footprints: Vec<Footprint> = root
        .find_all("footprint")
        .iter()
        .filter_map(|n| parse_footprint(n, &net_map))
        .collect();

    // 7. Parse traces (segment)
    let traces: Vec<Trace> = root
        .find_all("segment")
        .iter()
        .filter_map(|n| parse_trace(n, &net_map))
        .collect();

    // 8. Parse vias
    let vias: Vec<Via> = root
        .find_all("via")
        .iter()
        .filter_map(|n| parse_via(n, &net_map))
        .collect();

    // 9. Parse zones
    let zones: Vec<Zone> = root
        .find_all("zone")
        .iter()
        .filter_map(|n| parse_zone(n, &net_map))
        .collect();

    // The .kicad_pcb setup block rarely carries usable rules (net classes
    // live in the .kicad_pro project file), so a routed board would otherwise
    // import with the fat 0.25/0.2/0.8 defaults — hopeless on an HDI design
    // whose own copper is 0.1 mm tracks and 0.2 mm vias. The board's existing
    // copper is the best available evidence of its fab capability: calibrate
    // the default rules against it.
    let rules = calibrate_rules_from_copper(rules, &traces, &vias);

    Ok(Pcb {
        outline,
        stackup,
        nets,
        rules,
        footprints,
        traces,
        trace_arcs: vec![],
        vias,
        zones,
        keepouts: vec![],
        net_ties: vec![],
    })
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

fn parse_stackup(root: &SExpr<'_>) -> LayerStackup {
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
        copper_layers = vec![PcbLayer::FCu, PcbLayer::BCu];
    }

    // Sort: FCu first, then inner, then BCu
    copper_layers.sort_by_key(|l| l.copper_position().unwrap_or(u8::MAX));

    let thickness = root
        .find("general")
        .and_then(|g| child_f64(g, "thickness"))
        .unwrap_or(1.6);
    let n = copper_layers.len();
    let diel = if n > 1 {
        thickness / (n - 1) as f64
    } else {
        thickness
    };

    let layers: Vec<StackupLayer> = copper_layers
        .iter()
        .enumerate()
        .map(|(i, &layer)| StackupLayer {
            layer,
            copper_thickness: Some(0.035),
            dielectric_thickness: if i > 0 { Some(diel) } else { None },
            dielectric_er: if i > 0 { Some(4.5) } else { None },
            material: if i > 0 { Some("FR4".to_string()) } else { None },
        })
        .collect();

    LayerStackup { layers }
}

// ---------------------------------------------------------------------------
// Design Rules
// ---------------------------------------------------------------------------

fn parse_design_rules(root: &SExpr<'_>) -> DesignRules {
    let mut trace_width = 0.25;
    let mut clearance_val = 0.2;
    let mut via_diameter = 0.8;
    let mut via_drill = 0.4;

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
    }

    DesignRules {
        default_rules: NetClassRules {
            name: "Default".to_string(),
            trace_width,
            clearance: clearance_val,
            via_diameter,
            via_drill,
            diff_pair_gap: None,
            diff_pair_width: None,
        },
        class_rules: vec![],
        net_class_assignments: HashMap::new(),
        edge_clearance: 0.25,
        hole_to_hole: 0.25,
        min_annular_ring: 0.13,
        min_drill: 0.2,
    }
}

// ---------------------------------------------------------------------------
// Footprint
// ---------------------------------------------------------------------------

fn parse_footprint(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> Option<Footprint> {
    let children = node.children()?;

    // Footprint name is second element
    let footprint_name = children
        .get(1)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let (position, rotation) = read_at(node);

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

    // Parse pads
    let pads: Vec<Pad> = node
        .find_all("pad")
        .iter()
        .filter_map(|p| parse_pad(p, net_map))
        .collect();

    // Parse graphics (fp_line, fp_circle, fp_rect, fp_arc)
    let mut graphics = Vec::new();
    for line in node.find_all("fp_line") {
        if let Some(layer) = read_layer(line) {
            graphics.push(FootprintGraphic::Line {
                start: read_start(line),
                end: read_end(line),
                width: read_width(line),
                layer,
            });
        }
    }
    for circ in node.find_all("fp_circle") {
        if let Some(layer) = read_layer(circ) {
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
        if let Some(layer) = read_layer(rect) {
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

fn parse_pad(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> Option<Pad> {
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
        _ => PadType::SMD,
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
        _ => PadShape::Rect {
            width: sw,
            height: sh,
        }, // rect, trapezoid, custom
    };

    let layers = read_layers(node);

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

fn parse_trace(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> Option<Trace> {
    let start = read_start(node);
    let end = read_end(node);
    let width = read_width(node);
    let layer = read_layer(node)?;
    let net = read_net_str(node, net_map);

    Some(Trace {
        start,
        end,
        width,
        layer,
        net,
        source: None,
    })
}

// ---------------------------------------------------------------------------
// Via
// ---------------------------------------------------------------------------

fn parse_via(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> Option<Via> {
    let (position, _) = read_at(node);
    let diameter = child_f64(node, "size").unwrap_or(0.8);
    let drill = child_f64(node, "drill").unwrap_or(0.4);
    let net = read_net_str(node, net_map);

    let layers = read_layers(node);
    let start_layer = layers.first().copied().unwrap_or(PcbLayer::FCu);
    let end_layer = layers.last().copied().unwrap_or(PcbLayer::BCu);

    Some(Via {
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

fn parse_zone(node: &SExpr<'_>, net_map: &HashMap<u32, String>) -> Option<Zone> {
    let net = read_net_str(node, net_map);
    let layer = read_layer(node)?;
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
