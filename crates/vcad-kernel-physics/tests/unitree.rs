//! Integration tests: load the official Unitree URDFs from `examples/`,
//! build a physics world, and run a few steps. Verifies the URDF →
//! Document → PhysicsWorld pipeline on the real upstream descriptors.

use std::path::PathBuf;

use vcad_kernel_physics::{Action, GroundConfig, PhysicsWorld, RobotEnv};

fn examples_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/vcad-kernel-physics; examples/ lives
    // two parents up at the workspace root.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("examples");
    p
}

/// Sum the URDF-authored masses on every PartDef. Failing this means a
/// future change has dropped <inertial> on the floor.
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

fn assert_finite_states(world: &mut PhysicsWorld, steps: u32, label: &str) {
    for _ in 0..steps {
        world.step(1.0 / 240.0);
    }
    let states = world.get_joint_states();
    for (id, js) in &states {
        assert!(
            js.position.is_finite(),
            "{label}: non-finite q for {id}: {}",
            js.position
        );
        assert!(
            js.velocity.is_finite(),
            "{label}: non-finite qdot for {id}: {}",
            js.velocity
        );
    }
}

/// Official Unitree G1 23-DOF descriptor (from `unitreerobotics/unitree_ros`).
/// Mesh references resolve to STL placeholders here — the browser swaps
/// them for real triangle data, but physics only needs the inertials and
/// joint topology, both of which flow through unchanged.
#[test]
fn unitree_g1_loads_and_steps() {
    let urdf_path = examples_dir().join("unitree-g1.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-g1.urdf");

    let part_defs = doc.part_defs.as_ref().expect("part defs");
    assert_eq!(part_defs.len(), 33, "G1 23-DOF should have 33 links");
    let joints = doc.joints.as_ref().expect("joints");
    assert_eq!(joints.len(), 32, "G1 23-DOF should have 32 joints");

    // Real G1 is ~35 kg.
    let total_mass = total_authored_mass(&doc);
    assert!(
        total_mass > 25.0 && total_mass < 50.0,
        "G1 authored mass out of range: {total_mass} kg"
    );

    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    let joint_ids = world.joint_ids();
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
    assert_finite_states(&mut world, 60, "G1");
}

/// Official Unitree Go2 quadruped descriptor (from
/// `unitreerobotics/unitree_ros`). Mesh references are DAE files that the
/// browser parses with three.js; physics ignores the geometry and works
/// off the authored inertials.
#[test]
fn unitree_go2_loads_and_steps() {
    let urdf_path = examples_dir().join("unitree-go2.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-go2.urdf");

    let part_defs = doc.part_defs.as_ref().expect("part defs");
    assert_eq!(part_defs.len(), 42, "Go2 official should have 42 links");
    let joints = doc.joints.as_ref().expect("joints");
    assert_eq!(joints.len(), 41, "Go2 official should have 41 joints");

    // Real Go2 is ~15 kg.
    let total_mass = total_authored_mass(&doc);
    assert!(
        total_mass > 10.0 && total_mass < 25.0,
        "Go2 authored mass out of range: {total_mass} kg"
    );

    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    let joint_ids = world.joint_ids();
    for prefix in &["FL", "FR", "RL", "RR"] {
        for suffix in &["hip_joint", "thigh_joint", "calf_joint"] {
            let name = format!("{prefix}_{suffix}");
            assert!(
                joint_ids.iter().any(|j| j == &name),
                "expected joint '{name}' in {joint_ids:?}"
            );
        }
    }
    assert_finite_states(&mut world, 60, "Go2");
}

/// The gym path with ground contact enabled on a real humanoid: PD position
/// hold at the zero pose while the legs interact with a ground plane placed
/// at knee height, at the documented near-divergence regime (1/240 s × 4
/// substeps). The URDF base link is the fixed root here, so "standing" is a
/// stability claim, not a balance claim: with contact impulses acting on a
/// 23-DOF articulated chain every substep, nothing may go non-finite and no
/// link may be driven through the floor.
#[test]
fn unitree_g1_pd_hold_with_ground_contact_stays_finite() {
    let urdf_path = examples_dir().join("unitree-g1.urdf");
    let doc = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-g1.urdf");
    let instance_ids: Vec<String> = doc
        .instances
        .as_ref()
        .expect("instances")
        .iter()
        .map(|i| i.id.clone())
        .collect();

    // Find the lowest body origin so the plane can be placed where it
    // actually intersects the dangling legs.
    let mut probe = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    let n_actuated = probe.actuated_joint_ids().len();
    let zero_q = vec![0.0; n_actuated];
    let poses = probe.forward_kinematics_at(&zero_q).expect("fk");
    let min_z = poses
        .values()
        .map(|(p, _)| p[2])
        .fold(f64::INFINITY, f64::min);
    assert!(
        min_z.is_finite() && min_z < 0.0,
        "G1 legs should hang below the base"
    );

    let ground = GroundConfig {
        enabled: true,
        height: min_z + 0.05,
        friction: 0.8,
        restitution: 0.0,
    };
    let mut env =
        RobotEnv::new(doc, instance_ids.clone(), None, None, Some(ground)).expect("RobotEnv");

    let hold = vec![0.0; env.action_dim()];
    for _ in 0..120 {
        let (obs, _, _) = env.step(Action::PositionTarget(hold.clone()));
        for (i, v) in obs.joint_velocities.iter().enumerate() {
            assert!(
                v.is_finite(),
                "joint {i} velocity non-finite under PD+contact"
            );
        }
        for (i, pose) in obs.end_effector_poses.iter().enumerate() {
            assert!(
                pose.iter().all(|c| c.is_finite()),
                "instance {} pose non-finite",
                instance_ids[i]
            );
            assert!(
                pose[2] > ground.height - 0.2,
                "instance {} driven through the floor: z = {} (floor {})",
                instance_ids[i],
                pose[2],
                ground.height
            );
        }
    }
}

