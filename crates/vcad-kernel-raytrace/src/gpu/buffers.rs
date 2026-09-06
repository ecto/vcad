//! Packing BRep solids into the buffers kosm-render's geometry seam binds.

use bytemuck::{Pod, Zeroable};
use kosm_render::gpu::{GpuAreaLight, GpuMaterial};
use vcad_kernel_booleans::bbox::face_aabb;
use vcad_kernel_geom::{Surface, SurfaceKind};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_topo::FaceId;

use crate::bvh::{BrepBvh, Bvh};
use crate::trim;

/// Maximum number of surfaces supported in a single scene.
pub const MAX_SURFACES: usize = 1024;

/// Maximum number of faces supported in a single scene.
pub const MAX_FACES: usize = 4096;

/// Maximum BVH nodes.
///
/// Raised from the original 8192 when `vcad-render --photoreal --gpu` started
/// merging one mesh BLAS per solid into a single tree: a real assembly is
/// hundreds of thousands of triangles and its BVH has roughly one node per
/// two of them, so 8192 refused every scene bigger than a bracket. Nothing in
/// the shader is sized by this — the node array is a storage buffer sized
/// from the data — so the ceiling that actually matters is the device's
/// `max_storage_buffer_binding_size`, which [`GpuScene::validate`] checks
/// separately. This is the "obviously absurd" guard, not the real limit.
pub const MAX_BVH_NODES: usize = 4_000_000;

/// Deepest root-to-leaf path the WGSL tracer can walk.
///
/// `trace_bvh` holds its traversal stack in a fixed `array<u32, 64>` and
/// *silently drops* a push that would overflow it — geometry simply
/// disappears from the render. [`GpuScene::validate`] measures the packed
/// tree against this so an over-deep scene is a message instead of a hole.
pub const MAX_TRAVERSAL_DEPTH: usize = 64;

/// Maximum trim loop vertices.
pub const MAX_TRIM_VERTS: usize = 32768;

/// Maximum triangles in a single mesh-backed scene.
///
/// Deliberately far above [`MAX_FACES`]: those caps are sized for a BRep,
/// where a "face" is a whole trimmed surface and a few thousand is a large
/// part. A tessellated mesh counts in the hundreds of thousands for the same
/// object, so the mesh path gets its own ceiling. At
/// [`size_of::<GpuSurface>`](GpuSurface) = 144 bytes per triangle this bounds
/// the surface buffer at ~144 MB, which is above the 128 MB that
/// `maxStorageBufferBindingSize` defaults to — so a mesh near this cap can
/// still be refused by the driver. The limit is here to turn an absurd mesh
/// into a message rather than an OOM, not to certify that everything under it
/// uploads.
pub const MAX_MESH_TRIANGLES: usize = 1_000_000;

/// GPU-compatible surface representation.
///
/// Each surface type is packed into 32 floats:
/// - Type discriminant (as f32)
/// - Surface-specific parameters
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuSurface {
    /// Surface type: 0=Plane, 1=Cylinder, 2=Sphere, 3=Cone, 4=Torus,
    /// 5=Bilinear, 6=BSpline, 7=Triangle. See [`GpuSurface::type_name`].
    pub surface_type: u32,
    /// Padding for alignment
    pub _pad: [u32; 3],
    /// Surface parameters (32 floats, interpretation depends on type)
    pub params: [f32; 32],
}

impl GpuSurface {
    /// Create a GPU surface from a kernel surface.
    pub fn from_surface(surface: &dyn Surface) -> Self {
        let mut params = [0.0f32; 32];

        let surface_type = match surface.surface_type() {
            SurfaceKind::Plane => {
                if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                    // origin (3), x_dir (3), y_dir (3), normal (3)
                    params[0] = plane.origin.x as f32;
                    params[1] = plane.origin.y as f32;
                    params[2] = plane.origin.z as f32;
                    params[3] = plane.x_dir.x as f32;
                    params[4] = plane.x_dir.y as f32;
                    params[5] = plane.x_dir.z as f32;
                    params[6] = plane.y_dir.x as f32;
                    params[7] = plane.y_dir.y as f32;
                    params[8] = plane.y_dir.z as f32;
                    params[9] = plane.normal_dir.x as f32;
                    params[10] = plane.normal_dir.y as f32;
                    params[11] = plane.normal_dir.z as f32;
                }
                0
            }
            SurfaceKind::Cylinder => {
                if let Some(cyl) = surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
                {
                    // center (3), axis (3), ref_dir (3), radius (1)
                    params[0] = cyl.center.x as f32;
                    params[1] = cyl.center.y as f32;
                    params[2] = cyl.center.z as f32;
                    params[3] = cyl.axis.x as f32;
                    params[4] = cyl.axis.y as f32;
                    params[5] = cyl.axis.z as f32;
                    params[6] = cyl.ref_dir.x as f32;
                    params[7] = cyl.ref_dir.y as f32;
                    params[8] = cyl.ref_dir.z as f32;
                    params[9] = cyl.radius as f32;
                }
                1
            }
            SurfaceKind::Sphere => {
                if let Some(sph) = surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::SphereSurface>()
                {
                    // center (3), radius (1), ref_dir (3), axis (3)
                    params[0] = sph.center.x as f32;
                    params[1] = sph.center.y as f32;
                    params[2] = sph.center.z as f32;
                    params[3] = sph.radius as f32;
                    params[4] = sph.ref_dir.x as f32;
                    params[5] = sph.ref_dir.y as f32;
                    params[6] = sph.ref_dir.z as f32;
                    params[7] = sph.axis.x as f32;
                    params[8] = sph.axis.y as f32;
                    params[9] = sph.axis.z as f32;
                }
                2
            }
            SurfaceKind::Cone => {
                if let Some(cone) = surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::ConeSurface>()
                {
                    // apex (3), axis (3), ref_dir (3), half_angle (1)
                    params[0] = cone.apex.x as f32;
                    params[1] = cone.apex.y as f32;
                    params[2] = cone.apex.z as f32;
                    params[3] = cone.axis.x as f32;
                    params[4] = cone.axis.y as f32;
                    params[5] = cone.axis.z as f32;
                    params[6] = cone.ref_dir.x as f32;
                    params[7] = cone.ref_dir.y as f32;
                    params[8] = cone.ref_dir.z as f32;
                    params[9] = cone.half_angle as f32;
                }
                3
            }
            SurfaceKind::Torus => {
                if let Some(torus) = surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::TorusSurface>()
                {
                    // center (3), axis (3), ref_dir (3), major_radius (1), minor_radius (1)
                    params[0] = torus.center.x as f32;
                    params[1] = torus.center.y as f32;
                    params[2] = torus.center.z as f32;
                    params[3] = torus.axis.x as f32;
                    params[4] = torus.axis.y as f32;
                    params[5] = torus.axis.z as f32;
                    params[6] = torus.ref_dir.x as f32;
                    params[7] = torus.ref_dir.y as f32;
                    params[8] = torus.ref_dir.z as f32;
                    params[9] = torus.major_radius as f32;
                    params[10] = torus.minor_radius as f32;
                }
                4
            }
            SurfaceKind::Bilinear => {
                if let Some(bil) = surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::BilinearSurface>()
                {
                    // p00 (3), p10 (3), p01 (3), p11 (3)
                    params[0] = bil.p00.x as f32;
                    params[1] = bil.p00.y as f32;
                    params[2] = bil.p00.z as f32;
                    params[3] = bil.p10.x as f32;
                    params[4] = bil.p10.y as f32;
                    params[5] = bil.p10.z as f32;
                    params[6] = bil.p01.x as f32;
                    params[7] = bil.p01.y as f32;
                    params[8] = bil.p01.z as f32;
                    params[9] = bil.p11.x as f32;
                    params[10] = bil.p11.y as f32;
                    params[11] = bil.p11.z as f32;
                }
                5
            }
            SurfaceKind::BSpline => {
                // B-spline not directly supported on GPU - use tessellation fallback
                6
            }
        };

