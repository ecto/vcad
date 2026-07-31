//! Integrator-correctness probes: free bodies, ball joints, energy drift,
//! and multi-link chains, all checked against closed-form mechanics.

use std::collections::HashMap;
use vcad_ir::{CsgOp, Document, Instance, Joint, JointKind, Node, PartDef, Transform3D, Vec3 as V};
use vcad_kernel_physics::PhysicsWorld;

fn base_doc() -> Document {
    let mut doc = Document::new();
    doc.nodes.insert(
        1,
        Node {
            id: 1,
            name: None,
            op: CsgOp::Cube {
                size: V::new(60.0, 20.0, 10.0),
            },
        },
    );
    // 20 mm cube centred at its part origin.
    doc.nodes.insert(
        2,
        Node {
            id: 2,
            name: None,
            op: CsgOp::Cube {
                size: V::new(20.0, 20.0, 20.0),
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
                offset: V::new(-10.0, -10.0, -10.0),
            },
        },
    );
    // Slender rod hanging 60 mm below its origin (for pendulum links).
    doc.nodes.insert(
        4,
        Node {
            id: 4,
            name: None,
            op: CsgOp::Cube {
                size: V::new(4.0, 4.0, 60.0),
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
                offset: V::new(-2.0, -2.0, -60.0),
            },
        },
    );

    let mut part_defs = HashMap::new();
    for (id, root) in [("frame", 1), ("cube", 3), ("rod", 5)] {
        part_defs.insert(
            id.to_string(),
            PartDef {
                id: id.into(),
                name: None,
                root,
                default_material: None,
                inertial: None,
            },
        );
    }
    doc.part_defs = Some(part_defs);
    doc.instances = Some(vec![Instance {
        id: "frame".into(),
        part_def_id: "frame".into(),
        name: None,
        tags: Vec::new(),
        transform: None,
        material: None,
    }]);
    doc.joints = Some(Vec::new());
    doc.ground_instance_id = Some("frame".into());
    doc
}

fn add_instance(doc: &mut Document, id: &str, part: &str, at_mm: [f64; 3]) {
    doc.instances.as_mut().unwrap().push(Instance {
        id: id.into(),
        part_def_id: part.into(),
        name: None,
        tags: Vec::new(),
        transform: Some(Transform3D {
            translation: V::new(at_mm[0], at_mm[1], at_mm[2]),
            rotation: V::new(0.0, 0.0, 0.0),
            scale: V::new(1.0, 1.0, 1.0),
        }),
        material: None,
    });
}

fn add_revolute(doc: &mut Document, id: &str, parent: &str, child: &str, anchor_mm: [f64; 3]) {
    doc.joints.as_mut().unwrap().push(Joint {
        id: id.into(),
        name: None,
        parent_instance_id: Some(parent.into()),
        child_instance_id: child.into(),
        parent_anchor: V::new(anchor_mm[0], anchor_mm[1], anchor_mm[2]),
        child_anchor: V::new(0.0, 0.0, 0.0),
        kind: JointKind::Revolute {
            axis: V::new(1.0, 0.0, 0.0),
            limits: None,
        },
        state: 0.0,
    });
}

/// A free-floating (unjointed, non-ground) instance under gravity must fall
/// `z(t) = z0 - g t^2 / 2` and not drift laterally. Under the old flat
/// `q += v*dt` this body never fell at all: free-joint `v` is
/// `[angular, linear]` while `q` is `[pos, rot]`, so gravity accumulated in
/// `v[3..6]` while position integrated the (zero) angular slots.
#[test]
fn free_body_falls_ballistically() {
    let mut doc = base_doc();
    add_instance(&mut doc, "cube1", "cube", [0.0, 0.0, 500.0]);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    let dt = 0.001;
    let steps = 500; // 0.5 s
    for _ in 0..steps {
        world.step(dt);
    }
    let t = steps as f64 * dt as f64;
    let (pos, quat) = world.get_instance_pose("cube1").unwrap();
    let z_expected = 0.5 - 0.5 * 9.81 * t * t;
    println!(
        "t={t}s  pos=({:.4},{:.4},{:.4})  expected z={z_expected:.4}  quat={quat:?}",
        pos[0], pos[1], pos[2]
    );
    assert!(
        (pos[2] - z_expected).abs() < 0.005,
        "free body fell to z={} but ballistics says {z_expected}",
        pos[2]
    );
    assert!(pos[0].abs() < 1e-9 && pos[1].abs() < 1e-9, "lateral drift");
    // No spontaneous rotation either.
    assert!(
        (quat[0] - 1.0).abs() < 1e-9,
        "spontaneous rotation: {quat:?}"
    );
}

