//! Placing assembly instances in world space.
//!
//! Shared by the CLI's mesh exports and the native app's FFI exports, so an
//! exported assembly is posed exactly like the document says.

use vcad_ir::Document;

/// Assembly instances as world-space meshes, `(name, mesh)`, in instance
/// order. Each instance's part-definition mesh is placed by the pose forward
/// kinematics gives it from the joints' current states (falling back to the
/// instance's own transform), so a posed document renders posed. Roots are
/// not included — `scene.parts` already carries them.
pub fn placed_instance_meshes(
    doc: &Document,
    scene: &crate::EvaluatedScene,
) -> Vec<(String, crate::EvaluatedMesh)> {
    let (Some(instances), Some(defs)) = (&doc.instances, &scene.part_defs) else {
        return Vec::new();
    };
    let world = crate::solve_forward_kinematics(doc);
    instances
        .iter()
        .filter_map(|inst| {
            let def = defs.iter().find(|d| d.id == inst.part_def_id)?;
            if def.mesh.indices.is_empty() {
                return None;
            }
            let mut mesh = def.mesh.clone();
            if let Some(t) = world.get(&inst.id).or(inst.transform.as_ref()) {
                place_mesh(&mut mesh, t);
            }
            Some((inst.name.clone().unwrap_or_else(|| inst.id.clone()), mesh))
        })
        .collect()
}

/// Apply `T · Rz · Ry · Rx · S` (the convention `vcad_eval::kinematics`
/// emits, and vcad-render's `transform3d_to_kernel`) to a mesh in place.
fn place_mesh(mesh: &mut crate::EvaluatedMesh, t: &vcad_ir::Transform3D) {
    let r = rotation_zyx(t);
    let rot = |v: [f64; 3]| -> [f64; 3] {
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    };
    let (s, tr) = (t.scale, t.translation);
    for p in mesh.positions.as_chunks_mut::<3>().0 {
        let v = rot([p[0] as f64 * s.x, p[1] as f64 * s.y, p[2] as f64 * s.z]);
        p[0] = (v[0] + tr.x) as f32;
        p[1] = (v[1] + tr.y) as f32;
        p[2] = (v[2] + tr.z) as f32;
    }
    if let Some(normals) = mesh.normals.as_mut() {
        // Inverse-transpose of R·S is R·S⁻¹; renormalise afterwards.
        for n in normals.as_chunks_mut::<3>().0 {
            let v = rot([n[0] as f64 / s.x, n[1] as f64 / s.y, n[2] as f64 / s.z]);
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
            n[0] = (v[0] / len) as f32;
            n[1] = (v[1] / len) as f32;
            n[2] = (v[2] / len) as f32;
        }
    }
}

/// Rz · Ry · Rx for euler angles in degrees.
fn rotation_zyx(t: &vcad_ir::Transform3D) -> [[f64; 3]; 3] {
    let (sx, cx) = t.rotation.x.to_radians().sin_cos();
    let (sy, cy) = t.rotation.y.to_radians().sin_cos();
    let (sz, cz) = t.rotation.z.to_radians().sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}
