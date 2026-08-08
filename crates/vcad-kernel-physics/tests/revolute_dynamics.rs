//! Revolute-joint dynamics regressions.
//!
//! The assembly is the reported repro: a ground frame with identical
//! pendulums hung from it as sibling children, each bob a composed part
//! (`union` of two `translate`d primitives) rather than a bare primitive.

use std::collections::HashMap;
use vcad_ir::{CsgOp, Document, Instance, Joint, JointKind, Node, PartDef, Vec3 as V};
use vcad_kernel_physics::PhysicsWorld;

/// bob = union(translate(cube 1.6x1.6x40), translate(sphere 6))
fn doc_with(n_pendula: usize, axis: V, first_state: f64) -> Document {
    let mut doc = Document::new();
    doc.nodes.insert(
        1,
        Node {
            id: 1,
            name: None,
            op: CsgOp::Cube {
                size: V::new(60.0, 20.0, 60.0),
            },
        },
    );
    // bob geometry
    doc.nodes.insert(
        2,
        Node {
            id: 2,
            name: None,
            op: CsgOp::Cube {
                size: V::new(1.6, 1.6, 40.0),
            },
        },
    );
    doc.nodes.insert(
        3,
        Node {
            id: 3,
            name: None,
            op: CsgOp::Translate {
                child: 2,
                offset: V::new(-0.8, -0.8, -40.0),
            },
        },
    );
    doc.nodes.insert(
        4,
        Node {
            id: 4,
            name: None,
            op: CsgOp::Sphere {
                radius: 6.0,
                segments: 0,
            },
        },
    );
    doc.nodes.insert(
        5,
        Node {
            id: 5,
            name: None,
            op: CsgOp::Translate {
                child: 4,
                offset: V::new(0.0, 0.0, -46.0),
            },
        },
    );
    doc.nodes.insert(
        6,
        Node {
            id: 6,
            name: None,
            op: CsgOp::Union { left: 3, right: 5 },
        },
    );

    let mut part_defs = HashMap::new();
    part_defs.insert(
        "frame".into(),
        PartDef {
            id: "frame".into(),
            name: None,
            root: 1,
            default_material: None,
            inertial: None,
            colliders: None,
        },
    );
    part_defs.insert(
        "bob".into(),
        PartDef {
            id: "bob".into(),
            name: None,
            root: 6,
            default_material: None,
            inertial: None,
            colliders: None,
        },
    );
    doc.part_defs = Some(part_defs);

    let mut instances = vec![Instance {
        id: "frame".into(),
        part_def_id: "frame".into(),
        name: None,
        tags: Vec::new(),
        transform: None,
        material: None,
    }];
    let mut joints = Vec::new();
    for i in 0..n_pendula {
        let x = 20.0 + 20.0 * i as f64;
        instances.push(Instance {
            id: format!("bob{i}"),
            part_def_id: "bob".into(),
            name: None,
            tags: Vec::new(),
            transform: None,
            material: None,
        });
        joints.push(Joint {
            id: format!("j{i}"),
            name: None,
            parent_instance_id: Some("frame".into()),
            child_instance_id: format!("bob{i}"),
            parent_anchor: V::new(x, 10.0, 60.0),
            child_anchor: V::new(0.0, 0.0, 0.0),
            kind: JointKind::Revolute {
                axis,
                limits: Some((-80.0, 80.0)),
                effort_limit: None,
                velocity_limit: None,
            },
            state: if i == 0 { first_state } else { 0.0 },
        });
    }
    doc.instances = Some(instances);
    doc.joints = Some(joints);
    doc.ground_instance_id = Some("frame".into());
    doc
}

