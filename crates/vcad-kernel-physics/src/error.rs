//! Error types for physics simulation.

use thiserror::Error;

/// Errors that can occur during physics simulation.
#[derive(Error, Debug)]
pub enum PhysicsError {
    /// Document has no assembly data.
    #[error("Document has no assembly data (no instances or joints)")]
    NoAssembly,

    /// Missing part definition.
    #[error("Part definition not found: {0}")]
    MissingPartDef(String),

    /// Missing instance.
    #[error("Instance not found: {0}")]
    MissingInstance(String),

    /// Missing joint.
    #[error("Joint not found: {0}")]
    MissingJoint(String),

    /// Failed to create collision shape.
    #[error("Failed to create collision shape for {name}: {reason}")]
    CollisionShape {
        /// Part/instance name.
        name: String,
        /// Reason for failure.
        reason: String,
    },

    /// Invalid joint configuration.
    #[error("Invalid joint configuration: {0}")]
    InvalidJoint(String),

    /// No ground instance specified.
    #[error("No ground instance specified in document")]
    NoGroundInstance,

    /// Termination conditions reference a base pose no instance provides.
    ///
    /// Fail-closed guard: with `base_height_below` / `base_tilt_above_deg`
    /// configured but no observable base pose, those checks would silently
    /// never fire and every episode would run to `max_steps` — a run that
    /// reports confident survival while measuring nothing.
    #[error(
        "termination config sets base-pose conditions, but base instance {base_instance_id:?} \
         has no observable pose — the checks would silently never fire. Point \
         `EnvConfig::base_instance_id` at the floating-base instance (or import the \
         floating-base variant of the robot)"
    )]
    UnobservableBase {
        /// The configured (or defaulted) base instance id, if any.
        base_instance_id: Option<String>,
    },

    /// Evaluation error.
    #[error("Failed to evaluate geometry: {0}")]
    Evaluation(String),
}
