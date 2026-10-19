//! Joint range-of-motion sweep on a small authored mechanism.
//!
//! A 50 mm arm pivots about Z on a base. A wall stands on the +Y side, so
//! swinging positive runs the arm into it while swinging negative is free.
//! A hub cylinder on the base is coaxial with the pivot and overlaps the
//! arm's root at every angle — deliberate, and exactly the kind of zero-pose
//! overlap the sweep must not count as a collision.

use vcad_kernel_assembly::{check_joint_range, JointRangeOptions, RangeScope};

fn arm_source() -> &'static str {
    r#"
[assembly
  #[[part "base" [cube 100.0 100.0 10.0] "steel"]
    [part "hub"  [translate 0.0 5.0 30.0 [cylinder-n 8.0 10.0 64]] "steel"]
    [part "wall" [translate -20.0 30.0 20.0 [cube 80.0 10.0 30.0]] "steel"]
    [part "arm"  [cube 50.0 10.0 10.0] "aluminum"]
    [part "tip"  [translate 50.0 0.0 0.0 [cube 5.0 10.0 10.0]] "aluminum"]]
  #[[instance "base_i" "base" -50.0 -50.0 0.0]
    [instance "hub_i"  "hub"  0.0 0.0 0.0]
    [instance "wall_i" "wall" 0.0 0.0 0.0]
    [instance "arm_i"  "arm"  0.0 0.0 0.0]
    [instance "tip_i"  "tip"  0.0 0.0 0.0]]
  #[[fixed-joint "fix_hub" "base_i" 50.0 50.0 0.0 "hub_i" 0.0 0.0 0.0]
    [fixed-joint "fix_wall" "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]
    [revolute-joint "swing" 0.0 0.0 1.0 -90.0 90.0 "base_i" 50.0 55.0 30.0 "arm_i" 0.0 0.0 0.0]
    [fixed-joint "fix_tip" "arm_i" 0.0 0.0 0.0 "tip_i" 0.0 0.0 0.0]]
  "base_i"]
"#
}

#[test]
fn arm_collides_with_the_wall_on_one_side_only() {
    let doc = vcad_loon::eval_vcad(arm_source(), None).expect("loon evaluates");
    let ranges = check_joint_range(&doc, &JointRangeOptions::default()).expect("sweep runs");
    assert_eq!(ranges.len(), 1);
    let r = &ranges[0];
    assert_eq!(r.label(), "swing");

    // The fixed-joint chain rides along: the tip is part of the subtree.
    assert_eq!(r.moving, vec!["arm_i".to_string(), "tip_i".to_string()]);

    // The hub overlaps the arm root at zero; that is the baseline.
    assert!(
        r.baseline_overlap_mm3 > 100.0,
        "baseline {}",
        r.baseline_overlap_mm3
    );

    // Negative swing is free to the limit despite the constant hub overlap.
    assert!(r.negative.is_free(), "{}", r.summary());
    assert_eq!(r.negative.free_to_deg, -90.0);

    // Positive swing: the tip corner (55, 10) reaches the wall face 25 mm
    // off the pivot at 55·sinθ + 10·cosθ = 25 → θ ≈ 16.6°, and the overlap
    // passes 200 mm³ a step or two later.
    let hit = r
        .positive
        .first_collision_deg
        .expect("positive side collides");
    assert!((15.0..=35.0).contains(&hit), "first collision at {hit}");
    assert_eq!(r.positive.free_to_deg, hit - 5.0);
    let worst = &r.positive.hits[0];
    assert_eq!(worst.stationary, "wall_i");
    assert!(worst.moving == "tip_i" || worst.moving == "arm_i");
    assert!(r.positive.new_overlap_mm3.unwrap() > 200.0);
}

#[test]
fn chain_scope_ignores_geometry_off_the_kinematic_chain() {
    // Hang the wall off a second revolute joint: it becomes another "limb",
    // so in chain scope the arm no longer checks against it.
    let src = arm_source().replace(
        r#"[fixed-joint "fix_wall" "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]"#,
        r#"[revolute-joint "wall_hinge" 1.0 0.0 0.0 -10.0 10.0 "base_i" 50.0 50.0 0.0 "wall_i" 0.0 0.0 0.0]"#,
    );
    let doc = vcad_loon::eval_vcad(&src, None).expect("loon evaluates");
    let opts = JointRangeOptions {
        joints: vec!["swing".to_string()],
        scope: RangeScope::Chain,
        ..JointRangeOptions::default()
    };
    let r = &check_joint_range(&doc, &opts).expect("sweep runs")[0];
    assert!(r.positive.is_free(), "{}", r.summary());

    let all = JointRangeOptions {
        scope: RangeScope::All,
        ..opts
    };
    let r = &check_joint_range(&doc, &all).expect("sweep runs")[0];
    assert!(!r.positive.is_free(), "{}", r.summary());
}

#[test]
fn unknown_joint_is_an_error() {
    let doc = vcad_loon::eval_vcad(arm_source(), None).unwrap();
    let opts = JointRangeOptions {
        joints: vec!["nope".to_string()],
        ..JointRangeOptions::default()
    };
    let err = check_joint_range(&doc, &opts).unwrap_err().to_string();
    assert!(err.contains("nope") && err.contains("swing"), "{err}");
    let opts = JointRangeOptions {
        joints: vec!["fix_tip".to_string()],
        ..JointRangeOptions::default()
    };
    assert!(check_joint_range(&doc, &opts).is_err());
}

#[test]
fn rotating_about_a_coaxial_motor_is_not_a_collision() {
    // The mini humanoid's hip pitch: a Z-axis neck disc overlaps a motor can
    // whose axis IS the pitch axis. That overlap is rotation-invariant, so
    // the sweep must see no growth at any angle. (`rotate -90 0 0` lays the
    // can along Y. Note vcad's CSG rotate applies Z, then Y, then X, so the
    // two-angle `rotate 0 90 90` would lay it along X instead.)
    let src = r#"
[assembly
  #[[part "motor" [rotate -90.0 0.0 0.0 [translate 0 0 -32.0 [cylinder-n 44.0 64.0 96]]] "steel"]
    [part "neck"  [translate 5.0 -54.128 39.0 [translate 0 0 -7.1 [cylinder-n 41.25 14.2 96]]] "steel"]]
  #[[instance "motor_i" "motor" 0.0 0.0 0.0]
    [instance "neck_i"  "neck"  0.0 0.0 0.0]]
  #[[revolute-joint "pitch" 0.0 1.0 0.0 -90.0 90.0 "motor_i" 0.0 0.0 0.0 "neck_i" 0.0 0.0 0.0]]
  "motor_i"]
"#;
    let doc = vcad_loon::eval_vcad(src, None).expect("loon evaluates");
    let r = &check_joint_range(&doc, &JointRangeOptions::default()).expect("sweep runs")[0];
    assert!(
        r.baseline_overlap_mm3 > 1000.0,
        "{}",
        r.baseline_overlap_mm3
    );
    assert!(
        r.negative.is_free() && r.positive.is_free(),
        "{}\n{:?}",
        r.summary(),
        r.positive
    );
}
