#![warn(missing_docs)]

//! Physics simulation for vcad robotics using phyz.
//!
//! This crate provides physics simulation capabilities for robot assemblies,
//! enabling dynamics simulation, collision detection, and reinforcement learning
//! through a gym-like interface. Powered by phyz (Featherstone articulated-body
//! dynamics with penalty-based contacts).
//!
//! # Features
//!
//! - Convert vcad assemblies to physics simulation
//! - Joint dynamics with position/velocity/torque control (PD motors)
//! - Collision detection using mesh and box geometries
//! - Gym-style observation/action API for RL training
//!
//! # Example
//!
//! ```ignore
//! use vcad_kernel_physics::{PhysicsWorld, Action, RobotEnv};
//!
//! // Load a document with assembly data
//! let doc = vcad_ir::Document::from_json(&json_str).unwrap();
//!
//! // Create physics world
//! let mut world = PhysicsWorld::from_document(&doc).unwrap();
//!
//! // Step simulation
//! world.step(1.0 / 60.0);
//!
//! // Control joints directly
//! world.set_joint_position("joint1", 45.0);  // 45 degrees
//! ```

/// Mesh-to-collider conversion + mass / inertia estimation.
pub mod colliders;
/// Physics-rollout gradients: `dJ/dθ` of a simulation objective with respect
/// to CAD parameters, via the mass-property factorization (M8).
pub mod diff;
mod error;
mod gym;
/// Joint conversion utilities.
pub mod joints;
/// STL mesh loading for URDF `<mesh>` references.
pub mod stl;
mod world;

pub use diff::{
    contact_rollout_gradient, nominal_mass_props, rollout_gradient, rollout_gradient_adjoint,
    rollout_gradient_via_density, rollout_gradient_with_anchors, rollout_gradient_with_surface,
    surface_gradient, AnchorFdSteps, BodyMassProps, ContactConfig, DiffBody, MassPropFdSteps,
    SurfaceTerm,
};
pub use error::PhysicsError;
pub use gym::{Action, Observation, RobotEnv};
pub use world::{JointState, PhysicsWorld};
