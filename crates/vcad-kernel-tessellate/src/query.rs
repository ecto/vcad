//! Solid queries on evaluated meshes: point classification and clearance.
//!
//! This is the library half of "probe suites as a vcad feature" (issue #843).
//! A probe suite asserts material or void at explicit coordinates of an
//! *assembled* machine — the rana 60c mule carries 142 of them — and every
//! such assertion reduces to two queries:
//!
//! - [`SolidQuery::is_inside`] — is this point inside the solid?
//! - [`Assembly::clearance`] — how far apart are two posed parts?
//!
//! # Classification semantics
//!
//! Point classification is a **winding-count** ray cast: crossings ahead of
//! the point count `+1` when the ray enters through a triangle's outward face
//! and `-1` when it leaves, and the point is inside when the sum is non-zero.
//! Parity (odd crossing count) is the obvious alternative and is wrong for
//! the meshes probe suites actually run against: an exported part is often a
//! union of overlapping construction bodies, and inside the overlap parity
//! counts two crossings and reports void. Winding counts 2 and still says
//! material. Coincident duplicate faces cancel instead of flipping the answer
//! for the same reason.
//!
//! The sign convention is load-bearing. An inverted sum reports *nothing* as
//! inside, at which point a suite made mostly of void assertions passes
//! vacuously — the bug rana's first shell check shipped with
//! (`tools/support-check.py`, finding #11). [`SolidQuery::is_inside`] is
//! pinned against it by tests that assert material where material must be.
//!
//! Strict parity is still available as [`SolidQuery::is_inside_strict_parity`]
//! for the job it is right for: hunting cracks in a part that must be
//! parity-clean, because that is what a slicer's mesh analysis sees.
//!
//! Either cast is hardened for real meshes: directions that graze a triangle
//! edge are discarded, and the verdict is a majority vote over six skew ray
//! directions, so no single unlucky ray through a welded seam can flip the
//! answer. Rana's probes dodged this by hand-picking off-axis coordinates
//! (`x != y`, angles off the seam grid); here it is the API's job.

use crate::clearance::{point_in_mesh, point_in_mesh_winding, TriBvh};
use crate::{mesh_clearance, ClearanceResult, TriangleMesh};

/// A rigid placement: a 3x3 linear part plus a translation.
///
/// Built from the same primitives rana's probe scripts used — a half-turn
/// about X, rotation about Z, translation — composed left-to-right in
/// application order, so `Pose::flip_x().then(&Pose::rotate_z_deg(180.0))`
/// flips first and then rotates, matching `xform(flipx=True, rotz=180)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Row-major 3x3 linear part.
    pub linear: [[f64; 3]; 3],
    /// Translation applied after the linear part.
    pub translation: [f64; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Self::identity()
    }
}

