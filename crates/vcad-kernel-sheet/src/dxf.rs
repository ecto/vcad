//! Layered DXF export of a [`FlatPattern`].
//!
//! Emits DXF R12 (`AC1009`, the dialect SendCutSend / laser bureaus accept —
//! matching the existing `vcad` exporter) with the manufacturing layer
//! convention from `docs/features/sheet-metal.md`:
//!
//! | Layer       | Color        | Geometry                          |
//! |-------------|--------------|-----------------------------------|
//! | `CUT`       | red (1)      | panel outlines + holes (closed)   |
//! | `BEND_UP`   | blue (5)     | creases that fold *up*            |
//! | `BEND_DOWN` | cyan (4)     | creases that fold *down*          |
//!
//! Only the three foundation-tier layers are written; `FORM` / `ETCH` /
//! `GRAIN` land with the form-tool and flat-first authoring tiers when
//! there's data to put on them.
//!
//! Output is a `String` (not a file) so the same code path serves the CLI,
//! the WASM binding, and tests without touching the filesystem.

use crate::model::BendDirection;
use crate::unfold::FlatPattern;
use std::fmt::Write as _;
use vcad_kernel_math::Point2;

/// DXF layer name for cut geometry (outer boundary + holes).
pub const LAYER_CUT: &str = "CUT";
/// DXF layer name for bend-up creases.
pub const LAYER_BEND_UP: &str = "BEND_UP";
/// DXF layer name for bend-down creases.
pub const LAYER_BEND_DOWN: &str = "BEND_DOWN";

const COLOR_CUT: i32 = 1; // red
const COLOR_BEND_UP: i32 = 5; // blue
const COLOR_BEND_DOWN: i32 = 4; // cyan

/// Serialise a flat pattern to a layered DXF document.
///
/// Coordinates are emitted verbatim in millimetres (the flat pattern's own
/// global 2D frame); `$INSUNITS` is set to mm so a shop importing the file
/// gets true-scale geometry.
pub fn flat_pattern_to_dxf(flat: &FlatPattern) -> String {
    let mut s = String::new();
    write_header(&mut s);
    write_tables(&mut s);

    let _ = writeln!(s, "0\nSECTION\n2\nENTITIES");

    for outline in &flat.panel_outlines_2d {
        write_polyline(&mut s, outline, LAYER_CUT);
    }
    for panel_holes in &flat.panel_holes_2d {
        for hole in panel_holes {
            write_polyline(&mut s, hole, LAYER_CUT);
        }
    }
    for crease in &flat.creases {
        let layer = match crease.direction {
            BendDirection::Up => LAYER_BEND_UP,
            BendDirection::Down => LAYER_BEND_DOWN,
        };
        write_line(&mut s, crease.line.0, crease.line.1, layer);
    }

    let _ = writeln!(s, "0\nENDSEC");
    let _ = writeln!(s, "0\nEOF");
    s
}

fn write_header(s: &mut String) {
    let _ = writeln!(s, "0\nSECTION\n2\nHEADER");
    let _ = writeln!(s, "9\n$ACADVER\n1\nAC1009");
    let _ = writeln!(s, "9\n$INSUNITS\n70\n4"); // 4 = millimetres
    let _ = writeln!(s, "0\nENDSEC");
}

fn write_tables(s: &mut String) {
    let _ = writeln!(s, "0\nSECTION\n2\nTABLES");

    // Linetype table — a single CONTINUOUS entry keeps strict parsers happy.
    let _ = writeln!(s, "0\nTABLE\n2\nLTYPE\n70\n1");
    let _ = writeln!(
        s,
        "0\nLTYPE\n2\nCONTINUOUS\n70\n0\n3\nSolid line\n72\n65\n73\n0\n40\n0.0"
    );
    let _ = writeln!(s, "0\nENDTAB");

    // Layer table.
    let _ = writeln!(s, "0\nTABLE\n2\nLAYER\n70\n3");
    write_layer(s, LAYER_CUT, COLOR_CUT);
    write_layer(s, LAYER_BEND_UP, COLOR_BEND_UP);
    write_layer(s, LAYER_BEND_DOWN, COLOR_BEND_DOWN);
    let _ = writeln!(s, "0\nENDTAB");

    let _ = writeln!(s, "0\nENDSEC");
}

fn write_layer(s: &mut String, name: &str, color: i32) {
    let _ = writeln!(s, "0\nLAYER\n2\n{name}\n70\n0\n62\n{color}\n6\nCONTINUOUS");
}

