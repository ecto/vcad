//! Position/velocity servo stability on mm-scale assemblies.
//!
//! Regression: the fixed meter-scale PD defaults (kp=1000, clamp ±1000 Nm)
//! saturated against ~1e-8 kg·m² reflected inertias and the integrator
//! diverged to |v| > 1e6 within one step. Gains are now scaled to the
//! measured reflected inertia, so commands must stay bounded and converge.

use vcad_ir::Document;
use vcad_kernel_physics::{GroundConfig, PhysicsWorld, RobotEnv, GAIN_STABILITY_LIMIT};

/// A mm-scale crank + slider: 40mm flywheel on a Y-axis revolute plus a
/// vertical slider — the shape of an engine assembly.
fn mm_scale_doc() -> Document {
    serde_json::from_value(serde_json::json!({
        "version": "0.1",
        "nodes": {
            "0": {"id": 0, "op": {"type": "Cube", "size": {"x": 100.0, "y": 60.0, "z": 10.0}}},
            "1": {"id": 1, "op": {"type": "Cylinder", "radius": 40.0, "height": 8.0, "segments": 0}},
            "2": {"id": 2, "op": {"type": "Cylinder", "radius": 12.0, "height": 18.0, "segments": 0}}
        },
        "roots": [],
        "materials": {},
        "part_materials": {},
        "partDefs": {
            "base": {"id": "base", "name": "base", "root": 0},
            "wheel": {"id": "wheel", "name": "wheel", "root": 1},
            "piston": {"id": "piston", "name": "piston", "root": 2}
        },
        "instances": [
            {"id": "base", "partDefId": "base"},
            {"id": "wheel", "partDefId": "wheel"},
            {"id": "piston", "partDefId": "piston"}
        ],
        "joints": [
            {"id": "crank", "parentInstanceId": "base", "childInstanceId": "wheel",
             "parentAnchor": {"x": 0.0, "y": 0.0, "z": 90.0},
             "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
             "kind": {"type": "Revolute", "axis": {"x": 0.0, "y": 1.0, "z": 0.0},
                       "limits": [-36000.0, 36000.0]},
             "state": 0.0},
            {"id": "slide", "parentInstanceId": "base", "childInstanceId": "piston",
             "parentAnchor": {"x": 35.0, "y": 0.0, "z": 40.0},
             "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
             "kind": {"type": "Slider", "axis": {"x": 0.0, "y": 0.0, "z": 1.0},
                       "limits": [-15.0, 15.0]},
             "state": 0.0}
        ],
        "groundInstanceId": "base"
    }))
    .expect("valid doc")
}

#[test]
fn position_control_stays_bounded_and_converges() {
    let doc = mm_scale_doc();
    let mut world = PhysicsWorld::from_document(&doc).expect("world");
    world.set_joint_position("crank", 30.0); // degrees
    world.set_joint_position("slide", 5.0); // mm

    for _ in 0..480 {
        world.step(1.0 / 240.0);
        let states = world.get_joint_states();
        for (id, s) in &states {
            assert!(
                s.velocity.abs() < 1e5,
                "joint {id} velocity exploded: {}",
                s.velocity
            );
            assert!(s.position.is_finite(), "joint {id} position not finite");
        }
    }

    let states = world.get_joint_states();
    let crank = &states["crank"];
    // 2 seconds at ω=20 rad/s is plenty to settle; allow gravity droop.
    assert!(
        (crank.position - 30.0).abs() < 10.0,
        "crank did not approach 30°: {}",
        crank.position
    );
}

#[test]
fn velocity_control_stays_bounded_and_tracks() {
    let doc = mm_scale_doc();
    let mut world = PhysicsWorld::from_document(&doc).expect("world");
    world.set_joint_velocity("crank", 360.0); // deg/s

    let mut last = 0.0;
    for _ in 0..480 {
        world.step(1.0 / 240.0);
        let states = world.get_joint_states();
        let crank = &states["crank"];
        assert!(
            crank.velocity.abs() < 1e5,
            "crank velocity exploded: {}",
            crank.velocity
        );
        last = crank.velocity;
    }
    assert!(
        (last - 360.0).abs() < 120.0,
        "crank velocity did not track 360 deg/s: {last}"
    );
}

#[test]
fn unactuated_slider_stops_at_its_limit() {
    let doc = mm_scale_doc();
    let mut world = PhysicsWorld::from_document(&doc).expect("world");
    // No motors at all: gravity pulls the piston down. Limits are ±15mm.
    for _ in 0..2400 {
        world.step(1.0 / 240.0);
    }
    let states = world.get_joint_states();
    let slide = &states["slide"];
    assert!(
        slide.position >= -15.0 - 1e-6 && slide.position <= 15.0 + 1e-6,
        "slider blew through its limits: {}mm",
        slide.position
    );
    // It should be resting AT the lower stop, not oscillating past it.
    assert!(
        (slide.position - -15.0).abs() < 1.0,
        "slider not at lower stop: {}mm",
        slide.position
    );
}

