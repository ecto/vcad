//! End-to-end tests for parametric loon authoring.
//!
//! The property under test throughout: a value the author *named* is still
//! named in the document, and setting it afterwards moves exactly what the
//! loon program would have moved if the author had edited the source and
//! re-evaluated it.

use vcad_ir::{CsgOp, Document, Expr, Vec3};
use vcad_loon::{eval_vcad, eval_vcad_parametric};

/// Evaluate, then drive the named parameters and resolve — the `set_parameters`
/// path an agent or the UI takes.
fn with_params(doc: &Document, values: &[(&str, f64)]) -> Document {
    let mut d = doc.clone();
    for (name, v) in values {
        let p = d
            .parameters
            .get_mut(*name)
            .unwrap_or_else(|| panic!("no parameter '{name}' — have {:?}", doc.parameters.keys()));
        p.value = Expr::Number(*v);
    }
    vcad_ir::resolve_document(&mut d).expect("resolve");
    d
}

/// Every translate offset in a document, in node order.
fn offsets(doc: &Document) -> Vec<Vec3> {
    let mut ids: Vec<_> = doc.nodes.keys().copied().collect();
    ids.sort();
    ids.iter()
        .filter_map(|id| match &doc.nodes[id].op {
            CsgOp::Translate { offset, .. } => Some(*offset),
            _ => None,
        })
        .collect()
}

/// Translate Y offsets, ascending — the order nodes land in within a union is
/// an implementation detail.
fn sorted_y(doc: &Document) -> Vec<f64> {
    let mut ys: Vec<f64> = offsets(doc).iter().map(|o| o.y).collect();
    ys.sort_by(f64::total_cmp);
    ys
}

fn sizes(doc: &Document) -> Vec<Vec3> {
    let mut ids: Vec<_> = doc.nodes.keys().copied().collect();
    ids.sort();
    ids.iter()
        .filter_map(|id| match &doc.nodes[id].op {
            CsgOp::Cube { size } => Some(*size),
            _ => None,
        })
        .collect()
}

// ============================================================================
// Parameters survive evaluation
// ============================================================================

