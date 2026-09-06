//! Posing: turn an assembly document into world-space meshes.
//!
//! The document's `instances` (each a named part reference plus a transform)
//! are the single source of truth. Everything downstream — mate checks,
//! interference, exploded views, probes, renders, kit layouts — reads the
//! poses from here instead of re-deriving a z-stack from prose or from STL
//! extents.

use std::collections::HashMap;

use vcad_ir::{Document, Transform3D, Vec3};
use vcad_kernel_tessellate::TriangleMesh;

/// A 3×4 affine transform: a 3×3 linear part (row-major) plus a translation.
///
/// Built from a [`Transform3D`] with the same `Rz · Ry · Rx` Euler convention
/// the kinematics solver uses, so a pose computed here and a pose computed by
/// forward kinematics agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    /// Row-major 3×3 linear part (rotation composed with scale).
    pub linear: [[f64; 3]; 3],
    /// Translation, applied after the linear part.
    pub translation: [f64; 3],
}

impl Default for Affine {
    fn default() -> Self {
        Self::identity()
    }
}

impl Affine {
    /// The identity transform.
    pub fn identity() -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0; 3],
        }
    }

    /// Build from an IR [`Transform3D`] (Euler XYZ in degrees, `Rz · Ry · Rx`,
    /// then per-axis scale, then translation).
    pub fn from_transform3d(t: &Transform3D) -> Self {
        let (rx, ry, rz) = (
            t.rotation.x.to_radians(),
            t.rotation.y.to_radians(),
            t.rotation.z.to_radians(),
        );
        let (cx, sx) = (rx.cos(), rx.sin());
        let (cy, sy) = (ry.cos(), ry.sin());
        let (cz, sz) = (rz.cos(), rz.sin());
        // Rz * Ry * Rx — matches vcad_eval::kinematics::euler_to_matrix.
        let r = [
            [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
            [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
            [-sy, sx * cy, cx * cy],
        ];
        let s = [t.scale.x, t.scale.y, t.scale.z];
        let mut linear = [[0.0; 3]; 3];
        for (i, row) in linear.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = r[i][j] * s[j];
            }
        }
        Self {
            linear,
            translation: [t.translation.x, t.translation.y, t.translation.z],
        }
    }

    /// Transform a point.
    pub fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let mut out = self.translation;
        for (i, o) in out.iter_mut().enumerate() {
            for (j, pj) in p.iter().enumerate() {
                *o += self.linear[i][j] * pj;
            }
        }
        out
    }

    /// Transform a direction (ignores translation).
    pub fn direction(&self, v: [f64; 3]) -> [f64; 3] {
        let mut out = [0.0; 3];
        for (i, o) in out.iter_mut().enumerate() {
            for (j, vj) in v.iter().enumerate() {
                *o += self.linear[i][j] * vj;
            }
        }
        out
    }

    /// Determinant of the linear part. Negative means the pose mirrors —
    /// a flip about one axis, not a rotation.
    pub fn determinant(&self) -> f64 {
        let m = &self.linear;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Translate by `d` on top of this transform.
    pub fn translated(&self, d: [f64; 3]) -> Self {
        let mut out = *self;
        for (t, di) in out.translation.iter_mut().zip(d) {
            *t += di;
        }
        out
    }
}

/// One part reference, posed into world space.
#[derive(Debug, Clone)]
pub struct PosedPart {
    /// Instance id from the document.
    pub instance_id: String,
    /// Part definition the instance refers to.
    pub part_def_id: String,
    /// The world transform actually applied to the mesh.
    pub transform: Affine,
    /// World-space triangle mesh.
    pub mesh: TriangleMesh,
    /// Exploded-view offset declared on the instance, unscaled (mm).
    pub explode: [f64; 3],
}

impl PosedPart {
    /// Axis-aligned bounds of the posed mesh as `(min, max)`, or `None` when
    /// the mesh is empty.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        mesh_bounds(&self.mesh)
    }
}

/// An assembly evaluated to world-space meshes.
#[derive(Debug, Clone, Default)]
pub struct PosedAssembly {
    /// Posed parts, in document instance order.
    pub parts: Vec<PosedPart>,
}

impl PosedAssembly {
    /// Look up a posed part by instance id.
    pub fn get(&self, instance_id: &str) -> Option<&PosedPart> {
        self.parts.iter().find(|p| p.instance_id == instance_id)
    }

    /// The same assembly with every part pushed along its declared exploded
    /// offset, scaled by `factor` (`0.0` = assembled, `1.0` = fully exploded).
    ///
    /// The offsets are data on the document, so an exploded render, a build
    /// sheet, and a viewer all read the same numbers instead of each hard-
    /// coding their own.
    pub fn exploded(&self, factor: f64) -> PosedAssembly {
        PosedAssembly {
            parts: self
                .parts
                .iter()
                .map(|p| {
                    let d = [
                        p.explode[0] * factor,
                        p.explode[1] * factor,
                        p.explode[2] * factor,
                    ];
                    let mut mesh = p.mesh.clone();
                    translate_mesh(&mut mesh, d);
                    PosedPart {
                        instance_id: p.instance_id.clone(),
                        part_def_id: p.part_def_id.clone(),
                        transform: p.transform.translated(d),
                        mesh,
                        explode: p.explode,
                    }
                })
                .collect(),
        }
    }
}

/// Why an assembly could not be posed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoseError {
    /// The document carries no `instances` — it is not an assembly.
    NotAnAssembly,
    /// An instance refers to a part definition the document does not define.
    UnknownPartDef {
        /// The offending instance.
        instance_id: String,
        /// The part definition it named.
        part_def_id: String,
    },
    /// Geometry evaluation failed.
    Eval(String),
}