fn write_polyline(s: &mut String, pts: &[Point2], layer: &str) {
    if pts.len() < 2 {
        return;
    }
    let _ = writeln!(s, "0\nLWPOLYLINE\n8\n{layer}");
    let _ = writeln!(s, "90\n{}", pts.len());
    let _ = writeln!(s, "70\n1"); // closed
    for p in pts {
        let _ = writeln!(s, "10\n{:.6}\n20\n{:.6}", p.x, p.y);
    }
}

fn write_line(s: &mut String, a: Point2, b: Point2, layer: &str) {
    let _ = writeln!(s, "0\nLINE\n8\n{layer}");
    let _ = writeln!(s, "10\n{:.6}\n20\n{:.6}", a.x, a.y);
    let _ = writeln!(s, "11\n{:.6}\n21\n{:.6}", b.x, b.y);
}

/// One nested instance with the flat pattern that occupies it.
///
/// `dx_mm` / `dy_mm` translate the flat pattern's coordinates onto its
/// stock sheet; `rotated` rotates 90° (counter-clockwise around the
/// flat pattern's bbox-lower-left corner) before translation.
#[derive(Debug, Clone)]
pub struct NestedPlacement<'a> {
    /// Flat pattern for this part.
    pub flat: &'a FlatPattern,
    /// Sheet index (0-based). Each unique sheet gets its own DXF.
    pub sheet: usize,
    /// Translation in mm from the flat pattern's local origin.
    pub dx_mm: f64,
    /// Translation in mm from the flat pattern's local origin.
    pub dy_mm: f64,
    /// True for the 90° rotated orientation.
    pub rotated: bool,
}