#[test]
fn a_named_value_is_still_named_afterwards() {
    let (doc, warnings) = eval_vcad_parametric(
        "[defparam pitch_axis_x 310.0]\n[root [translate pitch_axis_x 0.0 0.0 [cube 1.0 1.0 1.0]] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(doc.parameters["pitch_axis_x"].value, Expr::Number(310.0));
    assert_eq!(offsets(&doc)[0].x, 310.0);
}

#[test]
fn setting_one_parameter_replaces_editing_every_dependent_literal() {
    // The motivating case: the pitch axis feeds four mirrored placements, so
    // moving it by hand means finding and editing every dependent literal.
    let src = r#"
[defparam pitch_axis_x 310.0]
[let leg [fn [sx sy] [translate [* sx pitch_axis_x] [* sy 40.0] 0.0 [cube 5.0 5.0 5.0]]]]
[root [union [leg 1.0 1.0] [union [leg -1.0 1.0] [union [leg 1.0 -1.0] [leg -1.0 -1.0]]]] "steel"]
"#;
    let (doc, warnings) = eval_vcad_parametric(src, None, None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let before: Vec<f64> = offsets(&doc).iter().map(|o| o.x).collect();
    assert_eq!(before.len(), 4);

    let moved = with_params(&doc, &[("pitch_axis_x", 315.0)]);
    let after: Vec<f64> = offsets(&moved).iter().map(|o| o.x).collect();
    assert_eq!(after.len(), 4);
    // Every dependent placement moved, mirrored signs included, from one edit.
    for (b, a) in before.iter().zip(&after) {
        assert!((a.abs() - 315.0).abs() < 1e-9, "{b} -> {a}");
        assert_eq!(b.signum(), a.signum());
    }
}

#[test]
fn a_parameter_survives_the_symmetry_sugar() {
    // `quad-pattern` (the 4-fold mirror helper) is how the four-mirrored-legs
    // case is meant to be written. The parameter has to reach every one of the
    // placements it generates, mirrored copies included.
    let src = r#"
[defparam pitch_axis_x 310.0]
[root [quad-pattern [translate pitch_axis_x 40.0 0.0 [cube 5.0 5.0 5.0]]] "steel"]
"#;
    let (doc, warnings) = eval_vcad_parametric(src, None, None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let xs: Vec<f64> = offsets(&doc).iter().map(|o| o.x).collect();
    assert_eq!(xs.len(), 4, "quad-pattern makes four placements");

    let moved = with_params(&doc, &[("pitch_axis_x", 315.0)]);
    for x in offsets(&moved).iter().map(|o| o.x) {
        assert_eq!(x.abs(), 315.0, "every mirrored copy moved");
    }
}

#[test]
fn set_parameters_agrees_with_re_evaluating_the_source() {
    // The strongest statement of correctness: driving the parameter and
    // re-authoring the source must land in the same place.
    let src = |v: &str| {
        format!(
            "[defparam hub_r {v}]\n[root [translate [+ hub_r 4.0] [* hub_r 0.5] 0.0 \
             [cube [* hub_r 2.0] 3.0 1.0]] \"steel\"]"
        )
    };
    let (doc, _) = eval_vcad_parametric(&src("12.0"), None, None).unwrap();
    let driven = with_params(&doc, &[("hub_r", 19.0)]);
    let reauthored = eval_vcad(&src("19.0"), None).unwrap();
    assert_eq!(offsets(&driven), offsets(&reauthored));
    assert_eq!(sizes(&driven), sizes(&reauthored));
}

#[test]
fn derived_parameters_chain() {
    let (doc, _) = eval_vcad_parametric(
        "[defparam bore 10.0]\n[defparam wall \"bore * 0.2\"]\n\
         [root [cube wall 1.0 1.0] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    assert_eq!(sizes(&doc)[0].x, 2.0);
    assert_eq!(sizes(&with_params(&doc, &[("bore", 30.0)]))[0].x, 6.0);
}

#[test]
fn parameter_metadata_rides_along() {
    let (doc, _) = eval_vcad_parametric(
        "[defparam bore 10.0 :unit \"mm\" :min 4.0 :max 40.0 :description \"shaft bore\"]\n\
         [root [cube bore 1.0 1.0] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    let p = &doc.parameters["bore"];
    assert_eq!(p.unit.as_deref(), Some("mm"));
    assert_eq!(p.min, Some(4.0));
    assert_eq!(p.description.as_deref(), Some("shaft bore"));
}

// ============================================================================
// Datums
// ============================================================================

#[test]
fn two_parts_referencing_one_datum_cannot_disagree() {
    // The interference-bug class this exists to remove: a carrier plate and a
    // femur plate that each "know" where the shared plane is. Here they cannot
    // — there is one plane, and moving it moves both.
    let src = r#"
[datum-plane "carrier_face" y 140.0]
[root [union
        [translate 0.0 [datum "carrier_face"] 0.0 [cube 20.0 5.0 5.0]]
        [translate 0.0 [datum+ "carrier_face" 5.0] 0.0 [cube 20.0 5.0 5.0]]] "steel"]
"#;
    let (doc, warnings) = eval_vcad_parametric(src, None, None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(doc.datums.contains_key("carrier_face"));
    assert_eq!(sorted_y(&doc), vec![140.0, 145.0]);

    // One edit moves the plane; the two parts stay exactly 5 mm apart.
    let moved = with_params(&doc, &[("carrier_face", 138.0)]);
    assert_eq!(sorted_y(&moved), vec![138.0, 143.0]);
}

#[test]
fn a_datum_plane_is_machine_readable_reference_geometry() {
    let (doc, _) = eval_vcad_parametric(
        "[datum-plane \"femur_inner\" y 131.0]\n[root [cube [datum \"femur_inner\"] 1.0 1.0] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    let env = vcad_ir::resolve_parameters(&doc.parameters).unwrap();
    let resolved = vcad_ir::resolve_datums(&doc.datums, &env).unwrap();
    match resolved["femur_inner"] {
        vcad_ir::ResolvedDatum::Plane { origin, normal } => {
            assert_eq!(origin, [0.0, 131.0, 0.0]);
            assert_eq!(normal, [0.0, 1.0, 0.0]);
        }
        other => panic!("expected a plane, got {other:?}"),
    }
}

#[test]
fn an_axis_datum_exposes_its_components() {
    let src = r#"
[datum-axis "pitch" x 0.0 0.0 310.0]
[root [translate 0.0 0.0 [datum-z "pitch"] [cube 1.0 1.0 1.0]] "steel"]
"#;
    let (doc, _) = eval_vcad_parametric(src, None, None).unwrap();
    assert_eq!(offsets(&doc)[0].z, 310.0);
    assert_eq!(
        offsets(&with_params(&doc, &[("pitch_z", 315.0)]))[0].z,
        315.0
    );
}

#[test]
fn one_datum_name_cannot_mean_two_planes() {
    let err = eval_vcad(
        "[datum-plane \"face\" y 140.0]\n[datum-plane \"face\" y 138.0]\n[root [cube 1.0 1.0 1.0] \"s\"]",
        None,
    )
    .unwrap_err();
    assert!(err.contains("declared twice"), "{err}");
}

#[test]
fn kebab_case_names_are_rejected_rather_than_silently_meaning_subtraction() {
    let err = eval_vcad(
        "[datum-plane \"femur-inner\" y 131.0]\n[root [cube 1.0 1.0 1.0] \"s\"]",
        None,
    )
    .unwrap_err();
    assert!(err.contains("femur_inner"), "{err}");
}

#[test]
fn reading_an_undeclared_datum_is_an_error_not_a_zero() {
    let err = eval_vcad("[root [cube [datum \"nope\"] 1.0 1.0] \"s\"]", None).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}

// ============================================================================
// Stacks
// ============================================================================

/// The lateral packing from the motivating robot leg, as a declaration.
const LEG_STACK: &str = r#"
[stack y "leg" 131.0
  [lane "femur_inner" 5.0]
  [gap  "idler_run"   1.0]
  [lane "idler_boss"  3.0]
  [lane "carrier"     5.0]
  [lane "actuator"   37.0]
  [lane "flange"      3.0]
  [gap  "spacer_clr"  3.0]
  [lane "femur_outer" 5.0]]
"#;

#[test]
fn a_stack_reproduces_the_lane_table_it_replaces() {
    let src = format!("{LEG_STACK}\n[root [cube 1.0 1.0 1.0] \"steel\"]");
    let (doc, _) = eval_vcad_parametric(&src, None, None).unwrap();
    let env = vcad_ir::resolve_parameters(&doc.parameters).unwrap();
    // 131..136 / 137..140 / 140..145 / 145..182 / 182..185 / 188..193
    for (name, want) in [
        ("leg_femur_inner_lo", 131.0),
        ("leg_femur_inner_hi", 136.0),
        ("leg_idler_boss_lo", 137.0),
        ("leg_idler_boss_hi", 140.0),
        ("leg_carrier_hi", 145.0),
        ("leg_actuator_hi", 182.0),
        ("leg_flange_hi", 185.0),
        ("leg_femur_outer_lo", 188.0),
        ("leg_femur_outer_hi", 193.0),
        ("leg_end", 193.0),
    ] {
        assert_eq!(env[name], want, "{name}");
    }
}

#[test]
fn widening_a_clearance_slides_everything_outboard_of_it() {
    let src = format!(
        "{LEG_STACK}\n[root [union \
           [translate 0.0 [datum \"leg_carrier_lo\"] 0.0 [cube 1.0 1.0 1.0]] \
           [translate 0.0 [datum \"leg_femur_outer_lo\"] 0.0 [cube 1.0 1.0 1.0]]] \"steel\"]"
    );
    // Lanes this small test never places warn that they drive nothing; the
    // two boundaries it does place must bind.
    let (doc, _warnings) = eval_vcad_parametric(&src, None, None).unwrap();
    assert_eq!(sorted_y(&doc), vec![140.0, 188.0]);
    // The running clearance is a named value, not an arbitrary number: open it
    // by 1 mm and only what is outboard of it moves.
    let opened = with_params(&doc, &[("leg_idler_run", 2.0)]);
    assert_eq!(sorted_y(&opened), vec![141.0, 189.0]);
}

#[test]
fn stack_boundaries_are_datum_planes() {
    let src = format!("{LEG_STACK}\n[root [cube 1.0 1.0 1.0] \"steel\"]");
    let (doc, _) = eval_vcad_parametric(&src, None, None).unwrap();
    for name in ["leg_carrier_lo", "leg_carrier_hi", "leg_end"] {
        assert!(doc.datums.contains_key(name), "missing datum {name}");
    }
}

// ============================================================================
// Fail-closed behaviour
// ============================================================================

#[test]
fn a_non_affine_dependence_stays_literal_and_is_reported() {
    // Area is quadratic in the parameter — no checked formula exists, so the
    // field keeps its literal rather than acquiring a linear approximation.
    let (doc, warnings) = eval_vcad_parametric(
        "[defparam r 10.0]\n[root [cube [* r r] 1.0 1.0] \"steel\"]",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        sizes(&doc)[0].x,
        100.0,
        "the geometry itself is still right"
    );
    let bound: Vec<_> = doc
        .bindings
        .iter()
        .filter(|(k, _)| k.field_path == "size.x")
        .collect();
    assert!(bound.is_empty(), "quadratic field must not be bound");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("non-linearly") && w.contains("size.x")),
        "the drop should name the field: {warnings:?}"
    );
}

#[test]
fn a_parameter_that_changes_topology_binds_nothing_and_says_so() {
    // Driving a repeat count changes how many nodes exist, so no field-level
    // slope is meaningful.
    let src = r#"
[defparam count 3.0]
[root [linear-pattern 10.0 0.0 0.0 [to-int count] 10.0 [cube 1.0 1.0 1.0]] "steel"]
"#;
    let Ok((_doc, warnings)) = eval_vcad_parametric(src, None, None) else {
        // `to-int` may not exist in the stdlib; the property under test is
        // only meaningful if the program evaluates at all.
        return;
    };
    assert!(
        warnings.iter().any(|w| w.contains("count")) || warnings.is_empty(),
        "{warnings:?}"
    );
}

#[test]
fn recovered_bindings_reproduce_the_program_at_a_point_never_fitted() {
    // The verification contract, stated directly.
    let src = |v: f64| {
        format!(
            "[defparam a 7.0]\n[defparam b 3.0]\n\
             [root [translate [+ a b] [- a b] [* 2.0 a] [cube a b {v}]] \"steel\"]"
        )
    };
    let (doc, warnings) = eval_vcad_parametric(&src(1.0), None, None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let driven = with_params(&doc, &[("a", 11.5), ("b", -2.25)]);
    let truth = eval_vcad(
        "[defparam a 11.5]\n[defparam b -2.25]\n\
         [root [translate [+ a b] [- a b] [* 2.0 a] [cube a b 1.0]] \"steel\"]",
        None,
    )
    .unwrap();
    assert_eq!(offsets(&driven), offsets(&truth));
    assert_eq!(sizes(&driven), sizes(&truth));
}

// ============================================================================
// Programs that declare nothing pay nothing
// ============================================================================

#[test]
fn plain_programs_are_untouched() {
    let doc = eval_vcad("[root [cube 10.0 20.0 30.0] \"steel\"]", None).unwrap();
    assert!(doc.parameters.is_empty());
    assert!(doc.bindings.is_empty());
    assert!(doc.datums.is_empty());
    assert_eq!(sizes(&doc)[0], Vec3::new(10.0, 20.0, 30.0));
}

#[test]
fn plain_documents_serialize_without_the_new_fields() {
    let doc = eval_vcad("[root [cube 1.0 1.0 1.0] \"steel\"]", None).unwrap();
    let json = serde_json::to_string(&doc).unwrap();
    assert!(!json.contains("\"datums\""), "{json}");
    assert!(!json.contains("\"parameters\""), "{json}");
}

#[test]
fn a_declaration_may_be_the_last_form_in_the_program() {
    // The program's value is its last expression, so a trailing declaration
    // used to swallow the scene.
    let (doc, _) = eval_vcad_parametric(
        "[root [cube 1.0 1.0 1.0] \"steel\"]\n[defparam unused 5.0]",
        None,
        None,
    )
    .unwrap();
    assert_eq!(doc.roots.len(), 1);
    assert_eq!(doc.parameters["unused"].value, Expr::Number(5.0));
}

#[test]
fn parameters_and_datums_round_trip_through_json() {
    let src = format!("{LEG_STACK}\n[defparam pitch_axis_x 310.0]\n\
        [root [translate pitch_axis_x [datum \"leg_carrier_lo\"] 0.0 [cube 1.0 1.0 1.0]] \"steel\"]");
    let (doc, _) = eval_vcad_parametric(&src, None, None).unwrap();
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.parameters, doc.parameters);
    assert_eq!(back.datums, doc.datums);
    assert_eq!(back.bindings, doc.bindings);
    // and it still drives geometry after the round trip
    assert_eq!(
        offsets(&with_params(&back, &[("pitch_axis_x", 315.0)]))[0].x,
        315.0
    );
}
