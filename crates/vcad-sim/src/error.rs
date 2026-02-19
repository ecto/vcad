//! Error types for the simulation pipeline.

use thiserror::Error;

/// Errors that can occur in the simulation pipeline.
#[derive(Error, Debug)]
pub enum SimError {
    /// Document has no assembly data.
    #[error("Document has no assembly data (no instances or joints)")]
    NoAssembly,

    /// Physics world construction failed.
    #[error("Failed to build physics world: {0}")]
    Physics(#[from] vcad_kernel_physics::PhysicsError),

    /// GPU initialization failed.
    #[error("GPU initialization failed: {0}")]
    Gpu(String),

    /// Invalid action dimensions.
    #[error("Expected {expected} actions, got {got}")]
    ActionMismatch {
        /// Expected number of actions.
        expected: usize,
        /// Actual number of actions.
        got: usize,
    },
}
