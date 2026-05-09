//! Integration tests: load Unitree URDFs from `examples/`, build a physics
//! world, and run a few steps. Verifies the URDF → Document → PhysicsWorld
//! pipeline end to end on representative full-size robots.

use std::path::PathBuf;

use vcad_kernel_physics::PhysicsWorld;

fn examples_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/vcad-kernel-physics; examples/ lives
    // two parents up at the workspace root.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("examples");
    p
}

#[test]
fn unitree_g1_loads_and_steps() {
    let urdf_path = examples_dir().join("unitree-g1.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-g1.urdf");

    // 23 actuated revolute joints + 3 fixed (head + two hands) = 26 joints
    // in the URDF. PhysicsWorld currently surfaces every joint (including
    // fixed) through joint_ids(); fixed entries simply have zero DOF.
    let urdf_joints = doc.joints.as_ref().expect("has joints").len();
    assert_eq!(urdf_joints, 26, "G1 URDF should expose 26 joints total");

    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");

    let joint_ids = world.joint_ids();
    assert_eq!(
        joint_ids.len(),
        26,
        "G1 PhysicsWorld should expose all 26 URDF joints (got {joint_ids:?})"
    );

    // Sanity-check a few critical joint names appear so future renames break tests.
    for expected in &[
        "left_knee_joint",
        "right_knee_joint",
        "waist_yaw_joint",
        "left_elbow_joint",
        "right_elbow_joint",
    ] {
        assert!(
            joint_ids.iter().any(|j| j == expected),
            "expected joint '{expected}' in {joint_ids:?}"
        );
    }

    // Step the simulation. Joints start at zero with no torques applied, so
    // the only forces are gravity + internal coupling. We just confirm the
    // step does not panic and produces finite states.
    for _ in 0..60 {
        world.step(1.0 / 240.0);
    }

    let states = world.get_joint_states();
    for (id, js) in &states {
        assert!(
            js.position.is_finite(),
            "non-finite q for {id}: {}",
            js.position
        );
        assert!(
            js.velocity.is_finite(),
            "non-finite qdot for {id}: {}",
            js.velocity
        );
    }
}

#[test]
fn unitree_go2_loads_and_steps() {
    let urdf_path = examples_dir().join("unitree-go2.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-go2.urdf");

    // 12 actuated + 4 fixed (one per foot) = 16 joints in URDF.
    let urdf_joints = doc.joints.as_ref().expect("has joints").len();
    assert_eq!(urdf_joints, 16, "Go2 URDF should expose 16 joints total");

    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");

    let joint_ids = world.joint_ids();
    assert_eq!(
        joint_ids.len(),
        16,
        "Go2 PhysicsWorld should expose all 16 URDF joints (got {joint_ids:?})"
    );

    // One hip / thigh / calf per leg, four legs.
    for prefix in &["FL", "FR", "RL", "RR"] {
        for suffix in &["hip_joint", "thigh_joint", "calf_joint"] {
            let name = format!("{prefix}_{suffix}");
            assert!(
                joint_ids.iter().any(|j| j == &name),
                "expected joint '{name}' in {joint_ids:?}"
            );
        }
    }

    for _ in 0..60 {
        world.step(1.0 / 240.0);
    }

    let states = world.get_joint_states();
    for (id, js) in &states {
        assert!(
            js.position.is_finite(),
            "non-finite q for {id}: {}",
            js.position
        );
        assert!(
            js.velocity.is_finite(),
            "non-finite qdot for {id}: {}",
            js.velocity
        );
    }
}