/// Empirical calibration of `GAIN_STABILITY_LIMIT`: sweep ω·dt on the
/// mm-scale crank and confirm the explicit servo is well-behaved at (and
/// well past) the limit, and genuinely broken above ω·dt ≈ 1.
#[test]
fn gain_stability_limit_is_conservative() {
    let dt = 1.0 / 240.0f64;
    let track_error = |omega_dt: f64| -> f64 {
        let doc = mm_scale_doc();
        let mut world = PhysicsWorld::from_document(&doc).expect("world");
        // Probe the reflected inertia through the warning itself, then pick
        // critically-damped gains landing exactly on this ω·dt.
        world.set_joint_gains("crank", 1.0, 0.0);
        let inertia = world
            .check_gain_stability(dt)
            .first()
            .map(|w| w.reflected_inertia)
            .unwrap_or(1.0);
        let omega = omega_dt / dt;
        world.set_joint_gains("crank", inertia * omega * omega, 2.0 * inertia * omega);
        world.set_joint_position("crank", 30.0);
        for _ in 0..480 {
            world.step(dt as f32);
        }
        (world.get_joint_states()["crank"].position - 30.0).abs()
    };

    // At and beyond the limit the servo still settles on target.
    for omega_dt in [GAIN_STABILITY_LIMIT, 0.5, 0.8] {
        let err = track_error(omega_dt);
        assert!(
            err < 1.0,
            "omega*dt = {omega_dt} should still track 30°, off by {err}°"
        );
    }
    // Past ω·dt ≈ 1 it is destroyed.
    let err = track_error(1.3);
    assert!(
        err > 30.0,
        "omega*dt = 1.3 should diverge off target, off by only {err}°"
    );
}

/// The reported case in miniature: gains that are stable in an implicitly
/// integrated simulator (booster_gym ships kp = 200 on the K1) land far
/// outside this crate's explicit stability region at a coarse substep. Here
/// the fixture's inertia is mm-scale, so the same situation is reproduced by
/// picking gains at ω·dt = 0.9 — unstable at 200 Hz substeps, comfortably
/// stable at 5× the substep rate with the control period unchanged.
#[test]
fn unstable_gains_warn_and_clear_at_5x_substeps() {
    let doc = mm_scale_doc();

    // Probe the joint's reflected inertia, then pick critically-damped gains
    // sitting at omega*dt = 0.9 for a 1/200 s substep.
    let dt = 1.0 / 200.0f64;
    let mut probe = PhysicsWorld::from_document(&doc).expect("world");
    probe.set_joint_gains("crank", 1.0, 0.0);
    let inertia = probe.check_gain_stability(dt)[0].reflected_inertia;
    let omega = 0.9 / dt;
    let (kp, kd) = (inertia * omega * omega, 2.0 * inertia * omega);

    let mut env = RobotEnv::new(
        doc.clone(),
        vec![],
        Some(dt as f32),
        Some(4),
        Some(GroundConfig::disabled()),
    )
    .expect("env");
    env.set_joint_gains("crank", kp, kd);

    let warnings = env.check_gain_stability();
    let crank = warnings
        .iter()
        .find(|w| w.joint_id == "crank")
        .unwrap_or_else(|| panic!("expected a warning naming 'crank', got {warnings:?}"));
    assert!(
        (crank.omega_dt - 0.9).abs() < 1e-6,
        "warning should report omega*dt = 0.9: {crank:?}"
    );
    assert!(crank.max_stable_kp < kp, "{crank:?}");
    assert!(
        crank.min_substeps >= 4 * 3,
        "3x the substeps are needed to reach 0.3: {crank:?}"
    );
    let message = crank.to_string();
    assert!(
        message.contains("crank"),
        "message names the joint: {message}"
    );
    assert!(
        message.contains("substeps"),
        "message names the fix: {message}"
    );

    // 5x the substeps at 5x the rate: same control period, same gains,
    // omega*dt = 0.18 — no warning.
    let mut fine = RobotEnv::new(
        doc,
        vec![],
        Some((dt / 5.0) as f32),
        Some(20),
        Some(GroundConfig::disabled()),
    )
    .expect("env");
    fine.set_joint_gains("crank", kp, kd);
    assert!(
        fine.check_gain_stability().is_empty(),
        "5x substeps should clear the warning, got {:?}",
        fine.check_gain_stability()
    );
}