/// A passive rollout released at 60 deg must swing *through* the hanging
/// equilibrium, not accelerate away from it to the limit, and must leave its
/// undriven sibling alone.
#[test]
fn passive_rollout_swings_through_equilibrium() {
    let doc = doc_with(2, V::new(1.0, 0.0, 0.0), 60.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    let mut min0 = f64::MAX;
    let mut max0 = f64::MIN;
    for step in 0..500 {
        world.step(0.004);
        let s = world.get_joint_states();
        min0 = min0.min(s["j0"].position);
        max0 = max0.max(s["j0"].position);
        if step % 50 == 0 || step == 499 {
            let ee = world.get_instance_pose("bob0").unwrap();
            println!(
                "  t={:.2}  j0={:8.3} ({:8.2}/s)  j1={:8.3} ({:8.2}/s)  bob0=({:.4},{:.4},{:.4})",
                (step + 1) as f64 * 0.004,
                s["j0"].position,
                s["j0"].velocity,
                s["j1"].position,
                s["j1"].velocity,
                ee.0[0],
                ee.0[1],
                ee.0[2]
            );
        }
    }
    println!("  j0 swing range: {min0:.2} .. {max0:.2}");

    // Gravity is restoring: the pendulum must swing through the hanging
    // equilibrium rather than run away to its limit.
    assert!(min0 < -30.0, "pendulum never swung past 0 (min {min0})");
    assert!(max0 <= 61.0, "pendulum gained energy (max {max0})");

    // A sibling joint at rest with no action stays at rest.
    let s = world.get_joint_states();
    assert!(
        s["j1"].position.abs() < 1e-6,
        "torque leaked to sibling: {}",
        s["j1"].position
    );
}

/// Oracle: a compound pendulum released at a small angle must oscillate at
/// `omega = sqrt(m g d / I_o)`, with `I_o = I_com + m d^2`. This pins the
/// whole units chain — mesh mm to kg/m/s, gravity direction, inertia
/// transport and the joint lever arm — against closed form.
#[test]
fn small_amplitude_period_matches_analytic() {
    let doc = doc_with(1, V::new(1.0, 0.0, 0.0), 3.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    let (m, d, i_com) = world.debug_body_props(1);
    let i_o = i_com + m * d * d;
    let omega = (m * 9.81 * d / i_o).sqrt();
    let t_analytic = 2.0 * std::f64::consts::PI / omega;

    // Measure the period from zero crossings.
    let dt: f32 = 1e-4;
    let mut prev = world.get_joint_states()["j0"].position;
    let mut crossings = Vec::new();
    for step in 0..200_000 {
        world.step(dt);
        let p = world.get_joint_states()["j0"].position;
        if prev > 0.0 && p <= 0.0 {
            crossings.push(step as f64 * dt as f64);
        }
        prev = p;
        if crossings.len() >= 5 {
            break;
        }
    }
    assert!(crossings.len() >= 3, "pendulum did not oscillate");
    let t_measured = (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64;

    let err = (t_measured - t_analytic).abs() / t_analytic;
    println!(
        "m={m:.4e} kg  d={d:.4} m  I_com={i_com:.4e}  I_o={i_o:.4e}\n\
         period analytic={t_analytic:.5} s  measured={t_measured:.5} s  err={:.3}%",
        err * 100.0
    );
    assert!(err < 0.02, "period off by {:.2}%", err * 100.0);
}

#[test]
fn end_effector_pose_tracks_joint_motion() {
    let doc = doc_with(1, V::new(1.0, 0.0, 0.0), 0.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    let rest = world.get_instance_pose("bob0").unwrap();

    // Drive the joint well off the rest pose.
    world.set_joint_position("j0", 70.0);
    for _ in 0..600 {
        world.step(0.004);
    }
    let moved = world.get_instance_pose("bob0").unwrap();
    // The part origin coincides with the child anchor, which sits on the
    // pivot — so only the orientation can change here.
    let dq: f64 = (0..4)
        .map(|i| (moved.1[i] - rest.1[i]).abs())
        .fold(0.0, f64::max);
    println!(
        "rest={:?} {:?}\nmoved={:?} {:?}",
        rest.0, rest.1, moved.0, moved.1
    );
    let s = world.get_joint_states();
    println!("j0 = {:.3} deg, max |dquat| = {dq:.4}", s["j0"].position);
    assert!(
        s["j0"].position.abs() > 10.0,
        "joint never moved: {}",
        s["j0"].position
    );
    assert!(
        dq > 1e-3,
        "end-effector orientation did not follow the joint (max |dquat| {dq})"
    );
}

/// Rule out the "revolute FK rotates about the world origin" failure for the
/// *physics* FK path: the bob tip must trace an arc centred on the joint
/// anchor, not on the world origin.
#[test]
fn revolute_fk_rotates_about_the_joint_anchor() {
    let doc = doc_with(1, V::new(1.0, 0.0, 0.0), 0.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    let pivot = [0.020, 0.010, 0.060]; // parent_anchor in metres
    let tip_local = [0.0, 0.0, -0.052]; // bob tip in part-local metres

    for &deg in &[0.0, 30.0, 90.0] {
        let poses = world.forward_kinematics_at(&[deg]).unwrap();
        let (pos, q) = poses["bob0"];
        // Rotate tip_local by the reported quaternion, add the origin.
        let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
        let (tx, ty, tz) = (tip_local[0], tip_local[1], tip_local[2]);
        // v' = v + 2w(q_v × v) + 2 q_v × (q_v × v)
        let cx = y * tz - z * ty;
        let cy = z * tx - x * tz;
        let cz = x * ty - y * tx;
        let ccx = y * cz - z * cy;
        let ccy = z * cx - x * cz;
        let ccz = x * cy - y * cx;
        let tip = [
            pos[0] + tx + 2.0 * w * cx + 2.0 * ccx,
            pos[1] + ty + 2.0 * w * cy + 2.0 * ccy,
            pos[2] + tz + 2.0 * w * cz + 2.0 * ccz,
        ];
        let r: f64 = (0..3)
            .map(|i| (tip[i] - pivot[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        println!(
            "q={deg:5.1} deg  tip=({:.4},{:.4},{:.4})  |tip-pivot|={r:.5} m",
            tip[0], tip[1], tip[2]
        );
        assert!(
            (r - 0.052).abs() < 1e-6,
            "tip is {r} m from the anchor at q={deg}, expected 0.052 — \
             rotation is not centred on the joint anchor"
        );
    }
}

/// Symptom 2 as reported: five sibling pendulums, torque on one only.
#[test]
fn torque_does_not_leak_to_sibling_joints() {
    let doc = doc_with(5, V::new(0.0, 1.0, 0.0), 0.0);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    world.apply_joint_torque("j2", 2e-6);
    for _ in 0..5 {
        world.step(0.002);
    }
    let s = world.get_joint_states();
    for i in 0..5 {
        let j = format!("j{i}");
        println!(
            "  {j}: pos={:12.6} vel={:12.6}",
            s[&j].position, s[&j].velocity
        );
    }
    assert!(s["j2"].position.abs() > 1e-9, "driven joint did not move");
    for i in [0, 1, 3, 4] {
        let j = format!("j{i}");
        assert!(
            s[&j].position.abs() < 1e-9 && s[&j].velocity.abs() < 1e-9,
            "torque leaked into {j}: pos={} vel={}",
            s[&j].position,
            s[&j].velocity
        );
    }
}