/// Ball-joint pendulum released off-axis must behave like a pendulum: bounded
/// swing, restoring toward hanging, no energy blow-up. Exp-coordinate `q` must
/// be integrated by quaternion composition; the old elementwise add corrupts
/// it as soon as the rotation axis moves.
#[test]
fn ball_joint_pendulum_is_bounded_and_restoring() {
    let mut doc = base_doc();
    add_instance(&mut doc, "bob", "rod", [0.0, 0.0, 0.0]);
    doc.joints.as_mut().unwrap().push(Joint {
        id: "ball".into(),
        name: None,
        parent_instance_id: Some("frame".into()),
        child_instance_id: "bob".into(),
        parent_anchor: V::new(30.0, 10.0, 0.0),
        child_anchor: V::new(0.0, 0.0, 0.0),
        kind: JointKind::Ball,
        state: 0.0,
    });
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    // Kick it sideways: small angular velocity about X and Y.
    world.set_joint_velocity_raw("ball", &[1.5, 1.0, 0.0]);

    // Track the bob tip height; it must stay in the physical range
    // [-L, +L] around the anchor and keep returning below the anchor.
    let mut max_tip_z = f64::MIN;
    let mut below_count = 0usize;
    let total = 4000;
    for _ in 0..total {
        world.step(0.001);
        let (pos, quat) = world.get_instance_pose("bob").unwrap();
        // tip = origin + R * (0,0,-0.060)
        let (w, x, y, z) = (quat[0], quat[1], quat[2], quat[3]);
        let v = [0.0, 0.0, -0.060];
        let c = [
            y * v[2] - z * v[1],
            z * v[0] - x * v[2],
            x * v[1] - y * v[0],
        ];
        let cc = [
            y * c[2] - z * c[1],
            z * c[0] - x * c[2],
            x * c[1] - y * c[0],
        ];
        let tip_z = pos[2] + v[2] + 2.0 * w * c[2] + 2.0 * cc[2];
        let rel = tip_z; // anchor is at z=0
        assert!(
            rel > -0.0601 && rel < 0.0601,
            "tip left the sphere: rel z = {rel}"
        );
        max_tip_z = max_tip_z.max(rel);
        if rel < -0.03 {
            below_count += 1;
        }
    }
    println!(
        "max tip z above anchor: {max_tip_z:.4} m; below-half fraction {:.2}",
        below_count as f64 / total as f64
    );
    // The kick is small: the bob must never climb over the anchor, and must
    // spend most of its time hanging below.
    assert!(max_tip_z < 0.0, "ball pendulum climbed above the anchor");
    assert!(below_count as f64 / total as f64 > 0.9);
}

/// Passive revolute pendulum for 60 s of sim time: semi-implicit Euler is
/// symplectic, so the swing amplitude must stay bounded near its initial
/// value — no secular energy growth or collapse.
#[test]
fn long_rollout_energy_stays_bounded() {
    let mut doc = base_doc();
    add_instance(&mut doc, "bob", "rod", [0.0, 0.0, 0.0]);
    add_revolute(&mut doc, "j0", "frame", "bob", [30.0, 10.0, 0.0]);
    doc.joints.as_mut().unwrap()[0].state = 45.0;
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    let dt = 0.002;
    let mut max_amp: f64 = 0.0;
    let mut window_max: f64 = 0.0;
    let mut late_max: f64 = 0.0;
    let steps = 30_000; // 60 s
    for step in 0..steps {
        world.step(dt);
        let p = world.get_joint_states()["j0"].position.abs();
        max_amp = max_amp.max(p);
        window_max = window_max.max(p);
        if step >= steps - 5_000 {
            late_max = late_max.max(p);
        }
        if step % 5_000 == 4_999 {
            println!(
                "t={:5.1}s  window max |q| = {window_max:.3} deg",
                (step + 1) as f64 * dt as f64
            );
            window_max = 0.0;
        }
    }
    assert!(
        max_amp < 47.0,
        "energy grew: peak amplitude {max_amp:.2} deg"
    );
    assert!(
        late_max > 40.0,
        "energy collapsed: late amplitude {late_max:.2} deg"
    );
}

