//! Two-level acceleration over placed vcad solids.
//!
//! The structure itself is [`kosm_render::Tlas`]; this is the vcad-shaped
//! door onto it. What stays here is the part that knows about `BRepSolid`:
//! building one BLAS per distinct solid, and the `vcad_kernel_math::Transform`
//! the rest of the kernel places things with.
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

use std::sync::Arc;

use vcad_kernel_booleans::bbox::Aabb3;
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::BRepSolid;

use crate::bvh::{BrepGeom, Bvh};
use crate::Ray;

/// A hit, tagged with the instance payload that produced it.
pub use kosm_render::InstanceHit;

/// A flattened TLAS node for GPU upload, mirroring
/// [`crate::bvh::FlatBvhNode`]: `(AABB, is_leaf, left_or_first,
/// right_or_count)`.
pub type FlatTlasNode = (Aabb3, bool, u32, u32);

/// `vcad_kernel_math::Transform` and `kosm_render::Transform` are the same
/// `tang::Mat4<f64>` wearing two names; this is the seam between them.
#[inline]
pub fn to_render_transform(t: &Transform) -> kosm_render::Transform {
    kosm_render::Transform { matrix: t.matrix }
}

/// One placed instance of a shared BLAS.
#[derive(Debug, Clone)]
pub struct Instance {
    inner: kosm_render::Instance<BrepGeom>,
    blas: Arc<Bvh>,
}

impl Instance {
    /// Place a shared BLAS with an object→world transform.
    ///
    /// Returns `None` when the BLAS is empty or the transform is singular
    /// (a ray cannot be mapped into a collapsed space).
    pub fn new(blas: Arc<Bvh>, to_world: Transform, payload: usize) -> Option<Self> {
        let inner = kosm_render::Instance::new(
            Arc::clone(blas.shared()),
            to_render_transform(&to_world),
            payload,
        )?;
        Some(Self { inner, blas })
    }

    /// Closest hit on this instance inside `(t_min, t_max)`, world units.
    pub fn trace_closest(&self, ray: &Ray, t_min: f64, t_max: f64) -> Option<InstanceHit> {
        self.inner.trace_closest(ray, t_min, t_max)
    }

    /// Any-hit against this instance over `(t_min, t_max)`, world units.
    pub fn occluded(&self, ray: &Ray, t_min: f64, t_max: f64) -> bool {
        self.inner.occluded(ray, t_min, t_max)
    }

    /// Place a shared BLAS at the identity transform.
    pub fn identity(blas: Arc<Bvh>, payload: usize) -> Option<Self> {
        Self::new(blas, Transform::identity(), payload)
    }

    /// The caller-supplied payload index.
    pub fn payload(&self) -> usize {
        self.inner.payload()
    }

    /// This instance's world-space bounds.
    pub fn world_aabb(&self) -> Aabb3 {
        let a = self.inner.world_aabb();
        Aabb3::new(a.min, a.max)
    }

    /// The instanced solid, when this instance is BRep-backed. `None` for a
    /// mesh BLAS, which has no analytic solid behind it.
    pub fn brep(&self) -> Option<&BRepSolid> {
        self.blas.brep()
    }
}

/// Top-level acceleration structure over placed [`Instance`]s.
#[derive(Debug, Clone, Default)]
pub struct Tlas {
    inner: kosm_render::Tlas<BrepGeom>,
    /// The same instances, in the same order, still wearing their vcad
    /// clothes — `instances()` is public API and callers want `brep()`.
    /// Cloning one is an `Arc` bump and a matrix.
    instances: Vec<Instance>,
}

impl Tlas {
    /// Build a TLAS over the given instances using the same SAH search the
    /// per-solid BVH uses, applied to instance world AABBs.
    pub fn build(instances: Vec<Instance>) -> Self {
        Self {
            inner: kosm_render::Tlas::build(
                instances.iter().map(|i| i.inner.clone()).collect(),
            ),
            instances,
        }
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
        self.inner.len()
    }

    /// Whether the structure holds no instances.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// The instances, in build order.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// World bounds of the whole scene, if non-empty.
    pub fn bounds(&self) -> Option<Aabb3> {
        self.inner.bounds().map(|a| Aabb3::new(a.min, a.max))
    }

    /// Closest hit in the scene, in world space.
    pub fn trace_closest(&self, ray: &Ray) -> Option<InstanceHit> {
        self.inner.trace_closest(ray)
    }

    /// Closest hit in the scene within the open interval `(t_min, t_max)`.
    ///
    /// `t_min` lets a caller skip the surface a ray just left without nudging
    /// the origin — and because it is pushed down into the BLAS rather than
    /// applied as a post-filter, a surface hidden behind the skipped one is
    /// still found.
    pub fn trace_closest_range(&self, ray: &Ray, t_min: f64, t_max: f64) -> Option<InstanceHit> {
        self.inner.trace_closest_range(ray, t_min, t_max)
    }

    /// Any-hit query: is anything in `(0, t_max)` along the ray?
    pub fn occluded(&self, ray: &Ray, t_max: f64) -> bool {
        self.inner.occluded(ray, t_max)
    }

