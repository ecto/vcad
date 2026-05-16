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
