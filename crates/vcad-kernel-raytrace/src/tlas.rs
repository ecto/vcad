//! Two-level acceleration structure: a TLAS over instances, each referencing
//! a per-solid BLAS (the [`Bvh`] in [`crate::bvh`]).
//!
//! # Why two levels
//!
//! A scene used to be traced by looping over every solid's BVH and keeping the
//! nearest hit — O(parts) per ray with no spatial culling *between* parts. The
//! TLAS is itself a SAH BVH, but built over per-instance world AABBs, so a ray
//! only descends into the handful of parts whose bounds it actually crosses.
//!
//! # Why instancing
//!
//! Rather than baking each instance's placement into a cloned `BRepSolid`, the
//! ray is transformed into the instance's local space and traced against the
//! *shared* BLAS. A linear pattern of 100 identical bolts builds one BVH, not
//! a hundred.
//!
//! The hit is mapped back to world space on the way out:
//!
//! - **`t`**: the local ray is renormalized, so `t_local` is measured in local
//!   units. With `L = |M⁻¹ d_world|` (and `d_world` a unit vector),
//!   `t_world = t_local / L`. For a rigid transform `L == 1` and this is a
//!   no-op; for a scaled instance it is the correction that keeps depth
//!   comparisons between instances meaningful.
//! - **normal**: transformed by the inverse transpose of the upper-left 3×3
//!   ([`Transform::apply_normal`]), which is the covector rule. This is also
//!   what makes mirrored (negative-determinant) instances come out right:
//!   `M⁻ᵀ n` preserves outward-ness under *any* invertible map, reproducing
//!   the face-orientation flip that baking a mirror transform into the BRep
//!   performs explicitly.

use std::sync::Arc;

use vcad_kernel_booleans::bbox::Aabb3;
use vcad_kernel_math::{Dir3, Point3, Transform};
use vcad_kernel_primitives::BRepSolid;

use crate::bvh::{item_bounds, sah_split, Bvh, SahItem};
use crate::{Ray, RayHit};

/// One placed instance of a shared BLAS.
#[derive(Debug, Clone)]
pub struct Instance {
    /// Shared bottom-level acceleration structure (the part's own BVH).
    blas: Arc<Bvh>,
    /// Object → world placement.
    to_world: Transform,
    /// World → object, precomputed for ray transformation.
    to_local: Transform,
    /// This instance's world-space bounds.
    world_aabb: Aabb3,
    /// Caller-supplied index, echoed back on every hit (material lookup, etc).
    payload: usize,
}

impl Instance {
    /// Place a shared BLAS with an object→world transform.
    ///
    /// Returns `None` when the BLAS is empty or the transform is singular
    /// (a ray cannot be mapped into a collapsed space).
    pub fn new(blas: Arc<Bvh>, to_world: Transform, payload: usize) -> Option<Self> {
        let local_bounds = blas.bounds()?;
        let to_local = to_world.inverse()?;
        let world_aabb = transform_aabb(&local_bounds, &to_world);
        Some(Self {
            blas,
            to_world,
            to_local,
            world_aabb,
            payload,
        })
    }

    /// Place a shared BLAS at the identity transform.
    pub fn identity(blas: Arc<Bvh>, payload: usize) -> Option<Self> {
        Self::new(blas, Transform::identity(), payload)
    }

    /// The caller-supplied payload index.
    pub fn payload(&self) -> usize {
        self.payload
    }

    /// This instance's world-space bounds.
    pub fn world_aabb(&self) -> Aabb3 {
        self.world_aabb
    }

    /// The instanced solid, when this instance is BRep-backed. `None` for a
    /// mesh BLAS, which has no analytic solid behind it.
    pub fn brep(&self) -> Option<&BRepSolid> {
        self.blas.brep()
    }

    /// Map a world ray into this instance's local space.
    ///
    /// Returns the local ray plus `L`, the length of the un-normalized local
    /// direction: `t_local = L * t_world`.
    fn local_ray(&self, ray: &Ray) -> Option<(Ray, f64)> {
        let origin = self.to_local.apply_point(&ray.origin);
        let dir = self.to_local.apply_vec(&ray.direction.into_inner());
        let len = dir.norm();
        if !(len.is_finite() && len > 0.0) {
            return None;
        }
        Some((Ray::new(origin, dir), len))
    }

