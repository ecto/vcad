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

/// Sum the URDF-authored masses on every PartDef. The test below sanity-checks
/// this value so a future change that drops <inertial> on the floor is loud.
fn total_authored_mass(doc: &vcad_ir::Document) -> f64 {
    doc.part_defs
        .as_ref()
        .map(|defs| {
            defs.values()
                .filter_map(|pd| pd.inertial.as_ref().map(|i| i.mass_kg))
                .sum()
        })
        .unwrap_or(0.0)
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

    // URDF inertials must propagate through the importer onto every PartDef,
    // not get reinvented from mesh density at physics time. The simplified G1
    // sums to ~25 kg of authored mass (real G1 is ~35 kg incl. battery + skin).
    let total_mass = total_authored_mass(&doc);
    assert!(
        total_mass > 20.0 && total_mass < 40.0,
        "G1 authored mass out of expected range: {total_mass} kg"
    );

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

/// The vendored official Unitree G1 23-DOF URDF (from
/// `unitreerobotics/unitree_ros`) loads cleanly through the importer
/// even when its STL meshes aren't on disk — the reader falls back to
/// 1cm placeholder cubes per link, while inertials and joint topology
/// flow through unchanged.
#[test]
fn unitree_g1_official_urdf_loads() {
    let urdf_path = examples_dir().join("unitree-g1-official.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse official G1 URDF");

    // The 23-DOF G1 has 33 links and 32 joints in the official descriptor.
    let part_defs = doc.part_defs.as_ref().expect("part defs");
    assert_eq!(part_defs.len(), 33, "G1 23-DOF should have 33 links");
    let joints = doc.joints.as_ref().expect("joints");
    assert_eq!(joints.len(), 32, "G1 23-DOF should have 32 joints");

    // Authored mass should sum to a sensible whole-robot ballpark — the
    // real G1 is ~35 kg.
    let total_mass = total_authored_mass(&doc);
    assert!(
        total_mass > 25.0 && total_mass < 50.0,
        "official G1 authored mass out of range: {total_mass} kg"
    );

    // PhysicsWorld must build without error even with placeholder geometry.
    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    for _ in 0..30 {
        world.step(1.0 / 240.0);
    }
}

/// The vendored official Unitree Go2 URDF (from
/// `unitreerobotics/unitree_ros`) loads through the importer. Go2's mesh
/// references are DAE files, which vcad doesn't load yet — the reader
/// substitutes 1cm placeholder cubes so the kinematic + inertial tree
/// still simulates with the authored mass/inertia.
#[test]
fn unitree_go2_official_urdf_loads() {
    let urdf_path = examples_dir().join("unitree-go2-official.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse official Go2 URDF");

    // Go2's full descriptor includes 12 actuated leg joints plus rotor /
    // sensor / mount fixtures (foam, IMU, lidar, head, etc.) — the
    // exact totals are 42 links / 41 joints.
    let part_defs = doc.part_defs.as_ref().expect("part defs");
    assert_eq!(part_defs.len(), 42, "Go2 official should have 42 links");
    let joints = doc.joints.as_ref().expect("joints");
    assert_eq!(joints.len(), 41, "Go2 official should have 41 joints");

    // Real Go2 is ~15 kg.
    let total_mass = total_authored_mass(&doc);
    assert!(
        total_mass > 10.0 && total_mass < 25.0,
        "official Go2 authored mass out of range: {total_mass} kg"
    );

    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    for _ in 0..30 {
        world.step(1.0 / 240.0);
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
