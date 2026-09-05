//! The BRep side of the seam: vcad's geometry, told to `kosm-render`.
//!
//! The hierarchy itself lives in [`kosm_render`] — it is the same SAH tree
//! whatever it is built over. What is vcad's, and stays here, is the answer
//! to "what does a ray find when it meets face 7": [`crate::intersect`] for
//! the analytic surface, [`crate::trim`] for whether the hit is inside the
//! face's boundary at all.
//!
//! [`Bvh`] is a thin wrapper rather than a bare alias for two reasons the
//! rest of the kernel depends on: it still knows whether it was built over a
//! solid or a mesh, and it still flattens to `FaceId`s for the GPU upload.

use std::sync::Arc;
use vcad_kernel_booleans::bbox::{face_aabb, Aabb3};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_topo::FaceId;

use kosm_render::{Aabb, Geometry, TriMesh};

use crate::intersect::{intersect_surface, surface_tangent};
use crate::trim::{face_normal, point_in_face};
use crate::{Ray, RayHit};

/// A flattened BVH node tuple for GPU upload.
/// Contains: (AABB, is_leaf, left_or_first, right_or_count)
pub type FlatBvhNode = (Aabb3, bool, u32, u32);

/// A node of the hierarchy.
pub use kosm_render::BvhNode;

/// The geometry a [`Bvh`] was built over, as `kosm-render` sees it.
///
/// Two kinds, because a solid may arrive either way: trimmed analytic faces
/// when there is a BRep to trace, triangles when there is not (frozen
/// `topology_optimize` results, imported STL/GLB parts).
#[derive(Debug, Clone)]
pub enum BrepGeom {
    /// Trimmed analytic faces of a BRep solid.
    BRep {
        /// The solid being traced.
        brep: Arc<BRepSolid>,
        /// Primitive index → face ID.
        faces: Vec<FaceId>,
    },
    /// Triangles of a mesh-only solid.
    Mesh(TriMesh),
}

impl BrepGeom {
    /// The face a primitive index refers to, for a BRep-backed geometry.
    ///
    /// This is where `RayHit::face_id` went: a hit carries the primitive it
    /// landed on, and the geometry says what vcad calls that primitive.
    pub fn face_id(&self, prim: u32) -> Option<FaceId> {
        match self {
            BrepGeom::BRep { faces, .. } => faces.get(prim as usize).copied(),
            BrepGeom::Mesh(_) => None,
        }
    }

    /// The face IDs, in primitive order. Empty for a mesh.
    pub fn face_ids(&self) -> &[FaceId] {
        match self {
            BrepGeom::BRep { faces, .. } => faces,
            BrepGeom::Mesh(_) => &[],
        }
    }
}

/// vcad's `Aabb3` and kosm-render's `Aabb` are the same two `Point3`s under
/// different names; these are the two-line translations between them.
#[inline]
fn to_render_aabb(a: &Aabb3) -> Aabb {
    Aabb::new(a.min, a.max)
}

#[inline]
fn from_render_aabb(a: &Aabb) -> Aabb3 {
    Aabb3::new(a.min, a.max)
}

impl Geometry for BrepGeom {
    fn len(&self) -> usize {
        match self {
            BrepGeom::BRep { faces, .. } => faces.len(),
            BrepGeom::Mesh(m) => m.len(),
        }
    }

    fn bounds(&self, i: usize) -> Aabb {
        match self {
            BrepGeom::BRep { brep, faces } => to_render_aabb(&face_aabb(brep, faces[i])),
            BrepGeom::Mesh(m) => m.bounds(i),
        }
    }

