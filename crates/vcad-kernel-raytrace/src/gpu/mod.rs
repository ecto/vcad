//! GPU-accelerated ray tracing using wgpu compute shaders.
//!
//! This module provides WebGPU-based ray tracing that renders BRep surfaces
//! directly without tessellation.

mod buffers;
mod pipeline;
pub mod shaders;

pub use buffers::{
    GpuBvhNode, GpuCamera, GpuFace, GpuMaterial, GpuRenderState, GpuScene, GpuSceneError,
    GpuSurface, GpuVec2,
};
pub use pipeline::RayTracePipeline;