    /// Closest hit on this instance nearer than `t_max` (world units).
    fn trace_closest(&self, ray: &Ray, t_max: f64) -> Option<InstanceHit> {
        let (local, len) = self.local_ray(ray)?;
        let local_hit = self.blas.trace_closest_limit(&local, t_max * len)?;

        let t = local_hit.t / len;
        let normal = self
            .to_world
            .apply_normal(&local_hit.normal.into_inner())
            .try_normalize()
            .map(Dir3::new_unchecked)
            .unwrap_or(local_hit.normal);

        Some(InstanceHit {
            hit: RayHit::new(t, ray.at(t), normal, local_hit.uv, local_hit.face_id),
            payload: self.payload,
        })
    }

    /// Any-hit test against this instance, in world units.
    fn occluded(&self, ray: &Ray, t_max: f64) -> bool {
        match self.local_ray(ray) {
            Some((local, len)) => self.blas.occluded(&local, t_max * len),
            None => false,
        }
    }
}

/// A hit, tagged with the instance payload that produced it.
#[derive(Debug, Clone, Copy)]
pub struct InstanceHit {
    /// The intersection, in world space.
    pub hit: RayHit,
    /// Payload of the instance that was hit.
    pub payload: usize,
}

/// A node of the top-level hierarchy.
#[derive(Debug, Clone)]
enum TlasNode {
    Leaf {
        aabb: Aabb3,
        instances: Vec<u32>,
    },
    Internal {
        aabb: Aabb3,
        left: Box<TlasNode>,
        right: Box<TlasNode>,
    },
}

impl TlasNode {
    fn aabb(&self) -> &Aabb3 {
        match self {
            TlasNode::Leaf { aabb, .. } | TlasNode::Internal { aabb, .. } => aabb,
        }
    }
}

/// A flattened TLAS node for GPU upload, mirroring
/// [`crate::bvh::FlatBvhNode`]: `(AABB, is_leaf, left_or_first,
/// right_or_count)`.
pub type FlatTlasNode = (Aabb3, bool, u32, u32);

/// Top-level acceleration structure over placed [`Instance`]s.
#[derive(Debug, Clone, Default)]
pub struct Tlas {
    root: Option<TlasNode>,
    instances: Vec<Instance>,
}

impl Tlas {
    /// Build a TLAS over the given instances using the same SAH search the
    /// per-solid BVH uses, applied to instance world AABBs.
    pub fn build(instances: Vec<Instance>) -> Self {
        if instances.is_empty() {
            return Self {
                root: None,
                instances,
            };
        }

        let mut items: Vec<SahItem<u32>> = instances
            .iter()
            .enumerate()
            .map(|(i, inst)| {
                let aabb = inst.world_aabb;
                let centroid = Point3::new(
                    (aabb.min.x + aabb.max.x) / 2.0,
                    (aabb.min.y + aabb.max.y) / 2.0,
                    (aabb.min.z + aabb.max.z) / 2.0,
                );
                (i as u32, aabb, centroid)
            })
            .collect();

        let root = Some(build_node(&mut items));
        Self { root, instances }
    }

    /// Build a TLAS from `(solid, transform, payload)` triples, sharing one
    /// BLAS per distinct `Arc<BRepSolid>` (compared by pointer identity, so
    /// callers that clone the `Arc` for each instance get the sharing for
    /// free).
    pub fn from_placed(placed: &[(Arc<BRepSolid>, Transform, usize)]) -> Self {
        let mut blas_cache: Vec<(*const BRepSolid, Arc<Bvh>)> = Vec::new();
        let mut instances = Vec::with_capacity(placed.len());

        for (solid, to_world, payload) in placed {
            let key = Arc::as_ptr(solid);
            let blas = match blas_cache.iter().find(|(k, _)| *k == key) {
                Some((_, blas)) => Arc::clone(blas),
                None => {
                    let blas = Arc::new(Bvh::build_shared(Arc::clone(solid)));
                    blas_cache.push((key, Arc::clone(&blas)));
                    blas
                }
            };
            if let Some(inst) = Instance::new(blas, to_world.clone(), *payload) {
                instances.push(inst);
            }
        }

        Self::build(instances)
    }

