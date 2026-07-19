//! Position/velocity servo stability on mm-scale assemblies.
//!
//! Regression: the fixed meter-scale PD defaults (kp=1000, clamp ±1000 Nm)
//! saturated against ~1e-8 kg·m² reflected inertias and the integrator
//! diverged to |v| > 1e6 within one step. Gains are now scaled to the
//! measured reflected inertia, so commands must stay bounded and converge.

use vcad_ir::Document;
use vcad_kernel_physics::PhysicsWorld;

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