/// Serialise a set of nested placements to one DXF per sheet.
///
/// Returns one DXF string per sheet (index 0 = sheet 0). Useful as the
/// shop-facing artifact for a nested job: each sheet's DXF carries every
/// part placed on it, on the same `CUT` / `BEND_UP` / `BEND_DOWN` layers
/// as the single-part exporter so shop post-processors don't have to
/// learn a new dialect.
pub fn nested_dxf(placements: &[NestedPlacement<'_>]) -> Vec<String> {
    let mut max_sheet = 0usize;
    for p in placements {
        if p.sheet > max_sheet {
            max_sheet = p.sheet;
        }
    }
    let num_sheets = if placements.is_empty() {
        0
    } else {
        max_sheet + 1
    };
    (0..num_sheets)
        .map(|sheet| build_sheet_dxf(sheet, placements))
        .collect()
}

fn build_sheet_dxf(sheet: usize, placements: &[NestedPlacement<'_>]) -> String {
    let mut s = String::new();
    write_header(&mut s);
    write_tables(&mut s);
    let _ = writeln!(s, "0\nSECTION\n2\nENTITIES");
    for p in placements.iter().filter(|p| p.sheet == sheet) {
        // Translate the flat pattern. The flat coordinates are referenced
        // to the FlatPattern's own bbox; we want each placement's bbox
        // lower-left to land at (dx, dy).
        let ((min_x, min_y), _) = p.flat.bbox();
        let xform = |pt: Point2| -> Point2 {
            let local = Point2::new(pt.x - min_x, pt.y - min_y);
            let rotated = if p.rotated {
                // 90° CCW about local origin: (x, y) → (-y, x). Then
                // shift back into +x, +y by adding the rotated bbox
                // dimensions.
                let (_, (max_x, max_y)) = p.flat.bbox();
                let (w, h) = (max_x - min_x, max_y - min_y);
                let _ = w; // height becomes the new x extent
                let _ = h;
                Point2::new(-local.y + (max_y - min_y), local.x)
            } else {
                local
            };
            Point2::new(rotated.x + p.dx_mm, rotated.y + p.dy_mm)
        };
        for outline in &p.flat.panel_outlines_2d {
            let pts: Vec<Point2> = outline.iter().map(|&q| xform(q)).collect();
            write_polyline(&mut s, &pts, LAYER_CUT);
        }
        for panel_holes in &p.flat.panel_holes_2d {
            for hole in panel_holes {
                let pts: Vec<Point2> = hole.iter().map(|&q| xform(q)).collect();
                write_polyline(&mut s, &pts, LAYER_CUT);
            }
        }
        for crease in &p.flat.creases {
            let layer = match crease.direction {
                crate::model::BendDirection::Up => LAYER_BEND_UP,
                crate::model::BendDirection::Down => LAYER_BEND_DOWN,
            };
            write_line(&mut s, xform(crease.line.0), xform(crease.line.1), layer);
        }
    }
    let _ = writeln!(s, "0\nENDSEC");
    let _ = writeln!(s, "0\nEOF");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams};
    use crate::model::BendDirection;
    use crate::unfold::unfold;
    use crate::FlangePosition;
    use std::f64::consts::FRAC_PI_2;

    fn l_bracket_flat() -> FlatPattern {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 0,
                length: 25.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Up,
                position: FlangePosition::MaterialInside,
                material: "Al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        unfold(&mut m).unwrap();
        FlatPattern::from_model(&m)
    }

    #[test]
    fn emits_well_formed_layered_dxf() {
        let dxf = flat_pattern_to_dxf(&l_bracket_flat());

        // Structure.
        assert!(dxf.starts_with("0\nSECTION\n2\nHEADER"));
        assert!(dxf.contains("$INSUNITS\n70\n4"), "units must be mm");
        assert!(dxf.contains("\nTABLES\n"));
        assert!(dxf.contains("\nENTITIES"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));

        // All three layers declared in the LAYER table.
        assert!(dxf.contains("0\nLAYER\n2\nCUT\n"));
        assert!(dxf.contains("0\nLAYER\n2\nBEND_UP\n"));
        assert!(dxf.contains("0\nLAYER\n2\nBEND_DOWN\n"));

        // Geometry: two panel outlines on CUT, one bend-up crease.
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 2);
        assert!(dxf.contains("0\nLINE\n8\nBEND_UP"));
        assert!(!dxf.contains("8\nBEND_DOWN\n10")); // no down creases here
    }

    #[test]
    fn down_bend_lands_on_bend_down_layer() {
        let mut m = base_flange_rect(40.0, 30.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 0,
                length: 10.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Down,
                position: FlangePosition::MaterialInside,
                material: "Al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        unfold(&mut m).unwrap();
        let dxf = flat_pattern_to_dxf(&FlatPattern::from_model(&m));
        assert!(dxf.contains("0\nLINE\n8\nBEND_DOWN"));
        assert!(!dxf.contains("0\nLINE\n8\nBEND_UP"));
    }

    #[test]
    fn nested_dxf_emits_one_per_sheet_with_translated_geometry() {
        let flat_a = l_bracket_flat();
        let flat_b = l_bracket_flat();
        let placements = vec![
            NestedPlacement {
                flat: &flat_a,
                sheet: 0,
                dx_mm: 100.0,
                dy_mm: 0.0,
                rotated: false,
            },
            NestedPlacement {
                flat: &flat_b,
                sheet: 0,
                dx_mm: 0.0,
                dy_mm: 200.0,
                rotated: true,
            },
        ];
        let dxfs = nested_dxf(&placements);
        assert_eq!(dxfs.len(), 1);
        // Both parts contribute outlines on CUT.
        let cut_count = dxfs[0].matches("0\nLWPOLYLINE\n8\nCUT").count();
        assert!(cut_count >= 4, "expected ≥4 outlines, got {cut_count}");
        // Some coordinate has been translated into the placement space.
        assert!(dxfs[0].contains("\n200.0"));
        assert!(dxfs[0].trim_end().ends_with("0\nEOF"));
    }

    #[test]
    fn nested_dxf_one_per_sheet() {
        let flat = l_bracket_flat();
        let placements = vec![
            NestedPlacement {
                flat: &flat,
                sheet: 0,
                dx_mm: 0.0,
                dy_mm: 0.0,
                rotated: false,
            },
            NestedPlacement {
                flat: &flat,
                sheet: 1,
                dx_mm: 0.0,
                dy_mm: 0.0,
                rotated: false,
            },
            NestedPlacement {
                flat: &flat,
                sheet: 1,
                dx_mm: 200.0,
                dy_mm: 0.0,
                rotated: false,
            },
        ];
        let dxfs = nested_dxf(&placements);
        assert_eq!(dxfs.len(), 2);
        // Sheet 1 has 2 instances → ~4 outlines, sheet 0 has ~2.
        let s1 = dxfs[1].matches("0\nLWPOLYLINE\n8\nCUT").count();
        let s0 = dxfs[0].matches("0\nLWPOLYLINE\n8\nCUT").count();
        assert!(s1 >= 2 * s0, "sheet 1 should have ≥2× the cuts of sheet 0");
    }

    #[test]
    fn empty_pattern_is_still_valid_dxf() {
        let flat = FlatPattern {
            thickness: 1.0,
            panel_outlines_2d: vec![],
            panel_holes_2d: vec![],
            creases: vec![],
            area_mm2: 0.0,
        };
        let dxf = flat_pattern_to_dxf(&flat);
        assert!(dxf.contains("\nENTITIES"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));
    }
}
