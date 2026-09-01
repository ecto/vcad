//! Variant parameter tables, tested against the real rana case.
//!
//! The property under test: a 0.6× print mule shrinks what the envelope
//! drives and nothing else, an override made on the mule is visible as an
//! override rather than as noise, and asking the scale to touch a held
//! parameter fails loudly instead of shrinking a gear module.

use vcad_loon::variants::{self, Source};

const RANA: &str = include_str!("fixtures/rana.params.loon");

fn set() -> variants::VariantSet {
    variants::parse(RANA).expect("parse rana table")
}

#[test]
fn base_table_resolves_to_its_own_values() {
    let r = set().resolve("rana").unwrap();
    assert_eq!(r.value("envelope_d"), Some(100.0));
    assert_eq!(r.value("gear_module"), Some(0.5));
    assert_eq!(r.value("pocket_clearance"), Some(0.2));
    assert_eq!(r.effective_scale, 1.0);
    // rotor_r = 100/2 - 2.4
    assert_eq!(r.value("rotor_r"), Some(47.6));
    // pocket_w = 10 + 2*0.2
    assert!((r.value("pocket_w").unwrap() - 10.4).abs() < 1e-12);
}

#[test]
fn envelope_scales_and_held_parameters_do_not() {
    let r = set().resolve("rana_60c").unwrap();
    assert_eq!(r.chain, vec!["rana".to_string(), "rana_60c".to_string()]);
    assert_eq!(r.effective_scale, 0.6);

    // Envelope-driven: scaled.
    assert!((r.value("envelope_d").unwrap() - 60.0).abs() < 1e-12);
    assert!((r.value("shell_wall").unwrap() - 1.44).abs() < 1e-12);
    // Derived from scaled inputs — recomputed, never scaled twice.
    assert!((r.value("rotor_r").unwrap() - (30.0 - 1.44)).abs() < 1e-12);

    // Held: the m0.5 / 0.4 wall / M3 / COTS-magnet class.
    assert_eq!(r.value("gear_module"), Some(0.5));
    assert_eq!(r.value("min_wall"), Some(0.4));
    assert_eq!(r.value("bolt_d"), Some(3.0));
    assert_eq!(r.value("magnet_block_w"), Some(10.0));
    assert_eq!(r.value("magnet_block_l"), Some(20.0));
    assert_eq!(r.value("magnet_block_h"), Some(3.0));
}

#[test]
fn mule_override_lands_verbatim_on_the_scaled_table() {
    let r = set().resolve("rana_60c").unwrap();
    // The mule measured 0.4 at 0.6× — not 0.2 × 0.6.
    assert_eq!(r.value("pocket_clearance"), Some(0.4));
    // And the pocket that consumes it recomputes: 10 (held) + 2 × 0.4.
    assert!((r.value("pocket_w").unwrap() - 10.8).abs() < 1e-12);
}

#[test]
fn every_resolved_value_knows_where_it_came_from() {
    let r = set().resolve("rana_60c").unwrap();

    match &r.get("envelope_d").unwrap().source {
        Source::ScaleDerived {
            variant,
            factor,
            from,
            explicit,
        } => {
            assert_eq!(variant, "rana_60c");
            assert_eq!((*factor, *from, *explicit), (0.6, 100.0, false));
        }
        other => panic!("envelope_d: expected scale-derived, got {other:?}"),
    }

    match &r.get("gear_module").unwrap().source {
        Source::Held {
            table,
            skipped_factor,
            flag,
        } => {
            assert_eq!(table, "rana");
            assert_eq!(*skipped_factor, 0.6);
            assert_eq!(*flag, "scale_with_envelope");
        }
        other => panic!("gear_module: expected held, got {other:?}"),
    }

    match &r.get("pocket_clearance").unwrap().source {
        Source::Override { variant, why } => {
            assert_eq!(variant, "rana_60c");
            assert!(why.as_deref().unwrap().contains("mule"));
        }
        other => panic!("pocket_clearance: expected override, got {other:?}"),
    }

    assert!(matches!(
        r.get("rotor_r").unwrap().source,
        Source::Derived { .. }
    ));
    assert!(matches!(
        set()
            .resolve("rana")
            .unwrap()
            .get("envelope_d")
            .unwrap()
            .source,
        Source::Base { .. }
    ));
}