    /// Number of placed instances (empty or singular ones are dropped at
    /// build time, so this can be less than what was handed in).
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether the structure holds no instances.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The instances, in build order.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// World bounds of the whole scene, if non-empty.
    pub fn bounds(&self) -> Option<Aabb3> {
        self.root.as_ref().map(|n| *n.aabb())
    }

    /// Closest hit in the scene, in world space.
    pub fn trace_closest(&self, ray: &Ray) -> Option<InstanceHit> {
        let root = self.root.as_ref()?;
        let mut best: Option<InstanceHit> = None;
        let mut best_t = f64::INFINITY;
        self.closest_node(ray, root, &mut best, &mut best_t);
        best
    }

    fn closest_node(
        &self,
        ray: &Ray,
        node: &TlasNode,
        best: &mut Option<InstanceHit>,
        best_t: &mut f64,
    ) {
        let Some((t_min, _)) = ray.intersect_aabb(node.aabb()) else {
            return;
        };
        if t_min >= *best_t {
            return;
        }

        match node {
            TlasNode::Leaf { instances, .. } => {
                for &i in instances {
                    let inst = &self.instances[i as usize];
                    if let Some(found) = inst.trace_closest(ray, *best_t) {
                        if found.hit.t < *best_t {
                            *best_t = found.hit.t;
                            *best = Some(found);
                        }
                    }
                }
            }
            TlasNode::Internal { left, right, .. } => {
                // Descend into the nearer child first so the far one is more
                // likely to be culled outright.
                let lt = ray.intersect_aabb(left.aabb()).map(|(t, _)| t);
                let rt = ray.intersect_aabb(right.aabb()).map(|(t, _)| t);
                let (first, second) = match (lt, rt) {
                    (Some(l), Some(r)) if r < l => (Some(right), Some(left)),
                    (Some(_), Some(_)) => (Some(left), Some(right)),
                    (Some(_), None) => (Some(left), None),
                    (None, Some(_)) => (Some(right), None),
                    (None, None) => (None, None),
                };
                if let Some(n) = first {
                    self.closest_node(ray, n, best, best_t);
                }
                if let Some(n) = second {
                    self.closest_node(ray, n, best, best_t);
                }
            }
        }
    }

    /// Any-hit query: is anything in `(0, t_max)` along the ray?
    ///
    /// Returns on the first blocker found, at both levels — the traversal
    /// shadow rays want, and strictly cheaper than ranking hits.
    pub fn occluded(&self, ray: &Ray, t_max: f64) -> bool {
        match self.root {
            Some(ref root) => self.occluded_node(ray, root, t_max),
            None => false,
        }
    }

    fn occluded_node(&self, ray: &Ray, node: &TlasNode, t_max: f64) -> bool {
        let Some((t_min, _)) = ray.intersect_aabb(node.aabb()) else {
            return false;
        };
        if t_min >= t_max {
            return false;
        }
        match node {
            TlasNode::Leaf { instances, .. } => instances
                .iter()
                .any(|&i| self.instances[i as usize].occluded(ray, t_max)),
            TlasNode::Internal { left, right, .. } => {
                self.occluded_node(ray, left, t_max) || self.occluded_node(ray, right, t_max)
            }
        }
    }

    /// Flatten the top level for GPU upload, mirroring [`Bvh::flatten`].
    ///
    /// Returns the node array plus the instance indices in leaf order; a leaf
    /// node's `(left_or_first, right_or_count)` slices into that index list.
    /// The per-instance transforms and BLAS offsets a GPU traversal would also
    /// need are not synthesized here — this exists so the top level has the
    /// same flat layout the BLAS already uploads in, leaving the GPU path a
    /// mechanical addition rather than a redesign.
    pub fn flatten(&self) -> (Vec<FlatTlasNode>, Vec<u32>) {
        let mut nodes = Vec::new();
        let mut indices = Vec::new();
        if let Some(root) = &self.root {
            flatten_node(root, &mut nodes, &mut indices);
        }
        (nodes, indices)
    }
}

