//! The acceptance assembly: a shaft and two discs carrying 10-pole patterns.
//!
//! This is the rana-100b bug reduced to its arithmetic. The front disc is
//! flipped about X and clocked about Z; the poles have a 36° pitch, so a
//! clocking of 60° lands 24° off (folded to a 12° misalignment) while 180°
//! lands exactly on a pole. The pattern-phase mate must fail the first and
//! pass the second — and it must do so from the poses alone, with nobody
//! having to redo the modular arithmetic by hand.

use vcad_ir::{Document, Instance, Mate, MateKind, Node, NodeId, PartDef, Transform3D, Vec3};
use vcad_kernel_assembly::{check_interference, check_mates, pose_document, InterferenceOptions};

const POLES: u32 = 10;

/// Build the toy document. `front_clock_deg` is the clocking applied to the
/// flipped front disc; `disc_gap` sets the front disc's z so the two discs are
/// separated (or, with a smaller value, deliberately overlapping).
fn toy(front_clock_deg: f64, front_z: f64) -> Document {
    let mut doc = Document::new();
    let mut next: NodeId = 1;
    let mut add = |doc: &mut Document, op: vcad_ir::CsgOp| -> NodeId {
        let id = next;
        next += 1;
        doc.nodes.insert(id, Node { id, op, name: None });
        id
    };

    // Shaft: a slender cylinder on the Z axis, spanning the whole stack.
    let shaft = add(
        &mut doc,
        vcad_ir::CsgOp::Cylinder {
            radius: 3.0,
            height: 40.0,
            segments: 32,
        },
    );
    // Disc: a plain cylinder standing in for a pole carrier. The magnet
    // pattern is described by the mate, not modelled — a pattern-phase mate
    // is arithmetic over poses, so it does not need the pockets to exist.
    let disc = add(
        &mut doc,
        vcad_ir::CsgOp::Cylinder {
            radius: 20.0,
            height: 4.0,
            segments: 48,
        },
    );

    let mut defs = std::collections::HashMap::new();
    defs.insert(
        "shaft".to_string(),
        PartDef {
            id: "shaft".into(),
            name: Some("shaft".into()),
            root: shaft,
            default_material: Some("steel".into()),
            inertial: None,
            colliders: None,
        },
    );
    defs.insert(
        "disc".to_string(),
        PartDef {
            id: "disc".into(),
            name: Some("disc".into()),
            root: disc,
            default_material: Some("aluminum".into()),
            inertial: None,
            colliders: None,
        },
    );
    doc.part_defs = Some(defs);

    let inst = |id: &str, part: &str, t: Transform3D, explode: Option<Vec3>| Instance {
        id: id.into(),
        part_def_id: part.into(),
        name: Some(id.into()),
        tags: Vec::new(),
        transform: Some(t),
        material: None,
        explode,
    };

    doc.instances = Some(vec![
        inst("shaft", "shaft", Transform3D::identity(), None),
        inst(
            "disc-rear",
            "disc",
            Transform3D {
                translation: Vec3::new(0.0, 0.0, 5.0),
                ..Transform3D::default()
            },
            Some(Vec3::new(0.0, 0.0, -15.0)),
        ),
        inst(
            "disc-front",
            "disc",
            // Flip about X, then clock about Z — the rana rotor convention.
            Transform3D {
                translation: Vec3::new(0.0, 0.0, front_z),
                rotation: Vec3::new(180.0, 0.0, front_clock_deg),
                ..Transform3D::default()
            },
            Some(Vec3::new(0.0, 0.0, 15.0)),
        ),
    ]);

    doc.mates = vec![
        Mate {
            id: "discs-coaxial".into(),
            name: None,
            instance_a: "disc-rear".into(),
            instance_b: "disc-front".into(),
            kind: MateKind::Coaxial {
                axis: Vec3::new(0.0, 0.0, 1.0),
                tolerance_mm: 0.01,
                tolerance_deg: 0.5,
            },
        },
        Mate {
            id: "disc-stack".into(),
            name: None,
            instance_a: "disc-rear".into(),
            instance_b: "disc-front".into(),
            kind: MateKind::PlanarOffset {
                axis: Vec3::new(0.0, 0.0, 1.0),
                offset: front_z - 5.0,
                tolerance_mm: 0.01,
            },
        },
        Mate {
            id: "pole-phase".into(),
            name: None,
            instance_a: "disc-rear".into(),
            instance_b: "disc-front".into(),
            kind: MateKind::PatternPhase {
                n_fold: POLES,
                axis: Vec3::new(0.0, 0.0, 1.0),
                phase_a_deg: 0.0,
                phase_b_deg: 0.0,
                expected_clock_deg: None,
                tolerance_deg: 0.5,
            },
        },
    ];

    doc
}

