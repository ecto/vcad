//! Layered DXF export of a [`FlatPattern`] — natively compatible with
//! SendCutSend and similar laser/bend bureaus.
//!
//! Emits DXF R12 (`AC1009`) with `$INSUNITS = 4` (millimetres) and the
//! manufacturing layer convention from `docs/features/sheet-metal.md`:
//!
//! | Layer       | Color    | Linetype   | Geometry                          |
//! |-------------|----------|------------|-----------------------------------|
//! | `CUT`       | red (1)  | CONTINUOUS | one exterior ring + hole rings    |
//! | `BEND_UP`   | blue (5) | DASHED     | bend centerlines that fold *up*   |
//! | `BEND_DOWN` | cyan (4) | DASHED     | bend centerlines that fold *down* |
//!
//! Three properties make the output upload-ready for a fab service:
//!
//! 1. **One closed exterior per part.** Panel outlines and bend-allowance
//!    strips are unioned into a single silhouette (see
//!    [`crate::silhouette`]); no disjoint per-panel regions, no
//!    allowance-width gaps that read as "open entities".
//! 2. **Dashed bend lines.** Services detect bend lines by *linetype =
//!    dashed*, not by layer name or color. Every bend `LINE` carries the
//!    `DASHED` linetype (declared in the `LTYPE` table, R12-compatible);
//!    cut geometry stays `CONTINUOUS`. The Up/Down layers are kept for
//!    human readability.
//! 3. **Bend lines on the bend center.** Each line lies on the allowance
//!    midline (parent edge + allowance/2 toward the child), which is the
//!    actual bend centerline — not the parent edge of the strip.
//!
//! No text, dimensions, or annotations are ever emitted — services reject
//! or ignore them and they pollute bend-line detection.
//!
//! The old per-panel output remains available behind
//! [`DxfOptions::legacy_panel_outlines`] for debugging; default off.
//!
//! Output is a `String` (not a file) so the same code path serves the CLI,
//! the WASM binding, and tests without touching the filesystem.

use crate::model::BendDirection;
use crate::silhouette::{silhouette, Silhouette, SilhouetteError};
use crate::unfold::FlatPattern;
use std::fmt::Write as _;
use vcad_kernel_math::Point2;

/// DXF layer name for cut geometry (outer boundary + holes).
pub const LAYER_CUT: &str = "CUT";
/// DXF layer name for bend-up creases.
pub const LAYER_BEND_UP: &str = "BEND_UP";
/// DXF layer name for bend-down creases.
pub const LAYER_BEND_DOWN: &str = "BEND_DOWN";
/// DXF layer name for surface-marking (laser engrave) geometry. Open
/// polylines, CONTINUOUS linetype — never merged into the cut silhouette
/// and never dashed (dashed reads as a bend line to fab parsers).
pub const LAYER_ENGRAVE: &str = "ENGRAVE";

const COLOR_CUT: i32 = 1; // red
const COLOR_BEND_UP: i32 = 5; // blue
const COLOR_BEND_DOWN: i32 = 4; // cyan
const COLOR_ENGRAVE: i32 = 6; // magenta

/// Linetype applied to every bend line. Fab services detect bend lines by
/// dashed linetype — solid lines are invisible to their parsers.
const LTYPE_DASHED: &str = "DASHED";
const LTYPE_CONTINUOUS: &str = "CONTINUOUS";

/// Export options for [`flat_pattern_to_dxf_with`].
#[derive(Debug, Clone, Default)]
pub struct DxfOptions {
    /// Emit the legacy per-panel outlines (each panel its own closed
    /// polyline, bend lines on the parent edge, no merged silhouette).
    /// Debugging aid only — fab services reject this format. Default off.
    pub legacy_panel_outlines: bool,
}