fn flatten_node(node: &TlasNode, nodes: &mut Vec<FlatTlasNode>, indices: &mut Vec<u32>) -> usize {
    let idx = nodes.len();
    match node {
        TlasNode::Leaf { aabb, instances } => {
            let start = indices.len() as u32;
            indices.extend(instances.iter().copied());
            nodes.push((*aabb, true, start, instances.len() as u32));
        }
        TlasNode::Internal { aabb, left, right } => {
            nodes.push((*aabb, false, 0, 0));
            let l = flatten_node(left, nodes, indices);
            let r = flatten_node(right, nodes, indices);
            nodes[idx].2 = l as u32;
            nodes[idx].3 = r as u32;
        }
    }
    idx
}

/// Recursively build the top level. Leaves hold a small number of instances,
/// since each one is itself an expensive BLAS descent.
fn build_node(items: &mut [SahItem<u32>]) -> TlasNode {
    let bounds = item_bounds(items);

    if items.len() <= 2 {
        return TlasNode::Leaf {
            aabb: bounds,
            instances: items.iter().map(|(i, _, _)| *i).collect(),
        };
    }

    let mid = sah_split(items, &bounds);
    let (left, right) = items.split_at_mut(mid);
    TlasNode::Internal {
        aabb: bounds,
        left: Box::new(build_node(left)),
        right: Box::new(build_node(right)),
    }
}

/// World AABB of a local AABB under a transform: transform all eight corners
/// and re-bound. (Tighter than transforming min/max alone, which is wrong for
/// anything but an axis-aligned scale.)
fn transform_aabb(local: &Aabb3, to_world: &Transform) -> Aabb3 {
    let mut out = Aabb3::empty();
    for i in 0..8 {
        let corner = Point3::new(
            if i & 1 == 0 { local.min.x } else { local.max.x },
            if i & 2 == 0 { local.min.y } else { local.max.y },
            if i & 4 == 0 { local.min.z } else { local.max.z },
        );
        out.include_point(&to_world.apply_point(&corner));
    }
    out
}

