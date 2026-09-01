//! The assembly document as authored: loon source → IR → posed → checked.
//!
//! This is the end the rest of the toolchain consumes. If a probe script, a
//! viewer and a kit layout all read the poses from one of these documents,
//! there is nothing left to keep in sync by hand.

use vcad_kernel_assembly::{check_interference, check_mates, pose_document, InterferenceOptions};

/// A two-rotor stack authored in loon. `clock` is the front rotor's clocking.
fn stack_source(clock: f64) -> String {
    format!(
        r#"
[let shaft [cylinder-n 3.0 40.0 32]]
[let disc  [cylinder-n 20.0 4.0 48]]

[assembly
  #[[part "shaft" shaft "steel"]
    [part "disc"  disc  "aluminum"]]
  #[[instance "shaft" "shaft" 0.0 0.0 0.0]
    ; rear rotor: seated at z5, exploded downward
    [instance-exploded "disc-rear"  "disc" 0.0 0.0  5.0  0.0 0.0 0.0   0.0 0.0 -15.0]
    ; front rotor: FLIPPED about X and clocked about Z
    [instance-exploded "disc-front" "disc" 0.0 0.0 15.0  180.0 0.0 {clock}  0.0 0.0 15.0]]
  #[]
  "shaft"]

[mate-coaxial       "discs-coaxial" "disc-rear" "disc-front" 0.0 0.0 1.0 0.01]
[mate-planar-offset "disc-stack"    "disc-rear" "disc-front" 0.0 0.0 1.0 10.0 0.01]
[mate-pattern-phase "pole-phase"    "disc-rear" "disc-front" 10.0 0.0 0.0 1.0 0.0 0.0 0.5]
"#
    )
}

fn checks_for(clock: f64) -> Vec<vcad_kernel_assembly::MateCheck> {
    let doc = vcad_loon::eval_vcad(&stack_source(clock), None).expect("loon evaluates");
    assert_eq!(doc.mates.len(), 3, "three mates should reach the document");
    let posed = pose_document(&doc).expect("assembly poses");
    assert_eq!(posed.parts.len(), 3);
    check_mates(&posed, &doc.mates).expect("mates resolve")
}

#[test]
fn a_loon_assembly_round_trips_into_posed_parts_and_mates() {
    let doc = vcad_loon::eval_vcad(&stack_source(180.0), None).unwrap();

    // Instances carry the full pose, not just a translation.
    let instances = doc.instances.as_ref().unwrap();
    let front = instances.iter().find(|i| i.id == "disc-front").unwrap();
    let t = front
        .transform
        .as_ref()
        .expect("front rotor has a transform");
    assert_eq!(t.rotation.x, 180.0);
    assert_eq!(t.rotation.z, 180.0);
    // ...and its exploded-view offset.
    assert_eq!(front.explode.unwrap().z, 15.0);

    // The shaft declares no offset — it is the datum of the exploded view.
    let shaft = instances.iter().find(|i| i.id == "shaft").unwrap();
    assert!(shaft.explode.is_none());

    let posed = pose_document(&doc).unwrap();
    // The flipped rotor really is upside down in world space.
    let z = posed
        .get("disc-front")
        .unwrap()
        .transform
        .direction([0.0, 0.0, 1.0]);
    assert!((z[2] + 1.0).abs() < 1e-9, "front rotor should be flipped");
}

#[test]
fn clock_60_fails_and_clock_180_passes_from_loon_source() {
    let bad = checks_for(60.0);
    let phase = bad.iter().find(|c| c.id == "pole-phase").unwrap();
    assert!(!phase.pass, "clock 60 must FAIL: {}", phase.summary());
    assert!(
        (phase.measured.abs() - 12.0).abs() < 1e-6,
        "{}",
        phase.summary()
    );

    let good = checks_for(180.0);
    for c in &good {
        assert!(c.pass, "clock 180 must pass every mate: {}", c.summary());
    }
}

#[test]
fn the_authored_stack_is_free_of_interference() {
    let doc = vcad_loon::eval_vcad(&stack_source(180.0), None).unwrap();
    let posed = pose_document(&doc).unwrap();
    let report = check_interference(
        &posed,
        &InterferenceOptions {
            // The discs are solid stand-ins, so the shaft passes through
            // them; a real carrier would have a bore.
            ignore_pairs: vec![
                ("shaft".into(), "disc-rear".into()),
                ("shaft".into(), "disc-front".into()),
            ],
            ..InterferenceOptions::default()
        },
    );
    assert!(report.is_clean(), "{}", report.summary());
    // The discs sit at z 5..9 and 11..15, so the broad phase rejects the one
    // remaining pair outright — no narrow-phase query needed.
    assert_eq!(report.pairs_tested, 0);
}

#[test]
fn a_non_integer_pole_count_is_rejected_at_convert_time() {
    let src = r#"
[assembly #[[part "d" [cylinder-n 5.0 2.0 16] "steel"]]
          #[[instance "a" "d" 0.0 0.0 0.0] [instance "b" "d" 0.0 0.0 5.0]]
          #[] "a"]
[mate-pattern-phase "bad" "a" "b" 10.5 0.0 0.0 1.0 0.0 0.0 0.5]
"#;
    let err = vcad_loon::eval_vcad(src, None).unwrap_err();
    assert!(err.contains("whole number"), "{err}");
}
