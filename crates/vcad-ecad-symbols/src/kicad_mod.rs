//! Parser for KiCad `.kicad_mod` footprint library files.
//!
//! The format is an S-expression tree rooted at `footprint` (or `module` in
//! older KiCad versions). Each footprint contains pads, graphical primitives,
//! and optional 3D model references.

use serde::{Deserialize, Serialize};

use crate::sexpr::{parse_sexpr, SExpr};
use crate::ParseError;

/// A parsed footprint library (a single `.kicad_mod` file contains one footprint,
/// but multiple files can be collected into a library).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintLib {
    /// All footprints in this library.
    pub footprints: Vec<FootprintDef>,
}

/// A single footprint definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintDef {
    /// Footprint name (e.g. "R_0805_2012Metric").
    pub name: String,
    /// Pads on this footprint.
    pub pads: Vec<PadDef>,
    /// Graphical elements (silkscreen lines, courtyard, fab layer, etc.).
    pub graphics: Vec<GraphicDef>,
    /// Path to a 3D model file, if present.
    pub model_3d: Option<String>,
}

/// A pad definition in a footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PadDef {
    /// Pad number/name (e.g. "1", "2", "A1").
    pub number: String,
    /// Pad mounting type (SMD, THT, NPTH).
    pub pad_type: vcad_ir::ecad::PadType,
    /// Pad shape.
    pub shape: vcad_ir::ecad::PadShape,
    /// Position relative to footprint origin.
    pub position: (f64, f64),
    /// Rotation in degrees.
    pub rotation: f64,
    /// Layers this pad exists on.
    pub layers: Vec<String>,
    /// Drill specification for through-hole pads.
    pub drill: Option<vcad_ir::ecad::DrillSpec>,
}

/// A graphical element on a footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GraphicDef {
    /// A line segment (`fp_line`).
    Line {
        /// Start point.
        start: (f64, f64),
        /// End point.
        end: (f64, f64),
        /// Stroke width in mm.
        width: f64,
        /// Layer name.
        layer: String,
    },
    /// A circle (`fp_circle`).
    Circle {
        /// Center point.
        center: (f64, f64),
        /// A point on the circle (used to compute radius).
        end: (f64, f64),
        /// Stroke width in mm.
        width: f64,
        /// Layer name.
        layer: String,
    },
    /// An arc (`fp_arc`).
    Arc {
        /// Start point.
        start: (f64, f64),
        /// Midpoint on the arc.
        mid: (f64, f64),
        /// End point.
        end: (f64, f64),
        /// Stroke width in mm.
        width: f64,
        /// Layer name.
        layer: String,
    },
    /// A rectangle (`fp_rect`).
    Rect {
        /// Start corner.
        start: (f64, f64),
        /// End corner.
        end: (f64, f64),
        /// Stroke width in mm.
        width: f64,
        /// Layer name.
        layer: String,
    },
    /// A polygon (`fp_poly`).
    Poly {
        /// Polygon vertices.
        points: Vec<(f64, f64)>,
        /// Stroke width in mm.
        width: f64,
        /// Layer name.
        layer: String,
    },
    /// A text element (`fp_text`).
    Text {
        /// Text type: "reference", "value", or "user".
        text_type: String,
        /// Text content.
        content: String,
        /// Position.
        position: (f64, f64),
        /// Layer name.
        layer: String,
    },
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Extract an f64 from a child node at the given index in a list.
fn child_f64(node: &SExpr<'_>, idx: usize) -> Option<f64> {
    node.children()?.get(idx)?.as_f64()
}

/// Extract a string from a child node at the given index in a list.
fn child_str<'a>(node: &'a SExpr<'a>, idx: usize) -> Option<&'a str> {
    node.children()?.get(idx)?.as_str()
}

/// Parse `(at X Y [angle])` into position + angle.
fn parse_at(node: &SExpr<'_>) -> Option<((f64, f64), f64)> {
    if node.tag_name() != Some("at") {
        return None;
    }
    let x = child_f64(node, 1)?;
    let y = child_f64(node, 2)?;
    let angle = child_f64(node, 3).unwrap_or(0.0);
    Some(((x, y), angle))
}

/// Parse `(start X Y)` or `(end X Y)` or `(center X Y)` into a point.
fn parse_point(node: &SExpr<'_>) -> Option<(f64, f64)> {
    Some((child_f64(node, 1)?, child_f64(node, 2)?))
}

/// Parse `(xy X Y)` into a tuple.
fn parse_xy(node: &SExpr<'_>) -> Option<(f64, f64)> {
    if node.tag_name() != Some("xy") {
        return None;
    }
    Some((child_f64(node, 1)?, child_f64(node, 2)?))
}

