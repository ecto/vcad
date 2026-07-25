//! Excellon drill file writer.
//!
//! Generates an Excellon NC drill file (`.drl`) from PCB data. Holes are
//! collected from through-hole pads (THT/NPTH) and vias, grouped by drill
//! diameter, and assigned sequential tool numbers.

use std::collections::BTreeMap;
use std::io::Write;

use vcad_ir::ecad::*;

/// Errors that can occur during Excellon generation.
#[derive(Debug, thiserror::Error)]
pub enum ExcellonError {
    /// An I/O error occurred while writing.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The PCB contains no drill holes.
    #[error("no drill holes found in PCB")]
    NoDrills,
}

/// A via layer span, normalized so the shallower (more front-ward) copper
/// layer comes first. `(FCu, BCu)` is a through-hole.
pub type DrillSpan = (PcbLayer, PcbLayer);

/// The through-hole span: front copper to back copper.
pub const THROUGH_SPAN: DrillSpan = (PcbLayer::FCu, PcbLayer::BCu);

/// Order a via's endpoints outer-first so spans compare and name consistently.
fn normalize_span(start: PcbLayer, end: PcbLayer) -> DrillSpan {
    let a = start.copper_position().unwrap_or(0);
    let b = end.copper_position().unwrap_or(u8::MAX);
    if a <= b {
        (start, end)
    } else {
        (end, start)
    }
}

/// Excellon filename for a drill span, following the KiCad convention:
/// `drill.drl` for through-holes, `drill-In1_Cu-In2_Cu.drl` for a buried span.
fn span_filename(span: DrillSpan) -> String {
    if span == THROUGH_SPAN {
        return "drill.drl".into();
    }
    format!("drill-{}-{}.drl", layer_token(span.0), layer_token(span.1))
}

fn layer_token(layer: PcbLayer) -> &'static str {
    match layer {
        PcbLayer::FCu => "F_Cu",
        PcbLayer::BCu => "B_Cu",
        PcbLayer::In1Cu => "In1_Cu",
        PcbLayer::In2Cu => "In2_Cu",
        PcbLayer::In3Cu => "In3_Cu",
        PcbLayer::In4Cu => "In4_Cu",
        PcbLayer::In5Cu => "In5_Cu",
        PcbLayer::In6Cu => "In6_Cu",
        PcbLayer::In7Cu => "In7_Cu",
        PcbLayer::In8Cu => "In8_Cu",
        _ => "Unknown",
    }
}

/// Every drill span present on the board, through-hole first, then blind and
/// buried spans in stack order. The through span is always included — pad
/// drills live there, and consumers expect a `drill.drl` to exist.
fn spans(pcb: &Pcb) -> Vec<DrillSpan> {
    let mut set: std::collections::BTreeSet<(u8, u8)> = Default::default();
    for via in &pcb.vias {
        let (a, b) = normalize_span(via.start_layer, via.end_layer);
        set.insert((
            a.copper_position().unwrap_or(0),
            b.copper_position().unwrap_or(u8::MAX),
        ));
    }
    set.remove(&(0, u8::MAX));

    // Through-hole first, then blind/buried spans in stack order. The through
    // file is always emitted, even on an all-SMD board: consumers expect a
    // `drill.drl` to exist, and an empty one is an honest answer.
    std::iter::once(THROUGH_SPAN)
        .chain(set.into_iter().map(|(a, b)| (layer_at(a), layer_at(b))))
        .collect()
}

fn layer_at(position: u8) -> PcbLayer {
    match position {
        0 => PcbLayer::FCu,
        1 => PcbLayer::In1Cu,
        2 => PcbLayer::In2Cu,
        3 => PcbLayer::In3Cu,
        4 => PcbLayer::In4Cu,
        5 => PcbLayer::In5Cu,
        6 => PcbLayer::In6Cu,
        7 => PcbLayer::In7Cu,
        8 => PcbLayer::In8Cu,
        _ => PcbLayer::BCu,
    }
}