/// The G1 URDF, like nearly every humanoid descriptor, declares no floating
/// joint — the convention is that the simulator supplies the free base.
/// Imported plainly, the root link is grounded and the robot is welded to
/// the world; with `floating_base` the importer synthesizes the world link
/// and a 6-DOF `Free` joint, and the base falls under gravity.
#[test]
fn unitree_g1_floating_base_falls_under_gravity() {
    use vcad_ir::JointKind;
    use vcad_kernel_urdf::UrdfReadOptions;

    let urdf_path = examples_dir().join("unitree-g1.urdf");

    // Baseline: default import is unchanged — no Free joint, root grounded.
    let welded = vcad_kernel_urdf::read_urdf(&urdf_path).expect("parse unitree-g1.urdf");
    assert!(
        !welded
            .joints
            .as_ref()
            .expect("joints")
            .iter()
            .any(|j| matches!(j.kind, JointKind::Free)),
        "default import must not synthesize a floating base"
    );

    let opts = UrdfReadOptions {
        urdf_dir: urdf_path.parent().map(|p| p.to_path_buf()),
        floating_base: true,
        spawn_height_mm: 900.0,
        ..UrdfReadOptions::default()
    };
    let doc = vcad_kernel_urdf::read_urdf_with_options(&urdf_path, &opts)
        .expect("parse unitree-g1.urdf with floating base");

    // Exactly one link and one joint more than the welded import.
    assert_eq!(
        doc.part_defs.as_ref().unwrap().len(),
        welded.part_defs.as_ref().unwrap().len() + 1
    );
    let joints = doc.joints.as_ref().expect("joints");
    let free: Vec<_> = joints
        .iter()
        .filter(|j| matches!(j.kind, JointKind::Free))
        .collect();
    assert_eq!(free.len(), 1, "exactly one synthesized floating joint");
    // parentAnchor.z carries the spawn height (a Free joint's scalar state
    // cannot).
    assert!((free[0].parent_anchor.z - 900.0).abs() < 1e-9);

    // The floating base is what the *root* link hangs from, so the welded
    // import's grounded instance is now the Free joint's child.
    let root_instance = welded.ground_instance_id.as_ref().expect("welded ground");
    assert_eq!(&free[0].child_instance_id, root_instance);
    assert_ne!(doc.ground_instance_id.as_ref(), Some(root_instance));

    // No ground plane: with a free base the whole robot accelerates
    // downward. Welded, that instance cannot move at all.
    let mut world = PhysicsWorld::from_document(&doc).expect("build PhysicsWorld");
    let z0 = world
        .get_instance_pose(root_instance)
        .expect("root instance pose")
        .0[2];
    for _ in 0..240 {
        world.step(1.0 / 240.0);
    }
    let z1 = world
        .get_instance_pose(root_instance)
        .expect("root instance pose")
        .0[2];
    assert!(
        z1.is_finite() && z1 < z0 - 1.0,
        "floating base should fall under gravity over 1 s: z {z0} -> {z1}"
    );

    let mut welded_world = PhysicsWorld::from_document(&welded).expect("build welded world");
    let wz0 = welded_world
        .get_instance_pose(root_instance)
        .expect("root instance pose")
        .0[2];
    for _ in 0..240 {
        welded_world.step(1.0 / 240.0);
    }
    let wz1 = welded_world
        .get_instance_pose(root_instance)
        .expect("root instance pose")
        .0[2];
    assert!(
        (wz1 - wz0).abs() < 1e-6,
        "welded root must stay pinned: z {wz0} -> {wz1}"
    );
}