fn phase_check(clock: f64) -> vcad_kernel_assembly::MateCheck {
    let doc = toy(clock, 15.0);
    let posed = pose_document(&doc).expect("assembly poses");
    let checks = check_mates(&posed, &doc.mates).expect("mates resolve");
    checks
        .into_iter()
        .find(|c| c.id == "pole-phase")
        .expect("pattern-phase check present")
}

#[test]
fn flip_and_clock_60_misaligns_a_10_pole_pattern_by_12_degrees() {
    let check = phase_check(60.0);
    assert!(
        !check.pass,
        "clock 60° on a 10-pole pair must FAIL: {}",
        check.summary()
    );
    // 60 mod 36 = 24, folded into (−18, 18] → −12°.
    assert!(
        (check.measured.abs() - 12.0).abs() < 1e-6,
        "expected a 12° misalignment, got {:.6}° — {}",
        check.measured,
        check.summary()
    );
    assert_eq!(check.unit, "deg");
}

#[test]
fn flip_and_clock_180_phase_aligns_a_10_pole_pattern() {
    let check = phase_check(180.0);
    assert!(
        check.pass,
        "clock 180° on a 10-pole pair must PASS: {}",
        check.summary()
    );
    assert!(
        check.measured.abs() < 1e-6,
        "expected exact alignment, got {:.6}°",
        check.measured
    );
}

#[test]
fn clock_300_lands_12_degrees_off_the_other_way() {
    // The third clocking the castellation admits — 300 mod 36 = 12.
    let check = phase_check(300.0);
    assert!(!check.pass, "{}", check.summary());
    assert!((check.measured - 12.0).abs() < 1e-6, "{}", check.summary());
}

#[test]
fn a_whole_pitch_of_clocking_is_no_clocking_at_all() {
    // 36° is exactly one pole pitch: the pattern maps onto itself.
    let check = phase_check(36.0);
    assert!(check.pass, "{}", check.summary());
}

#[test]
fn coaxial_and_planar_offset_hold_on_the_clean_stack() {
    let doc = toy(180.0, 15.0);
    let posed = pose_document(&doc).unwrap();
    let checks = check_mates(&posed, &doc.mates).unwrap();
    assert_eq!(checks.len(), 3);
    for c in &checks {
        assert!(c.pass, "{}", c.summary());
    }
    let coax = checks.iter().find(|c| c.id == "discs-coaxial").unwrap();
    assert!(coax.measured < 1e-9, "{}", coax.summary());
    let stack = checks.iter().find(|c| c.id == "disc-stack").unwrap();
    assert!((stack.measured - 10.0).abs() < 1e-9, "{}", stack.summary());
}

#[test]
fn a_wrong_planar_offset_is_reported_with_the_measured_value() {
    let mut doc = toy(180.0, 15.0);
    // Assert a stack height the transforms do not deliver.
    if let Some(MateKind::PlanarOffset { offset, .. }) = doc
        .mates
        .iter_mut()
        .find(|m| m.id == "disc-stack")
        .map(|m| &mut m.kind)
    {
        *offset = 12.0;
    }
    let posed = pose_document(&doc).unwrap();
    let checks = check_mates(&posed, &doc.mates).unwrap();
    let stack = checks.iter().find(|c| c.id == "disc-stack").unwrap();
    assert!(!stack.pass);
    assert!((stack.measured - 10.0).abs() < 1e-9);
    assert!((stack.expected - 12.0).abs() < 1e-9);
}

