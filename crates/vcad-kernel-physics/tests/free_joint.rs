//! A Free (6-DOF) joint — the physics realization of a URDF `floating`
//! joint — must let its child translate: a humanoid's floating base falls
//! under gravity instead of staying pinned in space (the old Ball mapping
//! kept 3 rotational DOF but silently dropped translation).

use vcad_ir::{Document, JointKind};
use vcad_kernel_physics::{Action, GroundConfig, PhysicsWorld, RobotEnv};

/// Ground base + a torso attached by a Free joint 500 mm up.
fn free_base_doc() -> Document {
    serde_json::from_value(serde_json::json!({
        "version": "0.1",
        "nodes": {
            "0": {"id": 0, "op": {"type": "Cube", "size": {"x": 100.0, "y": 100.0, "z": 20.0}}},
            "1": {"id": 1, "op": {"type": "Cube", "size": {"x": 60.0, "y": 40.0, "z": 120.0}}}
        },
        "roots": [],
        "materials": {},
        "part_materials": {},
        "partDefs": {
            "base": {"id": "base", "name": "base", "root": 0},
            "torso": {"id": "torso", "name": "torso", "root": 1}
        },
        "instances": [
            {"id": "base_inst", "partDefId": "base"},
            {"id": "torso_inst", "partDefId": "torso"}
        ],
        "joints": [
            {"id": "floating_base", "parentInstanceId": "base_inst",
             "childInstanceId": "torso_inst",
             "parentAnchor": {"x": 0.0, "y": 0.0, "z": 500.0},
             "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
             "kind": {"type": "Free"},
             "state": 0.0}
        ],
        "groundInstanceId": "base_inst"
    }))
    .expect("valid doc")
}

#[test]
fn free_joint_zero_pose_matches_document_fk() {
    let doc = free_base_doc();

    // Physics FK at the zero pose: torso at the parent anchor, unrotated.
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    let poses = world.forward_kinematics_at(&[0.0; 6]).unwrap();
    let (pos, quat) = poses["torso_inst"];
    assert!(
        pos[0].abs() < 1e-9 && pos[1].abs() < 1e-9 && (pos[2] - 0.5).abs() < 1e-9,
        "zero-pose torso at {pos:?}, expected [0, 0, 0.5]"
    );
    assert!(
        (quat[0].abs() - 1.0).abs() < 1e-9,
        "zero-pose orientation {quat:?}, expected identity"
    );

    // Document FK agrees: a Free joint renders at its zero pose.
    let fk = vcad_eval::solve_forward_kinematics(&doc);
    let t = fk.get("torso_inst").expect("torso solved");
    assert!(
        t.translation.x.abs() < 1e-9
            && t.translation.y.abs() < 1e-9
            && (t.translation.z - 500.0).abs() < 1e-9,
        "document FK torso at {:?}, expected [0, 0, 500] mm",
        t.translation
    );
}

#[test]
fn free_joint_base_falls_under_gravity() {
    let doc = free_base_doc();
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    let (start, _) = world.get_instance_pose("torso_inst").unwrap();
    assert!((start[2] - 0.5).abs() < 1e-6);

    // Half a second of free fall: Δz ≈ -g t²/2 ≈ -1.23 m.
    for _ in 0..120 {
        world.step(1.0 / 240.0);
    }
    let (end, _) = world.get_instance_pose("torso_inst").unwrap();
    let dz = end[2] - start[2];
    assert!(
        dz < -0.5,
        "free base should fall under gravity; Δz = {dz} m (was pinned at {} m)",
        start[2]
    );
    // Pure translation: gravity through the COM exerts no moment, so the
    // body must not have picked up significant lateral drift.
    assert!(
        end[0].abs() < 1e-3 && end[1].abs() < 1e-3,
        "free fall should be vertical, got {end:?}"
    );
}

#[test]
fn free_joint_gym_observation_layout() {
    let doc = free_base_doc();
    assert!(matches!(
        doc.joints.as_ref().unwrap()[0].kind,
        JointKind::Free
    ));

    // Ground off: this test measures free-fall dynamics and the observation
    // layout, not contact. With the default ground plane at z = 0 the base
    // (starting 500 mm up) would land after ~0.32 s and be at rest by the
    // end of the 0.5 s rollout, leaving nothing to observe falling.
    let mut env = RobotEnv::new(
        doc,
        vec!["torso_inst".to_string()],
        None,
        None,
        Some(GroundConfig::disabled()),
    )
    .unwrap();

    // The floating base is passive: it appears in observations (6 slots)
    // but not in the action space.
    assert_eq!(env.num_joints(), 1);
    assert_eq!(env.action_dim(), 0);
    // 6 position + 6 velocity slots, plus the end effector's 7 pose slots and
    // 5 contact slots (flag, normal force, center of pressure).
    assert_eq!(env.observation_dim(), 6 * 2 + 7 + 5);

    let obs = env.reset();
    assert_eq!(obs.joint_positions.len(), 6);
    assert_eq!(obs.joint_velocities.len(), 6);

    // Step with an empty action; the base falls. Position layout is
    // [x, y, z (mm), rx, ry, rz (deg)] — z (slot 2) decreases.
    let mut obs = obs;
    for _ in 0..30 {
        let (o, _, _) = env.step(Action::Torque(vec![]));
        obs = o;
    }
    assert!(
        obs.joint_positions[2] < -10.0,
        "free-base z should have dropped (mm); obs = {:?}",
        obs.joint_positions
    );
    // Velocity layout is [wx, wy, wz (deg/s), vx, vy, vz (mm/s)]: the
    // linear-z slot (5) carries the fall speed, the angular slots stay ~0.
    assert!(
        obs.joint_velocities[5] < -100.0,
        "linear z velocity should be negative (mm/s); obs = {:?}",
        obs.joint_velocities
    );
    assert!(
        obs.joint_velocities[0].abs() < 1e-6,
        "no angular velocity in pure free fall; obs = {:?}",
        obs.joint_velocities
    );
}