/// A single drill hit (hole location).
#[derive(Debug, Clone)]
struct DrillHit {
    x: f64,
    y: f64,
}

/// Collect all drill holes from the PCB grouped by diameter.
///
/// Returns a `BTreeMap` so tools are ordered by ascending diameter. The key is
/// the diameter in mm rounded to 4 decimal places (encoded as an integer in
/// units of 0.0001 mm for exact comparison).
fn collect_holes(pcb: &Pcb, span: DrillSpan) -> BTreeMap<i64, Vec<DrillHit>> {
    let mut holes: BTreeMap<i64, Vec<DrillHit>> = BTreeMap::new();

    // Holes from footprint pads. Pad drills are always through-holes, so they
    // only belong in the through-hole file.
    for fp in &pcb.footprints {
        if span != THROUGH_SPAN {
            break;
        }
        let cos_r = fp.rotation.to_radians().cos();
        let sin_r = fp.rotation.to_radians().sin();

        for pad in &fp.pads {
            if let Some(ref drill) = pad.drill {
                let lx = pad.position.x * cos_r - pad.position.y * sin_r;
                let ly = pad.position.x * sin_r + pad.position.y * cos_r;
                let x = fp.position.x + lx;
                let y = fp.position.y + ly;
                let key = (drill.diameter * 10_000.0).round() as i64;
                holes.entry(key).or_default().push(DrillHit { x, y });
            }
        }
    }

    // Holes from vias that belong to this span. A blind or buried via is
    // drilled in a separate operation from the through-holes (and from other
    // spans), which is how a fab prices and sequences the job — merging them
    // into one file silently asks for a through-hole where the board needs a
    // blind one.
    for via in &pcb.vias {
        if normalize_span(via.start_layer, via.end_layer) != span {
            continue;
        }
        let key = (via.drill * 10_000.0).round() as i64;
        holes.entry(key).or_default().push(DrillHit {
            x: via.position.x,
            y: via.position.y,
        });
    }

    holes
}

/// Generate Excellon drill file content.
///
/// Writes the complete Excellon file (header with tool definitions, drill hit
/// coordinates, and footer) to `writer`. Coordinates are in metric (mm) with
/// 4 decimal places.
/// Writes the **through-hole** drill file (pad drills plus full-stack vias).
/// Blind and buried vias live in their own per-span files — use
/// [`generate_drill_files`] to get the complete set.
pub fn write_excellon<W: Write>(writer: &mut W, pcb: &Pcb) -> Result<(), ExcellonError> {
    write_excellon_span(writer, pcb, THROUGH_SPAN)
}

/// Generate every Excellon drill file the board needs, as
/// `(filename, content)` pairs — one per distinct via span, plus the
/// through-hole file. Spans are emitted through-hole first.
pub fn generate_drill_files(pcb: &Pcb) -> Result<Vec<(String, String)>, ExcellonError> {
    let mut out = Vec::new();
    for span in spans(pcb) {
        let mut buf = Vec::new();
        write_excellon_span(&mut buf, pcb, span)?;
        out.push((
            span_filename(span),
            String::from_utf8_lossy(&buf).into_owned(),
        ));
    }
    Ok(out)
}

