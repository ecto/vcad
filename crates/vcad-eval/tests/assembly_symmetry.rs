//! Regression tests for the loon assembly-symmetry vocabulary
//! (`mirror-group-x/y/z`, `assembly-join`).
//!
//! Hand-mirroring an assembly is the error-prone case: an instance placement
//! is easy to flip, but a joint anchor must flip *together with its axis*, and
//! the axis rule is counterintuitive. Under a reflection M,
//! `M · R(a, θ) · M = R(−M a, θ)`, so mirroring across X leaves an X-axis
//! hinge alone and negates a Y- or Z-axis hinge. A naive "negate the axis"
//! implementation passes the Y-hinge case and fails the X-hinge one, so both
//! are covered.
//!
//! Every case reduces to the same measurement: the whole assembly's centre of
//! mass must lie on the mirror plane — at rest, and with both mirrored joints
//! driven to the same state.

use std::collections::HashMap;
use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{Document, Transform3D, Vec3};
use vcad_loon::eval_vcad;

/// Degrees to radians.
fn rad(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

/// Euler (XYZ degrees, applied Rz·Ry·Rx — the assembly convention) to matrix.
fn euler_to_matrix(a: &Vec3) -> [[f64; 3]; 3] {
    let (cx, sx) = (rad(a.x).cos(), rad(a.x).sin());
    let (cy, sy) = (rad(a.y).cos(), rad(a.y).sin());
    let (cz, sz) = (rad(a.z).cos(), rad(a.z).sin());
    [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ]
}

fn apply(t: &Transform3D, p: [f64; 3]) -> [f64; 3] {
    let m = euler_to_matrix(&t.rotation);
    let s = [p[0] * t.scale.x, p[1] * t.scale.y, p[2] * t.scale.z];
    [
        t.translation.x + m[0][0] * s[0] + m[0][1] * s[1] + m[0][2] * s[2],
        t.translation.y + m[1][0] * s[0] + m[1][1] * s[1] + m[1][2] * s[2],
        t.translation.z + m[2][0] * s[0] + m[2][1] * s[1] + m[2][2] * s[2],
    ]
}

/// Whole-assembly centre of mass: every instance's mesh, posed by the FK
/// world transform, integrated with the divergence theorem.
///
/// A mirrored solid tessellates with reversed winding, so its signed volume
/// comes out negative. `moment / volume` is invariant under that flip, and
/// the cross-instance weighting uses `|volume|`.
fn assembly_com(doc: &Document) -> [f64; 3] {
    let scene = evaluate_document(
        doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
        },
    )
    .expect("evaluate_document");
    let instances = scene.instances.as_ref().expect("assembly instances");
    assert!(!instances.is_empty(), "no instances evaluated");

    let mut total = 0.0;
    let mut moment = [0.0; 3];
    for inst in instances {
        let t = inst.transform.unwrap_or_else(Transform3D::identity);
        let mesh = &inst.mesh;
        let vert = |vi: usize| -> [f64; 3] {
            apply(
                &t,
                [
                    mesh.positions[vi * 3] as f64,
                    mesh.positions[vi * 3 + 1] as f64,
                    mesh.positions[vi * 3 + 2] as f64,
                ],
            )
        };
        let mut vol = 0.0;
        let mut m = [0.0; 3];
        for tri in 0..mesh.indices.len() / 3 {
            let a = vert(mesh.indices[tri * 3] as usize);
            let b = vert(mesh.indices[tri * 3 + 1] as usize);
            let c = vert(mesh.indices[tri * 3 + 2] as usize);
            let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
            let v = det / 6.0;
            vol += v;
            for (k, mk) in m.iter_mut().enumerate() {
                *mk += v * (a[k] + b[k] + c[k]) / 4.0;
            }
        }
        assert!(
            vol.abs() > 1e-9,
            "instance {} has no volume",
            inst.instance_id
        );
        let weight = vol.abs();
        total += weight;
        for k in 0..3 {
            moment[k] += weight * (m[k] / vol);
        }
    }
    [moment[0] / total, moment[1] / total, moment[2] / total]
}

/// World transforms keyed by instance id.
fn world(doc: &Document) -> HashMap<String, Transform3D> {
    vcad_eval::solve_forward_kinematics(doc)
}

