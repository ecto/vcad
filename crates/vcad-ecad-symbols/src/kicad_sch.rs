//! Parser for KiCad `.kicad_sch` schematic files.
//!
//! Converts a `.kicad_sch` S-expression file (KiCad 9 format, `version
//! 20250114` — the format [`crate::kicad_write::write_kicad_sch`] emits) into a
//! [`vcad_ir::ecad::SchematicSheet`].  Reuses the `sexpr`
//! S-expression parser and, like [`crate::kicad_pcb`], tolerates unknown
//! tokens by skipping them.
//!
//! Component pins are reconstructed from the `lib_symbols` pin definitions
//! (number, name, electrical type, position) attached to each instance's
//! `lib_id`; pin positions are carried verbatim in symbol-local coordinates,
//! exactly as the writer emits them.
//!
//! `sheet.nets` is reconstructed only in the straightforward case: global
//! labels whose position coincides with an unrotated, unmirrored component's
//! pin — directly or through a chain of wire segments.  Pin sheet positions
//! for rotated or mirrored instances depend on KiCad's symbol transform
//! (Y-up symbol frame inside the Y-down sheet), so those pins are left out of
//! reconstruction rather than guessed; when nothing can be reconstructed,
//! `nets` is `None`.

use std::collections::{BTreeMap, HashMap};

use vcad_ir::ecad::{
    LabelScope, PinType, SchematicComponent, SchematicJunction, SchematicLabel, SchematicPin,
    SchematicSheet, SchematicWire,
};
use vcad_ir::Vec2;

use crate::sexpr::{parse_sexpr, SExpr};
use crate::ParseError;

/// Parse a `.kicad_sch` file content into a [`SchematicSheet`].
pub fn parse_kicad_sch(input: &str) -> Result<SchematicSheet, ParseError> {
    let (rest, root) = parse_sexpr(input)?;
    if !rest.trim().is_empty() {
        return Err(ParseError::TrailingInput);
    }
    if root.tag_name() != Some("kicad_sch") {
        return Err(ParseError::Nom("expected (kicad_sch ...)".into()));
    }
    Ok(convert_sheet(&root))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Second element of a child list as a string: `(tag "value" ...)` → `value`.
fn child_str<'a>(node: &'a SExpr<'_>, tag: &str) -> Option<&'a str> {
    node.find(tag)
        .and_then(|n| n.children())
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str())
}

/// Inverse of the writer's `pin_type_token`.
fn parse_pin_type(s: &str) -> PinType {
    match s {
        "input" => PinType::Input,
        "output" => PinType::Output,
        "bidirectional" => PinType::Bidirectional,
        "tri_state" => PinType::TriState,
        "passive" => PinType::Passive,
        "power_in" => PinType::PowerInput,
        "power_out" => PinType::PowerOutput,
        "open_collector" => PinType::OpenCollector,
        "open_emitter" => PinType::OpenEmitter,
        "no_connect" => PinType::NotConnected,
        _ => PinType::Free,
    }
}

// ---------------------------------------------------------------------------
// lib_symbols — pin definitions per lib_id
// ---------------------------------------------------------------------------

/// Pin definitions (and default properties) parsed from one `lib_symbols`
/// entry.
struct LibSymbolDef {
    pins: Vec<SchematicPin>,
    value: Option<String>,
    footprint: Option<String>,
}

/// Collect `(pin ...)` definitions from a lib symbol, descending into the
/// nested unit sub-symbols (e.g. `"R_1_1"`).
fn collect_lib_pins(node: &SExpr<'_>, pins: &mut Vec<SchematicPin>) {
    for pin in node.find_all("pin") {
        let Some(c) = pin.children() else { continue };
        let pin_type = c
            .get(1)
            .and_then(|v| v.as_str())
            .map(parse_pin_type)
            .unwrap_or(PinType::Free);
        let (position, _angle) = read_at(pin);
        let name = child_str(pin, "name").unwrap_or("~").to_string();
        let number = child_str(pin, "number").unwrap_or("").to_string();
        pins.push(SchematicPin {
            number,
            name,
            pin_type,
            position,
        });
    }
    for sub in node.find_all("symbol") {
        collect_lib_pins(sub, pins);
    }
}

