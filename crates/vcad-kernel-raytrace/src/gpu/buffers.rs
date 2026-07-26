//! GPU buffer management for ray tracing data.

use bytemuck::{Pod, Zeroable};
use vcad_kernel_booleans::bbox::face_aabb;
use vcad_kernel_geom::{Surface, SurfaceKind};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::FaceId;

use crate::bvh::Bvh;
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

/// GPU-compatible material representation (PBR).
///
/// Mirrors [`crate::pathtrace::Pbr`] field-for-field so the GPU path tracer and
/// the CPU reference shade identically. The WGSL `GpuMaterial` struct in
/// `shaders/raytrace.wgsl` must match this layout exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuMaterial {
    /// Base color (linear RGB + alpha).
    pub color: [f32; 4],
    /// Metallic factor (0 = dielectric, 1 = metal).
    pub metallic: f32,
    /// Roughness factor (0 = smooth, 1 = rough).
    pub roughness: f32,
    /// Strength of the clearcoat layer (0 = none, 1 = full).
    pub clearcoat: f32,
    /// Perceptual roughness of the clearcoat layer.
    pub clearcoat_roughness: f32,
    /// Dielectric index of refraction, drives the base specular reflectance.
    pub ior: f32,
    /// Signed anisotropy in -1..1: positive stretches the specular highlight
    /// along the local tangent, negative along the bitangent, 0 = isotropic.
    pub anisotropy: f32,
    /// Padding for 16-byte alignment.
    pub _pad: [f32; 2],
}

impl Default for GpuMaterial {
    fn default() -> Self {
        Self {
            color: [0.7, 0.7, 0.7, 1.0], // Neutral gray
            metallic: 0.0,
            roughness: 0.5,
            clearcoat: 0.0,
            clearcoat_roughness: 0.1,
            ior: 1.5,
            anisotropy: 0.0,
            _pad: [0.0; 2],
        }
    }
}

impl GpuMaterial {
    /// Create a new material with the given color.
    pub fn with_color(r: f32, g: f32, b: f32) -> Self {
        Self {
            color: [r, g, b, 1.0],
            ..Default::default()
        }
    }

    /// Create a metallic material.
    pub fn metal(r: f32, g: f32, b: f32, roughness: f32) -> Self {
        Self {
            color: [r, g, b, 1.0],
            metallic: 1.0,
            roughness,
            ..Default::default()
        }
    }

    /// Create a plastic material, optionally clearcoated.
    pub fn plastic(r: f32, g: f32, b: f32, roughness: f32) -> Self {
        Self {
            color: [r, g, b, 1.0],
            metallic: 0.0,
            roughness,
            ..Default::default()
        }
    }

    /// Add a clearcoat layer of the given strength and roughness.
    pub fn with_clearcoat(mut self, clearcoat: f32, clearcoat_roughness: f32) -> Self {
        self.clearcoat = clearcoat;
        self.clearcoat_roughness = clearcoat_roughness;
        self
    }

    /// Build from the CPU reference material.
    ///
    /// Paired with [`crate::pathtrace::Pbr::from_material_def`], this is what
    /// makes the viewport and `--photoreal` derive the SAME material from the
    /// same IR definition — clearcoat heuristic, IOR and grain included.
    pub fn from_pbr(p: crate::pathtrace::Pbr) -> Self {
        Self {
            color: [p.base_color[0], p.base_color[1], p.base_color[2], 1.0],
            metallic: p.metallic,
            roughness: p.roughness,
            clearcoat: p.clearcoat,
            clearcoat_roughness: p.clearcoat_roughness,
            ior: p.ior,
            anisotropy: p.anisotropy,
            _pad: [0.0; 2],
        }
    }

    /// Convert to the CPU reference material, for cross-checking the two
    /// shading paths against each other.
    pub fn to_pbr(self) -> crate::pathtrace::Pbr {
        crate::pathtrace::Pbr {
            base_color: [self.color[0], self.color[1], self.color[2]],
            metallic: self.metallic,
            roughness: self.roughness,
            clearcoat: self.clearcoat,
            clearcoat_roughness: self.clearcoat_roughness,
            ior: self.ior,
            anisotropy: self.anisotropy,
            emissive: [0.0; 3],
        }
    }
}

/// GPU-compatible rectangular area light ("softbox").
///
/// Mirrors the WGSL `GpuAreaLight`. Built from [`crate::pathtrace::AreaLight`]
/// via [`GpuAreaLight::from_area_light`] so the GPU and CPU renderers light the
/// scene with the same rig — that is what makes specular highlights on metal
/// match between the viewport and `--photoreal`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuAreaLight {
    /// Centre of the rectangle (w unused).
    pub center: [f32; 4],
    /// Half-extent along the rectangle's first axis (w unused).
    pub u: [f32; 4],
    /// Half-extent along the second axis (w unused).
    pub v: [f32; 4],
    /// Emitted radiance (w unused).
    pub emission: [f32; 4],
}

