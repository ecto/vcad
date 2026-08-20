//! Bounding Volume Hierarchy for accelerated ray tracing.
//!
//! Uses Surface Area Heuristic (SAH) for construction.

use std::sync::Arc;
use vcad_kernel_booleans::bbox::{face_aabb, Aabb3};
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_topo::FaceId;

use crate::intersect::{intersect_surface, intersect_triangle, surface_tangent};
use crate::trim::{face_normal, FaceTrim};
use crate::{Ray, RayHit};

/// A flattened BVH node tuple for GPU upload.
/// Contains: (AABB, is_leaf, left_or_first, right_or_count)
pub type FlatBvhNode = (Aabb3, bool, u32, u32);

/// [`Bvh::flatten`] was called on a BVH the GPU pipeline cannot consume.
///
/// The GPU path tracer traces analytic BRep faces; there is no triangle BLAS
/// yet. Flattening a mesh-backed BVH used to yield empty node/face lists,
/// which the GPU scene builder happily uploaded and then rendered as a blank
/// frame. Failing here instead makes the missing capability visible at the
/// call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlattenUnsupported {
    /// Number of triangles the mesh-backed BVH holds.
    pub triangle_count: usize,
}

impl std::fmt::Display for FlattenUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot flatten a mesh-backed BVH ({} triangles) for GPU upload: \
             the GPU path tracer traces analytic BRep faces only (no triangle BLAS yet)",
            self.triangle_count
        )
    }
}

