//! Packing BRep solids into the buffers kosm-render's geometry seam binds.

use bytemuck::{Pod, Zeroable};
use kosm_render::gpu::{GpuAreaLight, GpuMaterial};
use vcad_kernel_booleans::bbox::face_aabb;
use vcad_kernel_geom::{Surface, SurfaceKind};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::FaceId;

use crate::bvh::{BrepBvh, Bvh};
use crate::trim;

/// Maximum number of surfaces supported in a single scene.
pub const MAX_SURFACES: usize = 1024;

/// Maximum number of faces supported in a single scene.
pub const MAX_FACES: usize = 4096;

/// Maximum BVH nodes.
pub const MAX_BVH_NODES: usize = 8192;

/// Maximum trim loop vertices.
pub const MAX_TRIM_VERTS: usize = 32768;

/// GPU-compatible surface representation.
///
/// Each surface type is packed into 32 floats:
/// - Type discriminant (as f32)
/// - Surface-specific parameters
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuSurface {
    /// Surface type: 0=Plane, 1=Cylinder, 2=Sphere, 3=Cone, 4=Torus, 5=Bilinear
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
}

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
        }
    }
}

impl std::error::Error for GpuSceneError {}

/// Build the studio softbox rig for a scene, sized to its BVH root bounds.
///
/// Delegates to [`crate::pathtrace::studio_rig`] — the SAME function
/// `vcad-render --photoreal` calls — so the viewport and the CPU renderer are
/// lit by an identical rig rather than by two hand-tuned approximations.
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
        let mut bvh_nodes = Vec::with_capacity(flat_nodes.len().max(1));

        if flat_nodes.is_empty() {
            // Empty BVH - add a dummy node
            bvh_nodes.push(GpuBvhNode::zeroed());
        } else {
            for (aabb, is_leaf, left_or_first, right_or_count) in &flat_nodes {
                if *is_leaf {
                    // For leaves: left_or_first is start index in faces array (which is now BVH-ordered)
                    bvh_nodes.push(GpuBvhNode {
                        aabb_min: [aabb.min.x as f32, aabb.min.y as f32, aabb.min.z as f32, 0.0],
                        aabb_max: [aabb.max.x as f32, aabb.max.y as f32, aabb.max.z as f32, 0.0],
                        left_or_first: *left_or_first,
                        right_or_count: *right_or_count,
                        is_leaf: 1,
                        _pad: 0,
                    });
                } else {
                    bvh_nodes.push(GpuBvhNode {
                        aabb_min: [aabb.min.x as f32, aabb.min.y as f32, aabb.min.z as f32, 0.0],
                        aabb_max: [aabb.max.x as f32, aabb.max.y as f32, aabb.max.z as f32, 0.0],
                        left_or_first: *left_or_first,
                        right_or_count: *right_or_count,
                        is_leaf: 0,
                        _pad: 0,
                    });
                }
            }
        }

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

        self.surfaces.extend(other.surfaces);
        self.faces.extend(adjusted_faces);
        self.materials.extend(other.materials);
        self.bvh_nodes = merged_bvh;
        self.trim_verts.extend(other.trim_verts);
        self.inner_loop_descs.extend(other.inner_loop_descs);

        self
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
