//! Bounding Volume Hierarchy for accelerated ray tracing.
//!
//! Uses Surface Area Heuristic (SAH) for construction.

use std::sync::Arc;
use vcad_kernel_booleans::bbox::{face_aabb, Aabb3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::FaceId;

use crate::intersect::intersect_surface;
use crate::trim::{face_normal, point_in_face};
use crate::{Ray, RayHit};

/// A flattened BVH node tuple for GPU upload.
/// Contains: (AABB, is_leaf, left_or_first, right_or_count)
pub type FlatBvhNode = (Aabb3, bool, u32, u32);

/// A BVH node - either a leaf containing faces or an internal node with children.
#[derive(Debug, Clone)]
pub enum BvhNode {
    /// Leaf node containing face indices.
    Leaf {
        /// Axis-aligned bounding box of this node.
        aabb: Aabb3,
        /// Face IDs contained in this leaf.
        faces: Vec<FaceId>,
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

/// Bounding Volume Hierarchy for accelerated ray-BRep intersection.
#[derive(Debug, Clone)]
pub struct Bvh {
    root: Option<BvhNode>,
    brep: Arc<BRepSolid>,
}

impl Bvh {
    /// Build a BVH from a BRep solid using SAH construction.
    pub fn build(brep: &BRepSolid) -> Self {
        let brep = Arc::new(brep.clone());

        // Collect all faces with their AABBs
        let mut face_data: Vec<(FaceId, Aabb3, vcad_kernel_math::Point3)> = brep
            .topology
            .faces
            .iter()
            .map(|(face_id, _)| {
                let aabb = face_aabb(&brep, face_id);
                let centroid = vcad_kernel_math::Point3::new(
                    (aabb.min.x + aabb.max.x) / 2.0,
                    (aabb.min.y + aabb.max.y) / 2.0,
                    (aabb.min.z + aabb.max.z) / 2.0,
                );
                (face_id, aabb, centroid)
            })
            .collect();

        let root = if face_data.is_empty() {
            None
        } else {
            Some(build_node(&mut face_data))
        };

        Self { root, brep }
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
            BvhNode::Leaf { aabb, faces } => {
                if ray.intersect_aabb(aabb).is_some() {
                    for &face_id in faces {
                        self.test_face(ray, face_id, hits);
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
            BvhNode::Leaf { aabb, faces } => {
                if let Some((t_min, _)) = ray.intersect_aabb(aabb) {
                    // Early out if AABB entry is beyond current closest
                    if t_min >= *closest_t {
                        return;
                    }

                    for &face_id in faces {
                        if let Some(hit) = self.test_face_single(ray, face_id) {
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

    /// Test a ray against a single face.
    fn test_face(&self, ray: &Ray, face_id: FaceId, hits: &mut Vec<RayHit>) {
        let face = &self.brep.topology.faces[face_id];
        let surface = &self.brep.geometry.surfaces[face.surface_index];

        let surface_hits = intersect_surface(ray, surface.as_ref());

        for hit in surface_hits {
            // Check if the hit is within the face's trim boundaries
            if point_in_face(&self.brep, face_id, hit.uv) {
                let point = ray.at(hit.t);
                let normal = face_normal(&self.brep, face_id, hit.uv);
                hits.push(RayHit::new(hit.t, point, normal, hit.uv, face_id));
            }
        }
    }

    /// Test a ray against a single face, returning only the closest hit.
    fn test_face_single(&self, ray: &Ray, face_id: FaceId) -> Option<RayHit> {
        let face = &self.brep.topology.faces[face_id];
        let surface = &self.brep.geometry.surfaces[face.surface_index];

        let surface_hits = intersect_surface(ray, surface.as_ref());

        let mut closest: Option<RayHit> = None;

        for hit in surface_hits {
            if point_in_face(&self.brep, face_id, hit.uv)
                && (closest.is_none() || hit.t < closest.as_ref().unwrap().t)
            {
                let point = ray.at(hit.t);
                let normal = face_normal(&self.brep, face_id, hit.uv);
                closest = Some(RayHit::new(hit.t, point, normal, hit.uv, face_id));
            }
        }

        closest
    }

    /// Get a reference to the underlying BRep solid.
    pub fn brep(&self) -> &BRepSolid {
        &self.brep
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
    pub fn flatten(&self) -> (Vec<FlatBvhNode>, Vec<FaceId>) {
        let mut nodes = Vec::new();
        let mut faces = Vec::new();

        if let Some(root) = &self.root {
            flatten_node(root, &mut nodes, &mut faces);
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
fn flatten_node(node: &BvhNode, nodes: &mut Vec<FlatBvhNode>, faces: &mut Vec<FaceId>) -> usize {
    let idx = nodes.len();

    match node {
        BvhNode::Leaf {
            aabb,
            faces: leaf_faces,
        } => {
            let start = faces.len() as u32;
            let count = leaf_faces.len() as u32;
            faces.extend(leaf_faces.iter().copied());
            nodes.push((*aabb, true, start, count));
        }
        BvhNode::Internal { aabb, left, right } => {
            // Reserve space for this node
            nodes.push((*aabb, false, 0, 0));

            // Recursively flatten children
            let left_idx = flatten_node(left, nodes, faces);
            let right_idx = flatten_node(right, nodes, faces);

            // Update this node with child indices
            nodes[idx].2 = left_idx as u32;
            nodes[idx].3 = right_idx as u32;
        }
    }

    idx
}

/// Build a BVH node recursively using SAH.
fn build_node(face_data: &mut [(FaceId, Aabb3, vcad_kernel_math::Point3)]) -> BvhNode {
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
            faces: face_data.iter().map(|(id, _, _)| *id).collect(),
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
fn find_best_split(
    face_data: &[(FaceId, Aabb3, vcad_kernel_math::Point3)],
    bounds: &Aabb3,
) -> (usize, f64) {
    const NUM_BUCKETS: usize = 12;

    let extent = vcad_kernel_math::Vec3::new(
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
fn partition_faces(
    face_data: &mut [(FaceId, Aabb3, vcad_kernel_math::Point3)],
    axis: usize,
    pos: f64,
) -> usize {
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
    let d = vcad_kernel_math::Vec3::new(
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
}
