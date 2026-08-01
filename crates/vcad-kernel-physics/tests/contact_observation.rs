//! Per-end-effector ground contact must reach the gym observation: a resting
//! body reports in-contact with a normal force equal to its supported weight,
//! and an airborne body reports no contact at all.
//!
//! This is the foot-force channel every real humanoid locomotion policy gets
//! (foot FSRs / ankle F/T). Without it a balance policy cannot tell a loaded
//! foot from a swinging one.

use vcad_ir::Document;
use vcad_kernel_physics::{Action, GroundConfig, PhysicsWorld, RobotEnv};

const G: f64 = 9.81;

/// Ground base + a torso on a Free joint, starting `z0` mm up.
fn free_base_doc(z0: f64) -> Document {
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
             "parentAnchor": {"x": 0.0, "y": 0.0, "z": z0},
             "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
             "kind": {"type": "Free"},
             "state": 0.0}
        ],
        "groundInstanceId": "base_inst"
    }))
    .expect("valid doc")
}

#[test]
fn resting_body_reports_contact_carrying_its_weight() {
    // Start just above the floor so the drop is short and the body has
    // settled well before the measurement.
    let doc = free_base_doc(30.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    world.set_ground(GroundConfig::default());
    let mass = world.get_instance_mass("torso_inst").expect("torso body");

    // 2 s at 1/240: land, bleed off the impact, settle.
    for _ in 0..480 {
        world.step(1.0 / 240.0);
    }

    let c = world
        .get_instance_contact("torso_inst")
        .expect("torso body");
    assert!(c.in_contact, "settled body reports no ground contact");

    let weight = mass * G;
    let err = (c.normal_force - weight).abs() / weight;
    assert!(
        err < 0.05,
        "normal force {} N should carry the body's weight {} N (rel err {err:.3})",
        c.normal_force,
        weight
    );

    // Center of pressure under the body, on the floor.
    assert!(
        c.point[0].abs() < 0.05 && c.point[1].abs() < 0.05 && c.point[2].abs() < 0.01,
        "center of pressure {:?} is not under the resting body at z≈0",
        c.point
    );
}

#[test]
fn airborne_body_reports_no_contact() {
    let doc = free_base_doc(2000.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    world.set_ground(GroundConfig::default());

    // A tenth of a second of free fall from 2 m: still far above the floor.
    for _ in 0..24 {
        world.step(1.0 / 240.0);
    }

    let c = world
        .get_instance_contact("torso_inst")
        .expect("torso body");
    assert!(!c.in_contact, "airborne body reports ground contact");
    assert_eq!(c.normal_force, 0.0);
}

#[test]
fn gym_observation_surfaces_end_effector_contact() {
    let mut env = RobotEnv::new(
        free_base_doc(30.0),
        vec!["torso_inst".to_string()],
        None,
        None,
        None, // default ground: plane at z = 0
    )
    .unwrap();

    // Fresh reset: no step has run yet, so nothing is in contact.
    let obs = env.reset();
    assert_eq!(obs.end_effector_contacts.len(), 1);
    assert!(!obs.end_effector_contacts[0].in_contact);

    // 120 env steps × 4 substeps = 2 s: landed and settled.
    let mut last = obs;
    for _ in 0..120 {
        let (o, _, _) = env.step(Action::Torque(vec![0.0; env.action_dim()]));
        last = o;
    }

    let c = last.end_effector_contacts[0];
    assert!(c.in_contact, "settled end effector reports no contact");
    assert!(
        c.normal_force > 0.0,
        "settled end effector carries no normal force"
    );

    // The layout the docs promise: 7 pose slots + 5 contact slots per EE.
    let joint_slots: usize = env.joint_slot_counts().iter().sum();
    assert_eq!(env.observation_dim(), joint_slots * 2 + 12);
}
