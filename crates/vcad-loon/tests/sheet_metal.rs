//! Sheet metal, faceting overrides and vendor imports authored in loon.
//!
//! The point of these bindings is that a cut-and-bent part written in loon
//! keeps its bends. The alternative — unioned plates with sharp corners —
//! makes the flat pattern something you have to infer back out of the solid
//! afterwards, and throws away information the author had at design time.

use vcad_ir::{CsgOp, Document, SheetMetalDirection, SheetMetalHemKind};
use vcad_loon::eval_vcad;

fn doc(src: &str) -> Document {
    eval_vcad(src, None).unwrap_or_else(|e| panic!("eval failed: {e}\n--- source ---\n{src}"))
}

fn err(src: &str) -> String {
    eval_vcad(src, None).expect_err("expected this to be rejected")
}

/// The ops in `doc`, in node-id order.
fn ops(doc: &Document) -> Vec<&CsgOp> {
    let mut ids: Vec<_> = doc.nodes.keys().copied().collect();
    ids.sort_unstable();
    ids.iter().map(|id| &doc.nodes[id].op).collect()
}

const BRACKET: &str = r#"
[pipe [sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]
      [sheet-edge-flange "east" 40.0 90.0]
      [sheet-edge-flange "west" 40.0 90.0]
      [sheet-hem "north" 8.0]
      [sheet-bend-relief]]
"#;

#[test]
fn bracket_chain_lands_as_sheet_metal_ops() {
    let d = doc(BRACKET);
    let ops = ops(&d);
    assert_eq!(ops.len(), 5, "one node per op in the chain");

    match ops[0] {
        CsgOp::SheetMetalBaseFlangeRect {
            width,
            depth,
            thickness,
            material,
            shop_profile,
            engravings,
        } => {
            assert_eq!((*width, *depth, *thickness), (200.0, 120.0, 3.0));
            assert_eq!(material, "al-soft");
            assert!(shop_profile.is_none());
            assert!(engravings.is_none());
        }
        other => panic!("expected a rect base flange, got {other:?}"),
    }

    // "east" is edge 1 — base_flange_rect emits (0,0)→(w,0)→(w,d)→(0,d), so
    // edge 0 runs along -Y and the compass names follow CCW from there.
    match ops[1] {
        CsgOp::SheetMetalEdgeFlange {
            parent,
            panel_id,
            edge_index,
            length,
            angle,
            radius,
            direction,
            manual_k,
        } => {
            assert_eq!(*parent, 0);
            assert_eq!(*panel_id, 0);
            assert_eq!(*edge_index, 1, "east");
            assert_eq!(*length, 40.0);
            assert!(
                (angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12,
                "degrees in, radians out"
            );
            assert!(radius.is_none(), "0.0 means the default, not a zero radius");
            assert_eq!(*direction, SheetMetalDirection::Up);
            assert!(manual_k.is_none());
        }
        other => panic!("expected an edge flange, got {other:?}"),
    }

    match ops[2] {
        CsgOp::SheetMetalEdgeFlange {
            parent, edge_index, ..
        } => {
            assert_eq!(*parent, 1, "chains off the previous flange, not the base");
            assert_eq!(*edge_index, 3, "west");
        }
        other => panic!("expected an edge flange, got {other:?}"),
    }

    match ops[3] {
        CsgOp::SheetMetalHem {
            parent,
            edge_index,
            kind,
            length,
            gap,
            ..
        } => {
            assert_eq!(*parent, 2);
            assert_eq!(*edge_index, 2, "north");
            assert_eq!(*kind, SheetMetalHemKind::Closed);
            assert_eq!(*length, 8.0);
            assert_eq!(*gap, 0.0);
        }
        other => panic!("expected a hem, got {other:?}"),
    }

    match ops[4] {
        CsgOp::SheetMetalBendRelief {
            parent,
            width,
            depth,
        } => {
            assert_eq!(*parent, 3);
            assert!(
                width.is_none() && depth.is_none(),
                "unsized = kernel default"
            );
        }
        other => panic!("expected bend relief, got {other:?}"),
    }
}

#[test]
fn edge_names_and_indices_agree() {
    let named = doc(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                            [sheet-edge-flange "north" 20.0 90.0]]"#);
    let indexed = doc(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                              [sheet-edge-flange 2 20.0 90.0]]"#);
    assert_eq!(ops(&named)[1], ops(&indexed)[1]);
}

#[test]
fn explicit_form_carries_radius_direction_and_k() {
    let d = doc(
        r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "steel-mild"]
                        [sheet-edge-flange-at 0 1 25.0 135.0 3.0 "down" 0.42]]"#,
    );
    match ops(&d)[1] {
        CsgOp::SheetMetalEdgeFlange {
            angle,
            radius,
            direction,
            manual_k,
            ..
        } => {
            assert!((angle - 135f64.to_radians()).abs() < 1e-12);
            assert_eq!(*radius, Some(3.0));
            assert_eq!(*direction, SheetMetalDirection::Down);
            assert_eq!(*manual_k, Some(0.42));
        }
        other => panic!("expected an edge flange, got {other:?}"),
    }
}