/// Serialise a flat pattern to a fab-ready layered DXF document.
///
/// Coordinates are emitted verbatim in millimetres (the flat pattern's own
/// global 2D frame); `$INSUNITS` is set to mm so a shop importing the file
/// gets true-scale geometry.
///
/// Fails with [`SilhouetteError::DisconnectedIslands`] when the panel/bend
/// graph does not merge into a single region — that part cannot be cut as
/// one piece, and silently emitting the per-panel format would just move
/// the failure to the fab's upload checker.
pub fn flat_pattern_to_dxf(flat: &FlatPattern) -> Result<String, SilhouetteError> {
    flat_pattern_to_dxf_with(flat, &DxfOptions::default())
}

/// [`flat_pattern_to_dxf`] with explicit options.
pub fn flat_pattern_to_dxf_with(
    flat: &FlatPattern,
    options: &DxfOptions,
) -> Result<String, SilhouetteError> {
    let mut s = String::new();
    write_header(&mut s);
    write_tables(&mut s);
    let _ = writeln!(s, "0\nSECTION\n2\nENTITIES");
    if options.legacy_panel_outlines {
        write_legacy_entities(&mut s, flat, &|p| p);
    } else {
        let sil = silhouette_or_empty(flat)?;
        write_silhouette_entities(&mut s, &sil, &|p| p);
    }
    write_engrave_entities(&mut s, flat, &|p| p);
    let _ = writeln!(s, "0\nENDSEC");
    let _ = writeln!(s, "0\nEOF");
    Ok(s)
}

/// An empty flat pattern still yields a valid (empty) DXF; only a *failed
/// union* of real geometry is an error.
fn silhouette_or_empty(flat: &FlatPattern) -> Result<Silhouette, SilhouetteError> {
    match silhouette(flat) {
        Ok(s) => Ok(s),
        Err(SilhouetteError::Empty) => Ok(Silhouette {
            exterior: Vec::new(),
            holes: Vec::new(),
            bend_lines: Vec::new(),
        }),
        Err(e) => Err(e),
    }
}

fn write_silhouette_entities(s: &mut String, sil: &Silhouette, xform: &dyn Fn(Point2) -> Point2) {
    if sil.exterior.len() >= 3 {
        let pts: Vec<Point2> = sil.exterior.iter().map(|&p| xform(p)).collect();
        write_polyline(s, &pts, LAYER_CUT);
    }
    for hole in &sil.holes {
        let pts: Vec<Point2> = hole.iter().map(|&p| xform(p)).collect();
        write_polyline(s, &pts, LAYER_CUT);
    }
    for bl in &sil.bend_lines {
        write_bend_line(s, xform(bl.line.0), xform(bl.line.1), bl.direction);
    }
}

fn write_legacy_entities(s: &mut String, flat: &FlatPattern, xform: &dyn Fn(Point2) -> Point2) {
    for outline in &flat.panel_outlines_2d {
        let pts: Vec<Point2> = outline.iter().map(|&p| xform(p)).collect();
        write_polyline(s, &pts, LAYER_CUT);
    }
    for panel_holes in &flat.panel_holes_2d {
        for hole in panel_holes {
            let pts: Vec<Point2> = hole.iter().map(|&p| xform(p)).collect();
            write_polyline(s, &pts, LAYER_CUT);
        }
    }
    for crease in &flat.creases {
        write_bend_line(
            s,
            xform(crease.line.0),
            xform(crease.line.1),
            crease.direction,
        );
    }
}

fn write_engrave_entities(s: &mut String, flat: &FlatPattern, xform: &dyn Fn(Point2) -> Point2) {
    for pl in &flat.engravings_2d {
        let pts: Vec<Point2> = pl.iter().map(|&p| xform(p)).collect();
        write_polyline_open(s, &pts, LAYER_ENGRAVE);
    }
}

fn write_bend_line(s: &mut String, a: Point2, b: Point2, direction: BendDirection) {
    let layer = match direction {
        BendDirection::Up => LAYER_BEND_UP,
        BendDirection::Down => LAYER_BEND_DOWN,
    };
    write_line(s, a, b, layer, LTYPE_DASHED);
}

