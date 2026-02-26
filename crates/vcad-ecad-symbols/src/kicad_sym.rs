//! Parser for KiCad `.kicad_sym` schematic symbol library files.
//!
//! The format is an S-expression tree rooted at `kicad_symbol_lib`. Each symbol
//! contains properties, graphical primitives, and pin definitions spread across
//! sub-symbols named `<Name>_<unit>_<style>`.

use serde::{Deserialize, Serialize};

use crate::sexpr::{parse_sexpr, SExpr};
use crate::{ParseError, Property};

/// A parsed KiCad symbol library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolLib {
    /// All symbols in this library.
    pub symbols: Vec<Symbol>,
}

/// A single schematic symbol definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    /// Symbol name (e.g. "R", "C", "ATmega328P").
    pub name: String,
    /// Symbol pins.
    pub pins: Vec<SymbolPin>,
    /// Symbol properties (Reference, Value, Footprint, Datasheet, etc.).
    pub properties: Vec<Property>,
    /// Graphical primitives (body outline).
    pub graphics: Vec<SymbolGraphic>,
}

/// A pin on a schematic symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolPin {
    /// Pin number (the physical designator, e.g. "1", "A3").
    pub number: String,
    /// Pin name (functional name, e.g. "VCC", "~" for unnamed).
    pub name: String,
    /// Electrical type.
    pub pin_type: vcad_ir::ecad::PinType,
    /// Position relative to symbol origin in mils.
    pub position: (f64, f64),
    /// Angle in degrees (0 = right, 90 = up, 180 = left, 270 = down).
    pub angle: f64,
    /// Pin length in mils.
    pub length: f64,
}

