//! GPU-accelerated geometry operations for vcad.
//!
//! This crate provides WebGPU-based compute shaders for geometry processing:
//! - Creased normal computation
//! - Mesh decimation for LOD generation
//! - Wavefront maze-routing distance fields (autorouter spike)

#![warn(missing_docs)]

pub mod cost_model;
mod decimate;
pub mod narrowphase;
mod normals;
pub mod router_state;
pub mod wavefront;
pub mod wavefront_batch;

// The wgpu device, adapter limits and buffer-mapping helpers moved to
// `kosm-render`: the raytrace pipeline that dictated them lives there now.
// Re-exported so every caller in this workspace keeps one `GpuContext` type.
pub use decimate::{decimate_mesh, DecimationResult};
pub use kosm_render::gpu::{GpuContext, GpuError};
pub use normals::compute_creased_normals;
