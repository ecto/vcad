//! Bounding Volume Hierarchy for accelerated ray tracing.
//!
//! Uses Surface Area Heuristic (SAH) for construction.

use std::sync::Arc;
use vcad_kernel_booleans::bbox::{face_aabb, Aabb3};
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_topo::FaceId;

use crate::intersect::{intersect_surface, intersect_triangle};
use crate::trim::{face_normal, point_in_face};
use crate::{Ray, RayHit};

/// A flattened BVH node tuple for GPU upload.
/// Contains: (AABB, is_leaf, left_or_first, right_or_count)
pub type FlatBvhNode = (Aabb3, bool, u32, u32);

/// A BVH node - either a leaf containing primitives or an internal node with
/// children.
#[derive(Debug, Clone)]
pub enum BvhNode {
    /// Leaf node containing primitive indices.
    Leaf {
        /// Axis-aligned bounding box of this node.
        aabb: Aabb3,
        /// Indices of the primitives in this leaf, into the owning
        /// [`Bvh`]'s geometry: BRep faces in build order for a BRep-backed
        /// BVH, triangles for a mesh-backed one.
        prims: Vec<u32>,
    },
    /// Internal node with two children.
    Internal {
        /// Axis-aligned bounding box of this node.
        aabb: Aabb3,
        /// Left child node.
        left: Box<BvhNode>,
        /// Right child node.
        right: Box<BvhNode>,
    },
}

/// Triangle geometry backing a mesh BVH.
///
/// Positions and normals are widened to `f64` once at build time — the
/// tracer works in `f64` throughout, and converting per intersection test
/// would put a cast in the innermost loop.
#[derive(Debug, Clone)]
struct MeshGeom {
    /// Vertex positions.
    positions: Vec<Point3>,
    /// Per-vertex normals, parallel to `positions`. Empty when the source
    /// mesh carried none, in which case hits report the geometric face
    /// normal instead.
    normals: Vec<Vec3>,
    /// Triangle corner indices into `positions`.
    tris: Vec<[u32; 3]>,
}

impl MeshGeom {
    /// Intersect a ray with one triangle, shading normal included.
    fn test(&self, ray: &Ray, tri: u32) -> Option<RayHit> {
        let [i0, i1, i2] = self.tris[tri as usize];
        let (v0, v1, v2) = (
            self.positions[i0 as usize],
            self.positions[i1 as usize],
            self.positions[i2 as usize],
        );

        let hit = intersect_triangle(ray, v0, v1, v2)?;

        // Geometric normal. Non-zero by construction: `intersect_triangle`
        // rejects degenerate triangles, so this is always a usable fallback.
        let geometric = (v1 - v0).cross(v2 - v0);

        // Smooth shading: barycentric blend of the vertex normals, so a mesh
        // part doesn't read as faceted next to an analytic one. Falls back
        // to the geometric normal when the mesh has no normals, or when the
        // blend cancels (opposed vertex normals on a degenerate crease).
        let smooth = if self.normals.is_empty() {
            None
        } else {
            let n = self.normals[i0 as usize] * hit.w()
                + self.normals[i1 as usize] * hit.u
                + self.normals[i2 as usize] * hit.v;
            (n.norm() > 1e-12).then_some(n)
        };

        Some(RayHit::triangle(
            hit.t,
            ray.at(hit.t),
            Dir3::new_normalize(smooth.unwrap_or(geometric)),
            Point2::new(hit.u, hit.v),
            tri,
        ))
    }
}

/// The geometry a [`Bvh`] was built over.
#[derive(Debug, Clone)]
enum BvhGeom {
    /// Trimmed analytic faces of a BRep solid.
    BRep {
        /// The solid being traced.
        brep: Arc<BRepSolid>,
        /// Primitive index -> face ID.
        faces: Vec<FaceId>,
    },
    /// Triangles of a mesh-only solid.
    Mesh(Arc<MeshGeom>),
}

/// Bounding Volume Hierarchy for accelerated ray-geometry intersection.
///
/// Builds over either the trimmed analytic faces of a [`BRepSolid`]
/// ([`Bvh::build`]) or the triangles of a [`TriangleMesh`]
/// ([`Bvh::build_mesh`]); tracing is identical for both.
#[derive(Debug, Clone)]
pub struct Bvh {
    root: Option<BvhNode>,
    geom: BvhGeom,
}