        Self {
            surface_type,
            _pad: [0; 3],
            params,
        }
    }

    /// Pack one mesh triangle into a surface record.
    ///
    /// A triangle is not a parametric surface, but the `params` block is 32
    /// floats of otherwise-idle space and a triangle needs 19 of them, so it
    /// rides in the surface array rather than in a vertex buffer of its own.
    /// That choice is what keeps the mesh path inside the browser's cap of
    /// ten storage-buffer bindings — see the binding block at the top of
    /// `shaders/raytrace.wgsl`. The cost is that shared vertices are stored
    /// once per incident triangle: 144 bytes per triangle flat, so a 500k-tri
    /// mesh is ~72 MB of surface buffer. On unified memory that is a fair
    /// trade for not forking the shader; a native-only indexed path would be
    /// roughly 4x leaner and is the obvious follow-up if it ever bites.
    ///
    /// Layout, mirrored by `intersect_triangle` and `compute_normal` in the
    /// WGSL:
    ///
    /// ```text
    ///   [0..3)   v0          [9..12)  n0
    ///   [3..6)   v1          [12..15) n1
    ///   [6..9)   v2          [15..18) n2
    ///   [18]     1.0 when the normals above are real, 0.0 otherwise
    /// ```
    pub fn triangle(tri: &crate::bvh::FlatTriangle) -> Self {
        let mut params = [0.0f32; 32];
        for (i, p) in tri.positions.iter().enumerate() {
            params[i * 3..i * 3 + 3].copy_from_slice(p);
        }
        // Absent normals stay zero AND are flagged, because zero is a
        // legitimate-looking vector the shader would happily normalize into
        // NaN. The flag is what makes the shader take the geometric-normal
        // fallback instead — the same fallback `MeshGeom::test` takes on the
        // CPU, so a normal-less mesh shades identically in both renderers.
        if let Some(normals) = &tri.normals {
            for (i, n) in normals.iter().enumerate() {
                params[9 + i * 3..12 + i * 3].copy_from_slice(n);
            }
            params[18] = 1.0;
        }

        Self {
            surface_type: SURFACE_TYPE_TRIANGLE,
            _pad: [0; 3],
            params,
        }
    }

    /// Whether the WGSL `intersect_surface` switch has a case for this
    /// surface type.
    ///
    /// Types 0-4 (plane, cylinder, sphere, cone, torus) are traced
    /// analytically and type 7 (triangle) by Möller-Trumbore. Bilinear (5)
    /// and B-spline (6) are packed by [`Self::from_surface`] but fall into
    /// the shader's `default` arm, which returns a miss — such a face would
    /// silently vanish from the render, so [`GpuScene::from_brep`] rejects a
    /// scene containing one.
    pub fn is_gpu_traceable(&self) -> bool {
        self.surface_type <= SURFACE_TYPE_TORUS || self.surface_type == SURFACE_TYPE_TRIANGLE
    }

    /// Human-readable name of a packed surface type code.
    pub fn type_name(surface_type: u32) -> &'static str {
        match surface_type {
            0 => "Plane",
            1 => "Cylinder",
            2 => "Sphere",
            3 => "Cone",
            4 => "Torus",
            5 => "Bilinear",
            6 => "BSpline",
            7 => "Triangle",
            _ => "Unknown",
        }
    }
}

/// Highest surface type code the WGSL `intersect_surface` switch handles
/// analytically.
const SURFACE_TYPE_TORUS: u32 = 4;

/// Surface type code for a mesh triangle.
///
/// Deliberately past B-spline (6) rather than reusing a hole: the codes 0-6
/// are a one-to-one image of [`SurfaceKind`] and a triangle is not one of
/// those, so it gets its own code and the mapping stays honest.
pub const SURFACE_TYPE_TRIANGLE: u32 = 7;

/// GPU-compatible face representation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuFace {
    /// Index into surface array.
    pub surface_idx: u32,
    /// Face orientation: 0=forward, 1=reversed.
    pub orientation: u32,
    /// Start index in trim vertex array (outer loop).
    pub trim_start: u32,
    /// Number of trim vertices (outer loop).
    pub trim_count: u32,
    /// AABB min.
    pub aabb_min: [f32; 4], // padded for alignment
    /// AABB max.
    pub aabb_max: [f32; 4], // padded for alignment
    /// Start index for inner loops (holes) in trim vertex array.
    pub inner_start: u32,
    /// Total number of vertices in all inner loops.
    pub inner_count: u32,
    /// Number of inner loops (holes).
    pub inner_loop_count: u32,
    /// Start index in inner_loop_descs for this face's inner loop sizes.
    pub inner_desc_start: u32,
    /// Index into material array.
    pub material_idx: u32,
    /// Padding for 16-byte alignment.
    pub _pad2: [u32; 3],
}

/// GPU-compatible BVH node.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuBvhNode {
    /// AABB min.
    pub aabb_min: [f32; 4],
    /// AABB max.
    pub aabb_max: [f32; 4],
    /// For leaves: start face index. For internal: left child index.
    pub left_or_first: u32,
    /// For leaves: face count. For internal: right child index.
    pub right_or_count: u32,
    /// Is this a leaf node? (0 = internal, 1 = leaf)
    pub is_leaf: u32,
    /// Padding.
    pub _pad: u32,
}

/// GPU-compatible 2D point for trim loops.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuVec2 {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
}

/// Maximum inner loop descriptors.
#[allow(dead_code)]
pub const MAX_INNER_LOOPS: usize = 8192;

/// Scene data prepared for GPU upload.
#[derive(Clone)]
pub struct GpuScene {
    /// Surfaces.
    pub surfaces: Vec<GpuSurface>,
    /// Faces.
    pub faces: Vec<GpuFace>,
    /// Materials.
    pub materials: Vec<GpuMaterial>,
    /// BVH nodes.
    pub bvh_nodes: Vec<GpuBvhNode>,
    /// Trim loop vertices (UV coordinates) - outer and inner loops.
    pub trim_verts: Vec<GpuVec2>,
    /// Inner loop descriptors: (start_offset, vertex_count) relative to face's inner_start.
    /// Stored as pairs of u32: [start0, count0, start1, count1, ...]
    pub inner_loop_descs: Vec<u32>,
    /// Mapping from FaceId to GPU face index.
    pub face_index_map: std::collections::HashMap<FaceId, u32>,
    /// Optional lat-long HDR environment. `None` uses the analytic studio
    /// gradient, matching `pathtrace::Environment::default()`.
    pub environment: Option<crate::pathtrace::GpuEnvPack>,
    /// Area lights (softboxes) illuminating the scene, derived from the scene
    /// bounds via [`crate::pathtrace::studio_rig`] — the same rig the CPU
    /// renderer uses, so highlights match between the two.
    pub lights: Vec<GpuAreaLight>,
}

/// Error building GPU scene.
#[derive(Debug)]
pub enum GpuSceneError {
    /// Too many surfaces (exceeds GPU limit).
    TooManySurfaces(usize),
    /// Too many faces (exceeds GPU limit).
    TooManyFaces(usize),
    /// Too many BVH nodes.
    TooManyBvhNodes(usize),
    /// Too many trim vertices.
    TooManyTrimVerts(usize),
    /// The packed hierarchy is deeper than a WGSL traversal stack can hold.
    ///
    /// Carries kosm-render's own message, since the limit and the stack are
    /// both the renderer's.
    TreeTooDeep(String),
    /// A surface kind the WGSL tracer has no intersection case for.
    ///
    /// Carries the surface's index in `brep.geometry.surfaces` and the packed
    /// type name, so the caller can say *which* geometry it cannot render
    /// instead of handing back a blank frame.
    UnsupportedSurface {
        /// Index into `brep.geometry.surfaces`.
        index: usize,
        /// Packed surface type code.
        surface_type: u32,
        /// Human-readable name of that type.
        name: &'static str,
    },
    /// Too many mesh triangles (exceeds [`MAX_MESH_TRIANGLES`]).
    TooManyTriangles(usize),
    /// [`GpuScene::from_mesh_bvh`] was handed a BRep-backed BVH.
    NotAMeshBvh,
    /// The packed BVH is deeper than the shader's traversal stack
    /// ([`MAX_TRAVERSAL_DEPTH`]).
    BvhTooDeep(usize),
    /// The largest storage buffer this scene needs is bigger than the
    /// device's `max_storage_buffer_binding_size`.
    ExceedsDeviceBinding {
        /// Bytes the largest single binding would need.
        bytes: u64,
        /// The device's limit.
        cap: u64,
    },
}