    /// Any-hit query over the open interval `(t_min, t_max)`.
    pub fn occluded_range(&self, ray: &Ray, t_min: f64, t_max: f64) -> bool {
        self.inner.occluded_range(ray, t_min, t_max)
    }

    /// Flatten the top level for GPU upload, mirroring [`Bvh::flatten`].
    pub fn flatten(&self) -> (Vec<FlatTlasNode>, Vec<u32>) {
        let (nodes, indices) = self.inner.flatten();
        (
            nodes
                .into_iter()
                .map(|(a, leaf, x, y)| (Aabb3::new(a.min, a.max), leaf, x, y))
                .collect(),
            indices,
        )
    }
}

/// Build a [`Transform`] from a **column-major** 4×4 matrix laid out as 16
/// contiguous `f64`s — the wire format `render_scene` and the FFI use, and the
/// one Three.js / glTF produce. The translation therefore lives at indices
/// 12, 13, 14, not 3, 7, 11.
pub fn transform_from_column_major(m: &[f64]) -> Option<Transform> {
    kosm_render::transform_from_column_major(m).map(|t| Transform { matrix: t.matrix })
}
#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Vec3};
    use vcad_kernel_primitives::{make_cube, make_cylinder, make_sphere};

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
                    if let Some(h) = inst.trace_closest(&ray, 0.0, f64::INFINITY) {
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

    /// `t_min` must be a real interval search, not a filter applied to the
    /// single closest hit. A ray fired through two stacked cubes with `t_min`
    /// past the first one has to report the *second*, not report a miss.
    #[test]
    fn t_min_finds_the_surface_behind_the_skipped_one() {
        let blas = cube_blas();
        let tlas = Tlas::build(vec![
            Instance::identity(Arc::clone(&blas), 0).unwrap(),
            Instance::new(blas, Transform::translation(0.0, 0.0, 40.0), 1).unwrap(),
        ]);

        // Cube A spans z 0..10, cube B spans z 40..50. Ray starts at z = -5.
        let ray = Ray::new(Point3::new(5.0, 5.0, -5.0), Vec3::new(0.0, 0.0, 1.0));

        // Unbounded: nearest face of A, at t = 5.
        let near = tlas.trace_closest(&ray).unwrap();
        assert!((near.hit.t - 5.0).abs() < 1e-9);
        assert_eq!(near.payload, 0);

        // Skipping past A's front face finds its back face, not a miss.
        let mid = tlas.trace_closest_range(&ray, 5.5, f64::INFINITY).unwrap();
        assert!((mid.hit.t - 15.0).abs() < 1e-9, "t = {}", mid.hit.t);
        assert_eq!(mid.payload, 0);

        // Skipping past A entirely crosses into the *other instance*.
        let far = tlas.trace_closest_range(&ray, 20.0, f64::INFINITY).unwrap();
        assert!((far.hit.t - 45.0).abs() < 1e-9, "t = {}", far.hit.t);
        assert_eq!(far.payload, 1);

        // Same rule for the any-hit path.
        assert!(tlas.occluded_range(&ray, 0.0, f64::INFINITY));
        assert!(tlas.occluded_range(&ray, 20.0, f64::INFINITY));
        // Nothing lives strictly between the two cubes.
        assert!(!tlas.occluded_range(&ray, 20.0, 40.0));
    }

    /// The anisotropic-roughness path reads `dpdu` off the hit; an instance
    /// must carry it through (rotated into world space) rather than dropping
    /// it, or every anisotropic material silently falls back to isotropic.
    #[test]
    fn instance_carries_the_surface_tangent() {
        let solid = make_cylinder(6.0, 20.0, 0);
        let blas = Arc::new(Bvh::build(&solid));

        let ray = Ray::new(Point3::new(0.0, -50.0, 10.0), Vec3::new(0.0, 1.0, 0.0));
        let bare = blas.trace_closest(&ray).expect("hits the cylinder");
        assert!(
            bare.dpdu.is_some(),
            "precondition: the analytic surface reports a tangent"
        );

        // Identity instance: the tangent must arrive unchanged.
        let flat = Tlas::build(vec![Instance::identity(Arc::clone(&blas), 0).unwrap()]);
        let via = flat.trace_closest(&ray).expect("hits through the TLAS");
        let (a, b) = (bare.dpdu.unwrap(), via.hit.dpdu.expect("tangent survives"));
        assert!(
            (a - b).norm() < 1e-9,
            "identity changed the tangent: {a:?} vs {b:?}"
        );

        // Rotated instance: the tangent rotates with the geometry, so it stays
        // perpendicular to the normal and is no longer the local one.
        let rot = Transform::rotation_z(std::f64::consts::FRAC_PI_2);
        let spun = Tlas::build(vec![Instance::new(blas, rot, 0).unwrap()]);
        let hit = spun.trace_closest(&ray).expect("still hits");
        let t = hit.hit.dpdu.expect("tangent survives rotation");
        let n = hit.hit.normal.into_inner();
        assert!(
            t.normalize().dot(n).abs() < 1e-9,
            "tangent must stay perpendicular to the normal, got {}",
            t.normalize().dot(n)
        );
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