impl Bvh {
    /// Build a BVH from a BRep solid using SAH construction.
    pub fn build(brep: &BRepSolid) -> Self {
        let brep = Arc::new(brep.clone());

        // Collect all faces with their AABBs
        let faces: Vec<FaceId> = brep.topology.faces.iter().map(|(id, _)| id).collect();
        let mut prim_data: Vec<PrimData> = faces
            .iter()
            .enumerate()
            .map(|(i, &face_id)| {
                let aabb = face_aabb(&brep, face_id);
                (i as u32, aabb, centroid_of(&aabb))
            })
            .collect();

        let root = if prim_data.is_empty() {
            None
        } else {
            Some(build_node(&mut prim_data))
        };

        Self {
            root,
            geom: BvhGeom::BRep { brep, faces },
        }
    }

    /// Build a BVH over the triangles of a mesh, using the same SAH
    /// construction as [`build`](Self::build).
    ///
    /// Mesh-only solids — frozen `topology_optimize` results, imported
    /// STL/GLB parts — carry no analytic surfaces, so they are traced as
    /// triangles. Degenerate triangles (repeated or out-of-range indices)
    /// are dropped at build time rather than wasting intersection tests.
    ///
    /// When the mesh carries vertex normals they are interpolated for
    /// smooth shading; otherwise hits report the geometric face normal.
    pub fn build_mesh(mesh: &TriangleMesh) -> Self {
        let vertex_count = mesh.vertices.len() / 3;
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

        let tris: Vec<[u32; 3]> = mesh
            .indices
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .filter(|t| {
                if !t.iter().all(|&i| (i as usize) < vertex_count) {
                    return false;
                }
                if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                    return false;
                }
                // Zero-area (collinear corners) too: such a triangle can
                // never be hit, so keeping it would inflate the BVH and —
                // worse — make a fully degenerate mesh look traceable when
                // it isn't. Same size-relative test the intersector uses,
                // so build and trace agree on what counts as degenerate.
                let (a, b, c) = (
                    positions[t[0] as usize],
                    positions[t[1] as usize],
                    positions[t[2] as usize],
                );
                let (e1, e2) = (b - a, c - a);
                e1.cross(e2).norm() > 1e-12 * (e1.norm() * e2.norm()).max(1.0)
            })
            .collect();