impl GpuAreaLight {
    /// Convert a CPU reference area light to its GPU representation.
    pub fn from_area_light(l: &crate::pathtrace::AreaLight) -> Self {
        Self {
            center: [l.center.x as f32, l.center.y as f32, l.center.z as f32, 0.0],
            u: [l.u.x as f32, l.u.y as f32, l.u.z as f32, 0.0],
            v: [l.v.x as f32, l.v.y as f32, l.v.z as f32, 0.0],
            emission: [l.emission[0], l.emission[1], l.emission[2], 0.0],
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

/// Camera parameters for the ray tracer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuCamera {
    /// Camera position.
    pub position: [f32; 4],
    /// Look-at target.
    pub target: [f32; 4],
    /// Up vector.
    pub up: [f32; 4],
    /// Field of view in radians.
    pub fov: f32,
    /// Image width.
    pub width: u32,
    /// Image height.
    pub height: u32,
    /// Padding.
    pub _pad: u32,
}

/// Render state for progressive rendering.
///
/// Layout (128 bytes, 16-byte aligned — matches `RenderState` in raytrace.wgsl):
/// offset  0–31:  eight u32/f32 scalars (frame_index … theme)
/// offset 32–47:  path tracing (max_depth, rr_start, light_count, env_intensity)
/// offset 48–63:  refine_sample_count, firefly_clamp, ground_enabled, stylize
/// offset 64–79:  silhouette_color vec4
/// offset 80–95:  crease_color vec4
/// offset 96–111: boundary_color vec4
/// offset 112–127: four f32 width/softness scalars
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuRenderState {
    /// Current frame index for accumulation (1-based).
    pub frame_index: u32,
    /// Jitter X offset for anti-aliasing (-0.5 to 0.5).
    pub jitter_x: f32,
    /// Jitter Y offset for anti-aliasing (-0.5 to 0.5).
    pub jitter_y: f32,
    /// Edge-type bit-flags: 0=off, bit0=silhouette, bit1=crease, bit2=boundary.
    pub enable_edges: u32,
    /// Edge detection threshold for depth discontinuity.
    pub edge_depth_threshold: f32,
    /// Edge detection threshold for normal discontinuity (degrees).
    pub edge_normal_threshold: f32,
    /// Debug render mode: 0=normal, 1=show normals, 2=show face_id, 3=show n_dot_l,
    /// 4=show orientation, 5=sample-count heatmap (blue=1 ray, red=max rays).
    pub debug_mode: u32,
    /// Theme: 0 = dark (default), 1 = light. Drives the visible background
    /// palette in `sky_color`; the IBL panels and direct lighting stay
    /// constant across themes so the model itself looks the same.
    pub theme: u32,
    // Path tracing
    /// Maximum path length (1 = direct lighting only). Escalated by the
    /// refinement scheduler: shallow on the draft frame, deeper as
    /// accumulation proceeds, so the first frame stays interactive.
    pub max_depth: u32,
    /// Depth at which Russian roulette begins.
    pub rr_start: u32,
    /// Number of valid entries in the area-light buffer.
    pub light_count: u32,
    /// Overall multiplier on the analytic studio environment.
    pub env_intensity: f32,
    // refinement + path tracing continued
    /// Number of additional refinement rays per edge pixel (0 = disabled).
    /// Actual rays fired = floor(sqrt(refine_sample_count))^2.
    pub refine_sample_count: u32,
    /// Clamp on indirect radiance to kill fireflies (0 = disabled).
    pub firefly_clamp: f32,
    /// Whether the implicit ground plane participates in the path trace.
    pub ground_enabled: u32,
    /// Non-zero enables non-photoreal stylisation (the Sobel edge overlay).
    /// Off in a photoreal viewport: edge lines fight photorealism.
    pub stylize: u32,
    // --- edge style (added for Fusion-style edge lines) ---
    /// Silhouette line color (RGBA linear, depth-gradient edges).
    pub silhouette_color: [f32; 4],
    /// Crease line color (RGBA linear, face-ID boundary edges).
    pub crease_color: [f32; 4],
    /// Boundary line color (RGBA linear, foreground→background edges).
    pub boundary_color: [f32; 4],
    /// Silhouette line apparent width (1.0 = one pixel).
    pub silhouette_width: f32,
    /// Crease line apparent width.
    pub crease_width: f32,
    /// Boundary line apparent width.
    pub boundary_width: f32,
    /// Sub-pixel softness factor (higher = softer AA transition).
    pub edge_softness: f32,
    /// Environment mode: 0 = analytic gradient, 1 = lat-long HDR image.
    pub env_mode: u32,
    /// Environment image width in texels (image mode only).
    pub env_width: u32,
    /// Environment image height in texels (image mode only).
    pub env_height: u32,
    /// Environment rotation about +Z, in radians.
    pub env_rotation: f32,
    /// Normaliser for the environment's uv-space PDF.
    pub env_marg_int: f32,
    /// Padding to a 16-byte multiple (required for uniform buffers).
    pub _pad3: [u32; 3],
}

/// Default silhouette line color: near-black, slightly cool.
const DEFAULT_SILHOUETTE_COLOR: [f32; 4] = [0.08, 0.08, 0.10, 1.0];
/// Default crease line color: slightly lighter than silhouette.
const DEFAULT_CREASE_COLOR: [f32; 4] = [0.12, 0.12, 0.14, 1.0];
/// Default boundary line color: darkest of the three types.
const DEFAULT_BOUNDARY_COLOR: [f32; 4] = [0.06, 0.06, 0.08, 1.0];

/// All three edge types on: bits 0 (silhouette) | 1 (crease) | 2 (boundary).
const EDGES_ALL: u32 = 7;

/// Full path depth, matching `PathTraceOptions::default().max_depth` so the
/// converged viewport image matches `vcad-render --photoreal`.
pub const DEFAULT_MAX_DEPTH: u32 = 6;
/// Depth at which Russian roulette begins, matching the CPU renderer.
pub const DEFAULT_RR_START: u32 = 3;
/// Environment multiplier, matching `Environment::default().intensity`.
pub const DEFAULT_ENV_INTENSITY: f32 = 0.35;
/// Indirect-radiance clamp, matching the CPU renderer's firefly clamp.
pub const DEFAULT_FIREFLY_CLAMP: f32 = 12.0;

/// Path depth to trace on a given accumulation frame.
///
/// Full path tracing is too slow for the viewport's draft frame, so depth
/// escalates with accumulation: the first frame traces shallow (direct
/// lighting plus one bounce) and lands fast, and by the time the `high` tier
/// is accumulating we are at the full depth that matches the CPU renderer.
/// The refinement scheduler in `RayTracedViewport.tsx` resets `frame_index` on
/// every camera change, so each gesture gets a cheap first frame.
///
/// Depth only ever increases, and the accumulation buffer is a running average,
/// so early shallow frames are progressively outweighed by deeper ones.
pub fn depth_for_frame(frame_index: u32, ceiling: u32) -> u32 {
    let d = match frame_index {
        0 | 1 => 2,
        2..=4 => 4,
        _ => DEFAULT_MAX_DEPTH,
    };
    d.min(ceiling.max(1))
}

impl GpuRenderState {
    /// Create a new render state for the given frame with default edge style.
    pub fn new(frame_index: u32) -> Self {
        let (jitter_x, jitter_y) = halton_2_3(frame_index);
        Self {
            frame_index,
            jitter_x,
            jitter_y,
            enable_edges: EDGES_ALL,
            edge_depth_threshold: 0.1,
            edge_normal_threshold: 30.0,
            debug_mode: 0,
            theme: 0,
            max_depth: depth_for_frame(frame_index, DEFAULT_MAX_DEPTH),
            rr_start: DEFAULT_RR_START,
            light_count: 0,
            env_intensity: DEFAULT_ENV_INTENSITY,
            refine_sample_count: 0,
            firefly_clamp: DEFAULT_FIREFLY_CLAMP,
            ground_enabled: 1,
            stylize: 1,
            silhouette_color: DEFAULT_SILHOUETTE_COLOR,
            crease_color: DEFAULT_CREASE_COLOR,
            boundary_color: DEFAULT_BOUNDARY_COLOR,
            silhouette_width: 1.0,
            crease_width: 0.75,
            boundary_width: 1.25,
            edge_softness: 1.5,
            env_mode: 0,
            env_width: 0,
            env_height: 0,
            env_rotation: 0.0,
            env_marg_int: 0.0,
            _pad3: [0; 3],
        }
    }

    /// Create a new render state with a specific debug mode.
    pub fn with_debug_mode(frame_index: u32, debug_mode: u32) -> Self {
        let mut state = Self::new(frame_index);
        state.debug_mode = debug_mode;
        state
    }

    /// Create a render state with edge detection disabled.
    #[allow(dead_code)]
    pub fn without_edges(frame_index: u32) -> Self {
        let mut state = Self::new(frame_index);
        state.enable_edges = 0;
        state
    }

    /// Create a render state with custom edge settings.
    pub fn with_edge_settings(
        frame_index: u32,
        debug_mode: u32,
        enable_edges: bool,
        edge_depth_threshold: f32,
        edge_normal_threshold: f32,
    ) -> Self {
        Self::with_full_settings(
            frame_index,
            debug_mode,
            enable_edges,
            edge_depth_threshold,
            edge_normal_threshold,
            0,
            0,
        )
    }

    /// Create a render state with all settings including theme and refinement.
    #[allow(clippy::too_many_arguments)]
    pub fn with_full_settings(
        frame_index: u32,
        debug_mode: u32,
        enable_edges: bool,
        edge_depth_threshold: f32,
        edge_normal_threshold: f32,
        theme: u32,
        refine_sample_count: u32,
    ) -> Self {
        let mut state = Self::new(frame_index);
        state.enable_edges = if enable_edges { EDGES_ALL } else { 0 };
        state.edge_depth_threshold = edge_depth_threshold;
        state.edge_normal_threshold = edge_normal_threshold;
        state.debug_mode = debug_mode;
        state.theme = theme;
        state.refine_sample_count = refine_sample_count;
        state
    }

    /// Create a fully-styled render state.
    ///
    /// `enable_silhouette`, `enable_crease`, `enable_boundary` control which
    /// edge types are rendered independently.
    #[allow(clippy::too_many_arguments)]
    pub fn new_styled(
        frame_index: u32,
        debug_mode: u32,
        enable_silhouette: bool,
        enable_crease: bool,
        enable_boundary: bool,
        edge_depth_threshold: f32,
        edge_normal_threshold: f32,
        theme: u32,
        silhouette_color: [f32; 4],
        crease_color: [f32; 4],
        boundary_color: [f32; 4],
        silhouette_width: f32,
        crease_width: f32,
        boundary_width: f32,
        edge_softness: f32,
    ) -> Self {
        let (jitter_x, jitter_y) = halton_2_3(frame_index);
        let enable_edges = (enable_silhouette as u32)
            | ((enable_crease as u32) << 1)
            | ((enable_boundary as u32) << 2);
        Self {
            frame_index,
            jitter_x,
            jitter_y,
            enable_edges,
            edge_depth_threshold,
            edge_normal_threshold,
            debug_mode,
            theme,
            max_depth: depth_for_frame(frame_index, DEFAULT_MAX_DEPTH),
            rr_start: DEFAULT_RR_START,
            light_count: 0,
            env_intensity: DEFAULT_ENV_INTENSITY,
            refine_sample_count: 0,
            firefly_clamp: DEFAULT_FIREFLY_CLAMP,
            ground_enabled: 1,
            stylize: 1,
            silhouette_color,
            crease_color,
            boundary_color,
            silhouette_width,
            crease_width,
            boundary_width,
            edge_softness,
            env_mode: 0,
            env_width: 0,
            env_height: 0,
            env_rotation: 0.0,
            env_marg_int: 0.0,
            _pad3: [0; 3],
        }
    }

    /// Create a render state with adaptive refinement enabled.
    pub fn with_refinement(
        frame_index: u32,
        debug_mode: u32,
        enable_edges: bool,
        edge_depth_threshold: f32,
        edge_normal_threshold: f32,
        theme: u32,
        refine_sample_count: u32,
    ) -> Self {
        Self::with_full_settings(
            frame_index,
            debug_mode,
            enable_edges,
            edge_depth_threshold,
            edge_normal_threshold,
            theme,
            refine_sample_count,
        )
    }
}

/// Generate Halton sequence sample for bases 2 and 3.
/// Returns values in range [-0.5, 0.5] for sub-pixel jittering.
fn halton_2_3(index: u32) -> (f32, f32) {
    (halton(index, 2) - 0.5, halton(index, 3) - 0.5)
}

/// Halton sequence generator for a given base.
fn halton(mut index: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    let base_f = base as f32;
    while index > 0 {
        f /= base_f;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

impl GpuCamera {
    /// Create a new camera for rendering.
    pub fn new(
        position: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
        fov: f32,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            position: [position[0], position[1], position[2], 1.0],
            target: [target[0], target[1], target[2], 1.0],
            up: [up[0], up[1], up[2], 0.0],
            fov,
            width,
            height,
            _pad: 0,
        }
    }
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
        let bvh = Bvh::build(brep);
        let (flat_nodes, bvh_faces) = bvh.flatten();

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
        self.materials[0] =
            GpuMaterial::from_pbr(crate::pathtrace::Pbr::from_material_def(mat, tint));
    }
}