/// A chassis symmetric about x = 0, plus one CHIRAL leg (the knob sits at +x
/// of the shank) hung off a hinge, mirrored across X. `axis` picks the hinge
/// direction so both the flipping and the non-flipping rule get exercised.
fn quadruped_source(axis: (f64, f64, f64)) -> String {
    let (ax, ay, az) = axis;
    format!(
        r#"
[let chassis
  [assembly
    #[[part "body" [translate -30.0 -20.0 0.0 [cube 60.0 40.0 10.0]] "aluminum"]]
    #[[instance "body-i" "body" 0.0 0.0 0.0]]
    #[]
    "body-i"]]

; One side: a shank down -Z with a knob offset to +x — chiral on purpose, so a
; copy that is translated but not reflected shifts the centre of mass.
[let leg-part
  [part "femur"
    [union [translate 4.0 -3.0 -20.0 [cube 6.0 6.0 6.0]]
           [translate -4.0 -4.0 -30.0 [cube 8.0 8.0 30.0]]]
    "abs"]]

[let one-side
  [assembly
    #[leg-part]
    #[[instance "leg" "femur" 25.0 12.0 0.0]]
    #[[revolute-joint "hip" {ax} {ay} {az} -90.0 90.0
        "body-i" 25.0 12.0 0.0 "leg" 0.0 0.0 0.0]]
    "body-i"]]

[assembly-join chassis [mirror-group-x "-r" one-side]]
"#
    )
}

fn doc_for(axis: (f64, f64, f64), state: f64) -> Document {
    let mut doc = eval_vcad(&quadruped_source(axis), None).expect("eval_vcad");
    let joints = doc.joints.as_mut().expect("joints");
    assert_eq!(joints.len(), 2, "one authored hip + one mirrored hip");
    for j in joints.iter_mut() {
        j.state = state;
    }
    doc
}

fn assert_on_plane(com: [f64; 3], what: &str) {
    assert!(
        com[0].abs() < 1e-6,
        "{what}: centre of mass off the mirror plane, x = {}",
        com[0]
    );
}

/// The mirrored group exists at all: suffixed part, instance, and joint, with
/// the joint's out-of-group parent (the shared body) left alone.
#[test]
fn mirror_group_expands_and_keeps_shared_parents() {
    let doc = doc_for((0.0, 1.0, 0.0), 0.0);
    let ids: Vec<&str> = doc
        .instances
        .as_ref()
        .unwrap()
        .iter()
        .map(|i| i.id.as_str())
        .collect();
    assert!(ids.contains(&"leg") && ids.contains(&"leg-r"), "{ids:?}");
    assert!(doc.part_defs.as_ref().unwrap().contains_key("femur-r"));

    let joints = doc.joints.as_ref().unwrap();
    for j in joints {
        assert_eq!(
            j.parent_instance_id.as_deref(),
            Some("body-i"),
            "a parent outside the mirrored group must keep its name"
        );
    }
    let mirrored = joints
        .iter()
        .find(|j| j.child_instance_id == "leg-r")
        .expect("mirrored hip");
    assert!((mirrored.parent_anchor.x + 25.0).abs() < 1e-12);
    assert!((mirrored.parent_anchor.y - 12.0).abs() < 1e-12);
}

/// The other joint kinds. A prismatic axis is a displacement, not a
/// pseudovector, so it mirrors the opposite way from a hinge: `a' = M a`, i.e.
/// the along-normal component flips. Fixed and ball joints carry anchors only.
#[test]
fn prismatic_fixed_and_ball_joints_mirror() {
    let src = r#"
[let side
  [assembly
    #[[part "p" [cube 4.0 4.0 4.0] "abs"]]
    #[[instance "a" "p" 10.0 0.0 0.0] [instance "b" "p" 10.0 0.0 20.0]]
    #[[prismatic-joint "slide" 1.0 2.0 0.0 0.0 50.0 "a" 10.0 3.0 0.0 "b" 1.0 2.0 3.0]
      [fixed-joint "weld" "a" 10.0 3.0 0.0 "b" 1.0 2.0 3.0]
      [ball-joint "socket" "a" 10.0 3.0 0.0 "b" 1.0 2.0 3.0]]
    "a"]]