/// Build a [`Transform`] from a **column-major** 4×4 matrix laid out as 16
/// contiguous `f64`s — the wire format `render_scene` and the FFI use, and the
/// one Three.js / glTF produce. The translation therefore lives at indices
/// 12, 13, 14, not 3, 7, 11.
pub fn transform_from_column_major(m: &[f64]) -> Option<Transform> {
    if m.len() < 16 {
        return None;
    }
    // `tang::Mat4::new` takes its arguments in *row-major* order, so feeding
    // `m[0], m[4], m[8], m[12]` as the first row is what reads the input as
    // column-major. Same convention the old `transform_brep` used.
    Some(Transform {
        matrix: tang::Mat4::new(
            m[0], m[4], m[8], m[12], m[1], m[5], m[9], m[13], m[2], m[6], m[10], m[14], m[3], m[7],
            m[11], m[15],
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Vec3;
    use vcad_kernel_primitives::{make_cube, make_sphere};

    fn cube_blas() -> Arc<Bvh> {
        Arc::new(Bvh::build(&make_cube(10.0, 10.0, 10.0)))
    }

    #[test]
    fn identity_instance_matches_bare_bvh() {
        let solid = make_cube(10.0, 10.0, 10.0);
        let bvh = Bvh::build(&solid);
        let tlas = Tlas::build(vec![Instance::identity(Arc::new(bvh.clone()), 0).unwrap()]);

        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        let direct = bvh.trace_closest(&ray).unwrap();
        let through = tlas.trace_closest(&ray).unwrap();

        assert!((direct.t - through.hit.t).abs() < 1e-12);
        assert!((direct.normal.dot(through.hit.normal) - 1.0).abs() < 1e-12);
        assert_eq!(through.payload, 0);
    }

    #[test]
    fn translated_instance_shares_one_blas() {
        let blas = cube_blas();
        let a = Instance::new(Arc::clone(&blas), Transform::identity(), 0).unwrap();
        let b = Instance::new(
            Arc::clone(&blas),
            Transform::translation(100.0, 0.0, 0.0),
            1,
        )
        .unwrap();
        // One allocation backing both instances.
        assert_eq!(Arc::strong_count(&blas), 3);

        let tlas = Tlas::build(vec![a, b]);

        let near = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(tlas.trace_closest(&near).unwrap().payload, 0);

        let far = Ray::new(Point3::new(105.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        let hit = tlas.trace_closest(&far).unwrap();
        assert_eq!(hit.payload, 1);
        assert!((hit.hit.t - 5.0).abs() < 1e-9, "t = {}", hit.hit.t);
        assert!((hit.hit.point.x - 105.0).abs() < 1e-9);
    }

    #[test]
    fn scaled_instance_reports_world_t() {
        let blas = cube_blas();
        // 2x scale: the cube spans 0..20 in world.
        let inst = Instance::new(Arc::clone(&blas), Transform::scale(2.0, 2.0, 2.0), 0).unwrap();
        let tlas = Tlas::build(vec![inst]);

        let ray = Ray::new(Point3::new(10.0, 10.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        let hit = tlas.trace_closest(&ray).unwrap();
        assert!((hit.hit.t - 5.0).abs() < 1e-9, "t = {}", hit.hit.t);
        assert!(hit.hit.point.z.abs() < 1e-9);

        // Exit face at z = 20 → t = 25.
        let inside = Ray::new(Point3::new(10.0, 10.0, 10.0), Vec3::new(0.0, 0.0, 1.0));
        let exit = tlas.trace_closest(&inside).unwrap();
        assert!((exit.hit.t - 10.0).abs() < 1e-9, "t = {}", exit.hit.t);
    }

    #[test]
    fn rotated_instance_normal_is_rotated() {
        let blas = cube_blas();
        // Rotate 90° about Z: the +X face (normal +X) becomes the +Y face.
        let rot = Transform::rotation_z(std::f64::consts::FRAC_PI_2);
        let tlas = Tlas::build(vec![Instance::new(blas, rot, 0).unwrap()]);

        // Cube now spans x in -10..0, y in 0..10. Shoot -Y at the far face.
        let ray = Ray::new(Point3::new(-5.0, 50.0, 5.0), Vec3::new(0.0, -1.0, 0.0));
        let hit = tlas.trace_closest(&ray).unwrap();
        let n = hit.hit.normal.into_inner();
        assert!(n.x.abs() < 1e-9 && n.z.abs() < 1e-9, "normal {n:?}");
        assert!(n.y.abs() > 0.99);
    }

    /// A mirrored instance must produce the same outward normals as baking the
    /// mirror into the BRep does (which flips face orientation explicitly).
    /// The inverse-transpose rule is what makes that fall out.
    #[test]
    fn mirrored_instance_normals_point_outward() {
        let blas = cube_blas();
        let mirror = Transform::scale(-1.0, 1.0, 1.0);
        let tlas = Tlas::build(vec![Instance::new(blas, mirror, 0).unwrap()]);
        assert!(tlas.bounds().unwrap().min.x < -9.0);

        // Mirrored cube spans x in -10..0. Hit its -X face from outside.
        let ray = Ray::new(Point3::new(-50.0, 5.0, 5.0), Vec3::new(1.0, 0.0, 0.0));
        let hit = tlas.trace_closest(&ray).unwrap();
        assert!((hit.hit.point.x + 10.0).abs() < 1e-9);
        let n = hit.hit.normal.into_inner();
        // Outward at x = -10 means pointing in -X.
        assert!(n.x < -0.99, "expected outward -X, got {n:?}");
    }

    #[test]
    fn occluded_agrees_with_closest_hit() {
        let blas = cube_blas();
        let tlas = Tlas::build(vec![
            Instance::identity(Arc::clone(&blas), 0).unwrap(),
            Instance::new(blas, Transform::translation(0.0, 0.0, 50.0), 1).unwrap(),
        ]);

        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(tlas.occluded(&ray, f64::INFINITY));
        assert!(tlas.occluded(&ray, 10.0));
        // Nothing before t = 1: the first face is at t = 5.
        assert!(!tlas.occluded(&ray, 1.0));

        let miss = Ray::new(Point3::new(500.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(!tlas.occluded(&miss, f64::INFINITY));
        assert!(tlas.trace_closest(&miss).is_none());
    }

    /// Exhaustive cross-check: for a grid of rays, the TLAS must agree with a
    /// linear scan over the same instances' BLASes.
    #[test]
    fn tlas_matches_linear_scan() {
        let cube = Arc::new(make_cube(6.0, 6.0, 6.0));
        let sphere = Arc::new(make_sphere(4.0, 0));

        let mut placed = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                let t = Transform::translation(i as f64 * 20.0, j as f64 * 20.0, 0.0);
                let solid = if (i + j) % 2 == 0 { &cube } else { &sphere };
                placed.push((Arc::clone(solid), t, i * 4 + j));
            }
        }
        let tlas = Tlas::from_placed(&placed);
        assert_eq!(tlas.len(), 16);

        for px in 0..24 {
            for py in 0..24 {
                let origin = Point3::new(px as f64 * 3.0 - 5.0, py as f64 * 3.0 - 5.0, -50.0);
                let ray = Ray::new(origin, Vec3::new(0.0, 0.0, 1.0));

                let mut expect: Option<(f64, usize)> = None;
                for inst in tlas.instances() {
                    if let Some(h) = inst.trace_closest(&ray, f64::INFINITY) {
                        if expect.is_none_or(|(t, _)| h.hit.t < t) {
                            expect = Some((h.hit.t, h.payload));
                        }
                    }
                }

                match (expect, tlas.trace_closest(&ray)) {
                    (None, None) => {}
                    (Some((t, p)), Some(got)) => {
                        assert!((t - got.hit.t).abs() < 1e-9, "t {t} vs {}", got.hit.t);
                        assert_eq!(p, got.payload);
                    }
                    (a, b) => panic!("mismatch at ({px},{py}): {a:?} vs {}", b.is_some()),
                }
            }
        }
    }

    #[test]
    fn from_placed_shares_blas_per_solid() {
        let cube = Arc::new(make_cube(1.0, 1.0, 1.0));
        let placed: Vec<_> = (0..50)
            .map(|i| {
                (
                    Arc::clone(&cube),
                    Transform::translation(i as f64 * 3.0, 0.0, 0.0),
                    i,
                )
            })
            .collect();
        let tlas = Tlas::from_placed(&placed);
        assert_eq!(tlas.len(), 50);
        // All 50 instances point at the same BLAS allocation.
        let first: &Bvh = tlas.instances()[0].blas.as_ref();
        assert!(tlas
            .instances()
            .iter()
            .all(|i| std::ptr::eq(i.blas.as_ref(), first)));
    }

    #[test]
    fn empty_tlas_is_inert() {
        let tlas = Tlas::build(Vec::new());
        assert!(tlas.is_empty());
        assert!(tlas.bounds().is_none());
        let ray = Ray::new(Point3::origin(), Vec3::new(0.0, 0.0, 1.0));
        assert!(tlas.trace_closest(&ray).is_none());
        assert!(!tlas.occluded(&ray, f64::INFINITY));
    }

    #[test]
    fn flatten_round_trips_leaf_indices() {
        let cube = Arc::new(make_cube(2.0, 2.0, 2.0));
        let placed: Vec<_> = (0..9)
            .map(|i| {
                (
                    Arc::clone(&cube),
                    Transform::translation(i as f64 * 5.0, 0.0, 0.0),
                    i,
                )
            })
            .collect();
        let tlas = Tlas::from_placed(&placed);
        let (nodes, indices) = tlas.flatten();

        assert!(!nodes.is_empty());
        let mut seen: Vec<u32> = indices.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..9u32).collect::<Vec<_>>());

        // Every leaf slice is in range; every internal child index is valid.
        for (_, is_leaf, a, b) in &nodes {
            if *is_leaf {
                assert!((*a + *b) as usize <= indices.len());
            } else {
                assert!((*a as usize) < nodes.len() && (*b as usize) < nodes.len());
            }
        }
    }
}