        let mut prim_data: Vec<PrimData> = tris
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut aabb = Aabb3::empty();
                for &vi in t {
                    aabb.include_point(&positions[vi as usize]);
                }
                (i as u32, aabb, centroid_of(&aabb))
            })
            .collect();

        let root = if prim_data.is_empty() {
            None
        } else {
            Some(build_node(&mut prim_data))
        };

        Self {
            root,
            geom: BvhGeom::Mesh(Arc::new(MeshGeom {
                positions,
                normals,
                tris,
            })),
        }
    }

    /// Trace a ray through the BVH, returning all intersections sorted by t.
    pub fn trace(&self, ray: &Ray) -> Vec<RayHit> {
        let mut hits = Vec::new();

        if let Some(ref root) = self.root {
            self.trace_node(ray, root, &mut hits);
        }

        hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        hits
    }

    /// Trace a ray and return only the closest hit.
    pub fn trace_closest(&self, ray: &Ray) -> Option<RayHit> {
        let mut closest: Option<RayHit> = None;
        let mut closest_t = f64::INFINITY;

        if let Some(ref root) = self.root {
            self.trace_node_closest(ray, root, &mut closest, &mut closest_t);
        }

        closest
    }

    /// Trace a ray through a single node.
    fn trace_node(&self, ray: &Ray, node: &BvhNode, hits: &mut Vec<RayHit>) {
        match node {
            BvhNode::Leaf { aabb, prims } => {
                if ray.intersect_aabb(aabb).is_some() {
                    for &prim in prims {
                        self.test_prim(ray, prim, hits);
                    }
                }
            }
            BvhNode::Internal { aabb, left, right } => {
                if ray.intersect_aabb(aabb).is_some() {
                    self.trace_node(ray, left, hits);
                    self.trace_node(ray, right, hits);
                }
            }
        }
    }

    /// Trace a ray, keeping only the closest hit.
    fn trace_node_closest(
        &self,
        ray: &Ray,
        node: &BvhNode,
        closest: &mut Option<RayHit>,
        closest_t: &mut f64,
    ) {
        match node {
            BvhNode::Leaf { aabb, prims } => {
                if let Some((t_min, _)) = ray.intersect_aabb(aabb) {
                    // Early out if AABB entry is beyond current closest
                    if t_min >= *closest_t {
                        return;
                    }

                    for &prim in prims {
                        if let Some(hit) = self.test_prim_single(ray, prim) {
                            if hit.t < *closest_t {
                                *closest_t = hit.t;
                                *closest = Some(hit);
                            }
                        }
                    }
                }
            }
            BvhNode::Internal { aabb, left, right } => {
                if let Some((t_min, _)) = ray.intersect_aabb(aabb) {
                    if t_min >= *closest_t {
                        return;
                    }

                    // Test children in order of AABB distance
                    let left_t = ray.intersect_aabb(&get_aabb(left)).map(|(t, _)| t);
                    let right_t = ray.intersect_aabb(&get_aabb(right)).map(|(t, _)| t);

                    match (left_t, right_t) {
                        (Some(lt), Some(rt)) => {
                            if lt < rt {
                                self.trace_node_closest(ray, left, closest, closest_t);
                                self.trace_node_closest(ray, right, closest, closest_t);
                            } else {
                                self.trace_node_closest(ray, right, closest, closest_t);
                                self.trace_node_closest(ray, left, closest, closest_t);
                            }
                        }
                        (Some(_), None) => {
                            self.trace_node_closest(ray, left, closest, closest_t);
                        }
                        (None, Some(_)) => {
                            self.trace_node_closest(ray, right, closest, closest_t);
                        }
                        (None, None) => {}
                    }
                }
            }
        }
    }

    /// Test a ray against a single primitive, appending every hit.
    fn test_prim(&self, ray: &Ray, prim: u32, hits: &mut Vec<RayHit>) {
        match &self.geom {
            BvhGeom::BRep { brep, faces } => {
                let face_id = faces[prim as usize];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                let surface_hits = intersect_surface(ray, surface.as_ref());

                for hit in surface_hits {
                    // Check if the hit is within the face's trim boundaries
                    if point_in_face(brep, face_id, hit.uv) {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        hits.push(RayHit::new(hit.t, point, normal, hit.uv, face_id));
                    }
                }
            }
            BvhGeom::Mesh(mesh) => {
                if let Some(hit) = mesh.test(ray, prim) {
                    hits.push(hit);
                }
            }
        }
    }

    /// Test a ray against a single primitive, returning only the closest hit.
    fn test_prim_single(&self, ray: &Ray, prim: u32) -> Option<RayHit> {
        match &self.geom {
            BvhGeom::BRep { brep, faces } => {
                let face_id = faces[prim as usize];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                let surface_hits = intersect_surface(ray, surface.as_ref());

                let mut closest: Option<RayHit> = None;

                for hit in surface_hits {
                    if point_in_face(brep, face_id, hit.uv)
                        && (closest.is_none() || hit.t < closest.as_ref().unwrap().t)
                    {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        closest = Some(RayHit::new(hit.t, point, normal, hit.uv, face_id));
                    }
                }

                closest
            }
            // A triangle is convex: at most one hit, so "closest" is "the" hit.
            BvhGeom::Mesh(mesh) => mesh.test(ray, prim),
        }
    }

    /// Get a reference to the underlying BRep solid, if this BVH was built
    /// over one. `None` for mesh-backed BVHs.
    pub fn brep(&self) -> Option<&BRepSolid> {
        match &self.geom {
            BvhGeom::BRep { brep, .. } => Some(brep),
            BvhGeom::Mesh(_) => None,
        }
    }

    /// Whether this BVH traces triangles rather than analytic BRep faces.
    pub fn is_mesh(&self) -> bool {
        matches!(self.geom, BvhGeom::Mesh(_))
    }

    /// Get a reference to the root node, if any.
    pub fn root(&self) -> Option<&BvhNode> {
        self.root.as_ref()
    }

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
    pub fn flatten(&self) -> (Vec<FlatBvhNode>, Vec<FaceId>) {
        let mut nodes = Vec::new();
        let mut faces = Vec::new();

        let BvhGeom::BRep {
            faces: face_ids, ..
        } = &self.geom
        else {
            return (nodes, faces);
        };

        if let Some(root) = &self.root {
            flatten_node(root, face_ids, &mut nodes, &mut faces);
        }

        (nodes, faces)
    }
}