/// Generate the Excellon file for one drill span.
pub fn write_excellon_span<W: Write>(
    writer: &mut W,
    pcb: &Pcb,
    span: DrillSpan,
) -> Result<(), ExcellonError> {
    let holes = collect_holes(pcb, span);

    // Header.
    writeln!(writer, "M48")?;
    writeln!(writer, ";Generated by vcad-ecad-export")?;
    writeln!(
        writer,
        ";TYPE=PLATED ;SPAN={},{}",
        layer_token(span.0),
        layer_token(span.1)
    )?;
    writeln!(writer, ";FORMAT={{-:-/ absolute / metric / decimal}}")?;
    writeln!(writer, "FMAT,2")?;
    writeln!(writer, "METRIC,TZ")?;

    // Tool definitions.
    let tools: Vec<(u32, f64, &Vec<DrillHit>)> = holes
        .iter()
        .enumerate()
        .map(|(i, (key, hits))| {
            let tool_num = (i + 1) as u32;
            let diameter = *key as f64 / 10_000.0;
            (tool_num, diameter, hits)
        })
        .collect();

    for &(tool_num, diameter, _) in &tools {
        writeln!(writer, "T{tool_num:02}C{diameter:.4}")?;
    }

    writeln!(writer, "%")?;

    // Drill body.
    writeln!(writer, "G90")?;
    writeln!(writer, "G05")?;

    for &(tool_num, _, hits) in &tools {
        writeln!(writer, "T{tool_num:02}")?;
        for hit in hits {
            writeln!(writer, "X{:.4}Y{:.4}", hit.x, hit.y)?;
        }
    }

    // Footer.
    writeln!(writer, "T0")?;
    writeln!(writer, "M30")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Vec2;

    fn test_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                }],
            },
            nets: vec![Net {
                id: "1".into(),
                name: "VCC".into(),
            }],
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
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![Footprint {
                reference: "J1".into(),
                value: "Conn_01x03".into(),
                footprint_name: "Connector:PinHeader_1x03_P2.54mm".into(),
                position: Vec2::new(10.0, 20.0),
                rotation: 0.0,
                front: true,
                pads: vec![
                    Pad {
                        number: "1".into(),
                        pad_type: PadType::THT,
                        shape: PadShape::Circle { diameter: 1.7 },
                        position: Vec2::new(0.0, 0.0),
                        rotation: 0.0,
                        drill: Some(DrillSpec {
                            diameter: 1.0,
                            oval: false,
                            oval_height: None,
                        }),
                        net: Some("1".into()),
                        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
                    },
                    Pad {
                        number: "2".into(),
                        pad_type: PadType::THT,
                        shape: PadShape::Circle { diameter: 1.7 },
                        position: Vec2::new(2.54, 0.0),
                        rotation: 0.0,
                        drill: Some(DrillSpec {
                            diameter: 1.0,
                            oval: false,
                            oval_height: None,
                        }),
                        net: None,
                        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
                    },
                    Pad {
                        number: "3".into(),
                        pad_type: PadType::THT,
                        shape: PadShape::Circle { diameter: 1.7 },
                        position: Vec2::new(5.08, 0.0),
                        rotation: 0.0,
                        drill: Some(DrillSpec {
                            diameter: 1.0,
                            oval: false,
                            oval_height: None,
                        }),
                        net: None,
                        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
                    },
                ],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            }],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(30.0, 20.0),
                diameter: 0.8,
                drill: 0.3,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".into(),
                source: None,
            }],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn excellon_header() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_excellon(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.starts_with("M48\n"), "missing M48 header");
        assert!(output.contains("METRIC"), "missing METRIC declaration");
    }

    #[test]
    fn excellon_tool_definitions() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_excellon(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Two tool sizes: 0.3 mm (via) and 1.0 mm (THT pads).
        assert!(output.contains("T01C0.3000"), "missing 0.3mm tool");
        assert!(output.contains("T02C1.0000"), "missing 1.0mm tool");
    }

    #[test]
    fn excellon_coordinates() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_excellon(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Via at (30, 20).
        assert!(
            output.contains("X30.0000Y20.0000"),
            "missing via coordinate"
        );
        // First THT pad at (10, 20).
        assert!(
            output.contains("X10.0000Y20.0000"),
            "missing pad 1 coordinate"
        );
    }

    #[test]
    fn excellon_footer() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_excellon(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("T0\n"), "missing T0 (tool deselect)");
        assert!(output.contains("M30\n"), "missing M30 end command");
    }

    #[test]
    fn excellon_groups_holes_by_diameter() {
        let pcb = test_pcb();
        let holes = collect_holes(&pcb, THROUGH_SPAN);

        // Two distinct diameters: 0.3 mm (via) and 1.0 mm (THT).
        assert_eq!(holes.len(), 2);

        let key_03 = (0.3_f64 * 10_000.0).round() as i64;
        let key_10 = (1.0_f64 * 10_000.0).round() as i64;

        assert_eq!(holes[&key_03].len(), 1, "expected 1 via hole");
        assert_eq!(holes[&key_10].len(), 3, "expected 3 THT pad holes");
    }

    /// A board with one through via and two blind/buried vias on distinct
    /// spans. Drills for different spans are separate fab operations, priced
    /// and sequenced separately — collapsing them into one file asks for a
    /// through-hole where the board needs a blind one.
    fn mixed_span_pcb() -> Pcb {
        let mut pcb = test_pcb();
        pcb.vias.push(Via {
            position: Vec2::new(31.0, 21.0),
            diameter: 0.4,
            drill: 0.2,
            // Deliberately reversed: normalization must order it In1→In2.
            start_layer: PcbLayer::In2Cu,
            end_layer: PcbLayer::In1Cu,
            net: "1".into(),
            source: None,
        });
        pcb.vias.push(Via {
            position: Vec2::new(32.0, 22.0),
            diameter: 0.4,
            drill: 0.2,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::In1Cu,
            net: "1".into(),
            source: None,
        });
        pcb
    }

    #[test]
    fn drill_files_are_split_by_span() {
        let files = generate_drill_files(&mixed_span_pcb()).unwrap();
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();

        assert_eq!(
            names,
            vec![
                "drill.drl",
                "drill-F_Cu-In1_Cu.drl",
                "drill-In1_Cu-In2_Cu.drl",
            ],
            "expected one drill file per span, through-hole first"
        );

        let by_name = |want: &str| -> &str { &files.iter().find(|(n, _)| n == want).unwrap().1 };

        // Each span's hole belongs to exactly one file.
        let through = by_name("drill.drl");
        assert!(through.contains("X30.0000Y20.0000"), "through via missing");
        assert!(
            !through.contains("X31.0000Y21.0000") && !through.contains("X32.0000Y22.0000"),
            "buried/blind via drilled as a through-hole"
        );

        let buried = by_name("drill-In1_Cu-In2_Cu.drl");
        assert!(buried.contains("X31.0000Y21.0000"), "buried via missing");
        assert!(
            !buried.contains("X30.0000Y20.0000") && !buried.contains("X32.0000Y22.0000"),
            "buried span file carries holes from another span"
        );

        let blind = by_name("drill-F_Cu-In1_Cu.drl");
        assert!(blind.contains("X32.0000Y22.0000"), "blind via missing");
        assert!(
            !blind.contains("X30.0000Y20.0000") && !blind.contains("X31.0000Y21.0000"),
            "blind span file carries holes from another span"
        );

        // Pad drills are through-holes and belong only in the through file.
        assert!(through.contains("X10.0000Y20.0000"), "THT pad missing");
        assert!(
            !buried.contains("X10.0000Y20.0000") && !blind.contains("X10.0000Y20.0000"),
            "pad drill duplicated into a blind/buried span file"
        );

        // Every file is a well-formed Excellon program.
        for (name, content) in &files {
            assert!(content.starts_with("M48\n"), "{name}: missing M48 header");
            assert!(content.contains("M30\n"), "{name}: missing M30 footer");
        }
    }

    /// `write_excellon` keeps its through-hole-only contract, so callers that
    /// only want the through file don't silently pick up blind vias.
    #[test]
    fn write_excellon_emits_only_through_holes() {
        let mut buf = Vec::new();
        write_excellon(&mut buf, &mixed_span_pcb()).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("X30.0000Y20.0000"));
        assert!(!output.contains("X31.0000Y21.0000"));
        assert!(!output.contains("X32.0000Y22.0000"));
    }
}