/// Parse `(size W H)` into (width, height).
fn parse_size(node: &SExpr<'_>) -> Option<(f64, f64)> {
    if node.tag_name() != Some("size") {
        return None;
    }
    Some((child_f64(node, 1)?, child_f64(node, 2)?))
}

/// Parse `(layers ...)` into a vec of layer name strings.
fn parse_layers(node: &SExpr<'_>) -> Vec<String> {
    if node.tag_name() != Some("layers") {
        return vec![];
    }
    node.children()
        .unwrap_or(&[])
        .iter()
        .skip(1)
        .filter_map(|c| c.as_str().map(|s| s.to_string()))
        .collect()
}

/// Parse pad type keyword.
fn parse_pad_type(s: &str) -> vcad_ir::ecad::PadType {
    match s {
        "smd" => vcad_ir::ecad::PadType::SMD,
        "thru_hole" => vcad_ir::ecad::PadType::THT,
        "np_thru_hole" => vcad_ir::ecad::PadType::NPTH,
        // Connect pads are used for edge connectors -- map to SMD
        "connect" => vcad_ir::ecad::PadType::SMD,
        _ => vcad_ir::ecad::PadType::SMD,
    }
}

/// Parse pad shape keyword with size into a PadShape.
fn parse_pad_shape(
    shape_str: &str,
    size: (f64, f64),
    roundrect_rratio: Option<f64>,
) -> vcad_ir::ecad::PadShape {
    match shape_str {
        "circle" => vcad_ir::ecad::PadShape::Circle { diameter: size.0 },
        "rect" => vcad_ir::ecad::PadShape::Rect {
            width: size.0,
            height: size.1,
        },
        "oval" => vcad_ir::ecad::PadShape::Oval {
            width: size.0,
            height: size.1,
        },
        "roundrect" => vcad_ir::ecad::PadShape::RoundRect {
            width: size.0,
            height: size.1,
            corner_ratio: roundrect_rratio.unwrap_or(0.25),
        },
        _ => vcad_ir::ecad::PadShape::Rect {
            width: size.0,
            height: size.1,
        },
    }
}

/// Parse a `(drill ...)` node into a DrillSpec.
fn parse_drill(node: &SExpr<'_>) -> Option<vcad_ir::ecad::DrillSpec> {
    if node.tag_name() != Some("drill") {
        return None;
    }
    let children = node.children()?;

    // Check for "oval" keyword
    let has_oval = children.iter().any(|c| c.as_str() == Some("oval"));

    // Collect numeric values (skip "drill" keyword and "oval" keyword)
    let nums: Vec<f64> = children.iter().skip(1).filter_map(|c| c.as_f64()).collect();

    if nums.is_empty() {
        return None;
    }

    let diameter = nums[0];
    let oval_height = if has_oval && nums.len() > 1 {
        Some(nums[1])
    } else {
        None
    };

    Some(vcad_ir::ecad::DrillSpec {
        diameter,
        oval: has_oval,
        oval_height,
    })
}

/// Parse a `(pad ...)` node.
fn parse_pad(node: &SExpr<'_>) -> Option<PadDef> {
    if node.tag_name() != Some("pad") {
        return None;
    }
    let _children = node.children()?;
    // (pad "NUMBER" TYPE SHAPE (at ...) (size ...) (layers ...) ...)
    let number = child_str(node, 1)?.to_string();
    let pad_type_str = child_str(node, 2)?;
    let shape_str = child_str(node, 3)?;

    let pad_type = parse_pad_type(pad_type_str);

    // (at X Y [angle])
    let at_node = node.find("at")?;
    let (position, rotation) = parse_at(at_node)?;

    // (size W H)
    let size_node = node.find("size")?;
    let size = parse_size(size_node)?;

    // (roundrect_rratio R)
    let roundrect_rratio = node.find("roundrect_rratio").and_then(|n| child_f64(n, 1));

    let shape = parse_pad_shape(shape_str, size, roundrect_rratio);

    // (layers ...)
    let layers_node = node.find("layers");
    let layers = layers_node.map(|n| parse_layers(n)).unwrap_or_default();

    // (drill ...)
    let drill = node.find("drill").and_then(|n| parse_drill(n));

    Some(PadDef {
        number,
        pad_type,
        shape,
        position,
        rotation,
        layers,
        drill,
    })
}