/// Parse the `(lib_symbols ...)` table: lib_id → pin definitions.
fn parse_lib_symbols(root: &SExpr<'_>) -> HashMap<String, LibSymbolDef> {
    let mut map = HashMap::new();
    let Some(lib) = root.find("lib_symbols") else {
        return map;
    };
    for sym in lib.find_all("symbol") {
        let Some(lib_id) = sym
            .children()
            .and_then(|c| c.get(1))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let mut pins = Vec::new();
        collect_lib_pins(sym, &mut pins);
        let mut value = None;
        let mut footprint = None;
        for prop in sym.find_all("property") {
            if let Some(c) = prop.children() {
                let key = c.get(1).and_then(|v| v.as_str()).unwrap_or("");
                let val = c.get(2).and_then(|v| v.as_str()).unwrap_or("");
                match key {
                    "Value" => value = Some(val.to_string()),
                    "Footprint" => footprint = Some(val.to_string()),
                    _ => {}
                }
            }
        }
        map.insert(
            lib_id.to_string(),
            LibSymbolDef {
                pins,
                value,
                footprint,
            },
        );
    }
    map
}

// ---------------------------------------------------------------------------
// Symbol instances
// ---------------------------------------------------------------------------

fn parse_symbol_instance(
    node: &SExpr<'_>,
    libs: &HashMap<String, LibSymbolDef>,
) -> Option<SchematicComponent> {
    let lib_id = child_str(node, "lib_id")?.to_string();
    let (position, rotation) = read_at(node);
    let mirror = node.find("mirror").is_some();
    let def = libs.get(&lib_id);

    let mut reference = String::new();
    let mut value = String::new();
    let mut footprint_id = String::new();
    let mut properties = std::collections::HashMap::new();
    for prop in node.find_all("property") {
        let Some(c) = prop.children() else { continue };
        let key = c.get(1).and_then(|v| v.as_str()).unwrap_or("");
        let val = c.get(2).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match key {
            "Reference" => reference = val,
            "Value" => value = val,
            "Footprint" => footprint_id = val,
            "Datasheet" | "" => {}
            other => {
                properties.insert(other.to_string(), val);
            }
        }
    }
    if value.is_empty() {
        if let Some(v) = def.and_then(|d| d.value.clone()) {
            value = v;
        }
    }
    if footprint_id.is_empty() {
        if let Some(f) = def.and_then(|d| d.footprint.clone()) {
            footprint_id = f;
        }
    }
    if reference.is_empty() {
        reference = lib_id;
    }

    Some(SchematicComponent {
        reference,
        value,
        footprint_id,
        position,
        rotation,
        mirror,
        pins: def.map(|d| d.pins.clone()).unwrap_or_default(),
        pads_override: None,
        properties,
    })
}

// ---------------------------------------------------------------------------
// Wires, junctions, labels
// ---------------------------------------------------------------------------

fn parse_wire(node: &SExpr<'_>) -> Option<SchematicWire> {
    let pts = node.find("pts")?;
    let xys = pts.find_all("xy");
    let point = |xy: &SExpr<'_>| {
        let c = xy.children()?;
        Some(Vec2::new(
            c.get(1).and_then(|v| v.as_f64())?,
            c.get(2).and_then(|v| v.as_f64())?,
        ))
    };
    let start = point(xys.first()?)?;
    let end = point(xys.get(1)?)?;
    Some(SchematicWire { start, end })
}

fn parse_label(node: &SExpr<'_>, scope: LabelScope) -> Option<SchematicLabel> {
    let name = node
        .children()
        .and_then(|c| c.get(1))
        .and_then(|v| v.as_str())?
        .to_string();
    let (position, rotation) = read_at(node);
    Some(SchematicLabel {
        name,
        position,
        rotation,
        scope,
    })
}

// ---------------------------------------------------------------------------
// Net reconstruction
// ---------------------------------------------------------------------------

/// Coincidence tolerance for connectivity points (mm).
const EPS: f64 = 0.01;