impl std::fmt::Display for GpuSceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManySurfaces(n) => {
                write!(f, "too many surfaces: {} (max {})", n, MAX_SURFACES)
            }
            Self::TooManyFaces(n) => write!(f, "too many faces: {} (max {})", n, MAX_FACES),
            Self::TooManyBvhNodes(n) => {
                write!(f, "too many BVH nodes: {} (max {})", n, MAX_BVH_NODES)
            }
            Self::TooManyTrimVerts(n) => {
                write!(f, "too many trim vertices: {} (max {})", n, MAX_TRIM_VERTS)
            }
            Self::TreeTooDeep(msg) => write!(f, "{msg}"),
            Self::UnsupportedSurface {
                index,
                surface_type,
                name,
            } => write!(
                f,
                "surface {index} is a {name} (type {surface_type}), which the GPU tracer \
                 cannot intersect; faces on it would render as empty space"
            ),
            Self::TooManyTriangles(n) => write!(
                f,
                "too many mesh triangles: {n} (max {MAX_MESH_TRIANGLES}) -- \
                 each triangle costs one {tri_bytes}-byte surface record, so \
                 this mesh would need {mb} MB of surface buffer alone",
                tri_bytes = std::mem::size_of::<GpuSurface>(),
                mb = n * std::mem::size_of::<GpuSurface>() / (1024 * 1024),
            ),
            Self::BvhTooDeep(d) => write!(
                f,
                "merged BVH is {d} levels deep (max {MAX_TRAVERSAL_DEPTH}) -- the \
                 GPU tracer's traversal stack cannot hold it and would silently \
                 drop geometry. Render fewer parts per pass, or fall back to the \
                 CPU tracer (drop --gpu)"
            ),
            Self::ExceedsDeviceBinding { bytes, cap } => write!(
                f,
                "scene needs a {} MB storage binding but this adapter caps one at \
                 {} MB -- split the render or drop --gpu to trace on the CPU",
                bytes / (1024 * 1024),
                cap / (1024 * 1024),
            ),
            Self::NotAMeshBvh => write!(
                f,
                "from_mesh_bvh needs a BVH built by Bvh::build_mesh; this one is \
                 BRep-backed. Use GpuScene::from_brep, which traces its surfaces \
                 analytically rather than a tessellation of them"
            ),
        }
    }
}

impl std::error::Error for GpuSceneError {}

/// Build the studio softbox rig for a scene, sized to its BVH root bounds.
///
/// Delegates to [`crate::pathtrace::studio_rig`] — the SAME function
/// `vcad-render --photoreal` calls — so the viewport and the CPU renderer are
/// lit by an identical rig rather than by two hand-tuned approximations.
/// Convert a flattened BVH into the shader's node layout.
///
/// An empty tree still yields one (zeroed) node: WebGPU rejects a zero-sized
/// storage buffer, and a zero AABB is missed by every ray, so the empty scene
/// renders as pure background rather than failing to bind.
/// Refuse a hierarchy the shader's fixed traversal stack cannot walk.
///
/// The stack depth is the renderer's, so the check is kosm-render's too — this
/// only re-spells vcad's `Aabb3`-flavoured nodes in the renderer's `Aabb` so
/// it can measure them, and re-wraps the message as a `GpuSceneError`.
fn check_tree_depth(flat_nodes: &[crate::bvh::FlatBvhNode]) -> Result<(), GpuSceneError> {
    let nodes: Vec<kosm_render::bvh::FlatBvhNode> = flat_nodes
        .iter()
        .map(|(a, leaf, x, y)| (kosm_render::Aabb::new(a.min, a.max), *leaf, *x, *y))
        .collect();
    kosm_render::gpu::validate_tree_depth(&nodes)
        .map_err(|e| GpuSceneError::TreeTooDeep(e.to_string()))
}

fn gpu_bvh_nodes(flat_nodes: &[crate::bvh::FlatBvhNode]) -> Vec<GpuBvhNode> {
    if flat_nodes.is_empty() {
        return vec![GpuBvhNode::zeroed()];
    }
    flat_nodes
        .iter()
        .map(
            |(aabb, is_leaf, left_or_first, right_or_count)| GpuBvhNode {
                aabb_min: [aabb.min.x as f32, aabb.min.y as f32, aabb.min.z as f32, 0.0],
                aabb_max: [aabb.max.x as f32, aabb.max.y as f32, aabb.max.z as f32, 0.0],
                // For leaves `left_or_first` is a start index into `faces`, which
                // is built in BVH leaf order, so it maps across directly.
                left_or_first: *left_or_first,
                right_or_count: *right_or_count,
                is_leaf: u32::from(*is_leaf),
                _pad: 0,
            },
        )
        .collect()
}

/// A triangle moved into world space by `t`.
///
/// Positions go through the full matrix; shading normals through
/// `apply_normal`, which is the inverse-transpose — under a non-uniform scale
/// a normal does *not* transform like the surface it sits on, and using the
/// plain vector transform would light a squashed part as if it were not.
/// Zero-length results are left alone so the shader's
/// "normals are real" flag keeps meaning what it says.
fn place_triangle(
    tri: &crate::bvh::FlatTriangle,
    t: &vcad_kernel_math::Transform,
) -> crate::bvh::FlatTriangle {
    use vcad_kernel_math::{Point3, Vec3};

    let mut out = *tri;
    for p in out.positions.iter_mut() {
        let w = t.apply_point(&Point3::new(p[0] as f64, p[1] as f64, p[2] as f64));
        *p = [w.x as f32, w.y as f32, w.z as f32];
    }
    if let Some(normals) = out.normals.as_mut() {
        for n in normals.iter_mut() {
            let w = t.apply_normal(&Vec3::new(n[0] as f64, n[1] as f64, n[2] as f64));
            let len = w.norm();
            if len > 1e-12 {
                *n = [(w.x / len) as f32, (w.y / len) as f32, (w.z / len) as f32];
            }
        }
    }
    out
}