fn write_header(s: &mut String) {
    let _ = writeln!(s, "0\nSECTION\n2\nHEADER");
    let _ = writeln!(s, "9\n$ACADVER\n1\nAC1009");
    let _ = writeln!(s, "9\n$INSUNITS\n70\n4"); // 4 = millimetres
    let _ = writeln!(s, "0\nENDSEC");
}

fn write_tables(s: &mut String) {
    let _ = writeln!(s, "0\nSECTION\n2\nTABLES");

    // Linetype table: CONTINUOUS for cuts plus DASHED for bend lines.
    // DASHED pattern (R12, drawing units = mm): total 19.05, 12.7 dash,
    // 6.35 gap — the stock ACAD dashed pattern fab parsers expect.
    let _ = writeln!(s, "0\nTABLE\n2\nLTYPE\n70\n2");
    let _ = writeln!(
        s,
        "0\nLTYPE\n2\nCONTINUOUS\n70\n0\n3\nSolid line\n72\n65\n73\n0\n40\n0.0"
    );
    let _ = writeln!(
        s,
        "0\nLTYPE\n2\nDASHED\n70\n0\n3\nDashed line\n72\n65\n73\n2\n40\n19.05\n49\n12.7\n49\n-6.35"
    );
    let _ = writeln!(s, "0\nENDTAB");

    // Layer table.
    let _ = writeln!(s, "0\nTABLE\n2\nLAYER\n70\n4");
    write_layer(s, LAYER_CUT, COLOR_CUT, LTYPE_CONTINUOUS);
    write_layer(s, LAYER_BEND_UP, COLOR_BEND_UP, LTYPE_DASHED);
    write_layer(s, LAYER_BEND_DOWN, COLOR_BEND_DOWN, LTYPE_DASHED);
    write_layer(s, LAYER_ENGRAVE, COLOR_ENGRAVE, LTYPE_CONTINUOUS);
    let _ = writeln!(s, "0\nENDTAB");

    let _ = writeln!(s, "0\nENDSEC");
}

fn write_layer(s: &mut String, name: &str, color: i32, ltype: &str) {
    let _ = writeln!(s, "0\nLAYER\n2\n{name}\n70\n0\n62\n{color}\n6\n{ltype}");
}

fn write_polyline(s: &mut String, pts: &[Point2], layer: &str) {
    write_polyline_flags(s, pts, layer, 1); // closed
}

/// Open polyline — no implicit closing segment. Engrave strokes are pen
/// paths, not loops; closing them would burn a spurious return stroke.
fn write_polyline_open(s: &mut String, pts: &[Point2], layer: &str) {
    write_polyline_flags(s, pts, layer, 0);
}

fn write_polyline_flags(s: &mut String, pts: &[Point2], layer: &str, flags: u32) {
    if pts.len() < 2 {
        return;
    }
    let _ = writeln!(s, "0\nLWPOLYLINE\n8\n{layer}");
    let _ = writeln!(s, "90\n{}", pts.len());
    let _ = writeln!(s, "70\n{flags}");
    for p in pts {
        let _ = writeln!(s, "10\n{:.6}\n20\n{:.6}", p.x, p.y);
    }
}

