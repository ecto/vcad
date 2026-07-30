//! Forward kinematics solver for assembly joints.
//!
//! Ported from `packages/engine/src/kinematics.ts`.

use std::collections::{HashMap, HashSet, VecDeque};
use vcad_ir::{Document, Joint, JointKind, Transform3D, Vec3};

/// Create an identity transform.
fn identity() -> Transform3D {
    Transform3D::identity()
}

/// Degrees to radians.
fn deg_to_rad(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

/// 3x3 rotation matrix stored row-major.
type Mat3 = [[f64; 3]; 3];

/// Euler angles (XYZ order, degrees) to rotation matrix.
fn euler_to_matrix(angles: &Vec3) -> Mat3 {
    let rx = deg_to_rad(angles.x);
    let ry = deg_to_rad(angles.y);
    let rz = deg_to_rad(angles.z);

    let (cx, sx) = (rx.cos(), rx.sin());
    let (cy, sy) = (ry.cos(), ry.sin());
    let (cz, sz) = (rz.cos(), rz.sin());

    // Rz * Ry * Rx
    [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ]
}

/// Multiply 3x3 matrix by Vec3.
fn mat_vec3(m: &Mat3, v: &Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

/// Multiply two 3x3 matrices.
fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

/// Extract Euler angles (degrees) from rotation matrix.
fn matrix_to_euler(m: &Mat3) -> Vec3 {
    let sy = -m[2][0];
    let cy = (m[0][0] * m[0][0] + m[1][0] * m[1][0]).sqrt();

    if cy > 1e-6 {
        Vec3::new(
            m[2][1].atan2(m[2][2]) * 180.0 / std::f64::consts::PI,
            sy.atan2(cy) * 180.0 / std::f64::consts::PI,
            m[1][0].atan2(m[0][0]) * 180.0 / std::f64::consts::PI,
        )
    } else {
        // Gimbal lock
        Vec3::new(
            (-m[1][2]).atan2(m[1][1]) * 180.0 / std::f64::consts::PI,
            sy.atan2(cy) * 180.0 / std::f64::consts::PI,
            0.0,
        )
    }
}

fn vec3_add(a: &Vec3, b: &Vec3) -> Vec3 {
    Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn vec3_sub(a: &Vec3, b: &Vec3) -> Vec3 {
    Vec3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

fn vec3_scale(v: &Vec3, s: f64) -> Vec3 {
    Vec3::new(v.x * s, v.y * s, v.z * s)
}

fn vec3_normalize(v: &Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if len < 1e-10 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        vec3_scale(v, 1.0 / len)
    }
}

/// Rotation matrix from axis-angle (axis normalized, angle in degrees).
fn axis_angle_to_matrix(axis: &Vec3, angle_deg: f64) -> Mat3 {
    let angle = deg_to_rad(angle_deg);
    let c = angle.cos();
    let s = angle.sin();
    let t = 1.0 - c;
    let (x, y, z) = (axis.x, axis.y, axis.z);

    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Compose two transforms: result = outer * inner.
fn compose_transforms(outer: &Transform3D, inner: &Transform3D) -> Transform3D {
    let scale = Vec3::new(
        outer.scale.x * inner.scale.x,
        outer.scale.y * inner.scale.y,
        outer.scale.z * inner.scale.z,
    );

    let outer_rot = euler_to_matrix(&outer.rotation);
    let inner_rot = euler_to_matrix(&inner.rotation);
    let composed_rot = mat_mul(&outer_rot, &inner_rot);
    let rotation = matrix_to_euler(&composed_rot);

    let scaled_inner_trans = Vec3::new(
        outer.scale.x * inner.translation.x,
        outer.scale.y * inner.translation.y,
        outer.scale.z * inner.translation.z,
    );
    let rotated_inner_trans = mat_vec3(&outer_rot, &scaled_inner_trans);
    let translation = vec3_add(&outer.translation, &rotated_inner_trans);

    Transform3D {
        translation,
        rotation,
        scale,
    }
}

/// Compute the transform induced by a joint at its current state.
fn compute_joint_transform(joint: &Joint) -> Transform3D {
    match &joint.kind {
        JointKind::Fixed => Transform3D {
            translation: vec3_sub(&joint.parent_anchor, &joint.child_anchor),
            rotation: Vec3::new(0.0, 0.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
        },

        JointKind::Revolute { axis, .. } => {
            let axis = vec3_normalize(axis);
            let rot_matrix = axis_angle_to_matrix(&axis, joint.state);
            let rotation = matrix_to_euler(&rot_matrix);
            let rotated_child = mat_vec3(&rot_matrix, &joint.child_anchor);
            let translation = vec3_sub(&joint.parent_anchor, &rotated_child);
            Transform3D {
                translation,
                rotation,
                scale: Vec3::new(1.0, 1.0, 1.0),
            }
        }

        JointKind::Slider { axis, .. } => {
            let axis = vec3_normalize(axis);
            let slide_offset = vec3_scale(&axis, joint.state);
            Transform3D {
                translation: vec3_add(
                    &vec3_sub(&joint.parent_anchor, &joint.child_anchor),
                    &slide_offset,
                ),
                rotation: Vec3::new(0.0, 0.0, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
            }
        }

        JointKind::Cylindrical { axis } => {
            let axis = vec3_normalize(axis);
            let rot_matrix = axis_angle_to_matrix(&axis, joint.state);
            let rotation = matrix_to_euler(&rot_matrix);
            let rotated_child = mat_vec3(&rot_matrix, &joint.child_anchor);
            let translation = vec3_sub(&joint.parent_anchor, &rotated_child);
            Transform3D {
                translation,
                rotation,
                scale: Vec3::new(1.0, 1.0, 1.0),
            }
        }

        JointKind::Ball => {
            let z_axis = Vec3::new(0.0, 0.0, 1.0);
            let rot_matrix = axis_angle_to_matrix(&z_axis, joint.state);
            let rotation = matrix_to_euler(&rot_matrix);
            let rotated_child = mat_vec3(&rot_matrix, &joint.child_anchor);
            let translation = vec3_sub(&joint.parent_anchor, &rotated_child);
            Transform3D {
                translation,
                rotation,
                scale: Vec3::new(1.0, 1.0, 1.0),
            }
        }
    }
}

/// Solve forward kinematics for an assembly document.
///
/// Starting from root instances (not children of any joint), traverses the
/// joint tree via BFS and computes world transforms for each instance.
///
/// Contract: a joint fully determines its child's pose. For a jointed child,
/// `world = parent_world · joint_transform(anchors, state)` — the instance's
/// own `transform` is ignored (it applies only to instances that are not the
/// child of any joint). Composing both would double-apply the placement
/// whenever an author sets the instance transform to the joint-anchor
/// location, which is the natural way to author an assembly.
pub fn solve_forward_kinematics(doc: &Document) -> HashMap<String, Transform3D> {
    let mut results = HashMap::new();

    let instances = match &doc.instances {
        Some(i) if !i.is_empty() => i,
        _ => return results,
    };

    let joints = doc.joints.as_deref().unwrap_or(&[]);

    // Build child → (joint, parent_id) map
    let mut joint_tree: HashMap<&str, (&Joint, Option<&str>)> = HashMap::new();
    let mut children_by_parent: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
    children_by_parent.insert(None, Vec::new());

    for joint in joints {
        let parent_id = joint.parent_instance_id.as_deref();
        joint_tree.insert(&joint.child_instance_id, (joint, parent_id));
        children_by_parent
            .entry(parent_id)
            .or_default()
            .push(&joint.child_instance_id);
    }

    // Find root instances (not a child of any joint)
    let child_ids: HashSet<&str> = joints
        .iter()
        .map(|j| j.child_instance_id.as_str())
        .collect();
    let root_instances: Vec<_> = instances
        .iter()
        .filter(|i| !child_ids.contains(i.id.as_str()))
        .collect();

    // Initialize root instances with their base transforms
    for inst in &root_instances {
        results.insert(inst.id.clone(), inst.transform.unwrap_or_else(identity));
    }

    // BFS from null (ground) and root instances
    let mut queue: VecDeque<Option<&str>> = VecDeque::new();
    queue.push_back(None);
    for inst in &root_instances {
        queue.push_back(Some(&inst.id));
    }

    let mut visited: HashSet<Option<&str>> = HashSet::new();
    visited.insert(None);

    let instance_by_id: HashMap<&str, _> = instances.iter().map(|i| (i.id.as_str(), i)).collect();

    while let Some(parent_id) = queue.pop_front() {
        let children = children_by_parent
            .get(&parent_id)
            .cloned()
            .unwrap_or_default();

        for child_id in children {
            if visited.contains(&Some(child_id)) {
                continue;
            }
            visited.insert(Some(child_id));

            let entry = match joint_tree.get(child_id) {
                Some(e) => e,
                None => continue,
            };
            if !instance_by_id.contains_key(child_id) {
                continue;
            }

            let parent_world = parent_id
                .and_then(|pid| results.get(pid))
                .cloned()
                .unwrap_or_else(identity);

            let joint_transform = compute_joint_transform(entry.0);
            let world = compose_transforms(&parent_world, &joint_transform);

            results.insert(child_id.to_string(), world);
            queue.push_back(Some(child_id));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A revolute joint whose parent anchor matches the child instance's own
    /// transform — the natural authoring pattern. The joint must fully place
    /// the child; the instance transform must NOT be applied on top.
    fn doc_with_instance_transform(state: f64) -> Document {
        serde_json::from_value(serde_json::json!({
            "version": "0.1",
            "nodes": {
                "0": {"id": 0, "op": {"type": "Cube", "size": {"x": 10.0, "y": 10.0, "z": 10.0}}}
            },
            "roots": [],
            "materials": {},
            "part_materials": {},
            "partDefs": {
                "p": {"id": "p", "name": "p", "root": 0}
            },
            "instances": [
                {"id": "base", "partDefId": "p"},
                {"id": "arm", "partDefId": "p",
                 "transform": {"translation": {"x": 10.0, "y": 0.0, "z": 90.0},
                                "rotation": {"x": 0.0, "y": 0.0, "z": 0.0},
                                "scale": {"x": 1.0, "y": 1.0, "z": 1.0}}}
            ],
            "joints": [
                {"id": "j0", "parentInstanceId": "base", "childInstanceId": "arm",
                 "parentAnchor": {"x": 10.0, "y": 0.0, "z": 90.0},
                 "childAnchor": {"x": 0.0, "y": 0.0, "z": 0.0},
                 "kind": {"type": "Revolute", "axis": {"x": 0.0, "y": 1.0, "z": 0.0}, "limits": [-360.0, 360.0]},
                 "state": state}
            ],
            "groundInstanceId": "base"
        }))
        .expect("valid doc")
    }

    #[test]
    fn jointed_child_ignores_instance_transform() {
        let doc = doc_with_instance_transform(0.0);
        let world = solve_forward_kinematics(&doc);
        let arm = world.get("arm").expect("arm solved");
        // Joint places the child at the parent anchor exactly once.
        assert!(
            (arm.translation.x - 10.0).abs() < 1e-9,
            "x = {}",
            arm.translation.x
        );
        assert!(
            (arm.translation.z - 90.0).abs() < 1e-9,
            "z = {}",
            arm.translation.z
        );
    }

    #[test]
    fn jointed_child_rotates_about_anchor() {
        let doc = doc_with_instance_transform(90.0);
        let world = solve_forward_kinematics(&doc);
        let arm = world.get("arm").expect("arm solved");
        // child_anchor is the origin, so the anchor point itself stays put.
        assert!((arm.translation.x - 10.0).abs() < 1e-9);
        assert!((arm.translation.z - 90.0).abs() < 1e-9);
        assert!(
            (arm.rotation.y - 90.0).abs() < 1e-6,
            "ry = {}",
            arm.rotation.y
        );
    }

    /// Apply a solved world transform to a part-local point.
    fn world_point(t: &Transform3D, p: Vec3) -> Vec3 {
        vec3_add(&t.translation, &mat_vec3(&euler_to_matrix(&t.rotation), &p))
    }

    /// The defining invariant: the child's anchor point lands on the parent's
    /// anchor point at *every* joint angle. A zero anchor cannot distinguish
    /// "rotate about the anchor" from "rotate about the world origin", so
    /// this pins a nonzero parent anchor and a nonzero child anchor together.
    #[test]
    fn anchor_points_stay_coincident_at_every_angle() {
        for state in [0.0, -30.0, 45.0, 137.0] {
            let mut doc = doc_with_instance_transform(state);
            let child_anchor = Vec3::new(3.0, -2.0, 7.0);
            doc.joints.as_mut().expect("joints")[0].child_anchor = child_anchor;
            let expect = doc.joints.as_ref().unwrap()[0].parent_anchor; // parent is ground-identity
            let world = solve_forward_kinematics(&doc);
            let arm = world.get("arm").expect("arm solved");

            let anchor = world_point(arm, child_anchor);
            assert!(
                (anchor.x - expect.x).abs() < 1e-9
                    && (anchor.y - expect.y).abs() < 1e-9
                    && (anchor.z - expect.z).abs() < 1e-9,
                "state {state}: anchor at {anchor:?}, want {expect:?}"
            );

            // And the pivot really is the anchor, not the world origin: a
            // material point keeps its distance to the anchor, not to (0,0,0).
            let probe = world_point(arm, Vec3::new(0.0, 0.0, 0.0));
            let d_anchor = ((probe.x - expect.x).powi(2)
                + (probe.y - expect.y).powi(2)
                + (probe.z - expect.z).powi(2))
            .sqrt();
            let want = (3.0f64.powi(2) + 2.0f64.powi(2) + 7.0f64.powi(2)).sqrt();
            assert!(
                (d_anchor - want).abs() < 1e-9,
                "state {state}: |probe - anchor| = {d_anchor}, want {want}"
            );
        }
    }

    #[test]
    fn root_instance_keeps_own_transform() {
        let mut doc = doc_with_instance_transform(0.0);
        doc.joints = Some(vec![]);
        let world = solve_forward_kinematics(&doc);
        let arm = world.get("arm").expect("arm solved");
        assert!((arm.translation.x - 10.0).abs() < 1e-9);
        assert!((arm.translation.z - 90.0).abs() < 1e-9);
    }
}