/// Get the AABB of a node.
fn get_aabb(node: &BvhNode) -> Aabb3 {
    match node {
        BvhNode::Leaf { aabb, .. } => *aabb,
        BvhNode::Internal { aabb, .. } => *aabb,
    }
}

/// Recursively flatten a BVH node into a vector.
fn flatten_node(
    node: &BvhNode,
    face_ids: &[FaceId],
    nodes: &mut Vec<FlatBvhNode>,
    faces: &mut Vec<FaceId>,
) -> usize {
    let idx = nodes.len();

    match node {
        BvhNode::Leaf { aabb, prims } => {
            let start = faces.len() as u32;
            let count = prims.len() as u32;
            faces.extend(prims.iter().map(|&p| face_ids[p as usize]));
            nodes.push((*aabb, true, start, count));
        }
        BvhNode::Internal { aabb, left, right } => {
            // Reserve space for this node
            nodes.push((*aabb, false, 0, 0));

            // Recursively flatten children
            let left_idx = flatten_node(left, face_ids, nodes, faces);
            let right_idx = flatten_node(right, face_ids, nodes, faces);

            // Update this node with child indices
            nodes[idx].2 = left_idx as u32;
            nodes[idx].3 = right_idx as u32;
        }
    }

    idx
}

/// One primitive's build-time record: index, bounds, centroid.
type PrimData = (u32, Aabb3, Point3);

/// Centre of an AABB.
fn centroid_of(aabb: &Aabb3) -> Point3 {
    Point3::new(
        (aabb.min.x + aabb.max.x) / 2.0,
        (aabb.min.y + aabb.max.y) / 2.0,
        (aabb.min.z + aabb.max.z) / 2.0,
    )
}

/// Build a BVH node recursively using SAH.
fn build_node(face_data: &mut [PrimData]) -> BvhNode {
    // Compute bounds of all faces
    let mut bounds = Aabb3::empty();
    for (_, aabb, _) in face_data.iter() {
        bounds.include_point(&aabb.min);
        bounds.include_point(&aabb.max);
    }

    // Base case: small number of faces -> leaf
    if face_data.len() <= 4 {
        return BvhNode::Leaf {
            aabb: bounds,
            prims: face_data.iter().map(|(id, _, _)| *id).collect(),
        };
    }

    // Find best split using SAH
    let (best_axis, best_pos) = find_best_split(face_data, &bounds);

    // Partition faces
    let mid = partition_faces(face_data, best_axis, best_pos);

    // Fallback if partition fails
    if mid == 0 || mid == face_data.len() {
        // Just split in the middle
        let mid = face_data.len() / 2;
        let (left_data, right_data) = face_data.split_at_mut(mid);
        return BvhNode::Internal {
            aabb: bounds,
            left: Box::new(build_node(left_data)),
            right: Box::new(build_node(right_data)),
        };
    }

    let (left_data, right_data) = face_data.split_at_mut(mid);

    BvhNode::Internal {
        aabb: bounds,
        left: Box::new(build_node(left_data)),
        right: Box::new(build_node(right_data)),
    }
}

