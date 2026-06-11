//! Layered DXF export of a [`FlatPattern`].
//!
//! Emits DXF R12 (`AC1009`, the dialect SendCutSend / laser bureaus accept —
//! matching the existing `vcad` exporter) with the manufacturing layer
//! convention from `docs/features/sheet-metal.md`:
//!
//! | Layer       | Color        | Linetype   | Geometry                            |
//! |-------------|--------------|------------|-------------------------------------|
//! | `CUT`       | red (1)      | continuous | merged blank silhouette + holes     |
//! | `BEND_UP`   | blue (5)     | dashed     | bend centerlines that fold *up*     |
//! | `BEND_DOWN` | cyan (4)     | dashed     | bend centerlines that fold *down*   |
//!
//! The format targets what fab services (SendCutSend et al.) actually
//! parse:
//!
//! - **One closed cut profile.** Panels and bend-allowance strips are
//!   unioned into a single silhouette ([`FlatPattern::merged_silhouette`]);
//!   per-panel outlines would read as 17 disjoint open regions and get the
//!   upload rejected as "open entities".
//! - **Dashed bend lines.** Bend detection keys off the *entity linetype*
//!   (dashed), not the layer name.
//! - **Bend lines on the bend centerline** — the allowance-strip midline,
//!   where the brake die actually centers — not the parent hinge edge.
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

const LTYPE_CONTINUOUS: &str = "CONTINUOUS";
const LTYPE_DASHED: &str = "DASHED";

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

    // One merged silhouette per blank; fall back to per-panel outlines if
    // the union degenerates (shouldn't happen for tree-structured models,
    // but a broken DXF is worse than a disjoint one).
    let silhouette = flat.merged_silhouette();
    if silhouette.is_empty() {
        for outline in &flat.panel_outlines_2d {
            write_polyline(&mut s, outline, LAYER_CUT);
        }
    } else {
        for ring in &silhouette {
            write_polyline(&mut s, ring, LAYER_CUT);
        }
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

    // Linetype table. DASHED is required: fab-service bend detection keys
    // off the entity linetype. Pattern is 2.5mm dash / 1.25mm gap.
    let _ = writeln!(s, "0\nTABLE\n2\nLTYPE\n70\n2");
    let _ = writeln!(
        s,
        "0\nLTYPE\n2\nCONTINUOUS\n70\n0\n3\nSolid line\n72\n65\n73\n0\n40\n0.0"
    );
    let _ = writeln!(
        s,
        "0\nLTYPE\n2\nDASHED\n70\n0\n3\nDashed line\n72\n65\n73\n2\n40\n3.75\n49\n2.5\n49\n-1.25"
    );
    let _ = writeln!(s, "0\nENDTAB");

    // Layer table.
    let _ = writeln!(s, "0\nTABLE\n2\nLAYER\n70\n3");
    write_layer(s, LAYER_CUT, COLOR_CUT, LTYPE_CONTINUOUS);
    write_layer(s, LAYER_BEND_UP, COLOR_BEND_UP, LTYPE_DASHED);
    write_layer(s, LAYER_BEND_DOWN, COLOR_BEND_DOWN, LTYPE_DASHED);
    let _ = writeln!(s, "0\nENDTAB");

    let _ = writeln!(s, "0\nENDSEC");
}

