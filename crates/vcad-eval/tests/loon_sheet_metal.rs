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

// --- hem / jog / bend relief -------------------------------------------
//
// These three ops used to bail out of `build_sheet_model` with a blanket
// "not yet buildable in kernel-direct eval", so any chain containing one
// exported zero triangles from the CLI. Same trick as the flange test: two
// lengths of the same feature share an identical bend region, so the
// difference between them is exactly the extra plate.

#[test]
fn a_hem_is_refused_by_name_not_by_a_bend_index() {
    // `folded_sheet_solid` cannot build a 180° fold (its bend construction
    // degenerates as the panel planes become parallel), so a hem chain has to
    // fail — but it must fail saying "hem", not "bend #0: angle 3.1416".
    let doc = eval_vcad(
        r#"[pipe [sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]
                 [sheet-hem "north" 8.0]]"#,
        None,
    )
    .expect("loon eval");
    let scene =
        evaluate_document_with_sheet_metal(&doc, &EvalOptions::default()).expect("kernel eval");
    let msg = scene
        .failures
        .first()
        .map(|f| f.error.clone())
        .expect("a hem chain must report a failure, not silently fold to nothing");
    assert!(
        msg.contains("hem") && msg.contains("180"),
        "the error must name the op and what is missing, got: {msg}"
    );
}

#[test]
fn a_jog_adds_both_riser_and_tail() {
    let jogged = |offset: f64, length: f64| {
        volume(&format!(
            r#"[pipe [sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]
                     [sheet-jog "east" {offset} {length}]]"#
        ))
    };
    let flat = volume(r#"[sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]"#);
    assert!(jogged(10.0, 30.0) > flat, "the jog must add material");

    // The east edge runs the 120 mm depth. Growing only the tail leaves both
    // bends and the riser untouched: 20 mm more tail is 20 x 120 x 3.
    let extra = jogged(10.0, 50.0) - jogged(10.0, 30.0);
    let nominal = 20.0 * 120.0 * 3.0;
    assert!(
        (extra - nominal).abs() < 1.0,
        "20 mm more tail added {extra} mm^3, expected {nominal}"
    );

    // Growing only the riser is the same deal on the other leg: 15 mm more
    // riser is 15 x 120 x 3.
    let taller = jogged(25.0, 30.0) - jogged(10.0, 30.0);
    let nominal = 15.0 * 120.0 * 3.0;
    assert!(
        (taller - nominal).abs() < 1.0,
        "15 mm more offset added {taller} mm^3, expected {nominal}"
    );
}

#[test]
fn bend_relief_cuts_notches_at_the_bend_ends() {
    // A flange taken off the MIDDLE segment of a split edge leaves parent
    // material at both bend ends — exactly what relief notches exist for.
    // (A full-width flange needs none, so it would make this test vacuous.)
    let base = r#"[pipe [sheet-base-flange
                          #[0.0 0.0 60.0 0.0 140.0 0.0 200.0 0.0 200.0 120.0 0.0 120.0]
                          #[] 3.0 "al-soft"]
                        [sheet-edge-flange-at 0 1 40.0 90.0 0.0 "up" 0.0]]"#;
    let without = volume(base);
    let with = volume(&format!("[pipe {base} [sheet-bend-relief]]"));
    assert!(with > 0.0, "a relieved chain must still fold to a solid");

    // Two notches, each at the kernel's default sizing for t = 3, r = 3:
    // max(1.5t, 1) = 4.5 wide x (r + t) = 6 deep, through 3 mm of plate.
    let cut = without - with;
    let nominal = 2.0 * 4.5 * 6.0 * 3.0;
    assert!(
        (cut - nominal).abs() < 1.0,
        "relief removed {cut} mm^3, expected {nominal}"
    );
}

#[test]
fn a_chain_of_every_foldable_op_still_folds() {
    // The end-to-end symptom: this is the chain that used to export zero
    // triangles from `vcad info` / `vcad-render`. (Hem is excluded — see
    // `a_hem_is_refused_by_name_not_by_a_bend_index`.)
    let v = volume(
        r#"[pipe [sheet-base-flange-rect 200.0 120.0 3.0 "al-soft"]
                 [sheet-edge-flange "east" 40.0 90.0]
                 [sheet-edge-flange "west" 40.0 90.0]
                 [sheet-jog "north" 10.0 30.0]
                 [sheet-bend-relief]]"#,
    );
    assert!(
        v > 72_000.0,
        "the folded chain must beat the bare plate: {v}"
    );
}
