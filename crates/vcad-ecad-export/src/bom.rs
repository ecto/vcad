//! BOM (Bill of Materials) CSV writer.
//!
//! Groups footprints by (value, package) and outputs a consolidated BOM with
//! quantities and aggregated reference designators.

use std::collections::BTreeMap;
use std::io::Write;

use vcad_ir::ecad::*;

/// A single BOM line item.
#[derive(Debug)]
struct BomEntry {
    refs: Vec<String>,
    value: String,
    package: String,
}

/// Generate BOM CSV content.
///
/// Groups footprints by their `value` and `footprint_name` (package), and
/// writes a CSV with columns: `Qty`, `Refs`, `Value`, `Package`.
/// Reference designators are sorted naturally (R1, R2, R10, not R1, R10, R2).
pub fn write_bom<W: Write>(writer: &mut W, pcb: &Pcb) -> Result<(), std::io::Error> {
    writeln!(writer, "Qty,Refs,Value,Package")?;

    // Group footprints by (value, package).
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for fp in &pcb.footprints {
        let key = (fp.value.clone(), fp.footprint_name.clone());
        groups.entry(key).or_default().push(fp.reference.clone());
    }

    // Build sorted entries.
    let mut entries: Vec<BomEntry> = groups
        .into_iter()
        .map(|((value, package), mut refs)| {
            refs.sort_by_key(|a| natural_sort_key(a));
            BomEntry {
                refs,
                value,
                package,
            }
        })
        .collect();

    // Sort entries by first reference designator.
    entries.sort_by(|a, b| {
        let a_key = a.refs.first().map(|r| natural_sort_key(r));
        let b_key = b.refs.first().map(|r| natural_sort_key(r));
        a_key.cmp(&b_key)
    });

    for entry in &entries {
        let refs_str = entry.refs.join(" ");
        writeln!(
            writer,
            "{},{},{},{}",
            entry.refs.len(),
            csv_escape(&refs_str),
            csv_escape(&entry.value),
            csv_escape(&entry.package),
        )?;
    }

    Ok(())
}

/// Escape a field for CSV output.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Generate a sort key for natural ordering of reference designators.
fn natural_sort_key(s: &str) -> (String, u64) {
    let prefix_end = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    let prefix = s[..prefix_end].to_string();
    let number: u64 = s[prefix_end..].parse().unwrap_or(0);
    (prefix, number)
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
            nets: vec![],
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
            footprints: vec![
                Footprint {
                    reference: "R1".into(),
                    value: "10k".into(),
                    footprint_name: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(10.0, 20.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
                Footprint {
                    reference: "R2".into(),
                    value: "10k".into(),
                    footprint_name: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(20.0, 20.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
                Footprint {
                    reference: "R10".into(),
                    value: "10k".into(),
                    footprint_name: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(30.0, 20.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
                Footprint {
                    reference: "C1".into(),
                    value: "100nF".into(),
                    footprint_name: "Capacitor_SMD:C_0402".into(),
                    position: Vec2::new(15.0, 25.0),
                    rotation: 90.0,
                    front: true,
                    pads: vec![],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
                Footprint {
                    reference: "U1".into(),
                    value: "ATmega328P".into(),
                    footprint_name: "Package_QFP:TQFP-32".into(),
                    position: Vec2::new(25.0, 25.0),
                    rotation: 0.0,
                    front: true,
                    pads: vec![],
                    graphics: vec![],
                    model_3d: None,
                    properties: Default::default(),
                },
            ],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn bom_header() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_bom(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let first_line = output.lines().next().unwrap();
        assert_eq!(first_line, "Qty,Refs,Value,Package");
    }

    #[test]
    fn bom_groups_by_value_and_package() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_bom(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // 3 unique (value, package) combos: 10k R_0805, 100nF C_0402, ATmega328P TQFP-32.
        let data_lines: Vec<&str> = output.lines().skip(1).collect();
        assert_eq!(data_lines.len(), 3, "expected 3 BOM groups");
    }

    #[test]
    fn bom_quantity_correct() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_bom(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Find the 10k resistor line -- should have qty 3.
        let resistor_line = output.lines().find(|l| l.contains("10k")).unwrap();
        assert!(
            resistor_line.starts_with("3,"),
            "expected qty 3 for 10k resistors, got: {resistor_line}"
        );
    }

    #[test]
    fn bom_refs_naturally_sorted() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_bom(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let resistor_line = output.lines().find(|l| l.contains("10k")).unwrap();
        // Refs field should be "R1 R2 R10" (naturally sorted).
        assert!(
            resistor_line.contains("R1 R2 R10"),
            "refs should be naturally sorted: {resistor_line}"
        );
    }

    #[test]
    fn bom_csv_escape() {
        assert_eq!(csv_escape("hello"), "hello");
        assert_eq!(csv_escape("1,2,3"), "\"1,2,3\"");
    }

    #[test]
    fn bom_entries_sorted_by_first_ref() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_bom(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let refs: Vec<&str> = output
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').nth(1))
            .collect();

        // C1 < R1 R2 R10 < U1 (alphabetical by prefix, then numeric).
        assert_eq!(refs.len(), 3);
        assert!(refs[0].starts_with("C1"), "first group should start with C");
        assert!(
            refs[1].starts_with("R1"),
            "second group should start with R"
        );
        assert!(refs[2].starts_with("U1"), "third group should start with U");
    }
}