impl std::error::Error for FlattenUnsupported {}

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
        /// Primitive index -> that face's trim boundary, projected into UV
        /// once at build time.
        ///
        /// Without this, every ray-face hit test reprojected the face's whole
        /// trim loop — Newton-iterating each vertex back onto the surface and
        /// allocating three `Vec`s — to answer one point-in-polygon query.
        /// The work does not depend on the query point, and a frame asks it
        /// millions of times.
        trims: Vec<FaceTrim>,
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
        Self::build_shared(Arc::new(brep.clone()))
    }

    /// Build a BVH over an already-shared BRep solid.
    ///
    /// Skips the clone `build` performs, so N instances of the same part can
    /// share one BLAS (see [`crate::tlas`]).
    pub fn build_shared(brep: Arc<BRepSolid>) -> Self {
        // Collect all faces with their AABBs
        let faces: Vec<FaceId> = brep.topology.faces.iter().map(|(id, _)| id).collect();
        let trims: Vec<FaceTrim> = faces
            .iter()
            .map(|&face_id| FaceTrim::build(&brep, face_id))
            .collect();
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
            geom: BvhGeom::BRep { brep, faces, trims },
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

        let tris: Vec<[u32; 3]> = (0..mesh.indices.len() / 3)
            .map(|i| {
                [
                    mesh.indices[i * 3],
                    mesh.indices[i * 3 + 1],
                    mesh.indices[i * 3 + 2],
                ]
            })
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
        self.trace_closest_limit(ray, f64::INFINITY)
    }

    /// Trace a ray and return the closest hit strictly nearer than `t_max`.
    ///
    /// Seeding the search with a known upper bound lets the SAH early-outs
    /// prune subtrees a caller has already beaten — the basis of TLAS
    /// traversal, where each instance inherits the best `t` found so far.
    pub fn trace_closest_limit(&self, ray: &Ray, t_max: f64) -> Option<RayHit> {
        self.trace_closest_range(ray, 0.0, t_max)
    }

    /// Trace a ray and return the closest hit in the open interval
    /// `(t_min, t_max)`.
    ///
    /// `t_min` is how callers dodge self-intersection without nudging the ray
    /// origin along the normal — the path tracer's convention. Note this is a
    /// genuine interval search, not a post-filter: a hit at `t <= t_min` is
    /// skipped and the search continues past it, so a surface hidden behind
    /// one is still found.
    pub fn trace_closest_range(&self, ray: &Ray, t_min: f64, t_max: f64) -> Option<RayHit> {
        let mut closest: Option<RayHit> = None;
        let mut closest_t = t_max;

        if let Some(ref root) = self.root {
            self.trace_node_closest(ray, root, t_min, &mut closest, &mut closest_t);
        }

        closest
    }

    /// Any-hit test: does the ray hit *anything* in `(0, t_max)`?
    ///
    /// Returns as soon as one hit is found — strictly less work than
    /// [`Bvh::trace_closest`], which must find the nearest. This is the
    /// traversal shadow and occlusion rays want.
    pub fn occluded(&self, ray: &Ray, t_max: f64) -> bool {
        self.occluded_range(ray, 0.0, t_max)
    }

    /// Any-hit test over the open interval `(t_min, t_max)`.
    pub fn occluded_range(&self, ray: &Ray, t_min: f64, t_max: f64) -> bool {
        match self.root {
            Some(ref root) => self.occluded_node(ray, root, t_min, t_max),
            None => false,
        }
    }

    fn occluded_node(&self, ray: &Ray, node: &BvhNode, t_min: f64, t_max: f64) -> bool {
        let Some((enter, exit)) = ray.intersect_aabb(node_aabb_of(node)) else {
            return false;
        };
        // Prune boxes wholly outside the interval on either side.
        if enter >= t_max || exit <= t_min {
            return false;
        }
        match node {
            BvhNode::Leaf { prims, .. } => prims
                .iter()
                .any(|&prim| self.prim_occludes(ray, prim, t_min, t_max)),
            BvhNode::Internal { left, right, .. } => {
                self.occluded_node(ray, left, t_min, t_max)
                    || self.occluded_node(ray, right, t_min, t_max)
            }
        }
    }

    /// Does this primitive block the ray inside `(t_min, t_max)`? Stops at the
    /// first qualifying hit rather than ranking them, which is the whole point
    /// of an any-hit query.
    fn prim_occludes(&self, ray: &Ray, prim: u32, t_min: f64, t_max: f64) -> bool {
        match &self.geom {
            BvhGeom::BRep { brep, faces, trims } => {
                let face_id = faces[prim as usize];
                let trim = &trims[prim as usize];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];
                intersect_surface(ray, surface.as_ref())
                    .into_iter()
                    .any(|hit| hit.t > t_min && hit.t < t_max && trim.contains(hit.uv))
            }
            // A triangle is convex: at most one hit, so there is nothing to
            // short-circuit past.
            BvhGeom::Mesh(mesh) => mesh
                .test(ray, prim)
                .is_some_and(|h| h.t > t_min && h.t < t_max),
        }
    }

    /// World-space (well, solid-space) bounds of the whole hierarchy.
    pub fn bounds(&self) -> Option<Aabb3> {
        self.root.as_ref().map(get_aabb)
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

    /// Trace a ray, keeping only the closest hit in `(t_min, closest_t)`.
    fn trace_node_closest(
        &self,
        ray: &Ray,
        node: &BvhNode,
        t_min: f64,
        closest: &mut Option<RayHit>,
        closest_t: &mut f64,
    ) {
        let Some((enter, exit)) = ray.intersect_aabb(node_aabb_of(node)) else {
            return;
        };
        // Early out if the box is beyond the current closest, or entirely
        // behind `t_min`.
        if enter >= *closest_t || exit <= t_min {
            return;
        }

        match node {
            BvhNode::Leaf { prims, .. } => {
                for &prim in prims {
                    if let Some(hit) = self.test_prim_single(ray, prim, t_min) {
                        if hit.t < *closest_t {
                            *closest_t = hit.t;
                            *closest = Some(hit);
                        }
                    }
                }
            }
            BvhNode::Internal { left, right, .. } => {
                // Test children in order of AABB distance
                let left_t = ray.intersect_aabb(&get_aabb(left)).map(|(t, _)| t);
                let right_t = ray.intersect_aabb(&get_aabb(right)).map(|(t, _)| t);

                match (left_t, right_t) {
                    (Some(lt), Some(rt)) => {
                        if lt < rt {
                            self.trace_node_closest(ray, left, t_min, closest, closest_t);
                            self.trace_node_closest(ray, right, t_min, closest, closest_t);
                        } else {
                            self.trace_node_closest(ray, right, t_min, closest, closest_t);
                            self.trace_node_closest(ray, left, t_min, closest, closest_t);
                        }
                    }
                    (Some(_), None) => {
                        self.trace_node_closest(ray, left, t_min, closest, closest_t);
                    }
                    (None, Some(_)) => {
                        self.trace_node_closest(ray, right, t_min, closest, closest_t);
                    }
                    (None, None) => {}
                }
            }
        }
    }

    /// Test a ray against a single primitive, appending every hit.
    fn test_prim(&self, ray: &Ray, prim: u32, hits: &mut Vec<RayHit>) {
        match &self.geom {
            BvhGeom::BRep { brep, faces, trims } => {
                let face_id = faces[prim as usize];
                let trim = &trims[prim as usize];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                let surface_hits = intersect_surface(ray, surface.as_ref());

                for hit in surface_hits {
                    // Check if the hit is within the face's trim boundaries
                    if trim.contains(hit.uv) {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        let tangent = surface_tangent(surface.as_ref(), hit.uv);
                        hits.push(
                            RayHit::new(hit.t, point, normal, hit.uv, face_id)
                                .with_tangent(tangent),
                        );
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

    /// Test a ray against a single primitive, returning only the closest hit
    /// strictly past `t_min`.
    fn test_prim_single(&self, ray: &Ray, prim: u32, t_min: f64) -> Option<RayHit> {
        match &self.geom {
            BvhGeom::BRep { brep, faces, trims } => {
                let face_id = faces[prim as usize];
                let trim = &trims[prim as usize];
                let face = &brep.topology.faces[face_id];
                let surface = &brep.geometry.surfaces[face.surface_index];

                let surface_hits = intersect_surface(ray, surface.as_ref());

                let mut closest: Option<RayHit> = None;

                for hit in surface_hits {
                    if hit.t > t_min
                        && trim.contains(hit.uv)
                        && (closest.is_none() || hit.t < closest.as_ref().unwrap().t)
                    {
                        let point = ray.at(hit.t);
                        let normal = face_normal(brep, face_id, hit.uv);
                        let tangent = surface_tangent(surface.as_ref(), hit.uv);
                        closest = Some(
                            RayHit::new(hit.t, point, normal, hit.uv, face_id)
                                .with_tangent(tangent),
                        );
                    }
                }

                closest
            }
            // A triangle is convex: at most one hit, so "closest" is "the" hit.
            BvhGeom::Mesh(mesh) => mesh.test(ray, prim).filter(|h| h.t > t_min),
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
    /// A mesh-backed BVH has no GPU representation yet and is reported as
    /// [`FlattenUnsupported`] rather than flattening to empty lists (which
    /// would silently render a blank frame).
    pub fn flatten(&self) -> Result<(Vec<FlatBvhNode>, Vec<FaceId>), FlattenUnsupported> {
        let mut nodes = Vec::new();
        let mut faces = Vec::new();

        let face_ids = match &self.geom {
            BvhGeom::BRep { faces, .. } => faces,
            BvhGeom::Mesh(mesh) => {
                return Err(FlattenUnsupported {
                    triangle_count: mesh.tris.len(),
                })
            }
        };

        if let Some(root) = &self.root {
            flatten_node(root, face_ids, &mut nodes, &mut faces);
        }

        Ok((nodes, faces))
    }
}

/// Borrow a node's AABB without copying it.
fn node_aabb_of(node: &BvhNode) -> &Aabb3 {
    match node {
        BvhNode::Leaf { aabb, .. } | BvhNode::Internal { aabb, .. } => aabb,
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
/// A BLAS primitive for the SAH builder: prim index, bounds, centroid.
/// Spelled as the generic [`SahItem`] so the per-solid BLAS and the scene
/// TLAS share one split implementation rather than keeping two copies.
type PrimData = SahItem<u32>;

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
    let bounds = item_bounds(face_data);

    // Base case: small number of faces -> leaf
    if face_data.len() <= 4 {
        return BvhNode::Leaf {
            aabb: bounds,
            prims: face_data.iter().map(|(id, _, _)| *id).collect(),
        };
    }

    let mid = sah_split(face_data, &bounds);
    let (left_data, right_data) = face_data.split_at_mut(mid);

    BvhNode::Internal {
        aabb: bounds,
        left: Box::new(build_node(left_data)),
        right: Box::new(build_node(right_data)),
    }
}

/// An item to be partitioned by the SAH builder: a payload, its bounds, and
/// its centroid. Generic over the payload so the same split search serves
/// both the per-solid BLAS (faces) and the scene TLAS (instances).
pub(crate) type SahItem<T> = (T, Aabb3, vcad_kernel_math::Point3);

/// Compute the bounds of a slice of SAH items.
pub(crate) fn item_bounds<T>(items: &[SahItem<T>]) -> Aabb3 {
    let mut bounds = Aabb3::empty();
    for (_, aabb, _) in items {
        bounds.include_point(&aabb.min);
        bounds.include_point(&aabb.max);
    }
    bounds
}

/// Split a slice of SAH items in two, returning the midpoint index.
///
/// Runs the bucketed SAH search and falls back to a median split when the
/// chosen plane leaves one side empty. Always returns `1..items.len()`, so
/// callers can recurse unconditionally.
pub(crate) fn sah_split<T>(items: &mut [SahItem<T>], bounds: &Aabb3) -> usize {
    let (axis, pos) = find_best_split(items, bounds);
    let mid = partition_items(items, axis, pos);
    if mid == 0 || mid == items.len() {
        items.len() / 2
    } else {
        mid
    }
}

/// Find the best split axis and position using SAH.
fn find_best_split<T>(face_data: &[SahItem<T>], bounds: &Aabb3) -> (usize, f64) {
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

/// Partition items by centroid along an axis.
fn partition_items<T>(face_data: &mut [SahItem<T>], axis: usize, pos: f64) -> usize {
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

    /// A hit on a cylinder wall must report the *circumferential* tangent —
    /// the direction a lathe tool travels. Anisotropic shading orients its
    /// specular lobe with this, so if it ever came back axial or radial,
    /// turned parts would get their grain rotated 90°.
    #[test]
    fn cylinder_hit_reports_circumferential_tangent() {
        use vcad_kernel_primitives::make_cylinder;

        let cyl = make_cylinder(5.0, 20.0, 32);
        let bvh = Bvh::build(&cyl);

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
        let bvh = Bvh::build(&cyl);
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
    fn mesh_bvh_flatten_is_an_error_not_an_empty_scene() {
        // The GPU pipeline traces analytic surfaces only; a mesh BVH must
        // fail loudly rather than flatten to nothing and render blank.
        let bvh = Bvh::build_mesh(&cube_mesh());
        let err = bvh.flatten().expect_err("mesh BVH must not flatten");
        assert_eq!(err.triangle_count, 12);
        let msg = err.to_string();
        assert!(msg.contains("mesh-backed"), "{msg}");
    }

    #[test]
    fn empty_mesh_bvh_flatten_is_also_an_error() {
        // Zero triangles is still "no GPU representation" — don't let the
        // empty case slip through as a success.
        let bvh = Bvh::build_mesh(&TriangleMesh::new());
        assert!(bvh.flatten().is_err());
    }

    #[test]
    fn brep_flatten_still_round_trips_face_ids() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&cube);
        let (nodes, faces) = bvh.flatten().expect("BRep BVH flattens");
        assert!(!nodes.is_empty());
        assert_eq!(faces.len(), cube.topology.faces.len());
        assert!(faces.iter().all(|&f| cube.topology.faces.contains_key(f)));
    }
}