impl Pose {
    /// The identity placement.
    pub fn identity() -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0; 3],
        }
    }

    /// Translation only.
    pub fn translate(xyz: [f64; 3]) -> Self {
        Self {
            translation: xyz,
            ..Self::identity()
        }
    }

    /// Rotation about the Z axis, in degrees.
    pub fn rotate_z_deg(deg: f64) -> Self {
        let (s, c) = deg.to_radians().sin_cos();
        Self {
            linear: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0; 3],
        }
    }

    /// Half-turn about the X axis: `(x, y, z) -> (x, -y, -z)`.
    ///
    /// This is rana's `flipx` — the flip that puts a second rotor face-to-face
    /// with the first. It is a rotation (determinant +1), so it preserves
    /// triangle winding and leaves the winding-count classification valid.
    pub fn flip_x() -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
            translation: [0.0; 3],
        }
    }

    /// `self` followed by `next`.
    pub fn then(&self, next: &Pose) -> Pose {
        let mut linear = [[0.0; 3]; 3];
        for (i, row) in linear.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| next.linear[i][k] * self.linear[k][j]).sum();
            }
        }
        let mut translation = next.translation;
        for (i, t) in translation.iter_mut().enumerate() {
            *t += (0..3)
                .map(|k| next.linear[i][k] * self.translation[k])
                .sum::<f64>();
        }
        Pose {
            linear,
            translation,
        }
    }

    /// Apply the placement to a point.
    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let mut out = self.translation;
        for (i, o) in out.iter_mut().enumerate() {
            *o += (0..3).map(|k| self.linear[i][k] * p[k]).sum::<f64>();
        }
        out
    }

    /// The mesh transformed by this placement.
    pub fn apply_to_mesh(&self, mesh: &TriangleMesh) -> TriangleMesh {
        let mut out = mesh.clone();
        for v in out.vertices.as_chunks_mut::<3>().0 {
            let p = self.apply([v[0] as f64, v[1] as f64, v[2] as f64]);
            v[0] = p[0] as f32;
            v[1] = p[1] as f32;
            v[2] = p[2] as f32;
        }
        // Normals rotate with the linear part (rigid: no rescale needed) and
        // are not used by any query here, but a mesh handed back to a caller
        // should still be self-consistent.
        for n in out.normals.as_chunks_mut::<3>().0 {
            let d = [n[0] as f64, n[1] as f64, n[2] as f64];
            let mut r = [0.0f64; 3];
            for (i, ri) in r.iter_mut().enumerate() {
                *ri = (0..3).map(|k| self.linear[i][k] * d[k]).sum();
            }
            n[0] = r[0] as f32;
            n[1] = r[1] as f32;
            n[2] = r[2] as f32;
        }
        out
    }
}

/// A mesh prepared for repeated point queries (BVH built once).
pub struct SolidQuery {
    mesh: TriangleMesh,
    bvh: TriBvh,
}

impl SolidQuery {
    /// Build the acceleration structure. `None` when the mesh has no triangles.
    pub fn new(mesh: TriangleMesh) -> Option<Self> {
        let bvh = TriBvh::build(&mesh)?;
        Some(Self { mesh, bvh })
    }

    /// Is `point` inside the solid? See the module docs for the semantics.
    pub fn is_inside(&self, point: [f64; 3]) -> bool {
        point_in_mesh_winding(point, &self.bvh)
    }

    /// Strict-parity classification: odd crossing count means inside.
    ///
    /// This is the *crack-hunting* verdict, and it disagrees with
    /// [`SolidQuery::is_inside`] exactly where a mesh welds overlapping
    /// bodies together: parity reads the overlap slab as a void, which is
    /// what a slicer's mesh analysis will see too. Use it to assert a part is
    /// parity-clean (rana's `support-check.py` does), not to probe material.
    pub fn is_inside_strict_parity(&self, point: [f64; 3]) -> bool {
        point_in_mesh(point, &self.bvh)
    }

    /// The mesh this query was built from (already posed, in an assembly).
    pub fn mesh(&self) -> &TriangleMesh {
        &self.mesh
    }
}

/// One-shot point classification. Prefer [`SolidQuery`] for repeated probes —
/// this rebuilds the BVH on every call.
pub fn is_inside(mesh: &TriangleMesh, point: [f64; 3]) -> bool {
    match TriBvh::build(mesh) {
        Some(bvh) => point_in_mesh_winding(point, &bvh),
        None => false,
    }
}

/// A named, posed part of an assembly.
pub struct AssemblyPart {
    /// Instance name used by probes (`"rotor-front"`, not the source part).
    pub name: String,
    /// Placement applied to the source mesh when the part was inserted.
    pub pose: Pose,
    /// The posed geometry.
    pub solid: SolidQuery,
}

/// A set of posed parts queried as one machine.
///
/// Probes name the instances they care about and take **any-inside** union
/// semantics, mirroring rana's `probe(..., parts=[...])`: a point is material
/// if it lies inside *any* listed part. Overlapping construction bodies are
/// therefore harmless as long as they are separate instances.
#[derive(Default)]
pub struct Assembly {
    parts: Vec<AssemblyPart>,
}

/// Something a probe or clearance query asked for that the assembly lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// A probe named a part that is not in the assembly.
    UnknownPart(String),
    /// A mesh had no triangles, so no query structure could be built.
    EmptyMesh(String),
    /// Clearance was requested between meshes it could not measure.
    NoClearance(String, String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::UnknownPart(n) => write!(f, "unknown part `{n}`"),
            QueryError::EmptyMesh(n) => write!(f, "part `{n}` has no triangles"),
            QueryError::NoClearance(a, b) => {
                write!(f, "no clearance measurable between `{a}` and `{b}`")
            }
        }
    }
}

