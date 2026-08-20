//! GPU-accelerated ray tracing using wgpu compute shaders.
//!
//! This module provides WebGPU-based ray tracing that renders BRep surfaces
//! directly without tessellation.

mod buffers;
mod pipeline;
pub mod shaders;

pub use buffers::{
    depth_for_frame, halton_jitter, GpuAreaLight, GpuBvhNode, GpuCamera, GpuFace, GpuMaterial,
    GpuRenderState, GpuScene, GpuSceneError, GpuSurface, GpuVec2, BACKGROUND_ENVIRONMENT,
    BACKGROUND_BLACK, BACKGROUND_SKY, CAMERA_BASIS_DERIVED, CAMERA_BASIS_EXPLICIT, DEFAULT_ENV_INTENSITY,
    DEFAULT_FIREFLY_CLAMP, DEFAULT_MAX_DEPTH, DEFAULT_RR_START, MAX_TRAVERSAL_DEPTH,
};
pub use pipeline::RayTracePipeline;

/// Offline (non-interactive) rendering: upload the scene once, accumulate N
/// samples, read back linear HDR radiance once. Native only — it blocks on
/// the device, which would deadlock the browser event loop.
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
pub use pipeline::{OfflineOptions, OfflineResult};