/// Re-fit a packed BVH node's AABB around the eight transformed corners of
/// its old one. Conservative under rotation — the new box is axis-aligned
/// around a rotated box — which costs some traversal and can never miss a
/// primitive the old box contained.
fn place_aabb(node: &mut GpuBvhNode, t: &vcad_kernel_math::Transform) {
    use vcad_kernel_math::Point3;

    let (lo, hi) = (node.aabb_min, node.aabb_max);
    if !lo[..3].iter().chain(&hi[..3]).all(|v| v.is_finite()) {
        return;
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for i in 0..8 {
        let c = Point3::new(
            if i & 1 == 0 { lo[0] } else { hi[0] } as f64,
            if i & 2 == 0 { lo[1] } else { hi[1] } as f64,
            if i & 4 == 0 { lo[2] } else { hi[2] } as f64,
        );
        let w = t.apply_point(&c);
        for (a, v) in [w.x, w.y, w.z].into_iter().enumerate() {
            min[a] = min[a].min(v as f32);
            max[a] = max[a].max(v as f32);
        }
    }
    node.aabb_min = [min[0], min[1], min[2], 0.0];
    node.aabb_max = [max[0], max[1], max[2], 0.0];
}

fn studio_lights_for_bvh(bvh_nodes: &[GpuBvhNode]) -> Vec<GpuAreaLight> {
    let Some(root) = bvh_nodes.first() else {
        return Vec::new();
    };
    let min = root.aabb_min;
    let max = root.aabb_max;
    let center = vcad_kernel_math::Point3::new(
        ((min[0] + max[0]) * 0.5) as f64,
        ((min[1] + max[1]) * 0.5) as f64,
        ((min[2] + max[2]) * 0.5) as f64,
    );
    // Bounding-sphere radius of the root AABB.
    let radius = 0.5
        * (((max[0] - min[0]) as f64).powi(2)
            + ((max[1] - min[1]) as f64).powi(2)
            + ((max[2] - min[2]) as f64).powi(2))
        .sqrt();
    if !radius.is_finite() || radius <= 0.0 {
        return Vec::new();
    }
    crate::pathtrace::studio_rig(center, radius)
        .iter()
        .map(GpuAreaLight::from_area_light)
        .collect()
}

impl GpuScene {
    /// Build GPU scene data from a BRep solid.
    ///
    /// This builds the BVH internally and converts all data to GPU-compatible format.
    pub fn from_brep(brep: &BRepSolid) -> Result<Self, GpuSceneError> {
        // Build surface list
        let mut surfaces = Vec::with_capacity(brep.geometry.surfaces.len());
        for (idx, surface) in brep.geometry.surfaces.iter().enumerate() {
            let gpu_surface = GpuSurface::from_surface(surface.as_ref());
            #[cfg(target_arch = "wasm32")]
            {
                let type_name = match gpu_surface.surface_type {
                    0 => "Plane",
                    1 => "Cylinder",
                    2 => "Sphere",
                    3 => "Cone",
                    4 => "Torus",
                    5 => "Bilinear",
                    _ => "Unknown",
                };
                // Log surface params for debugging
                if gpu_surface.surface_type == 0 {
                    // Plane: origin, x_dir, y_dir, normal
                    web_sys::console::log_1(
                        &format!(
                            "[RT] Surface {}: Plane origin=({:.2}, {:.2}, {:.2}) normal=({:.2}, {:.2}, {:.2})",
                            idx,
                            gpu_surface.params[0], gpu_surface.params[1], gpu_surface.params[2],
                            gpu_surface.params[9], gpu_surface.params[10], gpu_surface.params[11],
                        ).into(),
                    );
                } else {
                    web_sys::console::log_1(
                        &format!(
                            "[RT] Surface {}: type={} origin=({:.2}, {:.2}, {:.2})",
                            idx,
                            type_name,
                            gpu_surface.params[0],
                            gpu_surface.params[1],
                            gpu_surface.params[2]
                        )
                        .into(),
                    );
                }
            }
            let _ = idx; // Silence unused warning in non-WASM builds
            surfaces.push(gpu_surface);
        }
        if surfaces.len() > MAX_SURFACES {
            return Err(GpuSceneError::TooManySurfaces(surfaces.len()));
        }

        // Fail closed on surface kinds the WGSL tracer has no case for.
        // Packing them and uploading anyway makes those faces disappear from
        // the image with no diagnostic at all.
        if let Some((index, s)) = surfaces
            .iter()
            .enumerate()
            .find(|(_, s)| !s.is_gpu_traceable())
        {
            return Err(GpuSceneError::UnsupportedSurface {
                index,
                surface_type: s.surface_type,
                name: GpuSurface::type_name(s.surface_type),
            });
        }

        // Build BVH first to get the face ordering
        let bvh = <Bvh as BrepBvh>::build_brep(brep);
        let (flat_nodes, bvh_faces) = bvh.flatten_faces();

        // Build face list in BVH traversal order (so BVH leaf indices are contiguous)
        let mut faces = Vec::with_capacity(bvh_faces.len());
        let mut face_index_map = std::collections::HashMap::new();
        let mut trim_verts = Vec::new();
        let mut inner_loop_descs = Vec::new();

        for (gpu_idx, &face_id) in bvh_faces.iter().enumerate() {
            let face = &brep.topology.faces[face_id];

            // Compute face AABB
            let aabb = face_aabb(brep, face_id);

            // Get outer trim loop vertices in UV space
            let trim_start = trim_verts.len() as u32;
            let mut trim_count = 0u32;

            // Extract UV coordinates for the outer loop.
            //
            // A full cylinder/cone wall's outer loop projects to a zero-area
            // UV polygon (only seam vertices survive; the rim circles
            // collapse). The shader treats a degenerate outer loop as
            // untrimmed — correct for spheres/tori whose v is bounded, but
            // on a cylinder v is an unbounded length and the wall would
            // trace as an infinite tube. Collapse such loops to the
            // 2-vertex (v_min, v_max) form the shader's trim_count==2 path
            // already clamps against.
            let mut uvs = trim::extract_face_uv_loop(brep, face_id);
            let surface = &brep.geometry.surfaces[face.surface_index];
            // A planar cap bounded by a single closed circle edge is
            // degenerate too (the circle collapses to one UV point) — the
            // shader would reject every hit. Rebuild the circle polygon
            // from the adjacent surface's axis.
            if trim::polygon_area(&uvs).abs() < 1e-9 {
                if let Some(poly) = trim::synthesize_planar_cap_polygon(brep, face_id) {
                    uvs = poly;
                } else if let Some((v_min, v_max)) = trim::unbounded_v_range(surface.as_ref(), &uvs)
                {
                    uvs = vec![
                        vcad_kernel_math::Point2::new(0.0, v_min),
                        vcad_kernel_math::Point2::new(0.0, v_max),
                    ];
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                web_sys::console::log_1(
                    &format!(
                        "[RT] Face {} (id {:?}) outer loop: {} vertices",
                        gpu_idx,
                        face_id,
                        uvs.len()
                    )
                    .into(),
                );
                // Log first 4 UV coordinates for debugging
                for (j, uv) in uvs.iter().take(4).enumerate() {
                    web_sys::console::log_1(
                        &format!("[RT]   UV[{}]: ({:.2}, {:.2})", j, uv.x, uv.y).into(),
                    );
                }
            }
            for uv in &uvs {
                trim_verts.push(GpuVec2 {
                    x: uv.x as f32,
                    y: uv.y as f32,
                });
                trim_count += 1;
            }

            // Extract inner loops (holes)
            let inner_start = trim_verts.len() as u32;
            let inner_desc_start = inner_loop_descs.len() as u32;
            let mut inner_count = 0u32;
            let inner_loops = trim::extract_face_inner_loops(brep, face_id);
            let inner_loop_count = inner_loops.len() as u32;

            for (loop_idx, inner_uvs) in inner_loops.iter().enumerate() {
                #[cfg(target_arch = "wasm32")]
                web_sys::console::log_1(
                    &format!(
                        "[RT] Face {} inner loop {}: {} vertices",
                        gpu_idx,
                        loop_idx,
                        inner_uvs.len()
                    )
                    .into(),
                );
                let _ = loop_idx; // Silence unused warning in non-WASM builds
                                  // Store the vertex count for this inner loop
                inner_loop_descs.push(inner_uvs.len() as u32);
                for uv in inner_uvs {
                    trim_verts.push(GpuVec2 {
                        x: uv.x as f32,
                        y: uv.y as f32,
                    });
                    inner_count += 1;
                }
            }

            let orientation = match face.orientation {
                vcad_kernel_topo::Orientation::Forward => 0,
                vcad_kernel_topo::Orientation::Reversed => 1,
            };

            face_index_map.insert(face_id, gpu_idx as u32);

            faces.push(GpuFace {
                surface_idx: face.surface_index as u32,
                orientation,
                trim_start,
                trim_count,
                aabb_min: [aabb.min.x as f32, aabb.min.y as f32, aabb.min.z as f32, 0.0],
                aabb_max: [aabb.max.x as f32, aabb.max.y as f32, aabb.max.z as f32, 0.0],
                inner_start,
                inner_count,
                inner_loop_count,
                inner_desc_start,
                material_idx: 0, // Default material
                _pad2: [0; 3],
            });
        }

        if faces.len() > MAX_FACES {
            return Err(GpuSceneError::TooManyFaces(faces.len()));
        }
        if trim_verts.len() > MAX_TRIM_VERTS {
            return Err(GpuSceneError::TooManyTrimVerts(trim_verts.len()));
        }

        // Convert flattened BVH to GPU format
        // Faces are now in BVH order, so leaf indices map directly
        check_tree_depth(&flat_nodes)?;
        let bvh_nodes = gpu_bvh_nodes(&flat_nodes);

        if bvh_nodes.len() > MAX_BVH_NODES {
            return Err(GpuSceneError::TooManyBvhNodes(bvh_nodes.len()));
        }

        // Ensure inner_loop_descs is not empty (GPU requires non-zero buffer)
        if inner_loop_descs.is_empty() {
            inner_loop_descs.push(0);
        }

        // Create default material (neutral gray)
        let materials = vec![GpuMaterial::default()];

        let lights = studio_lights_for_bvh(&bvh_nodes);

        Ok(Self {
            surfaces,
            faces,
            materials,
            bvh_nodes,
            trim_verts,
            inner_loop_descs,
            face_index_map,
            environment: None,
            lights,
        })
    }

    /// Build GPU scene data from a triangle mesh.
    ///
    /// The counterpart of [`Self::from_brep`] for geometry that has no
    /// analytic surfaces: frozen `topology_optimize` results, imported
    /// STL/GLB parts, and the cached tessellations `--photoreal` traces by
    /// default. Builds the BVH with [`Bvh::build_mesh`] — the *same* BLAS the
    /// CPU path tracer walks — so the two renderers agree on geometry and
    /// differ only in arithmetic precision.
    pub fn from_mesh(mesh: &TriangleMesh) -> Result<Self, GpuSceneError> {
        Self::from_mesh_bvh(&Bvh::build_mesh(mesh))
    }

    /// Build GPU scene data from an already-built mesh BVH.
    ///
    /// Split out from [`Self::from_mesh`] so a caller that already traces the
    /// BVH on the CPU can upload that exact tree rather than rebuilding a
    /// second one that might partition differently.
    ///
    /// Rejects a BRep-backed BVH with [`GpuSceneError::NotAMeshBvh`] rather
    /// than silently producing an empty scene.
    ///
    /// Every face gets material 0, the GPU default grey. Use
    /// [`Self::from_mesh_bvh_placed`] to give the part its own material and
    /// world placement, which is what a multi-part scene needs.
    pub fn from_mesh_bvh(bvh: &Bvh) -> Result<Self, GpuSceneError> {
        Self::from_mesh_bvh_placed(bvh, GpuMaterial::default(), None)
    }

    /// [`Self::from_mesh_bvh`] with the part's own material and world
    /// transform.
    ///
    /// **The transform is baked into the packed vertices**, not carried as an
    /// instance: the WGSL tracer walks one flat node array with no instancing
    /// layer, so there is nowhere to put a per-object matrix. Positions go
    /// through `apply_point`, shading normals through `apply_normal` (the
    /// inverse-transpose — a non-uniform scale rotates a normal differently
    /// from the surface it belongs to, and using `apply_vec` here would shade
    /// a squashed part wrong), and the BVH node AABBs are re-fitted around
    /// their transformed corners. Re-fitting corners is conservative rather
    /// than tight under rotation, which costs a little traversal and cannot
    /// lose a hit.
    ///
    /// Baking means the result is a *static* snapshot: a new pose needs a new
    /// scene. That is why `--animate` stays on the CPU.
    pub fn from_mesh_bvh_placed(
        bvh: &Bvh,
        material: GpuMaterial,
        transform: Option<&vcad_kernel_math::Transform>,
    ) -> Result<Self, GpuSceneError> {
        let mut scene = Self::pack_mesh_bvh(bvh, transform)?;
        scene.materials = vec![material];
        Ok(scene)
    }

    fn pack_mesh_bvh(
        bvh: &Bvh,
        transform: Option<&vcad_kernel_math::Transform>,
    ) -> Result<Self, GpuSceneError> {
        let (flat_nodes, prims) = bvh.flatten_prims();
        let crate::bvh::FlatPrims::Triangles(tris) = prims else {
            return Err(GpuSceneError::NotAMeshBvh);
        };

        if tris.len() > MAX_MESH_TRIANGLES {
            return Err(GpuSceneError::TooManyTriangles(tris.len()));
        }

        // One surface and one face per triangle, in BVH leaf order, so a
        // leaf's (first, count) range indexes `faces` directly — exactly as
        // in the BRep path. The face carries no trim loops: a triangle's
        // Möller-Trumbore test already answers the containment question that
        // trimming answers for an analytic surface, and `point_in_face`
        // short-circuits on the triangle type code rather than running a
        // winding test over an empty polygon.
        let mut surfaces = Vec::with_capacity(tris.len());
        let mut faces = Vec::with_capacity(tris.len());
        for (i, tri) in tris.iter().enumerate() {
            let placed;
            let tri = match transform {
                None => tri,
                Some(t) => {
                    placed = place_triangle(tri, t);
                    &placed
                }
            };
            surfaces.push(GpuSurface::triangle(tri));

            let mut lo = tri.positions[0];
            let mut hi = tri.positions[0];
            for p in &tri.positions[1..] {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }

            faces.push(GpuFace {
                surface_idx: i as u32,
                // Triangle normals come from the mesh's own winding and
                // vertex normals; there is no topological orientation to
                // apply on top, and flipping here would invert the shading
                // relative to the CPU tracer.
                orientation: 0,
                trim_start: 0,
                trim_count: 0,
                aabb_min: [lo[0], lo[1], lo[2], 0.0],
                aabb_max: [hi[0], hi[1], hi[2], 0.0],
                inner_start: 0,
                inner_count: 0,
                inner_loop_count: 0,
                inner_desc_start: 0,
                material_idx: 0,
                _pad2: [0; 3],
            });
        }

        check_tree_depth(&flat_nodes)?;
        let mut bvh_nodes = gpu_bvh_nodes(&flat_nodes);
        if let Some(t) = transform {
            for n in bvh_nodes.iter_mut() {
                place_aabb(n, t);
            }
        }
        let lights = studio_lights_for_bvh(&bvh_nodes);

        Ok(Self {
            surfaces,
            faces,
            materials: vec![GpuMaterial::default()],
            bvh_nodes,
            // WebGPU refuses a zero-sized storage buffer, and a mesh scene
            // has nothing to put in either of these. One dummy element each,
            // referenced by nothing (every face has trim_count 0).
            trim_verts: vec![GpuVec2 { x: 0.0, y: 0.0 }],
            inner_loop_descs: vec![0],
            // Face IDs are a BRep concept; a triangle has none, so nothing
            // here can be keyed by one.
            face_index_map: std::collections::HashMap::new(),
            environment: None,
            lights,
        })
    }

    /// Merge another GpuScene into this one. Combines surfaces, faces,
    /// materials, trim verts, inner-loop descriptors, and BVH nodes.
    ///
    /// All `other`'s indices into the various buffers are offset by the
    /// current sizes of `self`'s buffers. The two BVH trees are unified
    /// under a new root node whose AABB is the union of both old roots'
    /// AABBs — this keeps traversal a single root-to-leaf walk in the
    /// shader without changes there.
    pub fn merge(mut self, other: Self) -> Self {
        let surface_offset = self.surfaces.len() as u32;
        let face_offset = self.faces.len() as u32;
        let material_offset = self.materials.len() as u32;
        let bvh_offset = self.bvh_nodes.len() as u32;
        let trim_offset = self.trim_verts.len() as u32;
        let inner_desc_offset = self.inner_loop_descs.len() as u32;

        // Adjust other's faces — every cross-buffer index needs a shift.
        let adjusted_faces: Vec<GpuFace> = other
            .faces
            .iter()
            .map(|f| {
                let mut nf = *f;
                nf.surface_idx += surface_offset;
                nf.material_idx += material_offset;
                nf.trim_start += trim_offset;
                nf.inner_start += trim_offset;
                nf.inner_desc_start += inner_desc_offset;
                nf
            })
            .collect();

        // Adjust other's BVH nodes:
        //   - leaf nodes: left_or_first is a face index in `faces`
        //   - internal nodes: both left_or_first and right_or_count are
        //     child node indices in bvh_nodes
        // After the merge below we'll prepend a new root, so internal-node
        // child indices need an extra +1 on top of the per-tree offset.
        let adjusted_nodes: Vec<GpuBvhNode> = other
            .bvh_nodes
            .iter()
            .map(|n| {
                let mut nn = *n;
                if nn.is_leaf == 1 {
                    nn.left_or_first += face_offset;
                    // right_or_count is a count, not an index — leave alone.
                } else {
                    nn.left_or_first += bvh_offset + 1;
                    nn.right_or_count += bvh_offset + 1;
                }
                nn
            })
            .collect();

        // Self's existing internal nodes need +1 for the new root we'll prepend.
        for n in self.bvh_nodes.iter_mut() {
            if n.is_leaf == 0 {
                n.left_or_first += 1;
                n.right_or_count += 1;
            } else {
                // Leaf face indices stay correct (faces are appended after).
            }
        }

        // New root spans both trees. Children: self's old root (now at 1),
        // other's old root (at 1 + self.bvh_nodes.len()).
        let self_root_min = self.bvh_nodes[0].aabb_min;
        let self_root_max = self.bvh_nodes[0].aabb_max;
        let other_root_min = adjusted_nodes[0].aabb_min;
        let other_root_max = adjusted_nodes[0].aabb_max;
        let new_root = GpuBvhNode {
            aabb_min: [
                self_root_min[0].min(other_root_min[0]),
                self_root_min[1].min(other_root_min[1]),
                self_root_min[2].min(other_root_min[2]),
                0.0,
            ],
            aabb_max: [
                self_root_max[0].max(other_root_max[0]),
                self_root_max[1].max(other_root_max[1]),
                self_root_max[2].max(other_root_max[2]),
                0.0,
            ],
            left_or_first: 1,
            right_or_count: bvh_offset + 1,
            is_leaf: 0,
            _pad: 0,
        };

        let mut merged_bvh = Vec::with_capacity(1 + self.bvh_nodes.len() + adjusted_nodes.len());
        merged_bvh.push(new_root);
        merged_bvh.extend(self.bvh_nodes);
        merged_bvh.extend(adjusted_nodes);

        // Re-derive the studio rig from the combined bounds. Keeping self's
        // rig would light the merged scene as if only self's half of it
        // existed — with a mesh part merged alongside a BRep one, the softbox
        // distances come out wrong for whichever half did not set them.
        self.lights = studio_lights_for_bvh(&merged_bvh);

        self.surfaces.extend(other.surfaces);
        self.faces.extend(adjusted_faces);
        self.materials.extend(other.materials);
        self.bvh_nodes = merged_bvh;
        self.trim_verts.extend(other.trim_verts);
        self.inner_loop_descs.extend(other.inner_loop_descs);

        self
    }

    /// Fold many scenes into one with a **balanced** tree of merges.
    ///
    /// [`Self::merge`] adds exactly one level of depth per call, so folding N
    /// parts linearly (`a.merge(b).merge(c)…`) costs N-1 levels of traversal
    /// stack on top of the deepest part's own tree. A pairwise fold costs
    /// `ceil(log2(N))` instead — for the 60-odd parts of a real assembly that
    /// is 6 levels rather than 59, which is the difference between fitting in
    /// the shader's stack and silently dropping geometry.
    ///
    /// Returns `None` for an empty input: a scene with nothing in it has no
    /// root AABB, and every downstream consumer would rather be told than
    /// handed a zeroed tree.
    pub fn merge_all(mut scenes: Vec<Self>) -> Option<Self> {
        if scenes.is_empty() {
            return None;
        }
        while scenes.len() > 1 {
            let mut next = Vec::with_capacity(scenes.len().div_ceil(2));
            let mut it = scenes.into_iter();
            while let Some(a) = it.next() {
                match it.next() {
                    Some(b) => next.push(a.merge(b)),
                    None => next.push(a),
                }
            }
            scenes = next;
        }
        scenes.pop()
    }

    /// Depth of the packed BVH, in nodes from root to deepest leaf.
    ///
    /// This is what the shader's traversal stack has to hold. Computed
    /// iteratively — a merged assembly tree is deep enough that a recursive
    /// walk is a real stack-overflow risk on the host too — and defensive
    /// against a malformed tree: a node index that repeats on the current
    /// path, or points past the array, terminates that branch rather than
    /// looping forever.
    pub fn bvh_depth(&self) -> usize {
        if self.bvh_nodes.is_empty() {
            return 0;
        }
        let mut best = 0usize;
        // (node index, depth). Depth is 1 at the root.
        let mut stack = vec![(0u32, 1usize)];
        let mut visited = vec![false; self.bvh_nodes.len()];
        while let Some((idx, depth)) = stack.pop() {
            let Some(node) = self.bvh_nodes.get(idx as usize) else {
                continue;
            };
            if std::mem::replace(&mut visited[idx as usize], true) {
                continue;
            }
            best = best.max(depth);
            if node.is_leaf == 0 {
                stack.push((node.left_or_first, depth + 1));
                stack.push((node.right_or_count, depth + 1));
            }
        }
        best
    }

    /// Check the packed scene against every limit that would otherwise fail
    /// silently or as a driver error, *before* anything is uploaded.
    ///
    /// `max_binding_bytes` is the device's `max_storage_buffer_binding_size`
    /// (`ctx.device.limits()`); pass `None` to skip that check.
    ///
    /// [`Self::merge`] deliberately does not validate — it is a building
    /// block, and checking N times while folding N parts would report the
    /// wrong totals. This is the gate to call once, on the finished scene.
    pub fn validate(&self, max_binding_bytes: Option<u64>) -> Result<(), GpuSceneError> {
        let mesh = self.is_mesh_scene();
        if mesh {
            if self.surfaces.len() > MAX_MESH_TRIANGLES {
                return Err(GpuSceneError::TooManyTriangles(self.surfaces.len()));
            }
        } else {
            if self.surfaces.len() > MAX_SURFACES {
                return Err(GpuSceneError::TooManySurfaces(self.surfaces.len()));
            }
            if self.faces.len() > MAX_FACES {
                return Err(GpuSceneError::TooManyFaces(self.faces.len()));
            }
        }
        if self.bvh_nodes.len() > MAX_BVH_NODES {
            return Err(GpuSceneError::TooManyBvhNodes(self.bvh_nodes.len()));
        }
        if self.trim_verts.len() > MAX_TRIM_VERTS {
            return Err(GpuSceneError::TooManyTrimVerts(self.trim_verts.len()));
        }
        let depth = self.bvh_depth();
        if depth > MAX_TRAVERSAL_DEPTH {
            return Err(GpuSceneError::BvhTooDeep(depth));
        }
        if let Some(cap) = max_binding_bytes {
            let bytes = (self.surfaces.len() * std::mem::size_of::<GpuSurface>())
                .max(self.faces.len() * std::mem::size_of::<GpuFace>())
                .max(self.bvh_nodes.len() * std::mem::size_of::<GpuBvhNode>())
                as u64;
            if bytes > cap {
                return Err(GpuSceneError::ExceedsDeviceBinding { bytes, cap });
            }
        }
        Ok(())
    }

    /// Whether this scene's geometry is triangles rather than trimmed
    /// analytic surfaces. Mesh scenes count in the hundreds of thousands and
    /// are held to [`MAX_MESH_TRIANGLES`], not to the BRep-scale caps.
    fn is_mesh_scene(&self) -> bool {
        self.surfaces
            .iter()
            .all(|s| s.surface_type == SURFACE_TYPE_TRIANGLE)
    }

    /// Set the material for all faces in the scene.
    ///
    /// This replaces the default gray material with the specified color.
    pub fn set_material(&mut self, r: f32, g: f32, b: f32, metallic: f32, roughness: f32) {
        if self.materials.is_empty() {
            self.materials.push(GpuMaterial::default());
        }
        self.materials[0] = GpuMaterial {
            color: [r, g, b, 1.0],
            metallic,
            roughness,
            ..Default::default()
        };
    }

    /// Light the scene with a lat-long HDR environment instead of the analytic
    /// gradient.
    ///
    /// The map is importance-sampled on the GPU exactly as it is on the CPU —
    /// same CDFs, same nearest-texel lookup, same solid-angle PDF — so both
    /// renderers converge to the same image.
    pub fn set_environment(&mut self, env: Option<&crate::pathtrace::EnvMap>) {
        self.environment = env.map(|e| e.pack_for_gpu());
    }

    /// Set the material for all faces from an IR material definition.
    ///
    /// Preferred over [`Self::set_material`]: it runs the same derivation the
    /// CPU renderer uses, so clearcoat, IOR and anisotropy reach the viewport
    /// instead of being silently dropped.
    pub fn set_material_from_def(
        &mut self,
        mat: Option<&vcad_ir::MaterialDef>,
        tint: Option<[f64; 3]>,
    ) {
        if self.materials.is_empty() {
            self.materials.push(GpuMaterial::default());
        }
        self.materials[0] = GpuMaterial::from_pbr(crate::pathtrace::from_material_def(mat, tint));
    }
}

// ---- placed instances -------------------------------------------------------
//
// `GpuScene::from_brep` packs a solid in the coordinates its BRep is authored
// in, and `merge` unions two of them without moving either. A scene assembled
// from *instances* — the same ball drawn at four poses, a court whose parts
// were each modelled at the origin — needs one more thing: the ability to say
// where a packed solid sits.
//
// The shader is not told about it. Every surface the GPU understands is
// analytic and given by a frame (an origin or centre, an axis, a reference
// direction) and a size, and a rigid placement acts on that frame alone:
// carry the points through as points, the directions as directions, and the
// radii are untouched. So `placed` moves the packed scene rather than the ray,
// and the traversal in `raytrace.wgsl` stays a single BVH walk in world space
// with no per-instance transform lookup. What it costs is a pass over the
// surface and node arrays per placement, which is cheap next to re-packing a
// BRep, and what it buys is that `merge` already does the rest.
//
// Only rigid placements are meaningful here: a non-uniform scale would turn a
// sphere into an ellipsoid, which none of these six surface types can express.

/// Which slots of a [`GpuSurface`]'s `params` are points and which are
/// directions, per surface type. Everything else is a radius or an angle,
/// which a rigid placement leaves alone.
const SURFACE_SLOTS: [(&[usize], &[usize]); 6] = [
    // Plane: origin; x_dir, y_dir, normal
    (&[0], &[3, 6, 9]),
    // Cylinder: centre; axis, ref_dir  (radius at 9)
    (&[0], &[3, 6]),
    // Sphere: centre (radius at 3); ref_dir, axis
    (&[0], &[4, 7]),
    // Cone: apex; axis, ref_dir  (half-angle at 9)
    (&[0], &[3, 6]),
    // Torus: centre; axis, ref_dir  (major/minor radius at 9, 10)
    (&[0], &[3, 6]),
    // Bilinear: four corners, no directions
    (&[0, 3, 6, 9], &[]),
];

/// The eight corners of an AABB, moved, re-bounded. A rotation makes the new
/// box looser than the old one; that costs traversal, never correctness.
fn placed_aabb(
    min: [f32; 4],
    max: [f32; 4],
    to_world: &vcad_kernel_math::Transform,
) -> ([f32; 4], [f32; 4]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for i in 0..8 {
        let corner = vcad_kernel_math::Point3::new(
            if i & 1 == 0 { min[0] } else { max[0] } as f64,
            if i & 2 == 0 { min[1] } else { max[1] } as f64,
            if i & 4 == 0 { min[2] } else { max[2] } as f64,
        );
        let p = to_world.apply_point(&corner);
        for (k, v) in [p.x, p.y, p.z].iter().enumerate() {
            lo[k] = lo[k].min(*v as f32);
            hi[k] = hi[k].max(*v as f32);
        }
    }
    ([lo[0], lo[1], lo[2], 0.0], [hi[0], hi[1], hi[2], 0.0])
}

impl GpuSurface {
    /// The same surface, rigidly placed by `to_world`.
    ///
    /// A B-spline surface (type 6) is not traced on the GPU at all, so it is
    /// returned unchanged rather than half-moved.
    #[must_use]
    pub fn placed(&self, to_world: &vcad_kernel_math::Transform) -> Self {
        let Some((points, vectors)) = SURFACE_SLOTS.get(self.surface_type as usize) else {
            return *self;
        };
        let mut out = *self;
        for &i in *points {
            let p = vcad_kernel_math::Point3::new(
                self.params[i] as f64,
                self.params[i + 1] as f64,
                self.params[i + 2] as f64,
            );
            let q = to_world.apply_point(&p);
            out.params[i] = q.x as f32;
            out.params[i + 1] = q.y as f32;
            out.params[i + 2] = q.z as f32;
        }
        for &i in *vectors {
            let v = vcad_kernel_math::Vec3::new(
                self.params[i] as f64,
                self.params[i + 1] as f64,
                self.params[i + 2] as f64,
            );
            let w = to_world.apply_vec(&v);
            out.params[i] = w.x as f32;
            out.params[i + 1] = w.y as f32;
            out.params[i + 2] = w.z as f32;
        }
        out
    }
}

impl GpuScene {
    /// The same packed scene, rigidly placed by `to_world` — the GPU
    /// equivalent of [`crate::pathtrace::Object::placed`].
    ///
    /// Pack a solid once with [`Self::from_brep`] and call this per instance;
    /// the result [`Self::merge`]s with anything else exactly as an unplaced
    /// scene does. Trim loops are untouched: they live in each surface's own
    /// UV, and the frame that UV is measured against moved with the surface.
    ///
    /// `to_world` must be rigid. A scale would ask a sphere to become an
    /// ellipsoid, and no surface type here can say that.
    #[must_use]
    pub fn placed(&self, to_world: &vcad_kernel_math::Transform) -> Self {
        let mut out = self.clone();
        for s in &mut out.surfaces {
            *s = s.placed(to_world);
        }
        for f in &mut out.faces {
            let (min, max) = placed_aabb(f.aabb_min, f.aabb_max, to_world);
            f.aabb_min = min;
            f.aabb_max = max;
        }
        for n in &mut out.bvh_nodes {
            let (min, max) = placed_aabb(n.aabb_min, n.aabb_max, to_world);
            n.aabb_min = min;
            n.aabb_max = max;
        }
        // Lights are left where they are. The rig `from_brep` attaches is a
        // studio rig sized to that one solid, and a scene of instances wants
        // the room's lights, not one per instance — the caller replaces
        // `lights` after merging. Dragging them along would also mean a
        // sphere turned about its own centre changed on screen, which is a
        // strange thing for a placement to do.
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_geom::BilinearSurface;
    use vcad_kernel_math::Point3;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn analytic_surface_types_are_traceable() {
        for t in 0..=4u32 {
            let s = GpuSurface {
                surface_type: t,
                _pad: [0; 3],
                params: [0.0; 32],
            };
            assert!(s.is_gpu_traceable(), "{} should be traceable", t);
        }
    }

    #[test]
    fn bilinear_and_bspline_are_not_traceable() {
        // Both are packed by `from_surface` but hit the WGSL `default` arm,
        // which reports a miss — so they must never reach the GPU.
        for t in [5u32, 6] {
            let s = GpuSurface {
                surface_type: t,
                _pad: [0; 3],
                params: [0.0; 32],
            };
            assert!(!s.is_gpu_traceable(), "{} must be rejected", t);
        }
    }

    #[test]
    fn cube_builds_a_gpu_scene() {
        let scene = GpuScene::from_brep(&make_cube(10.0, 10.0, 10.0)).expect("cube is analytic");
        assert_eq!(scene.faces.len(), 6);
    }

    #[test]
    fn unsupported_surface_names_the_offending_geometry() {
        // Swap one of the cube's planes for a bilinear patch: the shader has
        // no case for it, so building the scene must fail rather than drop
        // the face from the image.
        let mut cube = make_cube(10.0, 10.0, 10.0);
        cube.geometry.surfaces[2] = Box::new(BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        ));

        let Err(err) = GpuScene::from_brep(&cube) else {
            panic!("bilinear surface must be rejected, not silently dropped");
        };
        match err {
            GpuSceneError::UnsupportedSurface { index, name, .. } => {
                assert_eq!(index, 2);
                assert_eq!(name, "Bilinear");
            }
            other => panic!("wrong error: {other:?}"),
        }
        assert!(err.to_string().contains("Bilinear"), "{err}");
    }

    /// A triangle mesh, tessellated from a cube.
    fn cube_mesh() -> TriangleMesh {
        vcad_kernel_tessellate::tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16)
    }

    #[test]
    fn triangles_are_traceable() {
        // The counterpart of `bilinear_and_bspline_are_not_traceable`: the
        // shader DOES have a case for type 7, and `from_mesh` would reject
        // its own output if this said otherwise.
        let s = GpuSurface {
            surface_type: SURFACE_TYPE_TRIANGLE,
            _pad: [0; 3],
            params: [0.0; 32],
        };
        assert!(s.is_gpu_traceable());
        assert_eq!(GpuSurface::type_name(SURFACE_TYPE_TRIANGLE), "Triangle");
    }

    #[test]
    fn triangle_packing_round_trips_positions_and_normals() {
        let tri = crate::bvh::FlatTriangle {
            positions: [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            normals: Some([[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]),
        };
        let s = GpuSurface::triangle(&tri);

        assert_eq!(s.surface_type, SURFACE_TYPE_TRIANGLE);
        // Positions in slots 0..9, normals in 9..18, flag in 18. The WGSL
        // reads these by literal index, so the layout is load-bearing.
        assert_eq!(
            &s.params[0..9],
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]
        );
        assert_eq!(
            &s.params[9..18],
            &[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]
        );
        assert_eq!(s.params[18], 1.0);
        // Nothing may spill past the documented block.
        assert!(s.params[19..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn triangle_without_normals_clears_the_flag() {
        // The shader cannot distinguish an absent normal from a zero one, so
        // the flag is the only thing standing between a normal-less mesh and
        // normalize(vec3(0)) — i.e. NaN across the whole surface.
        let tri = crate::bvh::FlatTriangle {
            positions: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: None,
        };
        let s = GpuSurface::triangle(&tri);
        assert_eq!(s.params[18], 0.0);
        assert!(s.params[9..18].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn mesh_builds_a_gpu_scene_of_triangles() {
        let mesh = cube_mesh();
        let scene = GpuScene::from_mesh(&mesh).expect("mesh scene builds");

        let tris = mesh.indices.len() / 3;
        assert_eq!(scene.faces.len(), tris);
        assert_eq!(scene.surfaces.len(), tris);
        assert!(scene
            .surfaces
            .iter()
            .all(|s| s.surface_type == SURFACE_TYPE_TRIANGLE));

        // One surface per face, in step. A face pointing at the wrong
        // surface would render the wrong triangle in the right BVH slot,
        // which is exactly the kind of bug the image tests see only as noise.
        assert!(scene
            .faces
            .iter()
            .enumerate()
            .all(|(i, f)| f.surface_idx == i as u32));

        // Triangles carry no trim loops; the shader short-circuits on the
        // type code instead. If a nonzero count ever appeared here, every
        // mesh hit would be rejected by the winding test.
        assert!(scene.faces.iter().all(|f| f.trim_count == 0));
        assert!(scene.faces.iter().all(|f| f.inner_loop_count == 0));

        // Every BVH leaf must address the face array it was built alongside.
        for n in &scene.bvh_nodes {
            if n.is_leaf == 1 {
                assert!(
                    (n.left_or_first + n.right_or_count) as usize <= scene.faces.len(),
                    "leaf range runs past the end of the face array"
                );
            }
        }

        // WebGPU rejects zero-sized storage buffers, so even the unused
        // trim arrays must carry a dummy element.
        assert!(!scene.trim_verts.is_empty());
        assert!(!scene.inner_loop_descs.is_empty());

        // The rig has to be sized to the mesh, or a mesh-only scene renders
        // unlit.
        assert!(!scene.lights.is_empty());
    }

    #[test]
    fn from_mesh_bvh_rejects_a_brep_bvh() {
        // A BRep BVH flattens to face IDs, not triangles. Building a mesh
        // scene from it would produce an empty one; say so instead.
        let bvh = Bvh::build_brep(&make_cube(10.0, 10.0, 10.0));
        let Err(err) = GpuScene::from_mesh_bvh(&bvh) else {
            panic!("a BRep-backed BVH is not a mesh");
        };
        assert!(matches!(err, GpuSceneError::NotAMeshBvh));
        assert!(err.to_string().contains("from_brep"), "{err}");
    }

    #[test]
    fn merging_a_mesh_into_an_analytic_scene_rebases_every_index() {
        let brep = GpuScene::from_brep(&make_cube(10.0, 10.0, 10.0)).expect("analytic half");
        let mesh = GpuScene::from_mesh(&cube_mesh()).expect("mesh half");
        let (brep_faces, brep_surfaces) = (brep.faces.len(), brep.surfaces.len());
        let (mesh_faces, mesh_surfaces) = (mesh.faces.len(), mesh.surfaces.len());
        let mesh_nodes = mesh.bvh_nodes.len();

        let merged = brep.merge(mesh);

        assert_eq!(merged.faces.len(), brep_faces + mesh_faces);
        assert_eq!(merged.surfaces.len(), brep_surfaces + mesh_surfaces);

        // The merged tree gains a new root spanning both halves.
        assert_eq!(merged.bvh_nodes[0].is_leaf, 0);

        // No index may dangle after rebasing — this is the whole risk of the
        // merge, and a dangling one reads out of bounds in the shader.
        for f in &merged.faces {
            assert!((f.surface_idx as usize) < merged.surfaces.len());
            assert!((f.material_idx as usize) < merged.materials.len());
        }
        for n in &merged.bvh_nodes {
            if n.is_leaf == 1 {
                assert!((n.left_or_first + n.right_or_count) as usize <= merged.faces.len());
            } else {
                assert!((n.left_or_first as usize) < merged.bvh_nodes.len());
                assert!((n.right_or_count as usize) < merged.bvh_nodes.len());
            }
        }

        // The mesh half's triangles must still be triangles, and must still
        // be reachable from the faces that were rebased onto them.
        let mesh_tris = merged.faces[brep_faces..]
            .iter()
            .filter(|f| {
                merged.surfaces[f.surface_idx as usize].surface_type == SURFACE_TYPE_TRIANGLE
            })
            .count();
        assert_eq!(mesh_tris, mesh_faces, "merge lost track of the triangles");
        assert!(mesh_nodes > 0);
    }
}
