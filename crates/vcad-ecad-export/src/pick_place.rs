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
///
/// # Rotation convention
///
/// `Rotation` is the component's ABSOLUTE placement angle in the board frame —
/// `fp.rotation` verbatim, with no per-pad term and no bottom-side negation.
/// That is exactly what KiCad's own `.pos` exporter emits: verified against
/// `kicad-cli pcb export pos` on the 421-component CM5 fixture, where
/// `Rot - at_angle == 0` for all 115 top-side AND all 306 bottom-side parts.
/// See `pick_place_rotation_matches_kicad_export`.
///
/// Pads' own `pad.rotation` is deliberately absent: it is relative to the
/// footprint and describes pad *copper* orientation within the land pattern,
/// not how the machine turns the part. Adding it here would mis-place every
/// component whose land pattern contains a rotated pad.
///
/// `PosX`/`PosY` are `fp.position` verbatim, i.e. vcad's board frame — the
/// same frame the Gerber and Excellon writers emit, so this file registers
/// against vcad's own fab output. Note that vcad ingests KiCad's Y without
/// negating it, while KiCad's `.pos` writes `PosY = -at.y`; so for a board
/// imported from KiCad this column is the negation of KiCad's. That Y-sign
/// convention is pipeline-wide (Gerber, Excellon, render all share it), not a
/// pick-and-place bug, and is out of scope here.
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
            net_ties: vec![],
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

    /// Five real CM5 footprints (`.scratch/CM5RevEng.kicad_pcb`), spanning
    /// both board sides and four distinct non-zero angles, checked against
    /// KiCad's OWN pick-and-place export:
    ///
    /// ```text
    /// kicad-cli pcb export pos --format csv --units mm --side both \
    ///     -o cm5_kicad.pos.csv .scratch/CM5RevEng.kicad_pcb
    /// ```
    ///
    /// KiCad 9.0.3 emitted (Ref, PosX, PosY, Rot, Side):
    ///
    /// ```text
    /// C69  131.905000  -67.375000    45.000000  bottom
    /// J3   124.965000  -62.910000   180.000000  bottom
    /// R38  145.046450  -76.675000   -90.000000  bottom
    /// L1   105.390000  -83.730000    90.000000  top
    /// C1   107.400000  -67.130000    45.000000  top
    /// ```
    ///
    /// The IR values below are what `parse_kicad_pcb` produces for those
    /// footprints (KiCad's `(at x y a)` verbatim). The fixture is inlined
    /// rather than read from `.scratch/`, which is gitignored and 12 MB.
    ///
    /// This pins the two claims that matter for an assembly machine: the
    /// rotation column is the component's absolute angle and agrees with
    /// KiCad bit-for-bit on both sides (no bottom-side negation, no per-pad
    /// term), and X agrees directly while Y differs by the documented,
    /// pipeline-wide sign convention.
    #[test]
    fn pick_place_rotation_matches_kicad_export() {
        // (ref, ir_x, ir_y, ir_rotation, front) — straight from the .kicad_pcb.
        let cm5: [(&str, f64, f64, f64, bool); 5] = [
            ("C69", 131.905, 67.375, 45.0, false),
            ("J3", 124.965, 62.910, 180.0, false),
            ("R38", 145.046_45, 76.675, -90.0, false),
            ("L1", 105.390, 83.730, 90.0, true),
            ("C1", 107.400, 67.130, 45.0, true),
        ];
        // KiCad's own .pos rows: (ref, PosX, PosY, Rot, Side).
        let kicad: [(&str, f64, f64, f64, &str); 5] = [
            ("C69", 131.905, -67.375, 45.0, "Bottom"),
            ("J3", 124.965, -62.910, 180.0, "Bottom"),
            ("R38", 145.046_45, -76.675, -90.0, "Bottom"),
            ("L1", 105.390, -83.730, 90.0, "Top"),
            ("C1", 107.400, -67.130, 45.0, "Top"),
        ];

        let mut pcb = test_pcb();
        pcb.footprints = cm5
            .iter()
            .map(|&(reference, x, y, rotation, front)| Footprint {
                reference: reference.into(),
                value: "x".into(),
                footprint_name: "fp".into(),
                position: Vec2::new(x, y),
                rotation,
                front,
                // A land pattern whose pads carry their own (relative)
                // rotation: it must NOT leak into the placement angle.
                pads: vec![Pad {
                    number: "1".into(),
                    position: Vec2::new(0.5, 0.0),
                    rotation: 37.0,
                    shape: PadShape::Rect {
                        width: 0.6,
                        height: 0.3,
                    },
                    layers: vec![PcbLayer::FCu],
                    net: None,
                    pad_type: PadType::SMD,
                    drill: None,
                }],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            })
            .collect();

        let mut buf = Vec::new();
        write_pick_place(&mut buf, &pcb).unwrap();
        let output = String::from_utf8(buf).unwrap();

        for &(reference, kx, ky, krot, kside) in &kicad {
            let line = output
                .lines()
                .find(|l| l.starts_with(&format!("{reference},")))
                .unwrap_or_else(|| panic!("no row for {reference}"));
            let f: Vec<&str> = line.split(',').collect();
            let (px, py, rot, side) = (
                f[3].parse::<f64>().unwrap(),
                f[4].parse::<f64>().unwrap(),
                f[5].parse::<f64>().unwrap(),
                f[6],
            );

            // The acceptance criterion: rotation matches KiCad exactly.
            assert!(
                (rot - krot).abs() < 1e-9,
                "{reference}: rotation {rot} != KiCad {krot}"
            );
            assert_eq!(side, kside, "{reference}: side");
            // Position tolerance is the CSV's own 4-decimal quantum (0.1 µm,
            // vs KiCad's 6) — orders of magnitude below placement accuracy.
            assert!((px - kx).abs() < 1e-4, "{reference}: PosX {px} != {kx}");
            // Y: vcad's board frame is KiCad's negated (documented above).
            assert!(
                (py + ky).abs() < 1e-4,
                "{reference}: PosY {py} should be -({ky})"
            );
        }
    }

    #[test]
    fn csv_escape_handles_commas() {
        assert_eq!(csv_escape("hello"), "hello");
        assert_eq!(csv_escape("hello,world"), "\"hello,world\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