impl std::error::Error for QueryError {}

impl Assembly {
    /// An empty assembly.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a part under `name`, posed by `pose`.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        mesh: &TriangleMesh,
        pose: Pose,
    ) -> Result<(), QueryError> {
        let name = name.into();
        let posed = pose.apply_to_mesh(mesh);
        let solid = SolidQuery::new(posed).ok_or_else(|| QueryError::EmptyMesh(name.clone()))?;
        self.parts.push(AssemblyPart { name, pose, solid });
        Ok(())
    }

    /// Instance names, in insertion order.
    pub fn part_names(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().map(|p| p.name.as_str())
    }

    /// Look up one posed part.
    pub fn part(&self, name: &str) -> Option<&AssemblyPart> {
        self.parts.iter().find(|p| p.name == name)
    }

    /// Is `point` inside the named part?
    pub fn is_inside(&self, name: &str, point: [f64; 3]) -> Result<bool, QueryError> {
        self.part(name)
            .map(|p| p.solid.is_inside(point))
            .ok_or_else(|| QueryError::UnknownPart(name.to_string()))
    }

    /// Is `point` inside *any* of the named parts (rana's A-table semantics)?
    pub fn any_inside(
        &self,
        names: &[impl AsRef<str>],
        point: [f64; 3],
    ) -> Result<bool, QueryError> {
        let mut hit = false;
        for n in names {
            // Every name is resolved even after a hit, so a typo in a probe
            // is an error rather than a silently skipped part.
            hit |= self.is_inside(n.as_ref(), point)?;
        }
        Ok(hit)
    }

    /// Minimum surface-to-surface distance between two posed parts.
    ///
    /// Negative when they interpenetrate (see [`ClearanceResult`]).
    pub fn clearance(&self, a: &str, b: &str) -> Result<ClearanceResult, QueryError> {
        let pa = self
            .part(a)
            .ok_or_else(|| QueryError::UnknownPart(a.to_string()))?;
        let pb = self
            .part(b)
            .ok_or_else(|| QueryError::UnknownPart(b.to_string()))?;
        mesh_clearance(pa.solid.mesh(), pb.solid.mesh())
            .ok_or_else(|| QueryError::NoClearance(a.to_string(), b.to_string()))
    }
}

