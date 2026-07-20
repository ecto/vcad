//! Physics body poses must agree with the document FK solver
//! (`vcad_eval::solve_forward_kinematics`) — same contract: a joint fully
//! places its child; anchors are in each part's local mm frame.

use vcad_ir::Document;
use vcad_kernel_physics::PhysicsWorld;

fn doc(axis: [f64; 3], state: f64, child_anchor: [f64; 3]) -> Document {
    serde_json::from_value(serde_json::json!({
        "version": "0.1",
        "nodes": {
            "0": {"id": 0, "op": {"type": "Cube", "size": {"x": 20.0, "y": 20.0, "z": 20.0}}}
        },
        "roots": [],
        "materials": {},
        "part_materials": {},
        "partDefs": {"p": {"id": "p", "name": "p", "root": 0}},
        "instances": [
            {"id": "base", "partDefId": "p"},
            {"id": "arm", "partDefId": "p"}
        ],
        "joints": [
            {"id": "j0", "parentInstanceId": "base", "childInstanceId": "arm",
             "parentAnchor": {"x": 10.0, "y": 0.0, "z": 90.0},
             "childAnchor": {"x": child_anchor[0], "y": child_anchor[1], "z": child_anchor[2]},
             "kind": {"type": "Revolute",
                       "axis": {"x": axis[0], "y": axis[1], "z": axis[2]},
                       "limits": [-360.0, 360.0]},
             "state": state}
        ],
        "groundInstanceId": "base"
    }))
    .expect("valid doc")
}

fn quat_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    // v' = v + 2*qv × (qv × v + w*v)
    let qv = [x, y, z];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(
        qv,
        [
            cross(qv, v)[0] + w * v[0],
            cross(qv, v)[1] + w * v[1],
            cross(qv, v)[2] + w * v[2],
        ],
    );
    [v[0] + 2.0 * t[0], v[1] + 2.0 * t[1], v[2] + 2.0 * t[2]]
}

/// The world position of the child part's local point `p` (mm), per physics.
fn phys_world_point(world: &mut PhysicsWorld, theta_deg: f64, p_mm: [f64; 3]) -> [f64; 3] {
    let poses = world.forward_kinematics_at(&[theta_deg]).expect("fk");
    let (pos, quat) = poses.get("arm").expect("arm pose");
    // get_instance_pose / forward_kinematics_at return the PART pose: the
    // part-local origin's world placement. World point = pos + R * (p/1000).
    let p_m = [p_mm[0] / 1000.0, p_mm[1] / 1000.0, p_mm[2] / 1000.0];
    let r = quat_rotate(*quat, p_m);
    [pos[0] + r[0], pos[1] + r[1], pos[2] + r[2]]
}

/// Same point per the document FK solver (result in mm → converted to m).
fn eval_world_point(document: &Document, p_mm: [f64; 3]) -> [f64; 3] {
    let fk = vcad_eval::solve_forward_kinematics(document);
    let t = fk.get("arm").expect("arm fk");
    // Apply Transform3D (rotation is Euler XYZ degrees, Rz*Ry*Rx) to p.
    let rx = t.rotation.x.to_radians();
    let ry = t.rotation.y.to_radians();
    let rz = t.rotation.z.to_radians();
    let (cx, sx) = (rx.cos(), rx.sin());
    let (cy, sy) = (ry.cos(), ry.sin());
    let (cz, sz) = (rz.cos(), rz.sin());
    let m = [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ];
    let p = p_mm;
    let rp = [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
    ];
    [
        (t.translation.x + rp[0]) / 1000.0,
        (t.translation.y + rp[1]) / 1000.0,
        (t.translation.z + rp[2]) / 1000.0,
    ]
}

fn assert_close(a: [f64; 3], b: [f64; 3], tol: f64, label: &str) {
    for i in 0..3 {
        assert!(
            (a[i] - b[i]).abs() < tol,
            "{label}: component {i}: phys {} vs eval {} (phys {a:?} eval {b:?})",
            a[i],
            b[i]
        );
    }
}

fn check(axis: [f64; 3], theta: f64, child_anchor: [f64; 3]) {
    let d = doc(axis, theta, child_anchor);
    let mut world = PhysicsWorld::from_document(&d).expect("world");
    // Probe several material points of the child part.
    for p in [[0.0, 0.0, 0.0], child_anchor, [17.0, -3.0, 5.0]] {
        let phys = phys_world_point(&mut world, theta, p);
        let eval = eval_world_point(&d, p);
        assert_close(
            phys,
            eval,
            1e-6,
            &format!("axis {axis:?} theta {theta} p {p:?}"),
        );
    }
}

#[test]
fn revolute_z_axis_matches_eval_fk() {
    check([0.0, 0.0, 1.0], 0.0, [0.0, 0.0, 0.0]);
    check([0.0, 0.0, 1.0], 90.0, [5.0, 0.0, 0.0]);
}

#[test]
fn revolute_y_axis_matches_eval_fk() {
    check([0.0, 1.0, 0.0], 0.0, [0.0, 0.0, 0.0]);
    check([0.0, 1.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
    check([0.0, 1.0, 0.0], 37.0, [5.0, 2.0, -4.0]);
}

#[test]
fn revolute_x_axis_with_child_anchor_matches_eval_fk() {
    check([1.0, 0.0, 0.0], 45.0, [0.0, 8.0, 3.0]);
}