    fn intersect(&self, ray: &Ray, i: usize, t_min: f64, t_max: f64) -> Option<RayHit> {
        match self {
            BrepGeom::BRep { brep, faces } => {
                let face_id = faces[i];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                let mut closest: Option<RayHit> = None;

                for hit in intersect_surface(ray, surface.as_ref()) {
                    if hit.t > t_min
                        && hit.t < t_max
                        && point_in_face(brep, face_id, hit.uv)
                        && (closest.is_none() || hit.t < closest.as_ref().unwrap().t)
                    {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        let tangent = surface_tangent(surface.as_ref(), hit.uv);
                        closest = Some(
                            RayHit::new(hit.t, point, normal, hit.uv, i as u32)
                                .with_tangent(tangent),
                        );
                    }
                }

                closest
            }
            BrepGeom::Mesh(m) => m.intersect(ray, i, t_min, t_max),
        }
    }

    fn intersect_all(&self, ray: &Ray, i: usize, out: &mut Vec<RayHit>) {
        match self {
            BrepGeom::BRep { brep, faces } => {
                let face_id = faces[i];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                for hit in intersect_surface(ray, surface.as_ref()) {
                    // Only hits inside the face's trim boundary are on the
                    // solid at all; the rest are on the unbounded surface.
                    if point_in_face(brep, face_id, hit.uv) {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        let tangent = surface_tangent(surface.as_ref(), hit.uv);
                        out.push(
                            RayHit::new(hit.t, point, normal, hit.uv, i as u32)
                                .with_tangent(tangent),
                        );
                    }
                }
            }
            BrepGeom::Mesh(m) => m.intersect_all(ray, i, out),
        }
    }

    fn occludes(&self, ray: &Ray, i: usize, t_min: f64, t_max: f64) -> bool {
        match self {
            BrepGeom::BRep { brep, faces } => {
                let face_id = faces[i];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];
                intersect_surface(ray, surface.as_ref())
                    .into_iter()
                    .any(|hit| {
                        hit.t > t_min && hit.t < t_max && point_in_face(brep, face_id, hit.uv)
                    })
            }
            BrepGeom::Mesh(m) => m.occludes(ray, i, t_min, t_max),
        }
    }
}

/// Bounding volume hierarchy over a vcad solid.
///
/// The hierarchy is [`kosm_render::Bvh`]; what makes it vcad's is the
/// [`BrepGeom`] it is built over. The B-rep-shaped constructors and queries
/// live on the [`BrepBvh`] extension trait, so `Bvh::build_brep(&solid)` still
/// reads the way it always did.
pub type Bvh = kosm_render::Bvh<BrepGeom>;

/// The B-rep-shaped half of a [`Bvh`]: how to build one from a solid or a
/// mesh, and how to ask it the questions only vcad can answer.
pub trait BrepBvh: Sized {
    /// Build a BVH from a BRep solid using SAH construction.
    ///
    /// Not `build`: `kosm_render::Bvh` has an inherent one that takes the
    /// geometry ready-made, and an inherent method wins over a trait's.
    fn build_brep(brep: &BRepSolid) -> Self;

    /// Build a BVH over an already-shared BRep solid.
    ///
    /// Skips the clone `build` performs, so N instances of the same part can
    /// share one BLAS (see [`crate::tlas`]).
    fn build_brep_shared(brep: Arc<BRepSolid>) -> Self;

    /// Build a BVH over the triangles of a mesh, using the same SAH
    /// construction as [`build_brep`](Self::build_brep).
    ///
    /// Mesh-only solids — frozen `topology_optimize` results, imported
    /// STL/GLB parts — carry no analytic surfaces, so they are traced as
    /// triangles. Degenerate triangles (repeated or out-of-range indices)
    /// are dropped at build time rather than wasting intersection tests.
    ///
    /// When the mesh carries vertex normals they are interpolated for
    /// smooth shading; otherwise hits report the geometric face normal.
    fn build_mesh(mesh: &TriangleMesh) -> Self;

    /// The underlying BRep solid, if this BVH was built over one. `None` for
    /// mesh-backed BVHs.
    fn brep(&self) -> Option<&BRepSolid>;

    /// Whether this BVH traces triangles rather than analytic BRep faces.
    fn is_mesh(&self) -> bool;

    /// The face a hit landed on. `None` for mesh hits, which have no BRep
    /// face — this is where `RayHit::face_id` went.
    fn face_id(&self, hit: &RayHit) -> Option<FaceId>;