#[test]
fn the_assembled_stack_is_free_of_interference() {
    // Discs at z 5..9 and 11..15 (the front one flipped down from z=15),
    // shaft at r=3 through the disc bodies... the discs are solid, so the
    // shaft DOES pass through them: ignore that pair, as a real assembly
    // would model a bore.
    let doc = toy(180.0, 15.0);
    let posed = pose_document(&doc).unwrap();
    let report = check_interference(
        &posed,
        &InterferenceOptions {
            ignore_pairs: vec![
                ("shaft".into(), "disc-rear".into()),
                ("shaft".into(), "disc-front".into()),
            ],
            ..InterferenceOptions::default()
        },
    );
    assert!(report.is_clean(), "{}", report.summary());
}

#[test]
fn a_deliberate_0_1_mm_overlap_is_detected() {
    // Rear disc occupies z 5..9. Put the front disc's underside at 8.9 by
    // seating its flipped body at z = 12.9 — a 0.1 mm interpenetration.
    let doc = toy(180.0, 12.9);
    let posed = pose_document(&doc).unwrap();
    let report = check_interference(
        &posed,
        &InterferenceOptions {
            ignore_pairs: vec![
                ("shaft".into(), "disc-rear".into()),
                ("shaft".into(), "disc-front".into()),
            ],
            ..InterferenceOptions::default()
        },
    );
    assert!(!report.is_clean(), "0.1 mm overlap must be reported");
    let worst = report.worst().unwrap();
    let pair = {
        let mut p = [worst.instance_a.as_str(), worst.instance_b.as_str()];
        p.sort_unstable();
        p
    };
    assert_eq!(pair, ["disc-front", "disc-rear"], "{}", report.summary());
    assert!(
        (worst.depth_mm - 0.1).abs() < 0.02,
        "expected ~0.1 mm of overlap, got {:.4} — {}",
        worst.depth_mm,
        report.summary()
    );
}

#[test]
fn a_tolerance_above_the_overlap_accepts_it_as_modelling_slop() {
    let doc = toy(180.0, 12.9);
    let posed = pose_document(&doc).unwrap();
    let report = check_interference(
        &posed,
        &InterferenceOptions {
            tolerance_mm: 0.2,
            ignore_pairs: vec![
                ("shaft".into(), "disc-rear".into()),
                ("shaft".into(), "disc-front".into()),
            ],
            ..InterferenceOptions::default()
        },
    );
    assert!(report.is_clean(), "{}", report.summary());
    assert_eq!(report.pairs_within_tolerance, 1);
}

#[test]
fn exploded_offsets_move_parts_apart_and_are_declared_on_the_document() {
    let doc = toy(180.0, 15.0);
    let posed = pose_document(&doc).unwrap();
    let rear_z = posed.get("disc-rear").unwrap().transform.translation[2];

    let half = posed.exploded(0.5);
    assert!(
        (half.get("disc-rear").unwrap().transform.translation[2] - (rear_z - 7.5)).abs() < 1e-9
    );
    // The shaft declares no offset, so it stays put — the datum of the view.
    assert_eq!(half.get("shaft").unwrap().transform.translation, [0.0; 3]);

    // Fully exploded, the parts separate along Z.
    let full = posed.exploded(1.0);
    let (_, rear_max) = full.get("disc-rear").unwrap().bounds().unwrap();
    let (front_min, _) = full.get("disc-front").unwrap().bounds().unwrap();
    assert!(
        front_min[2] > rear_max[2],
        "exploded discs should separate: rear top {:.2}, front bottom {:.2}",
        rear_max[2],
        front_min[2]
    );
}

#[test]
fn a_mate_naming_a_missing_instance_is_an_error_not_a_failure() {
    let mut doc = toy(180.0, 15.0);
    doc.mates[0].instance_b = "nope".into();
    let posed = pose_document(&doc).unwrap();
    let err = check_mates(&posed, &doc.mates).unwrap_err();
    assert!(format!("{err}").contains("nope"), "{err}");
}
