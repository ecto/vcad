//! The registry's own tests.
//!
//! What is worth asserting here is not "the registry has entries" — that is
//! the existence test the house rules forbid. It is that every family
//! compiled in is reachable *and translates a real report*, that a schema
//! nobody registered is an error rather than an empty claim list, and that
//! the CAM ladder survives the round trip through the registry with its
//! statuses intact: a passing job's computed claims `Pass` on a verified
//! basis, its predictions `Pass` only on a predicted basis (so the receipt
//! rolls up Provisional, never Pass), and a gouging job `Fail`s.

use std::collections::BTreeMap;

use super::*;

#[test]
fn every_compiled_family_is_reachable_by_its_own_schema() {
    let all = families();
    assert!(
        !all.is_empty(),
        "a build with no families registered can never certify anything"
    );
    for f in &all {
        let found = find(f.schema).unwrap_or_else(|| panic!("{} is not findable", f.schema));
        assert_eq!(found.domain, f.domain);
        assert!(
            f.schema.starts_with("vcad.") && f.schema.ends_with("/1"),
            "{} is not a versioned vcad schema tag",
            f.schema
        );
        assert!(!f.domain.is_empty(), "{} has no receipt domain", f.schema);
    }
    // Schemas are the key: two families under one key would silently shadow.
    let mut seen: Vec<&str> = all.iter().map(|f| f.schema).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(before, seen.len(), "duplicate schema in the registry");
}

#[test]
fn the_default_build_carries_the_families_wave_1_and_2_shipped() {
    let schemas: Vec<&str> = families().iter().map(|f| f.schema).collect();
    for wanted in [
        "vcad.cam-claims/1",
        "vcad.tolerance-claims/1",
        "vcad.thermal-claims/1",
        "vcad.particle-claims/1",
        "vcad.fea-claims/1",
    ] {
        assert!(
            schemas.contains(&wanted),
            "{wanted} is missing from {schemas:?}"
        );
    }
}

#[test]
fn an_unregistered_schema_is_an_error_not_an_empty_claim_list() {
    // Fail-closed: an empty Vec here would let `build_receipt` merge nothing
    // and roll the receipt up as if the family had been checked.
    let e = claims_for("vcad.astrology-claims/1", "{}").unwrap_err();
    match &e {
        RegistryError::UnknownSchema { schema, known } => {
            assert_eq!(schema, "vcad.astrology-claims/1");
            // The message has to name what IS available, or the caller is
            // left guessing at the spelling.
            assert!(known.contains("vcad.cam-claims/1"), "{known}");
        }
        other => panic!("expected UnknownSchema, got {other:?}"),
    }
    assert!(find("vcad.astrology-claims/1").is_none());
}

#[cfg(feature = "cam")]
mod cam_family {
    use super::*;
    use vcad_kernel_cam::receipt as cam;
    use vcad_kernel_cam::verify2d::{verify_gcode, JobSpec, PartRegion, VerifyOptions};
    use vcad_receipt::{ClaimBasis, ClaimVerdict, DesignReceipt, ReceiptVerdict};

    const SCHEMA: &str = "vcad.cam-claims/1";