    /// Solid-space bounds of the whole hierarchy, in vcad's `Aabb3`.
    fn aabb(&self) -> Option<Aabb3>;

    /// Flatten the BVH into a vector of nodes for GPU upload.
    ///
    /// Returns a list of (AABB, is_leaf, left_or_first, right_or_count) tuples:
    /// - For internal nodes: left_or_first = left child index, right_or_count = right child index
    /// - For leaf nodes: left_or_first = start face index in faces array, right_or_count = face count
    ///
    /// Also returns the list of face IDs in leaf order.
    ///
    /// BRep-backed BVHs only — the GPU pipeline traces analytic surfaces.
    /// A mesh-backed BVH flattens to nothing.
    fn flatten_faces(&self) -> (Vec<FlatBvhNode>, Vec<FaceId>);
}

impl BrepBvh for Bvh {
    fn build_brep(brep: &BRepSolid) -> Self {
        Self::build_brep_shared(Arc::new(brep.clone()))
    }

    fn build_brep_shared(brep: Arc<BRepSolid>) -> Self {
        let faces: Vec<FaceId> = brep.topology.faces.iter().map(|(id, _)| id).collect();
        kosm_render::Bvh::build(BrepGeom::BRep { brep, faces })
    }

    fn build_mesh(mesh: &TriangleMesh) -> Self {
        let vertex_count = mesh.vertices.len() / 3;
        // Widened to `f64` once here: the tracer works in `f64` throughout,
        // and casting per intersection test would put it in the inner loop.
        let positions: Vec<Point3> = (0..vertex_count)
            .map(|i| {
                Point3::new(
                    mesh.vertices[i * 3] as f64,
                    mesh.vertices[i * 3 + 1] as f64,
                    mesh.vertices[i * 3 + 2] as f64,
                )
            })
            .collect();
        // Only trust the normal array when it matches the positions
        // one-for-one; a partial array can't be indexed by vertex.
        let normals: Vec<Vec3> = if mesh.normals.len() == mesh.vertices.len() {
            (0..vertex_count)
                .map(|i| {
                    Vec3::new(
                        mesh.normals[i * 3] as f64,
                        mesh.normals[i * 3 + 1] as f64,
                        mesh.normals[i * 3 + 2] as f64,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        kosm_render::Bvh::build(BrepGeom::Mesh(TriMesh::new(
            positions,
            normals,
            &mesh.indices,
        )))
    }

    fn brep(&self) -> Option<&BRepSolid> {
        match self.geometry() {
            BrepGeom::BRep { brep, .. } => Some(brep),
            BrepGeom::Mesh(_) => None,
        }
    }

    fn is_mesh(&self) -> bool {
        matches!(self.geometry(), BrepGeom::Mesh(_))
    }

    fn face_id(&self, hit: &RayHit) -> Option<FaceId> {
        self.geometry().face_id(hit.prim)
    }

    fn aabb(&self) -> Option<Aabb3> {
        self.bounds().map(|a| from_render_aabb(&a))
    }

    fn flatten_faces(&self) -> (Vec<FlatBvhNode>, Vec<FaceId>) {
        let BrepGeom::BRep {
            faces: face_ids, ..
        } = self.geometry()
        else {
            return (Vec::new(), Vec::new());
        };

        let (nodes, prims) = self.flatten();
        (
            nodes
                .into_iter()
                .map(|(aabb, leaf, a, b)| (from_render_aabb(&aabb), leaf, a, b))
                .collect(),
            prims.into_iter().map(|p| face_ids[p as usize]).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};
    use vcad_kernel_primitives::make_cube;

    /// A hit on a cylinder wall must report the *circumferential* tangent —
    /// the direction a lathe tool travels. Anisotropic shading orients its
    /// specular lobe with this, so if it ever came back axial or radial,
    /// turned parts would get their grain rotated 90°.
    #[test]
    fn cylinder_hit_reports_circumferential_tangent() {
        use vcad_kernel_primitives::make_cylinder;

        let cyl = make_cylinder(5.0, 20.0, 32);
        let bvh = Bvh::build_brep(&cyl);

        // Fire at the wall from +X, off-axis in Y so the tangent is not
        // trivially an axis vector.
        let ray = Ray::new(Point3::new(20.0, 2.0, 10.0), Vec3::new(-1.0, 0.0, 0.0));
        let hit = bvh.trace_closest(&ray).expect("ray should hit the wall");
        let t = hit
            .dpdu
            .expect("cylinder wall must carry a tangent")
            .normalize();

        // Circumferential means: perpendicular to the axis (Z) and
        // perpendicular to the outward radial direction.
        assert!(
            t.z.abs() < 1e-9,
            "tangent should not run along the axis: {t:?}"
        );
        let radial = Vec3::new(hit.point.x, hit.point.y, 0.0).normalize();
        assert!(
            t.dot(radial).abs() < 1e-9,
            "tangent should be perpendicular to the radial direction: {t:?}"
        );
        // And it must be a real direction, not a degenerate zero.
        assert!((t.norm() - 1.0).abs() < 1e-9);
    }

    /// The flat cap of the same cylinder is a plane: it still has a
    /// parameterisation, so it reports a tangent lying in the cap.
    #[test]
    fn planar_cap_tangent_lies_in_the_face() {
        use vcad_kernel_primitives::make_cylinder;

        let cyl = make_cylinder(5.0, 20.0, 32);
        let bvh = Bvh::build_brep(&cyl);
        let ray = Ray::new(Point3::new(1.0, 1.0, 40.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = bvh.trace_closest(&ray).expect("ray should hit the top cap");
        let t = hit.dpdu.expect("plane must carry a tangent");
        let n = hit.normal.into_inner();
        assert!(
            t.normalize().dot(n).abs() < 1e-9,
            "planar tangent must lie in the face"
        );
    }

    #[test]
    fn test_bvh_build() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);
        assert!(bvh.root().is_some());
    }

    #[test]
    fn test_bvh_trace_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);

        // Ray from outside, hitting two faces (entry and exit)
        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        let hits = bvh.trace(&ray);
        assert_eq!(hits.len(), 2);

        // First hit should be at z=0
        assert!((hits[0].point.z - 0.0).abs() < 1e-8);
        // Second hit should be at z=10
        assert!((hits[1].point.z - 10.0).abs() < 1e-8);
    }

    #[test]
    fn test_bvh_trace_miss() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);

        // Ray missing the cube
        let ray = Ray::new(Point3::new(50.0, 50.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        let hits = bvh.trace(&ray);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_bvh_trace_closest() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);

        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        let closest = bvh.trace_closest(&ray);
        assert!(closest.is_some());
        assert!((closest.unwrap().point.z - 0.0).abs() < 1e-8);
    }

    #[test]
    fn test_bvh_diagonal_ray() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);

        // Diagonal ray through cube corner
        let ray = Ray::new(Point3::new(-5.0, -5.0, -5.0), Vec3::new(1.0, 1.0, 1.0));

        let hits = bvh.trace(&ray);
        // Should hit at least 2 faces (entry and exit)
        assert!(hits.len() >= 2);
    }