#[test]
fn polygon_base_flange_takes_outline_and_holes() {
    let d = doc(
        r#"[sheet-base-flange #[0.0 0.0  80.0 0.0  80.0 40.0  0.0 40.0]
                              #[#[20.0 10.0  30.0 10.0  30.0 20.0  20.0 20.0]]
                              1.5 "5052"]"#,
    );
    match ops(&d)[0] {
        CsgOp::SheetMetalBaseFlangePolygon {
            outline,
            holes,
            thickness,
            material,
            ..
        } => {
            assert_eq!(outline.len(), 4);
            assert_eq!(outline[1].x, 80.0);
            assert_eq!(holes.len(), 1);
            assert_eq!(holes[0].len(), 4);
            assert_eq!(*thickness, 1.5);
            assert_eq!(material, "5052");
        }
        other => panic!("expected a polygon base flange, got {other:?}"),
    }
}

#[test]
fn jog_and_open_hem_round_out_the_vocabulary() {
    let d = doc(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                        [sheet-jog "east" 6.0 30.0]
                        [sheet-hem-open 0 12.0 1.0]]"#);
    match ops(&d)[1] {
        CsgOp::SheetMetalJog {
            edge_index,
            offset,
            length,
            ..
        } => {
            assert_eq!((*edge_index, *offset, *length), (1, 6.0, 30.0));
        }
        other => panic!("expected a jog, got {other:?}"),
    }
    match ops(&d)[2] {
        CsgOp::SheetMetalHem { kind, gap, .. } => {
            assert_eq!(*kind, SheetMetalHemKind::Open);
            assert_eq!(*gap, 1.0);
        }
        other => panic!("expected a hem, got {other:?}"),
    }
}