/// A graphical element in a symbol body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SymbolGraphic {
    /// A polyline (series of connected line segments).
    Polyline {
        /// Ordered points.
        points: Vec<(f64, f64)>,
    },
    /// A rectangle.
    Rectangle {
        /// Top-left corner.
        start: (f64, f64),
        /// Bottom-right corner.
        end: (f64, f64),
    },
    /// A circle.
    Circle {
        /// Center point.
        center: (f64, f64),
        /// Radius.
        radius: f64,
    },
    /// An arc.
    Arc {
        /// Start point.
        start: (f64, f64),
        /// Midpoint on the arc.
        mid: (f64, f64),
        /// End point.
        end: (f64, f64),
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

/// Parse KiCad pin electrical type keyword to vcad PinType.
fn parse_pin_type(s: &str) -> vcad_ir::ecad::PinType {
    match s {
        "input" => vcad_ir::ecad::PinType::Input,
        "output" => vcad_ir::ecad::PinType::Output,
        "bidirectional" => vcad_ir::ecad::PinType::Bidirectional,
        "tri_state" => vcad_ir::ecad::PinType::TriState,
        "passive" => vcad_ir::ecad::PinType::Passive,
        "power_in" => vcad_ir::ecad::PinType::PowerInput,
        "power_out" => vcad_ir::ecad::PinType::PowerOutput,
        "open_collector" => vcad_ir::ecad::PinType::OpenCollector,
        "open_emitter" => vcad_ir::ecad::PinType::OpenEmitter,
        "unconnected" | "no_connect" => vcad_ir::ecad::PinType::NotConnected,
        _ => vcad_ir::ecad::PinType::Free,
    }
}

/// Parse `(xy X Y)` into a tuple.
fn parse_xy(node: &SExpr<'_>) -> Option<(f64, f64)> {
    if node.tag_name() != Some("xy") {
        return None;
    }
    Some((child_f64(node, 1)?, child_f64(node, 2)?))
}

/// Parse `(at X Y [angle])` into position + angle.
fn parse_at(node: &SExpr<'_>) -> Option<(f64, f64, f64)> {
    if node.tag_name() != Some("at") {
        return None;
    }
    let x = child_f64(node, 1)?;
    let y = child_f64(node, 2)?;
    let angle = child_f64(node, 3).unwrap_or(0.0);
    Some((x, y, angle))
}

/// Parse a `(pts (xy ...) (xy ...) ...)` node into a vec of points.
fn parse_pts(node: &SExpr<'_>) -> Vec<(f64, f64)> {
    node.find_all("xy")
        .into_iter()
        .filter_map(|xy| parse_xy(xy))
        .collect()
}

/// Parse a `(property KEY VALUE (at X Y A) ...)` node.
fn parse_property(node: &SExpr<'_>) -> Option<Property> {
    if node.tag_name() != Some("property") {
        return None;
    }
    let key = child_str(node, 1)?.to_string();
    let value = child_str(node, 2).unwrap_or("").to_string();
    Some(Property { key, value })
}

/// Parse a `(pin TYPE STYLE (at X Y ANGLE) (length L) (name ...) (number ...))` node.
fn parse_pin(node: &SExpr<'_>) -> Option<SymbolPin> {
    if node.tag_name() != Some("pin") {
        return None;
    }
    let _children = node.children()?;
    // pin type is child[1], style is child[2]
    let pin_type_str = child_str(node, 1).unwrap_or("free");
    let pin_type = parse_pin_type(pin_type_str);

    // (at X Y ANGLE)
    let at_node = node.find("at")?;
    let (x, y, angle) = parse_at(at_node)?;

    // (length L)
    let length = node
        .find("length")
        .and_then(|n| child_f64(n, 1))
        .unwrap_or(2.54);

    // (name "NAME" ...)
    let name = node
        .find("name")
        .and_then(|n| child_str(n, 1))
        .unwrap_or("~")
        .to_string();

    // (number "NUM" ...)
    let number = node
        .find("number")
        .and_then(|n| child_str(n, 1))
        .unwrap_or("?")
        .to_string();

    Some(SymbolPin {
        number,
        name,
        pin_type,
        position: (x, y),
        angle,
        length,
    })
}

/// Parse a graphic element from a sub-symbol body.
fn parse_graphic(node: &SExpr<'_>) -> Option<SymbolGraphic> {
    match node.tag_name()? {
        "polyline" => {
            let pts_node = node.find("pts")?;
            let points = parse_pts(pts_node);
            if points.is_empty() {
                return None;
            }
            Some(SymbolGraphic::Polyline { points })
        }
        "rectangle" => {
            let start_node = node.find("start")?;
            let end_node = node.find("end")?;
            let start = (child_f64(start_node, 1)?, child_f64(start_node, 2)?);
            let end = (child_f64(end_node, 1)?, child_f64(end_node, 2)?);
            Some(SymbolGraphic::Rectangle { start, end })
        }
        "circle" => {
            let center_node = node.find("center")?;
            let radius_node = node.find("radius")?;
            let center = (child_f64(center_node, 1)?, child_f64(center_node, 2)?);
            let radius = child_f64(radius_node, 1)?;
            Some(SymbolGraphic::Circle { center, radius })
        }
        "arc" => {
            let start_node = node.find("start")?;
            let mid_node = node.find("mid")?;
            let end_node = node.find("end")?;
            let start = (child_f64(start_node, 1)?, child_f64(start_node, 2)?);
            let mid = (child_f64(mid_node, 1)?, child_f64(mid_node, 2)?);
            let end = (child_f64(end_node, 1)?, child_f64(end_node, 2)?);
            Some(SymbolGraphic::Arc { start, mid, end })
        }
        _ => None,
    }
}

/// Parse a single top-level `(symbol ...)` node into a [`Symbol`].
fn parse_symbol(node: &SExpr<'_>) -> Option<Symbol> {
    if node.tag_name() != Some("symbol") {
        return None;
    }
    let name = child_str(node, 1)?.to_string();

    let mut pins = Vec::new();
    let mut properties = Vec::new();
    let mut graphics = Vec::new();

    let children = node.children()?;
    for child in children.iter().skip(1) {
        match child.tag_name() {
            Some("property") => {
                if let Some(prop) = parse_property(child) {
                    properties.push(prop);
                }
            }
            Some("pin") => {
                if let Some(pin) = parse_pin(child) {
                    pins.push(pin);
                }
            }
            Some("symbol") => {
                // Sub-symbol: contains graphics and/or pins for a specific
                // unit/style combo (e.g. "R_0_1" for graphics, "R_1_1" for pins).
                if let Some(sub_children) = child.children() {
                    for sub_child in sub_children.iter().skip(1) {
                        match sub_child.tag_name() {
                            Some("pin") => {
                                if let Some(pin) = parse_pin(sub_child) {
                                    pins.push(pin);
                                }
                            }
                            Some("polyline") | Some("rectangle") | Some("circle") | Some("arc") => {
                                if let Some(g) = parse_graphic(sub_child) {
                                    graphics.push(g);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("polyline") | Some("rectangle") | Some("circle") | Some("arc") => {
                if let Some(g) = parse_graphic(child) {
                    graphics.push(g);
                }
            }
            _ => {}
        }
    }

    Some(Symbol {
        name,
        pins,
        properties,
        graphics,
    })
}

/// Parse a `.kicad_sym` file contents into a [`SymbolLib`].
///
/// # Errors
///
/// Returns [`ParseError`] if the input is not valid KiCad S-expression format
/// or does not start with a `kicad_symbol_lib` root node.
pub fn parse_symbol_lib(input: &str) -> Result<SymbolLib, ParseError> {
    let (rest, root) = parse_sexpr(input)?;
    if !rest.trim().is_empty() {
        return Err(ParseError::TrailingInput);
    }
    if root.tag_name() != Some("kicad_symbol_lib") {
        return Err(ParseError::Nom(
            "expected root node 'kicad_symbol_lib'".to_string(),
        ));
    }

    let symbols = root
        .find_all("symbol")
        .into_iter()
        .filter_map(|n| parse_symbol(n))
        .collect();

    Ok(SymbolLib { symbols })
}

/// Parse a `.kicad_sym` file from disk.
///
/// # Errors
///
/// Returns [`ParseError`] on I/O or parse failure.
pub fn parse_symbol_lib_file(path: &str) -> Result<SymbolLib, ParseError> {
    let contents = std::fs::read_to_string(path)?;
    parse_symbol_lib(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESISTOR_LIB: &str = r#"(kicad_symbol_lib (version 20211014) (generator kicad_symbol_editor)
  (symbol "R" (pin_names (offset 0)) (in_bom yes) (on_board yes)
    (property "Reference" "R" (at 2.032 0.508 0)
      (effects (font (size 1.27 1.27)) (justify left)))
    (property "Value" "R" (at 2.032 -1.016 0)
      (effects (font (size 1.27 1.27)) (justify left)))
    (property "Footprint" "" (at 0 0 0)
      (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "~" (at 0 0 0)
      (effects (font (size 1.27 1.27)) hide))
    (symbol "R_0_1"
      (polyline
        (pts (xy -1.016 -2.54) (xy -1.016 2.54))
        (stroke (width 0) (type default))
        (fill (type none)))
      (polyline
        (pts (xy 1.016 -2.54) (xy 1.016 2.54))
        (stroke (width 0) (type default))
        (fill (type none)))
    )
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "~" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 0 -3.81 90) (length 1.27)
        (name "~" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27)))))
    )
  )
)"#;

    #[test]
    fn parse_resistor_symbol() {
        let lib = parse_symbol_lib(RESISTOR_LIB).unwrap();
        assert_eq!(lib.symbols.len(), 1);

        let sym = &lib.symbols[0];
        assert_eq!(sym.name, "R");
        assert_eq!(sym.pins.len(), 2);
        assert_eq!(sym.properties.len(), 4);
        assert_eq!(sym.graphics.len(), 2);

        // Check properties
        assert_eq!(sym.properties[0].key, "Reference");
        assert_eq!(sym.properties[0].value, "R");
        assert_eq!(sym.properties[1].key, "Value");
        assert_eq!(sym.properties[1].value, "R");
        assert_eq!(sym.properties[2].key, "Footprint");
        assert_eq!(sym.properties[2].value, "");
        assert_eq!(sym.properties[3].key, "Datasheet");
        assert_eq!(sym.properties[3].value, "~");
    }

    #[test]
    fn parse_resistor_pins() {
        let lib = parse_symbol_lib(RESISTOR_LIB).unwrap();
        let sym = &lib.symbols[0];

        let pin1 = &sym.pins[0];
        assert_eq!(pin1.number, "1");
        assert_eq!(pin1.name, "~");
        assert_eq!(pin1.pin_type, vcad_ir::ecad::PinType::Passive);
        assert!((pin1.position.0 - 0.0).abs() < 1e-9);
        assert!((pin1.position.1 - 3.81).abs() < 1e-9);
        assert!((pin1.angle - 270.0).abs() < 1e-9);
        assert!((pin1.length - 1.27).abs() < 1e-9);

        let pin2 = &sym.pins[1];
        assert_eq!(pin2.number, "2");
        assert!((pin2.position.1 - (-3.81)).abs() < 1e-9);
        assert!((pin2.angle - 90.0).abs() < 1e-9);
    }

    #[test]
    fn parse_resistor_graphics() {
        let lib = parse_symbol_lib(RESISTOR_LIB).unwrap();
        let sym = &lib.symbols[0];

        // Two polylines for the resistor body
        assert_eq!(sym.graphics.len(), 2);
        match &sym.graphics[0] {
            SymbolGraphic::Polyline { points } => {
                assert_eq!(points.len(), 2);
                assert!((points[0].0 - (-1.016)).abs() < 1e-9);
                assert!((points[0].1 - (-2.54)).abs() < 1e-9);
                assert!((points[1].0 - (-1.016)).abs() < 1e-9);
                assert!((points[1].1 - 2.54).abs() < 1e-9);
            }
            other => panic!("expected polyline, got {:?}", other),
        }
    }

    #[test]
    fn parse_rectangle_symbol() {
        let input = r#"(kicad_symbol_lib (version 20211014) (generator test)
  (symbol "IC1" (in_bom yes) (on_board yes)
    (property "Reference" "U" (at 0 0 0))
    (symbol "IC1_0_1"
      (rectangle (start -5.08 5.08) (end 5.08 -5.08)
        (stroke (width 0.254) (type default))
        (fill (type background))))
    (symbol "IC1_1_1"
      (pin input line (at -10.16 2.54 0) (length 5.08)
        (name "IN" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin output line (at 10.16 2.54 180) (length 5.08)
        (name "OUT" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 10.16 270) (length 5.08)
        (name "VCC" (effects (font (size 1.27 1.27))))
        (number "3" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 -10.16 90) (length 5.08)
        (name "GND" (effects (font (size 1.27 1.27))))
        (number "4" (effects (font (size 1.27 1.27)))))
    )
  )
)"#;

        let lib = parse_symbol_lib(input).unwrap();
        assert_eq!(lib.symbols.len(), 1);
        let sym = &lib.symbols[0];
        assert_eq!(sym.name, "IC1");
        assert_eq!(sym.pins.len(), 4);

        // Rectangle graphic
        assert_eq!(sym.graphics.len(), 1);
        match &sym.graphics[0] {
            SymbolGraphic::Rectangle { start, end } => {
                assert!((start.0 - (-5.08)).abs() < 1e-9);
                assert!((start.1 - 5.08).abs() < 1e-9);
                assert!((end.0 - 5.08).abs() < 1e-9);
                assert!((end.1 - (-5.08)).abs() < 1e-9);
            }
            other => panic!("expected rectangle, got {:?}", other),
        }

        // Pin types
        assert_eq!(sym.pins[0].pin_type, vcad_ir::ecad::PinType::Input);
        assert_eq!(sym.pins[0].name, "IN");
        assert_eq!(sym.pins[1].pin_type, vcad_ir::ecad::PinType::Output);
        assert_eq!(sym.pins[1].name, "OUT");
        assert_eq!(sym.pins[2].pin_type, vcad_ir::ecad::PinType::PowerInput);
        assert_eq!(sym.pins[2].name, "VCC");
        assert_eq!(sym.pins[3].pin_type, vcad_ir::ecad::PinType::PowerInput);
        assert_eq!(sym.pins[3].name, "GND");
    }

    #[test]
    fn parse_circle_symbol() {
        let input = r#"(kicad_symbol_lib (version 20211014) (generator test)
  (symbol "LED" (in_bom yes) (on_board yes)
    (property "Reference" "D" (at 0 0 0))
    (symbol "LED_0_1"
      (circle (center 0 0) (radius 2.54)
        (stroke (width 0.254) (type default))
        (fill (type none))))
    (symbol "LED_1_1"
      (pin passive line (at -3.81 0 0) (length 1.27)
        (name "A" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 3.81 0 180) (length 1.27)
        (name "K" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27)))))
    )
  )
)"#;

        let lib = parse_symbol_lib(input).unwrap();
        let sym = &lib.symbols[0];
        assert_eq!(sym.name, "LED");
        assert_eq!(sym.graphics.len(), 1);
        match &sym.graphics[0] {
            SymbolGraphic::Circle { center, radius } => {
                assert!((center.0).abs() < 1e-9);
                assert!((center.1).abs() < 1e-9);
                assert!((radius - 2.54).abs() < 1e-9);
            }
            other => panic!("expected circle, got {:?}", other),
        }
    }

    #[test]
    fn parse_arc_symbol() {
        let input = r#"(kicad_symbol_lib (version 20211014) (generator test)
  (symbol "ARC_TEST" (in_bom yes) (on_board yes)
    (property "Reference" "X" (at 0 0 0))
    (symbol "ARC_TEST_0_1"
      (arc (start 0 -2.54) (mid 2.54 0) (end 0 2.54)
        (stroke (width 0) (type default))
        (fill (type none))))
  )
)"#;

        let lib = parse_symbol_lib(input).unwrap();
        let sym = &lib.symbols[0];
        assert_eq!(sym.graphics.len(), 1);
        match &sym.graphics[0] {
            SymbolGraphic::Arc { start, mid, end } => {
                assert!((start.0).abs() < 1e-9);
                assert!((start.1 - (-2.54)).abs() < 1e-9);
                assert!((mid.0 - 2.54).abs() < 1e-9);
                assert!((mid.1).abs() < 1e-9);
                assert!((end.0).abs() < 1e-9);
                assert!((end.1 - 2.54).abs() < 1e-9);
            }
            other => panic!("expected arc, got {:?}", other),
        }
    }

    #[test]
    fn parse_empty_lib() {
        let input = r#"(kicad_symbol_lib (version 20211014) (generator test))"#;
        let lib = parse_symbol_lib(input).unwrap();
        assert!(lib.symbols.is_empty());
    }

    #[test]
    fn parse_multi_symbol_lib() {
        let input = r#"(kicad_symbol_lib (version 20211014) (generator test)
  (symbol "R" (in_bom yes) (on_board yes)
    (property "Reference" "R" (at 0 0 0))
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "~" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
    )
  )
  (symbol "C" (in_bom yes) (on_board yes)
    (property "Reference" "C" (at 0 0 0))
    (symbol "C_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "~" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
    )
  )
)"#;
        let lib = parse_symbol_lib(input).unwrap();
        assert_eq!(lib.symbols.len(), 2);
        assert_eq!(lib.symbols[0].name, "R");
        assert_eq!(lib.symbols[1].name, "C");
    }

    #[test]
    fn reject_invalid_root() {
        let input = r#"(not_a_symbol_lib (version 1))"#;
        let err = parse_symbol_lib(input);
        assert!(err.is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let lib = parse_symbol_lib(RESISTOR_LIB).unwrap();
        let json = serde_json::to_string(&lib).unwrap();
        let restored: SymbolLib = serde_json::from_str(&json).unwrap();
        assert_eq!(lib, restored);
    }
}