/// Minimum surface-to-surface distance between two meshes.
///
/// Thin alias over [`crate::mesh_clearance`] so the query API reads as one
/// surface; BVH-accelerated (branch-and-bound closest pair), not brute force.
pub fn clearance(a: &TriangleMesh, b: &TriangleMesh) -> Option<ClearanceResult> {
    mesh_clearance(a, b)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Axis-aligned box mesh, corner-to-corner, outward winding.
    pub(crate) fn box_mesh(min: [f64; 3], max: [f64; 3]) -> TriangleMesh {
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] } as f32,
                if i & 2 == 0 { min[1] } else { max[1] } as f32,
                if i & 4 == 0 { min[2] } else { max[2] } as f32,
            ]
        };
        let mut vertices = Vec::new();
        for i in 0..8 {
            vertices.extend_from_slice(&corner(i));
        }
        // Each face as two CCW-outward triangles.
        let indices: Vec<u32> = vec![
            0, 2, 1, 1, 2, 3, // -z
            4, 5, 6, 5, 7, 6, // +z
            0, 1, 4, 1, 5, 4, // -y
            2, 6, 3, 3, 6, 7, // +y
            0, 4, 2, 2, 4, 6, // -x
            1, 3, 5, 3, 7, 5, // +x
        ];
        TriangleMesh {
            normals: vec![0.0; vertices.len()],
            vertices,
            indices,
            face_kinds: Vec::new(),
            face_ids: Vec::new(),
        }
    }

    #[test]
    fn classifies_points_against_a_box() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        assert!(is_inside(&m, [5.0, 5.1, 4.9]));
        assert!(!is_inside(&m, [5.0, 5.1, 10.7]));
        assert!(!is_inside(&m, [-0.3, 5.1, 4.9]));
    }

    #[test]
    fn winding_survives_a_welded_overlap_where_parity_does_not() {
        // One mesh holding two boxes that overlap in z 8..10 — an export of
        // unbooleaned construction bodies. Inside the overlap a ray crosses
        // two surfaces, so parity says void; winding counts 2 and says solid.
        let mut welded = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let upper = box_mesh([0.0, 0.0, 8.0], [10.0, 10.0, 18.0]);
        let base = (welded.vertices.len() / 3) as u32;
        welded.vertices.extend_from_slice(&upper.vertices);
        welded.normals.extend_from_slice(&upper.normals);
        welded
            .indices
            .extend(upper.indices.iter().map(|i| i + base));

        let q = SolidQuery::new(welded).unwrap();
        let in_overlap = [5.1, 4.9, 9.0];
        assert!(q.is_inside(in_overlap), "winding must see material");
        assert!(
            !q.is_inside_strict_parity(in_overlap),
            "parity reads the overlap as the void a slicer would see"
        );
        // Material outside the overlap is material under both casts, and
        // outside is outside under both. This is the guard against an
        // inverted winding sign, which would report nothing as inside and
        // let a suite of void assertions pass vacuously.
        assert!(q.is_inside([5.1, 4.9, 3.0]));
        assert!(q.is_inside_strict_parity([5.1, 4.9, 3.0]));
        assert!(!q.is_inside([5.1, 4.9, 19.0]));
        assert!(!q.is_inside_strict_parity([5.1, 4.9, 19.0]));
    }

    #[test]
    fn poses_compose_in_application_order() {
        // flip about X, then rotate 180 about Z, then lift — rana's rotor-front.
        let pose = Pose::flip_x()
            .then(&Pose::rotate_z_deg(180.0))
            .then(&Pose::translate([0.0, 0.0, 23.5]));
        let p = pose.apply([1.0, 2.0, 3.0]);
        // flip: (1, -2, -3); rot180: (-1, 2, -3); lift: (-1, 2, 20.5)
        for (got, want) in p.iter().zip([-1.0, 2.0, 20.5]) {
            assert!((got - want).abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn assembly_unions_named_parts() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let mut asm = Assembly::new();
        asm.insert("a", &m, Pose::identity()).unwrap();
        asm.insert("b", &m, Pose::translate([20.0, 0.0, 0.0]))
            .unwrap();
        let p = [25.0, 5.1, 4.9];
        assert!(!asm.is_inside("a", p).unwrap());
        assert!(asm.any_inside(&["a", "b"], p).unwrap());
        assert!(!asm.any_inside(&["a"], p).unwrap());
        assert_eq!(
            asm.any_inside(&["nope"], p),
            Err(QueryError::UnknownPart("nope".into()))
        );
    }

    #[test]
    fn overlapping_construction_bodies_stay_material() {
        // Two boxes sharing a 2mm slab — the shape an unbooleaned export
        // takes. Parity reads the overlap as a void; winding keeps it solid.
        let a = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = box_mesh([0.0, 0.0, 8.0], [10.0, 10.0, 18.0]);
        let mut asm = Assembly::new();
        asm.insert("lower", &a, Pose::identity()).unwrap();
        asm.insert("upper", &b, Pose::identity()).unwrap();
        for z in [1.0, 8.7, 9.3, 15.0] {
            assert!(
                asm.any_inside(&["lower", "upper"], [5.1, 4.9, z]).unwrap(),
                "z={z}"
            );
        }
    }

    #[test]
    fn assembly_clearance_measures_the_gap() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let mut asm = Assembly::new();
        asm.insert("a", &m, Pose::identity()).unwrap();
        asm.insert("b", &m, Pose::translate([13.5, 0.0, 0.0]))
            .unwrap();
        let c = asm.clearance("a", "b").unwrap();
        assert!(!c.intersecting);
        assert!((c.distance - 3.5).abs() < 1e-6, "{c:?}");
    }
}