#[test]
fn diff_names_the_override_and_separates_it_from_the_scale() {
    let d = set().diff("rana", "rana_60c").unwrap();
    let names = d.names();

    // The one own-override in the family.
    let clearance = d
        .entries
        .iter()
        .find(|e| e.name == "pocket_clearance")
        .expect("pocket_clearance differs");
    assert!(clearance.reason.contains("own override in 'rana_60c'"));
    assert!(clearance.reason.contains("mule"));
    assert_eq!(clearance.a.as_ref().unwrap().value, 0.2);
    assert_eq!(clearance.b.as_ref().unwrap().value, 0.4);

    // Scale-derived differences are reported as scale, not as edits.
    let envelope = d.entries.iter().find(|e| e.name == "envelope_d").unwrap();
    assert!(envelope.reason.contains("envelope scale"));

    // Held parameters do not appear at all.
    for held in [
        "gear_module",
        "min_wall",
        "bolt_d",
        "magnet_block_w",
        "magnet_block_l",
        "magnet_block_h",
    ] {
        assert!(
            !names.contains(&held),
            "{held} should not differ between rana and its 0.6x mule, diff was:\n{}",
            d.render()
        );
    }

    // Exactly one entry is an author's decision; the rest follow from scale.
    let overridden: Vec<_> = d
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.b.as_ref().map(|p| &p.source),
                Some(Source::Override { .. })
            )
        })
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(overridden, vec!["pocket_clearance"]);
}

#[test]
fn scaling_a_held_parameter_is_a_loud_error() {
    let src = format!(
        "{RANA}\n[defvariant rana_60c_bad :from rana :scale 0.6\n  [scale gear_module 0.6]]\n"
    );
    let err = variants::parse(&src)
        .unwrap()
        .resolve("rana_60c_bad")
        .unwrap_err();
    assert!(err.contains("gear_module"), "{err}");
    assert!(err.contains("scale_with_envelope"), "{err}");
    assert!(err.contains("PLA tooth floor"), "{err}");
    assert!(err.contains("[override gear_module"), "{err}");
}

#[test]
fn scaling_a_scalable_parameter_explicitly_is_allowed_and_recorded() {
    let src = format!(
        "{RANA}\n[defvariant rana_thin :from rana\n  [scale shell_wall 0.5 :why \"thin-wall trial\"]]\n"
    );
    let r = variants::parse(&src).unwrap().resolve("rana_thin").unwrap();
    assert_eq!(r.value("shell_wall"), Some(1.2));
    match &r.get("shell_wall").unwrap().source {
        Source::ScaleDerived { explicit, .. } => assert!(*explicit),
        other => panic!("expected explicit scale, got {other:?}"),
    }
}

#[test]
fn variants_chain_and_scales_compose() {
    let src = format!(
        "{RANA}\n[defvariant rana_30 :from rana_60c :scale 0.5\n  [override shell_wall 1.0]]\n"
    );
    let r = variants::parse(&src).unwrap().resolve("rana_30").unwrap();
    assert_eq!(r.chain, vec!["rana", "rana_60c", "rana_30"]);
    assert_eq!(r.effective_scale, 0.3);
    assert!((r.value("envelope_d").unwrap() - 30.0).abs() < 1e-12);
    // The inherited override rides through the second scale as a literal.
    assert!((r.value("pocket_clearance").unwrap() - 0.2).abs() < 1e-12);
    // Held stays held through the whole chain.
    assert_eq!(r.value("gear_module"), Some(0.5));
    assert_eq!(r.value("shell_wall"), Some(1.0));
}

#[test]
fn a_formula_cannot_be_flagged_no_scale() {
    let err = variants::parse(
        "[deftable t [defparam a 1.0] [defparam b \"a * 2\" :scale_with_envelope false]]",
    )
    .unwrap_err();
    assert!(err.contains("derived value follows its inputs"), "{err}");
}

#[test]
fn overlaying_an_undeclared_parameter_is_an_error() {
    let src = format!("{RANA}\n[defvariant oops :from rana [override nonesuch 1.0]]\n");
    let err = variants::parse(&src).unwrap().resolve("oops").unwrap_err();
    assert!(err.contains("nonesuch"), "{err}");
    assert!(err.contains("does not declare"), "{err}");
}

#[test]
fn an_unknown_parent_is_reported_at_parse_time() {
    let src = format!("{RANA}\n[defvariant orphan :from nowhere]\n");
    let err = variants::parse(&src).unwrap_err();
    assert!(err.contains("orphan"), "{err}");
    assert!(err.contains("nowhere"), "{err}");
}

#[test]
fn resolved_table_serializes_flat_with_provenance() {
    let r = set().resolve("rana_60c").unwrap();
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["name"], "rana_60c");
    assert_eq!(j["effective_scale"], 0.6);
    let params = j["params"].as_array().unwrap();
    let gear = params
        .iter()
        .find(|p| p["name"] == "gear_module")
        .expect("gear_module in json");
    assert_eq!(gear["value"], 0.5);
    assert_eq!(gear["scale_with_envelope"], false);
    assert_eq!(gear["source"]["kind"], "held");
    assert_eq!(gear["source"]["flag"], "scale_with_envelope");
}