/// End-to-end: a URDF humanoid with a `floating` base imports to a Free
/// joint and its pelvis falls under gravity instead of staying pinned.
#[test]
fn urdf_floating_base_falls() {
    let urdf = r#"<?xml version="1.0"?>
<robot name="mini_humanoid">
    <link name="base">
        <visual><geometry><box size="0.2 0.2 0.02"/></geometry></visual>
    </link>
    <link name="pelvis">
        <inertial>
            <origin xyz="0 0 0" rpy="0 0 0"/>
            <mass value="3.8"/>
            <inertia ixx="0.01" ixy="0" ixz="0" iyy="0.01" iyz="0" izz="0.008"/>
        </inertial>
        <visual><geometry><box size="0.1 0.08 0.1"/></geometry></visual>
    </link>
    <joint name="world_joint" type="floating">
        <origin xyz="0 0 0.8"/>
        <parent link="base"/>
        <child link="pelvis"/>
    </joint>
</robot>"#;

    let doc = vcad_kernel_urdf::read_urdf_from_str(urdf).unwrap();
    let joints = doc.joints.as_ref().unwrap();
    assert!(matches!(joints[0].kind, JointKind::Free));

    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    let pelvis_id = &joints[0].child_instance_id;
    let (start, _) = world.get_instance_pose(pelvis_id).unwrap();
    assert!(
        (start[2] - 0.8).abs() < 1e-6,
        "pelvis should start at the joint origin, got {start:?}"
    );

    for _ in 0..120 {
        world.step(1.0 / 240.0);
    }
    let (end, _) = world.get_instance_pose(pelvis_id).unwrap();
    assert!(
        end[2] - start[2] < -0.5,
        "URDF floating base must fall, Δz = {} m",
        end[2] - start[2]
    );
}

/// Gravity-compensation feedforward must be off on a floating base.
///
/// `rnea` at `qdd = 0` solves for the torques that hold a robot static with
/// its root *bolted to the world*; those torques include the 6-DOF wrench the
/// ground is supposed to supply. Handing them to a floating-base robot's
/// joints injects that wrench internally and the base tumbles — a Booster K1
/// commanded to hold its rest pose spun to 90° of tilt in 0.22 s of sim time.
/// Here: an airborne floating base whose one actuated limb is servoed to its
/// rest pose feels no external moment, so it must fall without rotating.
#[test]
fn floating_base_position_servo_adds_no_gravity_feedforward() {
    let mut doc = free_base_doc();
    // Hang an arm off the torso on a Y-axis revolute — the joint gravity
    // compensation would load the hardest.
    let extra: Document = serde_json::from_value(serde_json::json!({
        "version": "0.1",
        "nodes": {"2": {"id": 2, "op": {"type": "Cube", "size": {"x": 200.0, "y": 30.0, "z": 30.0}}}},
        "roots": [],
        "materials": {},
        "part_materials": {},
        "partDefs": {"arm": {"id": "arm", "name": "arm", "root": 2}},
        "instances": [],
        "joints": []
    }))
    .unwrap();
    doc.nodes.extend(extra.nodes);
    doc.part_defs
        .as_mut()
        .unwrap()
        .extend(extra.part_defs.unwrap());
    doc.instances.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({"id": "arm_inst", "partDefId": "arm"})).unwrap(),
    );
    doc.joints.as_mut().unwrap().push(
        serde_json::from_value(serde_json::json!({
            "id": "shoulder", "parentInstanceId": "torso_inst", "childInstanceId": "arm_inst",
            "parentAnchor": {"x": 0.0, "y": 0.0, "z": 100.0},
            "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
            "kind": {"type": "Revolute", "axis": {"x": 0.0, "y": 1.0, "z": 0.0},
                     "limits": [-90.0, 90.0]},
            "state": 0.0
        }))
        .unwrap(),
    );

    let mut env = RobotEnv::new(
        doc,
        vec!["arm_inst".into()],
        Some(1.0 / 240.0),
        Some(4),
        Some(GroundConfig::disabled()),
    )
    .expect("build env");
    env.reset();

    let mut max_tilt: f64 = 0.0;
    for _ in 0..30 {
        let r = env.step_full(Action::PositionTarget(vec![0.0; env.action_dim()]));
        max_tilt = max_tilt.max(r.info.base_tilt_deg.unwrap_or(0.0));
    }
    assert!(
        max_tilt < 1.0,
        "airborne floating base rotated {max_tilt:.2}° while its servo held the \
         rest pose — gravity feedforward is leaking the base wrench into the joints"
    );
}