    /// A 20 mm square part at (10,10)–(30,30) in a 40 × 40 × 3 mm blank —
    /// the same fixture `vcad-kernel-cam`'s own receipt tests use.
    fn square_part() -> PartRegion {
        PartRegion::new(
            vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]],
            vec![],
        )
        .unwrap()
    }

    /// An outside contour with the tool centre `offset` mm clear of the wall.
    /// At 2.0 a Ø4 cutter just clears the part; below that it eats into it.
    fn ring_job_gcode(offset: f64) -> String {
        let (lo, hi) = (10.0 - offset, 30.0 + offset);
        let mut g = String::from("G21\nG90\n");
        for z in [-0.9, -1.8, -2.7] {
            g.push_str("G0 Z5.000\n");
            g.push_str(&format!("G0 X{lo:.3} Y{lo:.3}\n"));
            g.push_str(&format!("G1 Z{z:.3} F100.000\n"));
            for (x, y) in [(hi, lo), (hi, hi), (lo, hi), (lo, lo)] {
                g.push_str(&format!("G1 X{x:.3} Y{y:.3} F500.000\n"));
            }
        }
        g.push_str("G0 Z5.000\n");
        g
    }

    fn square_spec() -> JobSpec {
        let mut s = JobSpec::new(square_part(), 3.0, 4.0);
        s.bottom_allowance = 0.3;
        s.stock_bbox = Some([0.0, 0.0, 40.0, 40.0]);
        s
    }

    /// A job's claim set, serialized, with the inputs it rests on.
    fn deposit(offset: f64) -> (String, BTreeMap<String, String>) {
        let gcode = ring_job_gcode(offset);
        let spec = square_spec();
        let opts = VerifyOptions::default();
        let v = verify_gcode(&gcode, &spec, &opts).unwrap();

        let inputs: BTreeMap<String, String> = [
            (
                "program".to_string(),
                serde_json::to_string(&gcode).unwrap(),
            ),
            (
                "outline".to_string(),
                serde_json::to_string(&spec.part.outer).unwrap(),
            ),
            ("tool".to_string(), "4.0".to_string()),
            ("stock".to_string(), serde_json::to_string(&spec).unwrap()),
        ]
        .into_iter()
        .collect();

        let mut fp = cam::Fingerprint::new();
        for (k, d) in fingerprint_of(&inputs) {
            fp = fp.with(k, d);
        }
        let set = cam::ClaimSet::new(
            fp,
            cam::Provenance {
                oracles: vec!["verify2d".into()],
                ..Default::default()
            },
            cam::job_claims(&v, &spec, &opts),
        );
        (serde_json::to_string(&set).unwrap(), inputs)
    }

    fn claim<'a>(claims: &'a [ReceiptClaim], id: &str) -> &'a ReceiptClaim {
        claims
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no claim {id} in {:?}", ids(claims)))
    }

    fn ids(claims: &[ReceiptClaim]) -> Vec<&str> {
        claims.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn a_clearing_job_passes_its_computed_claims_and_only_predicts_the_rest() {
        let (report, _) = deposit(2.0);
        let claims = claims_for(SCHEMA, &report).unwrap();

        // Geometry, not existence: the cutter stays out of the part, so the
        // gouge claim passes on a VERIFIED basis — no measurement needed,
        // because no measurement could make arithmetic truer.
        let gouge = claim(&claims, "cam.job.no_gouge");
        assert_eq!(gouge.verdict, ClaimVerdict::Pass);
        assert_eq!(gouge.effective_basis(), ClaimBasis::Verified);
        assert_eq!(gouge.domain, "cam");

        // The depth claim knows the job leaves the declared 0.3 mm skin.
        let depth = claim(&claims, "cam.job.depth");
        assert_eq!(depth.verdict, ClaimVerdict::Pass);

        // This job declares no tabs, so the tab claims are Unverifiable —
        // "nothing to audit" is not "tabs that hold".
        for id in ["cam.job.tabs", "cam.job.tabs_hold"] {
            assert_eq!(
                claim(&claims, id).verdict,
                ClaimVerdict::Unverifiable,
                "{id} should refuse to pass vacuously"
            );
        }
        // …and with no work offset or travel limits, neither does the
        // envelope check.
        assert_eq!(
            claim(&claims, "cam.job.envelope_in_travel").verdict,
            ClaimVerdict::Unverifiable
        );
    }

    #[test]
    fn a_gouging_job_fails_the_gouge_claim_with_the_distance_it_cut_in() {
        // 1.0 mm of tool-centre clearance for a Ø4 cutter: 1 mm of the
        // cutter is inside the wall on every pass.
        let (report, _) = deposit(1.0);
        let claims = claims_for(SCHEMA, &report).unwrap();
        let gouge = claim(&claims, "cam.job.no_gouge");
        assert_eq!(gouge.verdict, ClaimVerdict::Fail);

        // The measured value is how far in it went, and it is about 1 mm —
        // not merely "nonzero".
        let measured = match &gouge.measured.as_ref().expect("a gouge has a depth").value {
            vcad_receipt::ClaimValue::Number(v) => *v,
            other => panic!("expected a number, got {other:?}"),
        };
        assert!(
            (measured - 1.0).abs() < 0.05,
            "the cutter is 1 mm inside the wall; the claim says {measured}"
        );

        // One failing claim fails the whole receipt, on any basis.
        let receipt = DesignReceipt::with_claims(claims);
        assert_eq!(receipt.verdict(), ReceiptVerdict::Fail);
    }

    #[test]
    fn changing_an_input_re_states_the_claims_stale_not_holds() {
        let (report, inputs) = deposit(2.0);
        let before = claims_for(SCHEMA, &report).unwrap();
        assert_eq!(
            claim(&before, "cam.job.no_gouge").verdict,
            ClaimVerdict::Pass
        );

        // Unchanged inputs: the claims stand exactly as they were, and the
        // outcome says nothing moved.
        let intact = restate(SCHEMA, &report, &inputs).unwrap();
        assert!(!intact.is_stale(), "{:?}", intact.stale_claims);
        assert!(intact.drifted_inputs.is_empty());
        assert_eq!(
            claim(
                &claims_for(SCHEMA, &intact.report).unwrap(),
                "cam.job.no_gouge"
            )
            .verdict,
            ClaimVerdict::Pass
        );

        // Edit the program — one coordinate — and every claim resting on it
        // is Stale. Stale is Unverifiable in the unified schema: never a pass.
        let mut edited = inputs.clone();
        let program = edited.get_mut("program").unwrap();
        *program = program.replace("X8.000", "X7.500");
        assert_ne!(edited["program"], inputs["program"], "the edit must bite");

        let stale = restate(SCHEMA, &report, &edited).unwrap();
        assert!(stale.is_stale());
        // The outcome names what moved, so a caller answering Holds-or-Stale
        // never has to diff two reports to work it out.
        assert_eq!(stale.drifted_inputs, vec!["program".to_string()]);
        assert!(
            stale.stale_claims.iter().any(|c| c == "job.no_gouge"),
            "the gouge claim rests on the program: {:?}",
            stale.stale_claims
        );
        let after = claims_for(SCHEMA, &stale.report).unwrap();
        let gouge = claim(&after, "cam.job.no_gouge");
        assert_eq!(gouge.verdict, ClaimVerdict::Unverifiable);
        assert!(
            gouge.details.as_deref().unwrap_or("").contains("program"),
            "a stale claim has to name what moved: {:?}",
            gouge.details
        );

        // A dropped input is a change too — a receipt cannot certify a job
        // it can no longer identify. (It is also a refused *deposit*; here
        // the key is dropped after the fact, which is what an edit that
        // removes an input looks like.)
        let mut dropped = inputs.clone();
        dropped.insert("outline".to_string(), "\"something else\"".to_string());
        let lost = restate(SCHEMA, &report, &dropped).unwrap();
        assert!(lost.is_stale());
        assert_eq!(
            claim(
                &claims_for(SCHEMA, &lost.report).unwrap(),
                "cam.job.material_left"
            )
            .verdict,
            ClaimVerdict::Unverifiable
        );
    }

    #[test]
    fn a_deposit_that_records_no_basis_is_refused() {
        let (report, inputs) = deposit(2.0);

        // The inputs the job actually recorded are accepted.
        check_deposit(SCHEMA, &report, &inputs).expect("a full deposit is fine");

        // Nothing recorded: refused, and the message says what was needed.
        // This is the fail-open hole the check exists to close — a claim with
        // no basis can never be re-stated, so it can never go stale, and
        // "never moved" reads exactly like "still true" a month later.
        let e = check_deposit(SCHEMA, &report, &BTreeMap::new()).unwrap_err();
        match &e {
            RegistryError::MissingBasisInputs {
                schema,
                recorded,
                required,
                missing,
            } => {
                assert_eq!(schema, SCHEMA);
                assert_eq!(recorded, "none");
                for key in ["program", "outline", "tool", "stock"] {
                    assert!(required.contains(key), "{required} omits {key}");
                    assert!(missing.contains(key), "{missing} omits {key}");
                }
            }
            other => panic!("expected MissingBasisInputs, got {other:?}"),
        }
        // …and the whole message is actionable, not just a code.
        let text = e.to_string();
        assert!(text.contains("program"), "{text}");
        assert!(text.contains("never go stale"), "{text}");

        // A deposit that records SOMETHING but not what its own claims rest
        // on is refused too: this is the stricter, report-derived half. A job
        // report filed with a gear as its basis would otherwise read as a
        // permanently fresh claim about a program nobody kept.
        let mut wrong = BTreeMap::new();
        wrong.insert("gear".to_string(), "{}".to_string());
        wrong.insert("tool".to_string(), "4.0".to_string());
        let e = check_deposit(SCHEMA, &report, &wrong).unwrap_err();
        match &e {
            RegistryError::MissingBasisInputs { missing, .. } => {
                assert!(missing.contains("program"), "{missing}");
                assert!(missing.contains("stock"), "{missing}");
            }
            other => panic!("expected MissingBasisInputs, got {other:?}"),
        }

        // And re-stating a basis-less deposit is refused rather than
        // cheerfully reporting that nothing changed — the one answer that
        // must not be reachable without evidence.
        assert!(matches!(
            restate(SCHEMA, &report, &BTreeMap::new()).unwrap_err(),
            RegistryError::MissingBasisInputs { .. }
        ));
    }

    #[test]
    fn every_family_demands_a_basis_even_the_ones_that_track_none() {
        // The solver families record no per-claim basis, so their static
        // requirement is the whole of the gate. It must not be empty, or
        // wiring one of them up would silently reintroduce the hole.
        for family in families() {
            assert!(
                !family.required_basis.is_empty(),
                "{} would accept a deposit with no basis",
                family.schema
            );
        }
        // Concretely, for a family that is not CAM: an empty deposit is
        // refused without the report even having to parse.
        if find("vcad.thermal-claims/1").is_some() {
            let e = check_deposit("vcad.thermal-claims/1", "{}", &BTreeMap::new()).unwrap_err();
            assert!(
                matches!(e, RegistryError::MissingBasisInputs { ref required, .. } if required.contains("spec")),
                "{e:?}"
            );
        }
    }

    /// Module 1.0, 20 teeth — the milestone's brass planet.
    fn planet() -> vcad_kernel_cam::gear::SpurGear {
        vcad_kernel_cam::gear::SpurGear::external(1.0, 20)
            .with_tip_radius(10.89)
            .with_face_width(5.0)
    }

    fn gear_deposit() -> (String, f64) {
        let report = vcad_kernel_cam::gear::GearReport::new(&planet(), 1.0)
            .unwrap()
            .with_pin(&planet(), 1.5)
            .unwrap();
        let nominal = report.over_pins.expect("a pin was chosen").dimension;
        let set = cam::ClaimSet::new(
            cam::Fingerprint::new()
                .with_json(cam::BASIS_GEAR, &planet())
                .with_json(cam::BASIS_TOOL, &1.0),
            cam::Provenance {
                oracles: vec!["gear".into()],
                ..Default::default()
            },
            cam::gear_claims(&report, "planet-20T"),
        );
        (serde_json::to_string(&set).unwrap(), nominal)
    }

    #[test]
    fn a_prediction_is_provisional_until_a_measurement_closes_it() {
        let (report, nominal) = gear_deposit();
        let claims = claims_for(SCHEMA, &report).unwrap();
        let pins = claim(&claims, "cam.gear.over_pins");

        // The mutation check the whole ladder rests on: a predicted claim
        // must never reach a clean pass on its own.
        assert_eq!(pins.verdict, ClaimVerdict::Pass);
        assert_eq!(pins.effective_basis(), ClaimBasis::Predicted);
        assert_eq!(
            DesignReceipt::with_claims(vec![pins.clone()]).verdict(),
            ReceiptVerdict::Provisional,
            "a predicted claim alone must roll up Provisional, never Pass"
        );

        // A reading 3 µm off nominal, allowed 20 µm: it closes, MEASURED.
        let m = cam::Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            nominal + 0.003,
            0.02,
            "Ø1.5 gauge pins, Mitutoyo 293-340",
        );
        let context = serde_json::json!({ "gear": planet() });
        let bound = bind(
            SCHEMA,
            &report,
            &serde_json::to_string(&m).unwrap(),
            &context,
        )
        .unwrap();
        let closed = claims_for(SCHEMA, &bound.report).unwrap();
        let pins = claim(&closed, "cam.gear.over_pins");
        assert_eq!(pins.verdict, ClaimVerdict::Pass);
        assert_eq!(pins.effective_basis(), ClaimBasis::Measured);

        // And the compensation claim for the NEXT part rides alongside,
        // superseding the one just closed — and is itself only predicted.
        let next = claim(&closed, "cam.gear.over_pins.compensated");
        assert_eq!(next.effective_basis(), ClaimBasis::Predicted);
        assert!(
            next.details
                .as_deref()
                .unwrap_or("")
                .contains("\"supersedes\":\"gear.over_pins\""),
            "the compensated claim must say what it supersedes: {:?}",
            next.details
        );
        assert_eq!(bound.derived.len(), 1, "one compensation per reading");
        let offset = bound.derived[0]["tool_normal_offset"].as_f64().unwrap();
        assert!(
            offset.is_finite() && offset.abs() > 0.0 && offset.abs() < 0.01,
            "a 3 µm over-pins error implies a small, finite cutter offset, got {offset}"
        );
    }

    #[test]
    fn a_reading_outside_tolerance_violates_the_claim() {
        let (report, nominal) = gear_deposit();
        // 0.2 mm over nominal against a 0.02 mm band: the teeth are fat.
        let m = cam::Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            nominal + 0.2,
            0.02,
            "Ø1.5 gauge pins",
        );
        let bound = bind(
            SCHEMA,
            &report,
            &serde_json::to_string(&m).unwrap(),
            &serde_json::json!({ "gear": planet() }),
        )
        .unwrap();
        let claims = claims_for(SCHEMA, &bound.report).unwrap();
        let pins = claim(&claims, "cam.gear.over_pins");
        assert_eq!(pins.verdict, ClaimVerdict::Fail);
        assert!(
            pins.details.as_deref().unwrap_or("").contains("Violated"),
            "the violated claim keeps its own status in details: {:?}",
            pins.details
        );
        assert_eq!(
            DesignReceipt::with_claims(claims).verdict(),
            ReceiptVerdict::Fail
        );
    }

    #[test]
    fn a_measurement_of_an_arithmetic_claim_is_refused_not_ignored() {
        let (report, _) = deposit(2.0);
        // `job.no_gouge` is a property of the program. A caliper cannot
        // close it, and pretending it did would launder arithmetic as
        // evidence.
        let m = cam::Measurement::over_pins("job.no_gouge", None, 1.5, 12.0, 0.02, "calipers");
        let e = bind(
            SCHEMA,
            &report,
            &serde_json::to_string(&m).unwrap(),
            &serde_json::Value::Null,
        )
        .unwrap_err();
        assert!(
            matches!(e, RegistryError::Refused(ref s) if s.contains("arithmetic")),
            "expected a refusal naming the reason, got {e:?}"
        );
    }

    #[test]
    fn a_report_filed_under_the_wrong_family_is_refused() {
        let (report, _) = deposit(2.0);
        // Same bytes, wrong door. Translating a CAM set with the tolerance
        // ladder would read as evidence nobody produced.
        let mut other = families()
            .into_iter()
            .map(|f| f.schema)
            .filter(|s| *s != SCHEMA);
        let wrong = other.next().expect("more than one family is registered");
        let e = claims_for(wrong, &report).unwrap_err();
        assert!(
            matches!(
                e,
                RegistryError::SchemaMismatch { .. } | RegistryError::BadReport { .. }
            ),
            "expected a refusal, got {e:?}"
        );
    }

    #[test]
    fn a_family_without_the_machinery_says_so_rather_than_no_opping() {
        assert!(find(SCHEMA).unwrap().stale_aware());
        assert!(find(SCHEMA).unwrap().bindable());

        // Tolerance stackups carry no per-claim input basis, so re-stating
        // one has to be an error: silently returning it unchanged would let
        // a receipt read clean for a design that moved underneath it.
        if let Some(f) = find("vcad.tolerance-claims/1") {
            assert!(!f.stale_aware());
            let e = restate("vcad.tolerance-claims/1", "{}", &BTreeMap::new()).unwrap_err();
            assert!(matches!(e, RegistryError::NotStaleAware { .. }), "{e:?}");
        }
    }
}