/// Reconstruct `nets` from global labels connected to pin positions through
/// wire chains.  Only pins of unrotated, unmirrored components participate
/// (see module docs); clusters carrying zero or conflicting global label
/// names are skipped.  Returns `None` when nothing could be reconstructed.
fn reconstruct_nets(sheet: &SchematicSheet) -> Option<BTreeMap<String, Vec<String>>> {
    // Unique connectivity points with union-find over coincidence + wires.
    let mut points: Vec<Vec2> = Vec::new();
    let mut parent: Vec<usize> = Vec::new();
    let intern = |p: Vec2, points: &mut Vec<Vec2>, parent: &mut Vec<usize>| -> usize {
        for (i, q) in points.iter().enumerate() {
            if (q.x - p.x).abs() < EPS && (q.y - p.y).abs() < EPS {
                return i;
            }
        }
        points.push(p);
        parent.push(points.len() - 1);
        points.len() - 1
    };
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            let r = find(parent, parent[i]);
            parent[i] = r;
        }
        parent[i]
    }

    // Pins: `(cluster point, "REF.NUM")`. KiCad's symbol frame is Y-up inside
    // the Y-down sheet, so an unrotated pin's sheet position is pos + (px, -py).
    let mut pin_points: Vec<(usize, String)> = Vec::new();
    for comp in &sheet.components {
        if comp.rotation != 0.0 || comp.mirror {
            continue;
        }
        for pin in &comp.pins {
            let abs = Vec2::new(
                comp.position.x + pin.position.x,
                comp.position.y - pin.position.y,
            );
            let i = intern(abs, &mut points, &mut parent);
            pin_points.push((i, format!("{}.{}", comp.reference, pin.number)));
        }
    }

    // Wires merge their endpoints.
    for w in &sheet.wires {
        let a = intern(w.start, &mut points, &mut parent);
        let b = intern(w.end, &mut points, &mut parent);
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        parent[ra] = rb;
    }

    // Global labels name their cluster.
    let mut label_points: Vec<(usize, String)> = Vec::new();
    for l in &sheet.labels {
        if l.scope != LabelScope::Global {
            continue;
        }
        let i = intern(l.position, &mut points, &mut parent);
        label_points.push((i, l.name.clone()));
    }

    // Cluster → single label name (skip conflicts).
    let mut cluster_name: HashMap<usize, Option<String>> = HashMap::new();
    for (i, name) in &label_points {
        let r = find(&mut parent, *i);
        match cluster_name.entry(r).or_insert_with(|| Some(name.clone())) {
            Some(existing) if existing != name => {
                cluster_name.insert(r, None);
            }
            _ => {}
        }
    }

    let mut nets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, pin_ref) in &pin_points {
        let r = find(&mut parent, *i);
        if let Some(Some(name)) = cluster_name.get(&r) {
            nets.entry(name.clone()).or_default().push(pin_ref.clone());
        }
    }
    if nets.is_empty() {
        None
    } else {
        Some(nets)
    }
}

// ---------------------------------------------------------------------------
// Main converter
// ---------------------------------------------------------------------------

