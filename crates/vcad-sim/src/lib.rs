#![warn(missing_docs)]

//! Unified simulation pipeline for vcad.
//!
//! Provides both CPU single-environment (`SimPipeline`) and GPU batch
//! (`BatchSimPipeline`) simulation from vcad Documents.
//!
//! # Architecture
//!
//! ```text
//! vcad-ir Document (assembly) → phyz Model + State → Simulation
//!   ├── SimPipeline        (CPU, single env, phyz::Simulator)
//!   └── BatchSimPipeline   (GPU, N parallel envs, phyz-gpu::GpuBatchSimulator)
//! ```

mod batch;
mod error;
mod single;

pub use batch::BatchSimPipeline;
pub use error::SimError;
pub use single::SimPipeline;

/// Result of a simulation step.
#[derive(Debug, Clone)]
pub struct StepResult {
    /// Joint positions after step (degrees for revolute, mm for prismatic).
    pub joint_positions: Vec<f64>,
    /// Joint velocities after step.
    pub joint_velocities: Vec<f64>,
    /// Whether the simulation terminated (e.g. constraint violation).
    pub done: bool,
}

/// Observation of the current simulation state.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Joint positions (degrees for revolute, mm for prismatic).
    pub joint_positions: Vec<f64>,
    /// Joint velocities.
    pub joint_velocities: Vec<f64>,
}
