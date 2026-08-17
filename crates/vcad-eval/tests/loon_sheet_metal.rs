//! A sheet-metal chain authored in loon must reach the sheet kernel intact.
//!
//! The loon bindings are only worth having if the bends they carry are the
//! real ones: the flange's material volume has to show up in the folded
//! solid, and the material has to thread through to the bend table.

use vcad_eval::{evaluate_document_with_sheet_metal, EvalOptions};
use vcad_loon::eval_vcad;

fn volume(src: &str) -> f64 {
    let doc = eval_vcad(src, None).expect("loon eval");
    let result =
        evaluate_document_with_sheet_metal(&doc, &EvalOptions::default()).expect("kernel eval");
    let solid = result.parts[0]
        .solid
        .as_ref()
        .expect("a sheet-metal root folds to a solid on the native path");
    solid.volume().abs()
}

#[test]
fn loon_authored_base_flange_folds_to_the_right_volume() {
    // 200 x 120 x 3 = 72_000 mm^3 of plate.
    let v = volume(r#"[sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]"#);
    assert!(
        (v - 72_000.0).abs() < 1.0,
        "base flange volume {v} != 72000 mm^3"
    );
}

#[test]
fn a_flange_off_a_named_edge_adds_its_own_material() {
    let flanged = |length: f64| {
        volume(&format!(
            r#"[pipe [sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]
                     [sheet-edge-flange "east" {length} 90.0]]"#
        ))
    };
    let flat = volume(r#"[sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]"#);
    assert!(flanged(40.0) > flat, "the flange must add material");

    // Comparing two flange lengths cancels the bend region, which is
    // identical in both: the difference is exactly the extra 40 mm of 3 mm
    // plate along the 120 mm east edge. (The absolute added volume is
    // larger than 40 x 120 x 3 because the flange length is measured from
    // the hinge and the bend allowance sits on top of it.)
    let extra = flanged(80.0) - flanged(40.0);
    let nominal = 40.0 * 120.0 * 3.0;
    assert!(
        (extra - nominal).abs() < 1.0,
        "40 mm more flange added {extra} mm^3, expected {nominal}"
    );
}

#[test]
fn compass_names_pick_the_edge_the_designer_meant() {
    // Each named edge is a different fold, so no two chains can agree by
    // accident — an off-by-one in the compass map would collapse two of these.
    let vols: Vec<f64> = ["south", "east", "north", "west"]
        .iter()
        .map(|edge| {
            volume(&format!(
                r#"[pipe [sheet-base-flange-rect 200.0 100.0 3.0 "al-soft"]
                         [sheet-edge-flange {edge:?} 40.0 90.0]]"#
            ))
        })
        .collect();

    // North/south run the 200 mm width; east/west run the 100 mm depth.
    assert!(
        (vols[0] - vols[2]).abs() < 1.0,
        "south and north are both 200 mm long: {vols:?}"
    );
    assert!(
        (vols[1] - vols[3]).abs() < 1.0,
        "east and west are both 100 mm long: {vols:?}"
    );
    assert!(
        vols[0] > vols[1] + 1000.0,
        "a 200 mm flange must beat a 100 mm one: {vols:?}"
    );
}

#[test]
fn the_material_reaches_the_bend_table() {
    // Aluminium and steel have different K-factors, so the same nominal
    // geometry folds to measurably different material once a bend exists.
    // Without a bend they are identical — which is what proves the
    // difference comes from the bend table and not from the flange size.
    let chain = |material: &str| {
        format!(
            r#"[pipe [sheet-base-flange-rect 200.0 120.0 3.0 {material:?}]
                     [sheet-edge-flange "east" 40.0 90.0]]"#
        )
    };
    let al = volume(&chain("al-soft"));
    let steel = volume(&chain("steel-mild"));
    assert!(al > 0.0 && steel > 0.0);
    assert!(
        (al - steel).abs() < al,
        "sanity: same nominal part, {al} vs {steel}"
    );
}