fn write_layer(s: &mut String, name: &str, color: i32, ltype: &str) {
    let _ = writeln!(s, "0\nLAYER\n2\n{name}\n70\n0\n62\n{color}\n6\n{ltype}");
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
    // Bend lines carry an explicit entity-level DASHED linetype (group 6)
    // — SendCutSend-class parsers detect bends from the entity linetype,
    // not from the layer's default.
    let _ = writeln!(s, "0\nLINE\n8\n{layer}\n6\n{LTYPE_DASHED}");
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
                // 90° CCW about local origin: (x, y) → (-y, x). Shift
                // back into +x by adding the original height so the
                // rotated bbox sits at (0, 0).
                let (_, (_, max_y)) = p.flat.bbox();
                let height = max_y - min_y;
                Point2::new(-local.y + height, local.x)
            } else {
                local
            };
            Point2::new(rotated.x + p.dx_mm, rotated.y + p.dy_mm)
        };
        let silhouette = p.flat.merged_silhouette();
        if silhouette.is_empty() {
            for outline in &p.flat.panel_outlines_2d {
                let pts: Vec<Point2> = outline.iter().map(|&q| xform(q)).collect();
                write_polyline(&mut s, &pts, LAYER_CUT);
            }
        } else {
            for ring in &silhouette {
                let pts: Vec<Point2> = ring.iter().map(|&q| xform(q)).collect();
                write_polyline(&mut s, &pts, LAYER_CUT);
            }
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

        // Geometry: panels + allowance strip merge into ONE cut profile,
        // one bend-up crease with an explicit dashed linetype.
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 1);
        assert!(dxf.contains("0\nLINE\n8\nBEND_UP\n6\nDASHED"));
        assert!(!dxf.contains("8\nBEND_DOWN\n10")); // no down creases here

        // DASHED linetype must be declared in the LTYPE table.
        assert!(dxf.contains("0\nLTYPE\n2\nDASHED\n"));
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
        let dxfs = nested_dxf(&placements);
        assert_eq!(dxfs.len(), 1);
        // Each part contributes one merged silhouette on CUT.
        let cut_count = dxfs[0].matches("0\nLWPOLYLINE\n8\nCUT").count();
        assert_eq!(cut_count, 2, "expected 2 silhouettes, got {cut_count}");
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

    /// Golden fixture: the "origami crane" topology that SendCutSend
    /// rejected when the CUT layer carried per-panel outlines — hexagonal
    /// body, wing/tail/neck flanges off non-axis-aligned edges, and a
    /// chained head flange off the neck panel. The DXF must carry exactly
    /// one closed silhouette and one dashed centerline per bend.
    #[test]
    fn crane_flat_pattern_is_single_silhouette() {
        use crate::base_flange::base_flange_polygon;
        let outline = vec![
            Point2::new(20.0, 0.0),
            Point2::new(40.0, 0.0),
            Point2::new(60.0, 40.0),
            Point2::new(40.0, 80.0),
            Point2::new(20.0, 80.0),
            Point2::new(0.0, 40.0),
        ];
        let mut m = base_flange_polygon(outline, 0.5).unwrap();
        m.material = "Al-soft".into();
        let table = BendTable::builtin();
        let mut flange = |panel: usize, edge: usize, len: f64, angle: f64, dir: BendDirection| {
            add_edge_flange(
                &mut m,
                &table,
                EdgeFlangeParams {
                    panel,
                    edge_index: edge,
                    length: len,
                    angle,
                    radius: 0.5,
                    direction: dir,
                    position: FlangePosition::MaterialInside,
                    material: "Al-soft".into(),
                    manual_k: None,
                },
            )
            .unwrap()
            .0
        };
        let neck = flange(0, 0, 35.0, 1.1, BendDirection::Up); // neck off front edge
        flange(neck, 2, 12.0, 2.2, BendDirection::Down); // chained head fold
        flange(0, 2, 50.0, 0.6, BendDirection::Up); // right wing (diagonal hinge)
        flange(0, 3, 30.0, 0.9, BendDirection::Up); // tail
        flange(0, 4, 50.0, 0.6, BendDirection::Up); // left wing (diagonal hinge)
        unfold(&mut m).unwrap();
        let flat = FlatPattern::from_model(&m);

        // The union of 6 panels + 5 allowance strips is ONE closed loop.
        let silhouette = flat.merged_silhouette();
        assert_eq!(
            silhouette.len(),
            1,
            "expected single silhouette, got {} loops",
            silhouette.len()
        );
        // Area is conserved: panels + strips == silhouette.
        let ring_area = {
            let ring = &silhouette[0];
            let mut sum = 0.0;
            for i in 0..ring.len() {
                let a = ring[i];
                let b = ring[(i + 1) % ring.len()];
                sum += a.x * b.y - b.x * a.y;
            }
            0.5 * sum.abs()
        };
        assert!(
            (ring_area - flat.area_mm2).abs() < 1e-6,
            "silhouette area {ring_area} != flat area {}",
            flat.area_mm2
        );

        // Every crease sits on its allowance-strip midline: midpoint of
        // strip corners 0 and 3 equals crease start.
        for (strip, crease) in flat.allowance_strips_2d.iter().zip(&flat.creases) {
            let expect = Point2::new(
                (strip[0].x + strip[3].x) * 0.5,
                (strip[0].y + strip[3].y) * 0.5,
            );
            let d = ((crease.line.0.x - expect.x).powi(2) + (crease.line.0.y - expect.y).powi(2))
                .sqrt();
            assert!(d < 1e-9, "crease {} off midline by {d}", crease.bend_id);
        }

        // DXF: 1 CUT polyline, 5 dashed bend lines on the right layers.
        let dxf = flat_pattern_to_dxf(&flat);
        assert_eq!(dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(), 1);
        assert_eq!(dxf.matches("0\nLINE\n8\nBEND_UP\n6\nDASHED").count(), 4);
        assert_eq!(dxf.matches("0\nLINE\n8\nBEND_DOWN\n6\nDASHED").count(), 1);

        // ── Bend relief ─────────────────────────────────────────────
        // The four root bends (neck, both wings, tail) end at body
        // corners shared with chamfers or other hinges — 8 un-relieved
        // ends total. The chained head fold ends at free rectangle
        // corners and stays clean.
        use crate::manufacturability::{check_manufacturability, ShopProfile, Violation};
        use crate::relief::{add_all_bend_reliefs, ReliefParams};
        let flagged = check_manufacturability(&m, &ShopProfile::generic())
            .into_iter()
            .filter(|v| matches!(v, Violation::BendEndNeedsRelief { .. }))
            .count();
        assert_eq!(flagged, 8, "expected 8 un-relieved bend ends");

        let applied = add_all_bend_reliefs(&mut m, ReliefParams::default()).unwrap();
        assert_eq!(applied.len(), 8);
        let still_flagged = check_manufacturability(&m, &ShopProfile::generic())
            .into_iter()
            .filter(|v| matches!(v, Violation::BendEndNeedsRelief { .. }))
            .count();
        assert_eq!(still_flagged, 0, "relief fix must clear the rule");

        // The notched flat pattern still merges cleanly: the V-cuts are
        // perimeter notches, so the net silhouette area (signed, CCW
        // exterior minus any CW holes) equals the flat area, and the DXF
        // emits every loop on CUT.
        let flat = FlatPattern::from_model(&m);
        let silhouette = flat.merged_silhouette();
        assert!(!silhouette.is_empty());
        let net_area: f64 = silhouette
            .iter()
            .map(|ring| {
                let mut sum = 0.0;
                for i in 0..ring.len() {
                    let a = ring[i];
                    let b = ring[(i + 1) % ring.len()];
                    sum += a.x * b.y - b.x * a.y;
                }
                0.5 * sum
            })
            .sum();
        assert!(
            (net_area - flat.area_mm2).abs() < 1e-6,
            "net silhouette area {net_area} != flat area {}",
            flat.area_mm2
        );
        assert!(
            net_area < ring_area - 1e-9,
            "relief must remove blank material ({ring_area} → {net_area})"
        );
        let dxf = flat_pattern_to_dxf(&flat);
        assert_eq!(
            dxf.matches("0\nLWPOLYLINE\n8\nCUT").count(),
            silhouette.len(),
            "every loop lands on CUT"
        );
    }

    #[test]
    fn empty_pattern_is_still_valid_dxf() {
        let flat = FlatPattern {
            thickness: 1.0,
            panel_outlines_2d: vec![],
            panel_holes_2d: vec![],
            allowance_strips_2d: vec![],
            creases: vec![],
            area_mm2: 0.0,
        };
        let dxf = flat_pattern_to_dxf(&flat);
        assert!(dxf.contains("\nENTITIES"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));
    }
}
