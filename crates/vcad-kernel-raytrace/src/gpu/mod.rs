//! The BRep half of kosm-render's GPU seam.
//!
//! The renderer — the integrator, the BSDF, the environment, the accumulator,
//! the history and the denoiser — lives in `kosm-render`. This module packs
//! trimmed analytic faces into the five storage buffers it leaves for
//! geometry, and supplies the WGSL that traces them. Everything else here is a
//! re-export, so callers see one API.

mod buffers;
mod geometry;
pub mod shaders;

pub use buffers::{
    GpuBvhNode, GpuFace, GpuScene, GpuSceneError, GpuSurface, GpuVec2, MAX_BVH_NODES, MAX_FACES,
    MAX_INNER_LOOPS, MAX_SURFACES, MAX_TRIM_VERTS, SURFACE_TYPE_TRIANGLE,
};
pub use geometry::BrepGeometry;

// The renderer, re-exported. `RayTracePipeline::new` is the one signature that
// changed: it now takes the geometry module the pipeline is built for, which
// for this crate is always `BrepGeometry::module()`.
pub use kosm_render::gpu::{
    depth_for_frame, halton_jitter, validate_tree_depth, GeometryModule, GeometrySlab,
    GpuAreaLight, GpuCamera, GpuContext, GpuDenoiseParams, GpuError, GpuGeometry, GpuMaterial,
    GpuRenderState, History, HistoryBuffers, HistoryPipeline, RayTracePipeline, ResidentScene,
    SceneRef, BACKGROUND_BLACK, BACKGROUND_ENVIRONMENT, BACKGROUND_SKY, CAMERA_BASIS_DERIVED,
    CAMERA_BASIS_EXPLICIT, DEFAULT_ENV_INTENSITY, DEFAULT_FIREFLY_CLAMP, DEFAULT_MAX_DEPTH,
    DEFAULT_RR_START, FLAG_CAMERA_VISIBLE_LIGHTS, FLAG_RAW_SAMPLE, MAX_DENOISE_ITERS,
    MAX_TRAVERSAL_DEPTH,
};

/// Offline (non-interactive) rendering: upload the scene once, accumulate N
/// samples, read back linear HDR radiance once. Native only — it blocks on
/// the device, which would deadlock the browser event loop.
#[cfg(not(target_arch = "wasm32"))]
pub use kosm_render::gpu::{OfflineOptions, OfflineResult};

/// A [`RayTracePipeline`] over the BRep geometry module — the only kind this
/// crate builds. Sugar for `crate::gpu::brep_pipeline(ctx, &BrepGeometry::module())`.
pub fn brep_pipeline(ctx: &GpuContext) -> Result<RayTracePipeline, GpuError> {
    RayTracePipeline::new(ctx, &BrepGeometry::module())
}