/// Find the best split axis and position using SAH.
fn find_best_split(face_data: &[PrimData], bounds: &Aabb3) -> (usize, f64) {
    const NUM_BUCKETS: usize = 12;

    let extent = Vec3::new(
        bounds.max.x - bounds.min.x,
        bounds.max.y - bounds.min.y,
        bounds.max.z - bounds.min.z,
    );

    let mut best_cost = f64::INFINITY;
    let mut best_axis = 0;
    let mut best_pos = 0.0;

    // Try each axis
    for axis in 0..3 {
        let axis_extent = match axis {
            0 => extent.x,
            1 => extent.y,
            _ => extent.z,
        };

        if axis_extent < 1e-10 {
            continue;
        }

        let axis_min = match axis {
            0 => bounds.min.x,
            1 => bounds.min.y,
            _ => bounds.min.z,
        };

        // Initialize buckets
        let mut bucket_counts = [0usize; NUM_BUCKETS];
        let mut bucket_bounds = [Aabb3::empty(); NUM_BUCKETS];

        // Assign faces to buckets
        for (_, aabb, centroid) in face_data {
            let c = match axis {
                0 => centroid.x,
                1 => centroid.y,
                _ => centroid.z,
            };

            let b = ((c - axis_min) / axis_extent * NUM_BUCKETS as f64) as usize;
            let b = b.min(NUM_BUCKETS - 1);

            bucket_counts[b] += 1;
            bucket_bounds[b].include_point(&aabb.min);
            bucket_bounds[b].include_point(&aabb.max);
        }

        // Sweep to find best split
        for split in 1..NUM_BUCKETS {
            let mut left_count = 0;
            let mut left_bounds = Aabb3::empty();
            for i in 0..split {
                left_count += bucket_counts[i];
                if bucket_counts[i] > 0 {
                    left_bounds.include_point(&bucket_bounds[i].min);
                    left_bounds.include_point(&bucket_bounds[i].max);
                }
            }

            let mut right_count = 0;
            let mut right_bounds = Aabb3::empty();
            for i in split..NUM_BUCKETS {
                right_count += bucket_counts[i];
                if bucket_counts[i] > 0 {
                    right_bounds.include_point(&bucket_bounds[i].min);
                    right_bounds.include_point(&bucket_bounds[i].max);
                }
            }

            if left_count == 0 || right_count == 0 {
                continue;
            }

            // SAH cost: traversal + P(left) * N_left + P(right) * N_right
            let left_area = surface_area(&left_bounds);
            let right_area = surface_area(&right_bounds);
            let total_area = surface_area(bounds);

            let cost = 0.125 // traversal cost
                + left_area / total_area * left_count as f64
                + right_area / total_area * right_count as f64;

            if cost < best_cost {
                best_cost = cost;
                best_axis = axis;
                best_pos = axis_min + (split as f64 / NUM_BUCKETS as f64) * axis_extent;
            }
        }
    }

    (best_axis, best_pos)
}

/// Partition faces by centroid along an axis.
fn partition_faces(face_data: &mut [PrimData], axis: usize, pos: f64) -> usize {
    let mut left = 0;
    let mut right = face_data.len();

    while left < right {
        let c = match axis {
            0 => face_data[left].2.x,
            1 => face_data[left].2.y,
            _ => face_data[left].2.z,
        };

        if c < pos {
            left += 1;
        } else {
            right -= 1;
            face_data.swap(left, right);
        }
    }

    left
}

/// Compute surface area of an AABB.
fn surface_area(aabb: &Aabb3) -> f64 {
    let d = Vec3::new(
        aabb.max.x - aabb.min.x,
        aabb.max.y - aabb.min.y,
        aabb.max.z - aabb.min.z,
    );
    2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_bvh_build() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);
        assert!(bvh.root.is_some());
    }

    #[test]
    fn test_bvh_trace_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);

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
        let bvh = Bvh::build(&cube);

        // Ray missing the cube
        let ray = Ray::new(Point3::new(50.0, 50.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        let hits = bvh.trace(&ray);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_bvh_trace_closest() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);

        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        let closest = bvh.trace_closest(&ray);
        assert!(closest.is_some());
        assert!((closest.unwrap().point.z - 0.0).abs() < 1e-8);
    }

    #[test]
    fn test_bvh_diagonal_ray() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);

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
        assert!(hits.iter().all(|h| h.tri.is_some()));

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
        let (nodes, faces) = bvh.flatten();
        assert!(nodes.is_empty() && faces.is_empty());
    }

    #[test]
    fn brep_flatten_still_round_trips_face_ids() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);
        let (nodes, faces) = bvh.flatten();
        assert!(!nodes.is_empty());
        assert_eq!(faces.len(), cube.topology.faces.len());
        assert!(faces.iter().all(|&f| cube.topology.faces.contains_key(f)));
    }
}