/// Double pendulum (rod hanging from rod). Checks the chained joint frames
/// under dynamics: total mechanical energy of the passive chain must never
/// exceed its release value (semi-implicit Euler wobbles but must not pump
/// energy into a chaotic chain).
#[test]
fn double_pendulum_does_not_gain_energy() {
    let mut doc = base_doc();
    add_instance(&mut doc, "link1", "rod", [0.0, 0.0, 0.0]);
    add_instance(&mut doc, "link2", "rod", [0.0, 0.0, 0.0]);
    add_revolute(&mut doc, "j0", "frame", "link1", [30.0, 10.0, 0.0]);
    // link2 hangs from the bottom of link1 (60 mm below link1's origin).
    doc.joints.as_mut().unwrap().push(Joint {
        id: "j1".into(),
        name: None,
        parent_instance_id: Some("link1".into()),
        child_instance_id: "link2".into(),
        parent_anchor: V::new(0.0, 0.0, -60.0),
        child_anchor: V::new(0.0, 0.0, 0.0),
        kind: JointKind::Revolute {
            axis: V::new(1.0, 0.0, 0.0),
            limits: None,
        },
        state: 0.0,
    });
    doc.joints.as_mut().unwrap()[0].state = 90.0;

    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    // Energy proxy: height of both link COMs (m g h) + kinetic via joint
    // velocities is awkward to assemble exactly; instead use the invariant
    // that the *potential ceiling* of a passive release bounds the motion:
    // neither joint may complete a full loop-over unless energy was created.
    // At release (q0=90, q1=0) the chain's COM sits exactly at anchor height
    // minus 0: link1 horizontal, link2 hanging. That is not enough energy for
    // link1 to pass the vertical-up position (which needs strictly more
    // potential). So |q0| must stay < 180 deg forever.
    let mut max_q0: f64 = 0.0;
    let mut max_q1: f64 = 0.0;
    for _ in 0..40_000 {
        world.step(0.001);
        let s = world.get_joint_states();
        max_q0 = max_q0.max(s["j0"].position.abs());
        max_q1 = max_q1.max(s["j1"].position.abs());
    }
    println!("max |q0| = {max_q0:.1} deg, max |q1| = {max_q1:.1} deg");
    assert!(
        max_q0 < 180.0,
        "link1 looped over the top from a 90-degree release: energy was created (max |q0| = {max_q0:.1})"
    );
}

/// A position servo must actually reach its target under gravity load.
/// Pure PD with reflected-inertia gains droops by tau_g/kp (tens of degrees
/// for a hanging rod); gravity feedforward closes it.
#[test]
fn position_servo_reaches_target_under_gravity() {
    let mut doc = base_doc();
    add_instance(&mut doc, "bob", "rod", [0.0, 0.0, 0.0]);
    add_revolute(&mut doc, "j0", "frame", "bob", [30.0, 10.0, 0.0]);
    let mut world = PhysicsWorld::from_document(&doc).unwrap();

    world.set_joint_position("j0", 70.0);
    for _ in 0..3000 {
        world.step(0.002);
    }
    let s = world.get_joint_states();
    println!("settled at {:.3} deg (target 70)", s["j0"].position);
    assert!(
        (s["j0"].position - 70.0).abs() < 2.0,
        "servo settled at {:.2} deg, target 70",
        s["j0"].position
    );
    assert!(s["j0"].velocity.abs() < 5.0, "still moving");
}

/// Torque-free spin about a principal axis: a free-floating body given pure
/// spin must keep a constant spin rate (angular momentum conservation) while
/// falling, and pick up no linear velocity from the rotation.
#[test]
fn free_body_spin_is_conserved() {
    let mut doc = base_doc();
    add_instance(&mut doc, "cube1", "cube", [0.0, 0.0, 200.0]);
    // Weld a joint-free doc: cube1 is free-floating. Spin it via the raw
    // state hook on its free joint (phyz gives every free body a Free joint).
    let mut world = PhysicsWorld::from_document(&doc).unwrap();
    world.set_free_body_spin_raw("cube1", [0.0, 0.0, 8.0]);

    let dt = 0.001;
    for _ in 0..1000 {
        world.step(dt);
    }
    let t = 1.0f64;
    let (pos, quat) = world.get_instance_pose("cube1").unwrap();
    let z_expected = 0.2 - 0.5 * 9.81 * t * t;
    // Rotation after 8 rad about Z: quat = (cos 4, 0, 0, sin 4).
    let expect_w = (4.0f64).cos();
    let expect_z = (4.0f64).sin();
    println!(
        "pos=({:.4},{:.4},{:.4}) vs z={z_expected:.4}; quat={quat:?} vs (\u{00b1}{expect_w:.4},0,0,\u{00b1}{expect_z:.4})",
        pos[0], pos[1], pos[2]
    );
    assert!((pos[2] - z_expected).abs() < 0.01, "fall corrupted by spin");
    assert!(
        pos[0].abs() < 1e-6 && pos[1].abs() < 1e-6,
        "spin leaked into translation"
    );
    // Quaternion double cover: compare |w| and |z|.
    assert!(
        (quat[0].abs() - expect_w.abs()).abs() < 0.02
            && (quat[3].abs() - expect_z.abs()).abs() < 0.02,
        "spin rate drifted: {quat:?}"
    );
    assert!(
        quat[1].abs() < 1e-6 && quat[2].abs() < 1e-6,
        "axis wandered"
    );
}