    /// Tessellated unit-ish cube spanning (0,0,0)..(10,10,10), so the mesh
    /// path can be checked against the analytic path's expectations.
    fn cube_mesh() -> TriangleMesh {
        vcad_kernel_tessellate::tessellate_brep(&make_cube(10.0, 10.0, 10.0), 32)
    }

    #[test]
    fn mesh_bvh_traces_a_tessellated_cube() {
        let bvh = Bvh::build_mesh(&cube_mesh());
        assert!(bvh.is_mesh());
        assert!(bvh.brep().is_none());

        // Off the face diagonal: a ray down the shared edge of a face's two
        // triangles legitimately hits both, which would mask a real
        // double-count.
        let ray = Ray::new(Point3::new(3.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        let hits = bvh.trace(&ray);

        // Entry at z=0 and exit at z=10, same as the analytic cube.
        assert_eq!(hits.len(), 2, "hits: {hits:?}");
        assert!((hits[0].point.z).abs() < 1e-9);
        assert!((hits[1].point.z - 10.0).abs() < 1e-9);
        // Mesh hits carry a triangle index and no face: `face_id` is the
        // geometry's answer, and a mesh has none.
        assert!(hits.iter().all(|h| bvh.face_id(h).is_none()));

        let closest = bvh.trace_closest(&ray).expect("should hit");
        assert!(closest.point.z.abs() < 1e-9);
    }

    #[test]
    fn mesh_bvh_misses_cleanly() {
        let bvh = Bvh::build_mesh(&cube_mesh());
        let ray = Ray::new(Point3::new(50.0, 50.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(bvh.trace(&ray).is_empty());
        assert!(bvh.trace_closest(&ray).is_none());
    }

    #[test]
    fn mesh_bvh_uses_vertex_normals_when_present() {
        // A single triangle in the z=0 plane whose vertex normals are tilted
        // away from the geometric +Z: a smooth-shaded hit must follow the
        // interpolated normal, not the facet.
        let mesh = TriangleMesh {
            vertices: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
            indices: vec![0, 1, 2],
            // All three tilted the same way, so any barycentric blend gives
            // the same direction — the test isolates "vertex or geometric?".
            normals: vec![0.0, 0.6, 0.8, 0.0, 0.6, 0.8, 0.0, 0.6, 0.8],
            face_kinds: Vec::new(),
        };
        let bvh = Bvh::build_mesh(&mesh);

        let ray = Ray::new(Point3::new(2.0, 2.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = bvh.trace_closest(&ray).expect("should hit");
        // Tolerance is f32-sized: TriangleMesh stores normals as f32.
        assert!(
            (hit.normal.y - 0.6).abs() < 1e-6,
            "normal: {:?}",
            hit.normal
        );
        assert!(
            (hit.normal.z - 0.8).abs() < 1e-6,
            "normal: {:?}",
            hit.normal
        );
    }

    #[test]
    fn mesh_bvh_falls_back_to_the_geometric_normal() {
        let mesh = TriangleMesh {
            vertices: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
            indices: vec![0, 1, 2],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };
        let bvh = Bvh::build_mesh(&mesh);

        let ray = Ray::new(Point3::new(2.0, 2.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
        let hit = bvh.trace_closest(&ray).expect("should hit");
        assert!(hit.normal.x.abs() < 1e-12 && hit.normal.y.abs() < 1e-12);
        assert!((hit.normal.z.abs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mesh_bvh_drops_degenerate_and_out_of_range_triangles() {
        let mesh = TriangleMesh {
            vertices: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
            // One good triangle, one with a repeated corner, one indexing
            // past the vertex array.
            indices: vec![0, 1, 2, 0, 0, 1, 0, 1, 99],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };
        let bvh = Bvh::build_mesh(&mesh);

        let ray = Ray::new(Point3::new(2.0, 2.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(bvh.trace(&ray).len(), 1);
    }

    #[test]
    fn empty_mesh_builds_an_empty_bvh() {
        let bvh = Bvh::build_mesh(&TriangleMesh::new());
        assert!(bvh.root().is_none());
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0));
        assert!(bvh.trace(&ray).is_empty());
    }

    #[test]
    fn mesh_bvh_flattens_to_nothing() {
        // The GPU pipeline traces analytic surfaces only; a mesh BVH must
        // not hand it face IDs it doesn't have.
        let bvh = Bvh::build_mesh(&cube_mesh());
        let (nodes, faces) = bvh.flatten_faces();
        assert!(nodes.is_empty() && faces.is_empty());
    }

    #[test]
    fn brep_flatten_still_round_trips_face_ids() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build_brep(&cube);
        let (nodes, faces) = bvh.flatten_faces();
        assert!(!nodes.is_empty());
        assert_eq!(faces.len(), cube.topology.faces.len());
        assert!(faces.iter().all(|&f| cube.topology.faces.contains_key(f)));
    }
}
