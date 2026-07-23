//! GPU-accelerated geometry operations for vcad.
//!
//! This crate provides WebGPU-based compute shaders for geometry processing:
//! - Creased normal computation
//! - Mesh decimation for LOD generation
//! - Wavefront maze-routing distance fields (autorouter spike)

#![warn(missing_docs)]

mod context;
mod decimate;
mod normals;
pub mod router_state;
pub mod wavefront;
pub mod wavefront_batch;

pub use context::{GpuContext, GpuError};
pub use decimate::{decimate_mesh, DecimationResult};
pub use normals::compute_creased_normals;