#[test]
fn shop_profile_and_engravings_reach_the_ir() {
    let d = doc(r#"[sheet-base-flange-rect-shop 100.0 50.0 2.0 "al-soft" "sendcutsend"]"#);
    match ops(&d)[0] {
        CsgOp::SheetMetalBaseFlangeRect { shop_profile, .. } => {
            assert_eq!(shop_profile.as_deref(), Some("sendcutsend"));
        }
        other => panic!("expected a rect base flange, got {other:?}"),
    }

    let d = doc(r#"[sheet-base-flange-rect-engraved 100.0 50.0 2.0 "al-soft"
              #[[engrave-text "A4" 5.0 5.0 6.0]
                [engrave-path #[0.0 0.0  10.0 0.0  10.0 10.0]]]]"#);
    match ops(&d)[0] {
        CsgOp::SheetMetalBaseFlangeRect { engravings, .. } => {
            let marks = engravings.as_ref().expect("engravings");
            assert_eq!(marks.len(), 2);
        }
        other => panic!("expected a rect base flange, got {other:?}"),
    }
}

// --- fail-closed behaviour ------------------------------------------------

#[test]
fn a_solid_is_not_a_sheet_chain() {
    let e = err(r#"[pipe [cube 10.0 10.0 2.0] [sheet-edge-flange 0 10.0 90.0]]"#);
    assert!(e.contains("sheet-metal chain"), "unhelpful error: {e}");
}

#[test]
fn edge_names_are_refused_where_they_are_undefined() {
    // Panel 1 is a flange, not the rectangle — its outline is not known here,
    // so a compass name would be a guess.
    let e = err(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                         [sheet-edge-flange-at 1 "east" 10.0 90.0 0.0 "up" 0.0]]"#);
    assert!(e.contains("numeric edge index"), "unhelpful error: {e}");

    // Likewise on a polygon base flange, whose edge 1 is whatever the author
    // drew.
    let e = err(
        r#"[pipe [sheet-base-flange #[0.0 0.0 80.0 0.0 80.0 40.0 0.0 40.0] #[] 1.5 "al-soft"]
                         [sheet-edge-flange "east" 10.0 90.0]]"#,
    );
    assert!(e.contains("numeric edge index"), "unhelpful error: {e}");

    let e = err(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                         [sheet-edge-flange "starboard" 10.0 90.0]]"#);
    assert!(e.contains("unknown edge name"), "unhelpful error: {e}");
}

#[test]
fn out_of_range_dimensions_are_rejected() {
    assert!(err(r#"[sheet-base-flange-rect 100.0 50.0 0.0 "al-soft"]"#).contains("thickness"));
    assert!(err(r#"[sheet-base-flange-rect -1.0 50.0 2.0 "al-soft"]"#).contains("width"));

    let bad_angle = err(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                                 [sheet-edge-flange 0 10.0 200.0]]"#);
    assert!(
        bad_angle.contains("(0, 180]"),
        "unhelpful error: {bad_angle}"
    );

    let open_no_gap = err(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                                   [sheet-hem-open 0 10.0 0.0]]"#);
    assert!(
        open_no_gap.contains("gap"),
        "unhelpful error: {open_no_gap}"
    );

    let two_points = err(r#"[sheet-base-flange #[0.0 0.0 10.0 0.0] #[] 1.5 "al-soft"]"#);
    assert!(
        two_points.contains("at least 3 points"),
        "unhelpful: {two_points}"
    );

    let odd = err(r#"[sheet-base-flange #[0.0 0.0 10.0 0.0 10.0] #[] 1.5 "al-soft"]"#);
    assert!(odd.contains("even number"), "unhelpful: {odd}");

    let dir = err(r#"[pipe [sheet-base-flange-rect 100.0 50.0 2.0 "al-soft"]
                           [sheet-edge-flange-at 0 0 10.0 90.0 0.0 "sideways" 0.0]]"#);
    assert!(dir.contains("\"up\" or \"down\""), "unhelpful: {dir}");
}

// --- faceting -------------------------------------------------------------

#[test]
fn segment_count_is_authorable() {
    let d = doc("[cylinder-n 10.0 20.0 128]");
    match ops(&d)[0] {
        CsgOp::Cylinder { segments, .. } => assert_eq!(*segments, 128),
        other => panic!("expected a cylinder, got {other:?}"),
    }

    // The plain forms still say "auto" — 0, not a pinned 32.
    let d = doc("[cylinder 10.0 20.0]");
    match ops(&d)[0] {
        CsgOp::Cylinder { segments, .. } => assert_eq!(*segments, 0),
        other => panic!("expected a cylinder, got {other:?}"),
    }

    for src in [
        "[sphere-n 5.0 64]",
        "[cone-n 5.0 2.0 10.0 48]",
        "[torus-n 20.0 4.0 96]",
    ] {
        let d = doc(src);
        let seg = match ops(&d)[0] {
            CsgOp::Sphere { segments, .. }
            | CsgOp::Cone { segments, .. }
            | CsgOp::Torus { segments, .. } => *segments,
            other => panic!("unexpected op for {src}: {other:?}"),
        };
        assert!(seg >= 48, "{src} lost its segment count");
    }

    // Below 3 a circle cannot close a face loop; say so instead of silently
    // clamping, since the author clearly meant something else.
    assert!(err("[cylinder-n 10.0 20.0 2]").contains("at least 3"));
}

// --- vendor imports -------------------------------------------------------

#[test]
fn imports_are_reachable_and_paths_resolve_relatively() {
    let d = doc(r#"[import-step "vendor/x6-60.step"]"#);
    match ops(&d)[0] {
        CsgOp::StepImport { path, solid_index } => {
            assert_eq!(
                path, "vendor/x6-60.step",
                "no base dir: leave the path alone"
            );
            assert!(solid_index.is_none());
        }
        other => panic!("expected a STEP import, got {other:?}"),
    }

    let d = eval_vcad(
        r#"[import-step-body "vendor/x6-60.step" 2]"#,
        Some(std::path::Path::new("/models/pond-v1")),
    )
    .unwrap();
    match ops(&d)[0] {
        CsgOp::StepImport { path, solid_index } => {
            assert_eq!(path, "/models/pond-v1/vendor/x6-60.step");
            assert_eq!(*solid_index, Some(2));
        }
        other => panic!("expected a STEP import, got {other:?}"),
    }

    // An absolute path is left as given.
    let d = eval_vcad(
        r#"[import-mesh "/opt/parts/bracket.stl"]"#,
        Some(std::path::Path::new("/models/pond-v1")),
    )
    .unwrap();
    match ops(&d)[0] {
        CsgOp::MeshImport { path, scale } => {
            assert_eq!(path, "/opt/parts/bracket.stl");
            assert!(scale.is_none(), "unit scale is no scale");
        }
        other => panic!("expected a mesh import, got {other:?}"),
    }

    let d = doc(r#"[import-mesh-scaled 1000.0 1000.0 1000.0 "metres.stl"]"#);
    match ops(&d)[0] {
        CsgOp::MeshImport { scale, .. } => {
            assert_eq!(scale.map(|s| s.x), Some(1000.0));
        }
        other => panic!("expected a mesh import, got {other:?}"),
    }

    assert!(err(r#"[import-step ""]"#).contains("path"));
}

#[test]
fn an_import_composes_with_booleans_like_any_other_solid() {
    // The whole reason to reach for an import is fit-checking against the
    // real envelope, which means it has to take part in booleans.
    let d = doc(r#"[pipe [cube 100.0 100.0 20.0]
                        [difference [import-step "vendor/x6-60.step"]]]"#);
    assert!(ops(&d)
        .iter()
        .any(|op| matches!(op, CsgOp::Difference { .. })));
    assert!(ops(&d)
        .iter()
        .any(|op| matches!(op, CsgOp::StepImport { .. })));
}

// --- round-trip -----------------------------------------------------------

#[test]
fn sheet_metal_survives_a_document_to_loon_round_trip() {
    let original = doc(BRACKET);
    let (source, unsupported) = vcad_ir::to_loon::document_to_loon_checked(&original);
    assert!(
        unsupported.is_empty(),
        "sheet metal should no longer be lossy: {unsupported:?}"
    );
    let round_tripped = doc(&source);
    assert_eq!(ops(&round_tripped), ops(&original), "\n{source}");
}

#[test]
fn faceting_and_imports_survive_a_round_trip() {
    let original = doc(r#"#[[root [cylinder-n 10.0 20.0 128] "default"]
             [root [import-step-body "a.step" 3] "default"]
             [root [import-mesh-scaled 25.4 25.4 25.4 "b.stl"] "default"]
             [root [torus-n 20.0 4.0 96] "default"]]"#);
    let (source, unsupported) = vcad_ir::to_loon::document_to_loon_checked(&original);
    assert!(unsupported.is_empty(), "{unsupported:?}");
    assert_eq!(ops(&doc(&source)), ops(&original), "\n{source}");
}