[mirror-group-x "-r" side]
"#;
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let joints = doc.joints.as_ref().unwrap();
    assert_eq!(joints.len(), 6, "three authored + three mirrored");

    for j in joints.iter().filter(|j| j.child_instance_id == "b-r") {
        assert_eq!(j.parent_instance_id.as_deref(), Some("a-r"));
        // Anchors mirror on both ends; both endpoints are inside the group.
        assert!((j.parent_anchor.x + 10.0).abs() < 1e-12);
        assert!((j.parent_anchor.y - 3.0).abs() < 1e-12);
        assert!((j.child_anchor.x + 1.0).abs() < 1e-12);
        assert!((j.child_anchor.y - 2.0).abs() < 1e-12);
        if let vcad_ir::JointKind::Slider { axis, limits, .. } = &j.kind {
            assert!(
                (axis.x + 1.0).abs() < 1e-12 && (axis.y - 2.0).abs() < 1e-12,
                "a prismatic axis mirrors as M·a, got {axis:?}"
            );
            assert_eq!(*limits, Some((0.0, 50.0)), "travel limits carry over");
        }
    }
}

/// At rest, the mirrored assembly's centre of mass is on the plane. The leg is
/// chiral, so this fails unless the mirrored PART is reflected too — not just
/// placed at a negated position.
#[test]
fn mirrored_assembly_com_is_on_the_plane_at_rest() {
    assert_on_plane(assembly_com(&doc_for((0.0, 1.0, 0.0), 0.0)), "y-hinge rest");
    assert_on_plane(assembly_com(&doc_for((1.0, 0.0, 0.0), 0.0)), "x-hinge rest");
}

/// Driven: both hips at the same state must swing symmetrically, so the COM
/// stays on the plane. A Y-axis hinge must have its axis NEGATED by an X
/// mirror — leaving it alone swings both legs the same way and walks the COM
/// off the plane.
#[test]
fn mirrored_y_hinge_drives_symmetrically() {
    for state in [15.0, 35.0, -60.0] {
        let doc = doc_for((0.0, 1.0, 0.0), state);
        assert_on_plane(assembly_com(&doc), &format!("y-hinge at {state}°"));

        let w = world(&doc);
        let l = w.get("leg").expect("leg");
        let r = w.get("leg-r").expect("leg-r");
        assert!(
            (l.translation.x + r.translation.x).abs() < 1e-9,
            "hip x should mirror: {} vs {}",
            l.translation.x,
            r.translation.x
        );
        assert!((l.translation.y - r.translation.y).abs() < 1e-9);
        assert!(
            (l.rotation.y + r.rotation.y).abs() < 1e-6,
            "a Y hinge mirrored across X must turn the opposite way: {} vs {}",
            l.rotation.y,
            r.rotation.y
        );
    }
}

/// The counterintuitive half of the same rule: an X-axis hinge mirrored across
/// X keeps its axis. Negating it here would break symmetry just as badly.
#[test]
fn mirrored_x_hinge_keeps_its_axis() {
    for state in [15.0, 35.0, -60.0] {
        let doc = doc_for((1.0, 0.0, 0.0), state);
        assert_on_plane(assembly_com(&doc), &format!("x-hinge at {state}°"));

        let hip = doc
            .joints
            .as_ref()
            .unwrap()
            .iter()
            .find(|j| j.child_instance_id == "leg-r")
            .unwrap();
        let axis = match &hip.kind {
            vcad_ir::JointKind::Revolute { axis, .. } => *axis,
            other => panic!("expected a revolute hip, got {other:?}"),
        };
        assert!(
            (axis.x - 1.0).abs() < 1e-12 && axis.y.abs() < 1e-12 && axis.z.abs() < 1e-12,
            "an X axis must survive an X mirror unchanged, got {axis:?}"
        );

        let w = world(&doc);
        let l = w.get("leg").unwrap();
        let r = w.get("leg-r").unwrap();
        assert!(
            (l.rotation.x - r.rotation.x).abs() < 1e-6,
            "an X hinge mirrored across X turns the SAME way: {} vs {}",
            l.rotation.x,
            r.rotation.x
        );
    }
}