fn convert_sheet(root: &SExpr<'_>) -> SchematicSheet {
    let libs = parse_lib_symbols(root);

    let components: Vec<SchematicComponent> = root
        .find_all("symbol")
        .iter()
        .filter_map(|n| parse_symbol_instance(n, &libs))
        .collect();

    let wires: Vec<SchematicWire> = root
        .find_all("wire")
        .iter()
        .filter_map(|n| parse_wire(n))
        .collect();

    let junctions: Vec<SchematicJunction> = root
        .find_all("junction")
        .iter()
        .map(|n| SchematicJunction {
            position: read_at(n).0,
        })
        .collect();

    let mut labels: Vec<SchematicLabel> = Vec::new();
    for (tag, scope) in [
        ("label", LabelScope::Local),
        ("global_label", LabelScope::Global),
        ("hierarchical_label", LabelScope::Hierarchical),
    ] {
        for n in root.find_all(tag) {
            if let Some(l) = parse_label(n, scope) {
                labels.push(l);
            }
        }
    }

    let title = root
        .find("title_block")
        .and_then(|tb| child_str(tb, "title"))
        .map(|s| s.to_string());

    let mut sheet = SchematicSheet {
        title,
        components,
        wires,
        junctions,
        labels,
        nets: None,
    };
    sheet.nets = reconstruct_nets(&sheet);
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kicad_write::write_kicad_sch;

    fn sample_sheet() -> SchematicSheet {
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

    /// (a) write sample_sheet → parse → structural equality on
    /// components/pins/wires/labels.
    #[test]
    fn sample_sheet_round_trips() {
        let sheet = sample_sheet();
        let text = write_kicad_sch(&sheet);
        let reparsed = parse_kicad_sch(&text).expect("re-parse exported schematic");

        assert_eq!(reparsed.components.len(), sheet.components.len());
        for (a, b) in sheet.components.iter().zip(&reparsed.components) {
            assert_eq!(a.reference, b.reference);
            assert_eq!(a.value, b.value);
            assert_eq!(a.footprint_id, b.footprint_id);
            assert_eq!(a.position, b.position);
            assert_eq!(a.rotation, b.rotation);
            assert_eq!(a.mirror, b.mirror);
            assert_eq!(a.pins, b.pins);
        }
        assert_eq!(reparsed.wires, sheet.wires);
        assert_eq!(reparsed.junctions, sheet.junctions);
        assert_eq!(reparsed.labels, sheet.labels);

        // The VCC global label sits on R1 pin 1; the wire from R1 pin 2 leads
        // to C1's body center but C1 is rotated, so only R1.1 is recovered.
        assert_eq!(
            reparsed.nets,
            Some(BTreeMap::from([(
                "VCC".to_string(),
                vec!["R1.1".to_string()]
            )]))
        );
    }

    /// (b) parse a hand-written real KiCad 9 snippet → write → parse; the
    /// structure is stable and the export is a fixpoint (byte-identical
    /// second write).
    #[test]
    fn import_export_reimport_is_stable() {
        let input = r#"(kicad_sch
  (version 20250114)
  (generator "eeschema")
  (generator_version "9.0")
  (uuid "e63e39d7-6ac0-4ffd-8aa3-1841a4541b55")
  (paper "A4")
  (title_block
    (title "test sheet")
  )
  (lib_symbols
    (symbol "Device:R"
      (pin_numbers
        (hide yes)
      )
      (pin_names
        (offset 0)
      )
      (exclude_from_sim no)
      (in_bom yes)
      (on_board yes)
      (property "Reference" "R"
        (at 2.032 0 90)
        (effects
          (font
            (size 1.27 1.27)
          )
        )
      )
      (property "Value" "R"
        (at 0 0 90)
        (effects
          (font
            (size 1.27 1.27)
          )
        )
      )
      (symbol "R_0_1"
        (rectangle
          (start -1.016 -2.54)
          (end 1.016 2.54)
          (stroke
            (width 0.254)
            (type default)
          )
          (fill
            (type none)
          )
        )
      )
      (symbol "R_1_1"
        (pin passive line
          (at 0 3.81 270)
          (length 1.27)
          (name "~"
            (effects
              (font
                (size 1.27 1.27)
              )
            )
          )
          (number "1"
            (effects
              (font
                (size 1.27 1.27)
              )
            )
          )
        )
        (pin passive line
          (at 0 -3.81 90)
          (length 1.27)
          (name "~"
            (effects
              (font
                (size 1.27 1.27)
              )
            )
          )
          (number "2"
            (effects
              (font
                (size 1.27 1.27)
              )
            )
          )
        )
      )
    )
  )
  (symbol
    (lib_id "Device:R")
    (at 100 50 0)
    (unit 1)
    (exclude_from_sim no)
    (in_bom yes)
    (on_board yes)
    (dnp no)
    (uuid "0b6da9d3-b4a5-46bc-9c2c-761933a5d5f6")
    (property "Reference" "R1"
      (at 102.54 48.9 0)
      (effects
        (font
          (size 1.27 1.27)
        )
      )
    )
    (property "Value" "10k"
      (at 102.54 51.4 0)
      (effects
        (font
          (size 1.27 1.27)
        )
      )
    )
    (property "Footprint" "Resistor_SMD:R_0805_2012Metric"
      (at 98.222 50 90)
      (effects
        (font
          (size 1.27 1.27)
        )
        (hide yes)
      )
    )
    (pin "1"
      (uuid "6a1c39ec-58f1-40a3-8802-3c0b678d5aaf")
    )
    (pin "2"
      (uuid "d61693e4-13cb-4b0d-8f3e-b40cf42e10a3")
    )
    (instances
      (project "test"
        (path "/e63e39d7-6ac0-4ffd-8aa3-1841a4541b55"
          (reference "R1")
          (unit 1)
        )
      )
    )
  )
  (wire
    (pts
      (xy 100 46.19)
      (xy 100 40)
    )
    (stroke
      (width 0)
      (type default)
    )
    (uuid "8a05e04f-6247-4b1e-84a2-8c9d1a35d3f2")
  )
  (junction
    (at 100 40)
    (diameter 0)
    (color 0 0 0 0)
    (uuid "3e5b0d54-9a51-4a52-a4c8-6b8cf62fbc74")
  )
  (global_label "VCC"
    (shape input)
    (at 100 40 90)
    (effects
      (font
        (size 1.27 1.27)
      )
    )
    (uuid "bd4b6d1f-6a3d-45f5-9bc7-30df31be09aa")
  )
  (sheet_instances
    (path "/"
      (page "1")
    )
  )
)"#;

        let sheet1 = parse_kicad_sch(input).expect("import schematic");

        assert_eq!(sheet1.title.as_deref(), Some("test sheet"));
        assert_eq!(sheet1.components.len(), 1);
        let r1 = &sheet1.components[0];
        assert_eq!(r1.reference, "R1");
        assert_eq!(r1.value, "10k");
        assert_eq!(r1.footprint_id, "Resistor_SMD:R_0805_2012Metric");
        assert_eq!(r1.position, Vec2::new(100.0, 50.0));
        assert_eq!(r1.rotation, 0.0);
        assert!(!r1.mirror);
        assert_eq!(r1.pins.len(), 2);
        assert_eq!(r1.pins[0].number, "1");
        assert_eq!(r1.pins[0].name, "~");
        assert_eq!(r1.pins[0].pin_type, PinType::Passive);
        assert_eq!(r1.pins[0].position, Vec2::new(0.0, 3.81));
        assert_eq!(r1.pins[1].number, "2");
        assert_eq!(r1.pins[1].position, Vec2::new(0.0, -3.81));

        assert_eq!(sheet1.wires.len(), 1);
        assert_eq!(sheet1.wires[0].start, Vec2::new(100.0, 46.19));
        assert_eq!(sheet1.junctions.len(), 1);
        assert_eq!(sheet1.labels.len(), 1);
        assert_eq!(sheet1.labels[0].name, "VCC");
        assert_eq!(sheet1.labels[0].scope, LabelScope::Global);

        // R1 pin 1 sits at (100, 50 - 3.81) = (100, 46.19), wired down to the
        // VCC global label at (100, 40): the net is reconstructed.
        assert_eq!(
            sheet1.nets,
            Some(BTreeMap::from([(
                "VCC".to_string(),
                vec!["R1.1".to_string()]
            )]))
        );

        // Export → re-import → export: structure stable, second write byte-
        // identical (fixpoint).
        let exported = write_kicad_sch(&sheet1);
        let sheet2 = parse_kicad_sch(&exported).expect("re-import exported schematic");
        assert_eq!(sheet1.components.len(), sheet2.components.len());
        assert_eq!(
            sheet1.components[0].reference,
            sheet2.components[0].reference
        );
        assert_eq!(sheet1.components[0].value, sheet2.components[0].value);
        assert_eq!(sheet1.components[0].pins, sheet2.components[0].pins);
        assert_eq!(sheet1.wires, sheet2.wires);
        assert_eq!(sheet1.junctions, sheet2.junctions);
        assert_eq!(sheet1.labels, sheet2.labels);
        assert_eq!(exported, write_kicad_sch(&sheet2));
    }

    /// (c) a nets-declared sheet with degenerate pin geometry survives
    /// write → parse with refs, values, footprints, and label names intact.
    #[test]
    fn nets_flow_sheet_survives_round_trip() {
        let sheet = SchematicSheet {
            title: None,
            components: vec![
                SchematicComponent {
                    reference: "U1".into(),
                    value: "NE555".into(),
                    footprint_id: "Package_DIP:DIP-8_W7.62mm".into(),
                    position: Vec2::new(50.0, 50.0),
                    rotation: 0.0,
                    mirror: false,
                    pins: vec![
                        SchematicPin {
                            number: "1".into(),
                            name: "GND".into(),
                            pin_type: PinType::PowerInput,
                            position: Vec2::new(0.0, 0.0),
                        },
                        SchematicPin {
                            number: "8".into(),
                            name: "VCC".into(),
                            pin_type: PinType::PowerInput,
                            position: Vec2::new(0.0, 0.0),
                        },
                    ],
                    pads_override: None,
                    properties: std::collections::HashMap::new(),
                },
                SchematicComponent {
                    reference: "R1".into(),
                    value: "10k".into(),
                    footprint_id: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(80.0, 50.0),
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
                },
            ],
            wires: vec![],
            junctions: vec![],
            labels: vec![
                SchematicLabel {
                    name: "VCC".into(),
                    position: Vec2::new(50.0, 40.0),
                    rotation: 0.0,
                    scope: LabelScope::Global,
                },
                SchematicLabel {
                    name: "GND".into(),
                    position: Vec2::new(50.0, 60.0),
                    rotation: 0.0,
                    scope: LabelScope::Global,
                },
            ],
            nets: Some(BTreeMap::from([
                (
                    "VCC".to_string(),
                    vec!["U1.8".to_string(), "R1.1".to_string()],
                ),
                ("GND".to_string(), vec!["U1.1".to_string()]),
            ])),
        };

        let text = write_kicad_sch(&sheet);
        let reparsed = parse_kicad_sch(&text).expect("re-parse nets-flow sheet");

        assert_eq!(reparsed.components.len(), 2);
        for (a, b) in sheet.components.iter().zip(&reparsed.components) {
            assert_eq!(a.reference, b.reference);
            assert_eq!(a.value, b.value);
            assert_eq!(a.footprint_id, b.footprint_id);
            assert_eq!(a.pins.len(), b.pins.len());
            for (pa, pb) in a.pins.iter().zip(&b.pins) {
                assert_eq!(pa.number, pb.number);
                assert_eq!(pa.name, pb.name);
                assert_eq!(pa.pin_type, pb.pin_type);
            }
        }
        // Label names survive. The sheet's own two labels are joined by the
        // generated net stubs — the declared netlist has three pin refs whose
        // pins no drawn label touches, and emitting those is the whole point
        // of the nets flow — so assert on the distinct names, not the count.
        let names: std::collections::BTreeSet<&str> =
            reparsed.labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["VCC", "GND"]),
            "unexpected label names after round trip"
        );
        // Every declared net reached the file (the pre-fix exporter dropped
        // them silently, leaving KiCad with no netlist at all).
        for net in sheet.nets.as_ref().unwrap().keys() {
            assert!(
                reparsed.labels.iter().any(|l| &l.name == net),
                "declared net {net} has no label after round trip"
            );
        }
    }

    /// Unknown tokens are skipped, mirroring the PCB parser's tolerance.
    #[test]
    fn unknown_tokens_are_skipped() {
        let input = r#"(kicad_sch
  (version 20250114)
  (generator "eeschema")
  (embedded_fonts no)
  (future_token (nested stuff) 42)
  (text "free note" (at 10 10 0) (effects (font (size 1.27 1.27))))
  (label "NET_A" (at 20 20 0) (effects (font (size 1.27 1.27))))
  (hierarchical_label "H1" (shape input) (at 30 30 0))
)"#;
        let sheet = parse_kicad_sch(input).expect("parse with unknown tokens");
        assert!(sheet.components.is_empty());
        assert_eq!(sheet.labels.len(), 2);
        assert_eq!(sheet.labels[0].scope, LabelScope::Local);
        assert_eq!(sheet.labels[0].name, "NET_A");
        assert_eq!(sheet.labels[1].scope, LabelScope::Hierarchical);
        assert!(sheet.nets.is_none());
    }

    #[test]
    fn rejects_non_schematic() {
        assert!(parse_kicad_sch("(kicad_pcb (version 1))").is_err());
        assert!(parse_kicad_sch("(kicad_sch) trailing").is_err());
    }
}