/// Extract stroke width from a `(stroke (width W) ...)` child, falling
/// back to a `(width W)` child for older format.
fn extract_stroke_width(node: &SExpr<'_>) -> f64 {
    // New format: (stroke (width W) ...)
    if let Some(stroke) = node.find("stroke") {
        if let Some(w) = stroke.find("width").and_then(|n| child_f64(n, 1)) {
            return w;
        }
    }
    // Old format: (width W) directly on the element
    node.find("width")
        .and_then(|n| child_f64(n, 1))
        .unwrap_or(0.0)
}

/// Extract layer string from a `(layer "NAME")` child.
fn extract_layer(node: &SExpr<'_>) -> String {
    node.find("layer")
        .and_then(|n| child_str(n, 1))
        .unwrap_or("F.SilkS")
        .to_string()
}

/// Parse a graphical element (fp_line, fp_circle, fp_arc, fp_rect, fp_poly, fp_text).
fn parse_graphic(node: &SExpr<'_>) -> Option<GraphicDef> {
    match node.tag_name()? {
        "fp_line" => {
            let start = node.find("start").and_then(|n| parse_point(n))?;
            let end = node.find("end").and_then(|n| parse_point(n))?;
            let width = extract_stroke_width(node);
            let layer = extract_layer(node);
            Some(GraphicDef::Line {
                start,
                end,
                width,
                layer,
            })
        }
        "fp_circle" => {
            let center = node.find("center").and_then(|n| parse_point(n))?;
            let end = node.find("end").and_then(|n| parse_point(n))?;
            let width = extract_stroke_width(node);
            let layer = extract_layer(node);
            Some(GraphicDef::Circle {
                center,
                end,
                width,
                layer,
            })
        }
        "fp_arc" => {
            let start = node.find("start").and_then(|n| parse_point(n))?;
            let mid = node.find("mid").and_then(|n| parse_point(n))?;
            let end = node.find("end").and_then(|n| parse_point(n))?;
            let width = extract_stroke_width(node);
            let layer = extract_layer(node);
            Some(GraphicDef::Arc {
                start,
                mid,
                end,
                width,
                layer,
            })
        }
        "fp_rect" => {
            let start = node.find("start").and_then(|n| parse_point(n))?;
            let end = node.find("end").and_then(|n| parse_point(n))?;
            let width = extract_stroke_width(node);
            let layer = extract_layer(node);
            Some(GraphicDef::Rect {
                start,
                end,
                width,
                layer,
            })
        }
        "fp_poly" => {
            let pts_node = node.find("pts")?;
            let points: Vec<(f64, f64)> = pts_node
                .find_all("xy")
                .into_iter()
                .filter_map(|xy| parse_xy(xy))
                .collect();
            if points.is_empty() {
                return None;
            }
            let width = extract_stroke_width(node);
            let layer = extract_layer(node);
            Some(GraphicDef::Poly {
                points,
                width,
                layer,
            })
        }
        "fp_text" => {
            let text_type = child_str(node, 1)?.to_string();
            let content = child_str(node, 2).unwrap_or("").to_string();
            let at_node = node.find("at")?;
            let (position, _) = parse_at(at_node)?;
            let layer = extract_layer(node);
            Some(GraphicDef::Text {
                text_type,
                content,
                position,
                layer,
            })
        }
        _ => None,
    }
}

/// Parse a single `.kicad_mod` file (one footprint) from a string.
fn parse_single_footprint(node: &SExpr<'_>) -> Option<FootprintDef> {
    // Root is either (footprint ...) or (module ...) for older KiCad versions
    let tag = node.tag_name()?;
    if tag != "footprint" && tag != "module" {
        return None;
    }
    let name = child_str(node, 1)?.to_string();

    let pads: Vec<PadDef> = node
        .find_all("pad")
        .into_iter()
        .filter_map(|n| parse_pad(n))
        .collect();

    let graphic_tags = [
        "fp_line",
        "fp_circle",
        "fp_arc",
        "fp_rect",
        "fp_poly",
        "fp_text",
    ];
    let mut graphics = Vec::new();
    if let Some(children) = node.children() {
        for child in children {
            if let Some(tag) = child.tag_name() {
                if graphic_tags.contains(&tag) {
                    if let Some(g) = parse_graphic(child) {
                        graphics.push(g);
                    }
                }
            }
        }
    }

    // (model "path" ...)
    let model_3d = node
        .find("model")
        .and_then(|n| child_str(n, 1))
        .map(|s| s.to_string());

    Some(FootprintDef {
        name,
        pads,
        graphics,
        model_3d,
    })
}