fn write_line(s: &mut String, a: Point2, b: Point2, layer: &str, ltype: &str) {
    let _ = writeln!(s, "0\nLINE\n8\n{layer}\n6\n{ltype}");
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
/// Returns one DXF string per sheet (index 0 = sheet 0). Each part is
/// emitted as its merged silhouette + dashed bend centerlines — the same
/// fab-ready convention as the single-part exporter.
pub fn nested_dxf(placements: &[NestedPlacement<'_>]) -> Result<Vec<String>, SilhouetteError> {
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

fn build_sheet_dxf(
    sheet: usize,
    placements: &[NestedPlacement<'_>],
) -> Result<String, SilhouetteError> {
    let mut s = String::new();
    write_header(&mut s);
    write_tables(&mut s);
    let _ = writeln!(s, "0\nSECTION\n2\nENTITIES");
    for p in placements.iter().filter(|p| p.sheet == sheet) {
        // Translate the flat pattern. The flat coordinates are referenced
        // to the FlatPattern's own bbox; we want each placement's bbox
        // lower-left to land at (dx, dy).
        let ((min_x, min_y), (_, max_y)) = p.flat.bbox();
        let height = max_y - min_y;
        let xform = move |pt: Point2| -> Point2 {
            let local = Point2::new(pt.x - min_x, pt.y - min_y);
            let rotated = if p.rotated {
                // 90° CCW about local origin: (x, y) → (-y, x). Shift
                // back into +x by adding the original height so the
                // rotated bbox sits at (0, 0).
                Point2::new(-local.y + height, local.x)
            } else {
                local
            };
            Point2::new(rotated.x + p.dx_mm, rotated.y + p.dy_mm)
        };
        let sil = silhouette_or_empty(p.flat)?;
        write_silhouette_entities(&mut s, &sil, &xform);
        write_engrave_entities(&mut s, p.flat, &xform);
    }
    let _ = writeln!(s, "0\nENDSEC");
    let _ = writeln!(s, "0\nEOF");
    Ok(s)
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
        let dxf = flat_pattern_to_dxf(&l_bracket_flat()).unwrap();

        // Structure.
        assert!(dxf.starts_with("0\nSECTION\n2\nHEADER"));
        assert!(dxf.contains("$ACADVER\n1\nAC1009"));
        assert!(dxf.contains("$INSUNITS\n70\n4"), "units must be mm");
        assert!(dxf.contains("\nTABLES\n"));
        assert!(dxf.contains("\nENTITIES"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));

        // All three layers declared in the LAYER table.
        assert!(dxf.contains("0\nLAYER\n2\nCUT\n"));
        assert!(dxf.contains("0\nLAYER\n2\nBEND_UP\n"));
        assert!(dxf.contains("0\nLAYER\n2\nBEND_DOWN\n"));

        // DASHED linetype declared in the LTYPE table.
        assert!(dxf.contains("0\nLTYPE\n2\nDASHED\n"));
        assert!(dxf.contains("40\n19.05\n49\n12.7\n49\n-6.35"));

        // Geometry: ONE merged exterior on CUT (not two per-panel
        // outlines), one dashed bend-up line.
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 1);
        assert!(dxf.contains("0\nLINE\n8\nBEND_UP\n6\nDASHED"));
        assert!(!dxf.contains("8\nBEND_DOWN\n6")); // no down creases here
    }

    #[test]
    fn bend_line_is_recentered_on_allowance_midline() {
        let flat = l_bracket_flat();
        let dxf = flat_pattern_to_dxf(&flat).unwrap();
        let ba = crate::silhouette::crease_allowance(&flat.creases[0], flat.thickness);
        let mid_y = -ba / 2.0;
        // The LINE's y coordinates must be the midline, not 0 (the parent
        // edge of the allowance strip).
        let expect = format!("20\n{mid_y:.6}");
        assert!(dxf.contains(&expect), "missing midline y {mid_y:.6}");
        assert!(!dxf.contains("0\nLINE\n8\nBEND_UP\n6\nDASHED\n10\n0.000000\n20\n0.000000"));
    }

    #[test]
    fn legacy_flag_restores_per_panel_outlines() {
        let dxf = flat_pattern_to_dxf_with(
            &l_bracket_flat(),
            &DxfOptions {
                legacy_panel_outlines: true,
            },
        )
        .unwrap();
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 2);
    }

    #[test]
    fn never_emits_text_or_dimensions() {
        let dxf = flat_pattern_to_dxf(&l_bracket_flat()).unwrap();
        assert!(!dxf.contains("\nTEXT"));
        assert!(!dxf.contains("\nMTEXT"));
        assert!(!dxf.contains("\nDIMENSION"));
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
        let dxf = flat_pattern_to_dxf(&FlatPattern::from_model(&m)).unwrap();
        assert!(dxf.contains("0\nLINE\n8\nBEND_DOWN\n6\nDASHED"));
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
        let dxfs = nested_dxf(&placements).unwrap();
        assert_eq!(dxfs.len(), 1);
        // Each part contributes ONE merged exterior on CUT.
        let cut_count = dxfs[0].matches("0\nLWPOLYLINE\n8\nCUT").count();
        assert_eq!(cut_count, 2, "expected 2 merged outlines, got {cut_count}");
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
        let dxfs = nested_dxf(&placements).unwrap();
        assert_eq!(dxfs.len(), 2);
        let s1 = dxfs[1].matches("0\nLWPOLYLINE\n8\nCUT").count();
        let s0 = dxfs[0].matches("0\nLWPOLYLINE\n8\nCUT").count();
        assert_eq!(s0, 1);
        assert_eq!(s1, 2);
    }

    #[test]
    fn engravings_land_on_engrave_layer_as_open_polylines() {
        let mut flat = l_bracket_flat();
        flat.engravings_2d = crate::font::text_to_polylines("A4", 30.0, 20.0, 6.0, 0.0).unwrap();
        let dxf = flat_pattern_to_dxf(&flat).unwrap();
        // Layer declared, geometry present, CONTINUOUS (not dashed — dashed
        // reads as a bend line to fab parsers).
        assert!(dxf.contains("0\nLAYER\n2\nENGRAVE\n"));
        let n = dxf.matches("0\nLWPOLYLINE\n8\nENGRAVE\n").count();
        assert_eq!(n, flat.engravings_2d.len());
        // Open flag (70\n0) on engrave polylines; the CUT exterior stays closed.
        assert!(dxf.contains("0\nLWPOLYLINE\n8\nENGRAVE\n90\n3\n70\n0\n"));
        assert!(!dxf.contains("8\nENGRAVE\n6\nDASHED"));
        // Engraving must not change the cut silhouette.
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 1);
    }

    #[test]
    fn engrave_layer_declared_even_when_unused() {
        let dxf = flat_pattern_to_dxf(&l_bracket_flat()).unwrap();
        assert!(dxf.contains("0\nLAYER\n2\nENGRAVE\n"));
        assert!(!dxf.contains("0\nLWPOLYLINE\n8\nENGRAVE"));
    }

    #[test]
    fn nested_dxf_transforms_engravings() {
        let mut flat = l_bracket_flat();
        flat.engravings_2d = vec![vec![Point2::new(1.0, 1.0), Point2::new(5.0, 1.0)]];
        let placements = vec![NestedPlacement {
            flat: &flat,
            sheet: 0,
            dx_mm: 300.0,
            dy_mm: 0.0,
            rotated: false,
        }];
        let dxfs = nested_dxf(&placements).unwrap();
        // Flat bbox min is (0, -BA-25); the engrave x should be shifted by
        // 300 - min_x = 301.
        assert!(dxfs[0].contains("0\nLWPOLYLINE\n8\nENGRAVE"));
        assert!(dxfs[0].contains("10\n301.000000"));
    }

    #[test]
    fn empty_pattern_is_still_valid_dxf() {
        let flat = FlatPattern {
            thickness: 1.0,
            panel_outlines_2d: vec![],
            panel_holes_2d: vec![],
            creases: vec![],
            engravings_2d: vec![],
            area_mm2: 0.0,
        };
        let dxf = flat_pattern_to_dxf(&flat).unwrap();
        assert!(dxf.contains("\nENTITIES"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));
    }
}