impl std::fmt::Display for PoseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoseError::NotAnAssembly => {
                write!(f, "document has no instances — not an assembly")
            }
            PoseError::UnknownPartDef {
                instance_id,
                part_def_id,
            } => write!(
                f,
                "instance {instance_id:?} refers to undefined part definition {part_def_id:?}"
            ),
            PoseError::Eval(e) => write!(f, "assembly geometry evaluation failed: {e}"),
        }
    }
}

impl std::error::Error for PoseError {}

/// Evaluate an assembly document to world-space meshes.
///
/// Instance transforms come from forward kinematics where the document has a
/// joint graph, and from `Instance::transform` otherwise — the same rule the
/// viewport uses, so a posed assembly here and a posed assembly on screen
/// cannot disagree.
pub fn pose_document(doc: &Document) -> Result<PosedAssembly, PoseError> {
    let instances = doc.instances.as_ref().ok_or(PoseError::NotAnAssembly)?;
    if let Some(defs) = doc.part_defs.as_ref() {
        for inst in instances {
            if !defs.contains_key(&inst.part_def_id) {
                return Err(PoseError::UnknownPartDef {
                    instance_id: inst.id.clone(),
                    part_def_id: inst.part_def_id.clone(),
                });
            }
        }
    }

    let scene = vcad_eval::evaluate_document(doc, &vcad_eval::EvalOptions::default())
        .map_err(|e| PoseError::Eval(format!("{e:?}")))?;
    if let Some(failure) = scene.failures.first() {
        return Err(PoseError::Eval(format!(
            "{}: {}",
            failure.scope, failure.error
        )));
    }
    let eval_instances = scene.instances.ok_or(PoseError::NotAnAssembly)?;

    let explodes: HashMap<&str, Vec3> = instances
        .iter()
        .filter_map(|i| i.explode.map(|e| (i.id.as_str(), e)))
        .collect();

    let mut parts = Vec::with_capacity(eval_instances.len());
    for inst in eval_instances {
        let transform = inst
            .transform
            .as_ref()
            .map(Affine::from_transform3d)
            .unwrap_or_default();
        let mesh = posed_mesh(&inst.mesh, &transform);
        let explode = explodes
            .get(inst.instance_id.as_str())
            .map(|e| [e.x, e.y, e.z])
            .unwrap_or([0.0; 3]);
        parts.push(PosedPart {
            instance_id: inst.instance_id,
            part_def_id: inst.part_def_id,
            transform,
            mesh,
            explode,
        });
    }
    Ok(PosedAssembly { parts })
}

/// Apply `transform` to an evaluated mesh, producing a world-space
/// [`TriangleMesh`]. A mirroring transform flips triangle winding so the
/// result keeps outward-facing normals.
fn posed_mesh(mesh: &vcad_eval::EvaluatedMesh, transform: &Affine) -> TriangleMesh {
    let mut out = TriangleMesh::new();
    out.vertices.reserve(mesh.positions.len());
    for p in mesh.positions.as_chunks::<3>().0 {
        let w = transform.point([p[0] as f64, p[1] as f64, p[2] as f64]);
        out.vertices.extend([w[0] as f32, w[1] as f32, w[2] as f32]);
    }
    let flip = transform.determinant() < 0.0;
    for tri in mesh.indices.as_chunks::<3>().0 {
        if flip {
            out.indices.extend([tri[0], tri[2], tri[1]]);
        } else {
            out.indices.extend([tri[0], tri[1], tri[2]]);
        }
    }
    out
}

fn translate_mesh(mesh: &mut TriangleMesh, d: [f64; 3]) {
    for v in mesh.vertices.as_chunks_mut::<3>().0 {
        v[0] += d[0] as f32;
        v[1] += d[1] as f32;
        v[2] += d[2] as f32;
    }
}

/// Axis-aligned bounds of a triangle mesh as `(min, max)`.
pub fn mesh_bounds(mesh: &TriangleMesh) -> Option<([f64; 3], [f64; 3])> {
    if mesh.vertices.len() < 3 {
        return None;
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for v in mesh.vertices.as_chunks::<3>().0 {
        for k in 0..3 {
            min[k] = min[k].min(v[k] as f64);
            max[k] = max[k].max(v[k] as f64);
        }
    }
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_about_x_negates_y_and_z() {
        let a = Affine::from_transform3d(&Transform3D {
            rotation: Vec3::new(180.0, 0.0, 0.0),
            ..Transform3D::default()
        });
        let p = a.point([1.0, 2.0, 3.0]);
        assert!((p[0] - 1.0).abs() < 1e-9);
        assert!((p[1] + 2.0).abs() < 1e-9);
        assert!((p[2] + 3.0).abs() < 1e-9);
        // A 180° rotation is still a rotation, not a mirror.
        assert!(a.determinant() > 0.0);
    }

    #[test]
    fn negative_scale_is_a_mirror() {
        let a = Affine::from_transform3d(&Transform3D {
            scale: Vec3::new(1.0, 1.0, -1.0),
            ..Transform3D::default()
        });
        assert!(a.determinant() < 0.0);
    }

    #[test]
    fn rotation_composes_z_after_x() {
        // rot 180 about X, then 90 about Z — the rana rotor convention.
        let a = Affine::from_transform3d(&Transform3D {
            rotation: Vec3::new(180.0, 0.0, 90.0),
            ..Transform3D::default()
        });
        // Local +X → flip leaves it at +X → clock 90 sends it to +Y.
        let x = a.direction([1.0, 0.0, 0.0]);
        assert!(x[0].abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9);
        // Local +Z → flip sends it to −Z, clocking about Z leaves it there.
        let z = a.direction([0.0, 0.0, 1.0]);
        assert!((z[2] + 1.0).abs() < 1e-9);
    }
}
