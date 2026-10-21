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
///
/// Natively this also catches a driver that loses the device while compiling
/// the tracer (seen on NVIDIA's Linux Vulkan driver): wgpu still hands back a
/// pipeline, and the first buffer upload then panics with an "invalid buffer"
/// error that points nowhere near the cause. Reported as an error here
/// instead, so callers can say so or fall back to the CPU.
pub fn brep_pipeline(ctx: &GpuContext) -> Result<RayTracePipeline, GpuError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::{Arc, Mutex};
        let lost: Arc<Mutex<Option<String>>> = Arc::default();
        let sink = lost.clone();
        ctx.device.set_device_lost_callback(move |reason, msg| {
            *sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{reason:?}: {msg}"));
        });
        let pipeline = RayTracePipeline::new(ctx, &BrepGeometry::module())?;
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
        if let Some(why) = lost.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return Err(GpuError::InvalidInput(format!(
                "the GPU device was lost while building the path-tracing pipeline \
                 ({why}); the driver rejected the tracer shader"
            )));
        }
        Ok(pipeline)
    }
    #[cfg(target_arch = "wasm32")]
    RayTracePipeline::new(ctx, &BrepGeometry::module())
}
