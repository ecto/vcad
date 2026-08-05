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
/// Reinforcement learning: derivative-free policy search over `RobotEnv`.
pub mod rl;
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

/// Raw phyz state read back from a simulator, in **physics units**.
///
/// Named `RawState` rather than `Observation` on purpose. There is a
/// `vcad_kernel_physics::Observation` too, and it is a different type with a
/// different shape *and different units* — degrees and millimetres, plus base
/// pose, base velocity and contact channels. When both were called
/// `Observation`, passing one where the other was expected compiled fine in
/// any module that imported only one of them, and scaled every joint angle by
/// 180/pi with no error at any layer.
///
/// If you want the thing a policy consumes, use
/// [`BatchSimPipeline::batch_observe_gym`] — it decodes through the CPU env's
/// own conversions rather than reimplementing them here.
#[derive(Debug, Clone)]
pub struct RawState {
    /// Joint positions in physics units: radians, metres. A `Free` joint's
    /// six slots are angular-first (`[rx, ry, rz, x, y, z]`).
    pub joint_positions: Vec<f64>,
    /// Joint velocities in physics units: rad/s, m/s. Angular-first for a
    /// `Free` joint, and its linear part is body-frame.
    pub joint_velocities: Vec<f64>,
}