/// Parse a `.kicad_mod` file contents into a [`FootprintLib`] with one footprint.
///
/// A `.kicad_mod` file contains a single footprint. To build a multi-footprint
/// library, parse multiple files and combine.
///
/// # Errors
///
/// Returns [`ParseError`] if the input is not valid KiCad S-expression format
/// or does not start with a `footprint` root node.
pub fn parse_footprint_lib(input: &str) -> Result<FootprintLib, ParseError> {
    let (rest, root) = parse_sexpr(input)?;
    if !rest.trim().is_empty() {
        return Err(ParseError::TrailingInput);
    }

    let footprint = parse_single_footprint(&root)
        .ok_or_else(|| ParseError::Nom("expected root node 'footprint' or 'module'".to_string()))?;

    Ok(FootprintLib {
        footprints: vec![footprint],
    })
}

/// Parse a `.kicad_mod` file from disk.
///
/// # Errors
///
/// Returns [`ParseError`] on I/O or parse failure.
pub fn parse_footprint_file(path: &str) -> Result<FootprintLib, ParseError> {
    let contents = std::fs::read_to_string(path)?;
    parse_footprint_lib(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMD_RESISTOR: &str = r#"(footprint "R_0805_2012Metric" (version 20211014) (generator pcbnew)
  (layer "F.Cu")
  (fp_text reference "REF**" (at 0 -1.65) (layer "F.SilkS")
    (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value "R_0805_2012Metric" (at 0 1.65) (layer "F.Fab")
    (effects (font (size 1 1) (thickness 0.15))))
  (fp_line (start -0.261252 -0.735) (end 0.261252 -0.735)
    (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (fp_line (start -0.261252 0.735) (end 0.261252 0.735)
    (stroke (width 0.12) (type solid)) (layer "F.SilkS"))
  (fp_line (start -1.68 -0.95) (end 1.68 -0.95)
    (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
  (fp_line (start -1.68 0.95) (end 1.68 0.95)
    (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
  (fp_line (start -1.68 -0.95) (end -1.68 0.95)
    (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
  (fp_line (start 1.68 -0.95) (end 1.68 0.95)
    (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
  (pad "1" smd roundrect (at -0.9375 0) (size 0.975 1.4)
    (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25))
  (pad "2" smd roundrect (at 0.9375 0) (size 0.975 1.4)
    (layers "F.Cu" "F.Paste" "F.Mask") (roundrect_rratio 0.25))
  (model "${KICAD6_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0805_2012Metric.wrl"
    (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))
)"#;

    #[test]
    fn parse_smd_resistor() {
        let lib = parse_footprint_lib(SMD_RESISTOR).unwrap();
        assert_eq!(lib.footprints.len(), 1);

        let fp = &lib.footprints[0];
        assert_eq!(fp.name, "R_0805_2012Metric");
        assert_eq!(fp.pads.len(), 2);
    }

    #[test]
    fn parse_pads() {
        let lib = parse_footprint_lib(SMD_RESISTOR).unwrap();
        let fp = &lib.footprints[0];

        let pad1 = &fp.pads[0];
        assert_eq!(pad1.number, "1");
        assert_eq!(pad1.pad_type, vcad_ir::ecad::PadType::SMD);
        assert!((pad1.position.0 - (-0.9375)).abs() < 1e-9);
        assert!((pad1.position.1).abs() < 1e-9);
        assert_eq!(pad1.rotation, 0.0);
        assert_eq!(pad1.layers, vec!["F.Cu", "F.Paste", "F.Mask"]);

        match &pad1.shape {
            vcad_ir::ecad::PadShape::RoundRect {
                width,
                height,
                corner_ratio,
            } => {
                assert!((width - 0.975).abs() < 1e-9);
                assert!((height - 1.4).abs() < 1e-9);
                assert!((corner_ratio - 0.25).abs() < 1e-9);
            }
            other => panic!("expected RoundRect, got {:?}", other),
        }

        let pad2 = &fp.pads[1];
        assert_eq!(pad2.number, "2");
        assert!((pad2.position.0 - 0.9375).abs() < 1e-9);
    }

    #[test]
    fn parse_graphics() {
        let lib = parse_footprint_lib(SMD_RESISTOR).unwrap();
        let fp = &lib.footprints[0];

        // 6 fp_line + 2 fp_text = 8 graphics
        assert_eq!(fp.graphics.len(), 8);

        // First two are fp_text (reference and value)
        match &fp.graphics[0] {
            GraphicDef::Text {
                text_type,
                content,
                layer,
                ..
            } => {
                assert_eq!(text_type, "reference");
                assert_eq!(content, "REF**");
                assert_eq!(layer, "F.SilkS");
            }
            other => panic!("expected Text, got {:?}", other),
        }

        // Lines
        let lines: Vec<_> = fp
            .graphics
            .iter()
            .filter(|g| matches!(g, GraphicDef::Line { .. }))
            .collect();
        assert_eq!(lines.len(), 6);

        // Check silkscreen line
        match &lines[0] {
            GraphicDef::Line {
                start,
                end,
                width,
                layer,
            } => {
                assert!((start.0 - (-0.261252)).abs() < 1e-6);
                assert!((start.1 - (-0.735)).abs() < 1e-6);
                assert!((end.0 - 0.261252).abs() < 1e-6);
                assert!((width - 0.12).abs() < 1e-9);
                assert_eq!(layer, "F.SilkS");
            }
            other => panic!("expected Line, got {:?}", other),
        }
    }

    #[test]
    fn parse_3d_model() {
        let lib = parse_footprint_lib(SMD_RESISTOR).unwrap();
        let fp = &lib.footprints[0];

        assert_eq!(
            fp.model_3d.as_deref(),
            Some("${KICAD6_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0805_2012Metric.wrl")
        );
    }

    #[test]
    fn parse_tht_footprint() {
        let input = r#"(footprint "DIP-8_W7.62mm" (version 20211014) (generator pcbnew)
  (layer "F.Cu")
  (pad "1" thru_hole rect (at 0 0) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "2" thru_hole circle (at 2.54 0) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "3" thru_hole circle (at 5.08 0) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "4" thru_hole circle (at 7.62 0) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "5" thru_hole circle (at 7.62 7.62) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "6" thru_hole circle (at 5.08 7.62) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "7" thru_hole circle (at 2.54 7.62) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
  (pad "8" thru_hole circle (at 0 7.62) (size 1.6 1.6) (drill 0.8) (layers "*.Cu" "*.Mask"))
)"#;

        let lib = parse_footprint_lib(input).unwrap();
        let fp = &lib.footprints[0];
        assert_eq!(fp.name, "DIP-8_W7.62mm");
        assert_eq!(fp.pads.len(), 8);

        // Pin 1 is rectangular
        let pin1 = &fp.pads[0];
        assert_eq!(pin1.pad_type, vcad_ir::ecad::PadType::THT);
        match &pin1.shape {
            vcad_ir::ecad::PadShape::Rect { width, height } => {
                assert!((width - 1.6).abs() < 1e-9);
                assert!((height - 1.6).abs() < 1e-9);
            }
            other => panic!("expected Rect, got {:?}", other),
        }

        // Drill
        let drill = pin1.drill.as_ref().unwrap();
        assert!((drill.diameter - 0.8).abs() < 1e-9);
        assert!(!drill.oval);

        // Pin 2 is circular
        match &fp.pads[1].shape {
            vcad_ir::ecad::PadShape::Circle { diameter } => {
                assert!((diameter - 1.6).abs() < 1e-9);
            }
            other => panic!("expected Circle, got {:?}", other),
        }
    }

    #[test]
    fn parse_oval_drill() {
        let input = r#"(footprint "Connector" (version 20211014) (generator pcbnew)
  (layer "F.Cu")
  (pad "1" thru_hole oval (at 0 0) (size 2.0 1.5) (drill oval 1.0 0.5) (layers "*.Cu" "*.Mask"))
)"#;

        let lib = parse_footprint_lib(input).unwrap();
        let pad = &lib.footprints[0].pads[0];
        let drill = pad.drill.as_ref().unwrap();
        assert!(drill.oval);
        assert!((drill.diameter - 1.0).abs() < 1e-9);
        assert_eq!(drill.oval_height, Some(0.5));
    }

    #[test]
    fn parse_no_model() {
        let input = r#"(footprint "Test" (version 20211014) (generator pcbnew)
  (layer "F.Cu")
  (pad "1" smd rect (at 0 0) (size 1.0 1.0) (layers "F.Cu"))
)"#;

        let lib = parse_footprint_lib(input).unwrap();
        assert!(lib.footprints[0].model_3d.is_none());
    }

    #[test]
    fn reject_invalid_root() {
        let input = r#"(not_a_footprint "test")"#;
        let err = parse_footprint_lib(input);
        assert!(err.is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let lib = parse_footprint_lib(SMD_RESISTOR).unwrap();
        let json = serde_json::to_string(&lib).unwrap();
        let restored: FootprintLib = serde_json::from_str(&json).unwrap();
        assert_eq!(lib, restored);
    }
}
