//! Pick-and-place CSV writer.
//!
//! Generates a pick-and-place file for SMT assembly machines. Each row
//! describes the placement location of one component on the PCB.

use std::io::Write;

use vcad_ir::ecad::*;

/// Generate pick-and-place CSV content.
///
/// Writes a CSV with columns: `Ref`, `Value`, `Package`, `PosX`, `PosY`,
/// `Rotation`, `Side`. All coordinates are in millimetres, rotation in degrees.
pub fn write_pick_place<W: Write>(writer: &mut W, pcb: &Pcb) -> Result<(), std::io::Error> {
    writeln!(writer, "Ref,Value,Package,PosX,PosY,Rotation,Side")?;

    let mut footprints: Vec<&Footprint> = pcb.footprints.iter().collect();
    footprints.sort_by_key(|fp| natural_sort_key(&fp.reference));

    for fp in footprints {
        let side = if fp.front { "Top" } else { "Bottom" };
        writeln!(
            writer,
            "{},{},{},{:.4},{:.4},{:.1},{}",
            csv_escape(&fp.reference),
            csv_escape(&fp.value),
            csv_escape(&fp.footprint_name),
            fp.position.x,
            fp.position.y,
            fp.rotation,
            side,
        )?;
    }

    Ok(())
}

/// Escape a field for CSV output. Wraps in quotes if the value contains a
/// comma, quote, or newline.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Generate a sort key that handles reference designators naturally
/// (e.g. R1, R2, R10 rather than R1, R10, R2).
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
                    reference: "R10".into(),
                    value: "4.7k".into(),
                    footprint_name: "Resistor_SMD:R_0805".into(),
                    position: Vec2::new(30.0, 20.0),
                    rotation: 180.0,
                    front: false,
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
            ],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
        }
    }

    #[test]
    fn pick_place_header() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let first_line = output.lines().next().unwrap();
        assert_eq!(first_line, "Ref,Value,Package,PosX,PosY,Rotation,Side");
    }

    #[test]
    fn pick_place_row_count() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Header + 4 footprints.
        assert_eq!(output.lines().count(), 5);
    }

    #[test]
    fn pick_place_side_field() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // R10 is on the back side.
        let r10_line = output.lines().find(|l| l.starts_with("R10,")).unwrap();
        assert!(r10_line.ends_with("Bottom"), "R10 should be on Bottom");

        // R1 is on the front side.
        let r1_line = output.lines().find(|l| l.starts_with("R1,")).unwrap();
        assert!(r1_line.ends_with("Top"), "R1 should be on Top");
    }

    #[test]
    fn pick_place_natural_sort() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let refs: Vec<&str> = output
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').next())
            .collect();

        // Should be sorted as C1, R1, R2, R10 (natural ordering).
        assert_eq!(refs, vec!["C1", "R1", "R2", "R10"]);
    }

    #[test]
    fn pick_place_coordinates() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let r1_line = output.lines().find(|l| l.starts_with("R1,")).unwrap();
        assert!(
            r1_line.contains("10.0000,20.0000"),
            "R1 coordinates should be 10.0000,20.0000"
        );
    }

    #[test]
    fn csv_escape_handles_commas() {
        assert_eq!(csv_escape("hello"), "hello");
        assert_eq!(csv_escape("hello,world"), "\"hello,world\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
