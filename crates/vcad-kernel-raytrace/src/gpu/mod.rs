//! GPU-accelerated ray tracing using wgpu compute shaders.
//!
//! This module provides WebGPU-based ray tracing that renders BRep surfaces
//! directly without tessellation.

mod buffers;
#[cfg(feature = "gpu")]
mod history;
mod pipeline;
#[cfg(feature = "gpu")]
mod resident;
pub mod shaders;

pub use buffers::{
    depth_for_frame, GpuAreaLight, GpuBvhNode, GpuCamera, GpuFace, GpuMaterial, GpuRenderState,
    GpuScene, GpuSceneError, GpuSurface, GpuVec2, DEFAULT_ENV_INTENSITY, DEFAULT_FIREFLY_CLAMP,
    DEFAULT_MAX_DEPTH, DEFAULT_RR_START,
};
#[cfg(feature = "gpu")]
pub use history::{GpuDenoiseParams, History, HistoryBuffers, HistoryPipeline, MAX_DENOISE_ITERS};
pub use pipeline::RayTracePipeline;
#[cfg(feature = "gpu")]
pub use resident::ResidentScene;
