//! Joint conversion from vcad to phyz.

use phyz::phyz_math::{Mat3, SpatialTransform, Vec3};
use phyz::phyz_model::Joint as PhyzJoint;
use vcad_ir::{Joint as VcadJoint, JointKind};

use crate::error::PhysicsError;

/// Default proportional gain for PD motor control.
pub const DEFAULT_MOTOR_KP: f64 = 1000.0;
/// Default derivative gain for PD motor control.
pub const DEFAULT_MOTOR_KD: f64 = 100.0;
/// Default maximum force/torque for motor control.
pub const DEFAULT_MAX_FORCE: f64 = 1000.0;

/// Motor control mode.
#[derive(Debug, Clone, Copy)]
pub enum MotorMode {
    /// PD position control.
    Position,
    /// Velocity control.
    Velocity,
    /// Direct torque/force.
    Torque,
}

/// Motor target for PD control of a joint.
#[derive(Debug, Clone)]
pub struct MotorTarget {
    /// Control mode.
    pub mode: MotorMode,
    /// Target value (radians for revolute, meters for prismatic).
    pub target: f64,
    /// Proportional gain.
    pub kp: f64,
    /// Derivative gain.
    pub kd: f64,
    /// Maximum force/torque.
    pub max_force: f64,
}

impl Default for MotorTarget {
    fn default() -> Self {
        Self {
            mode: MotorMode::Position,
            target: 0.0,
            kp: DEFAULT_MOTOR_KP,
            kd: DEFAULT_MOTOR_KD,
            max_force: DEFAULT_MAX_FORCE,
        }
    }
}

impl MotorTarget {
    /// Compute the control torque given current position and velocity.
    pub fn compute_torque(&self, position: f64, velocity: f64) -> f64 {
        let torque = match self.mode {
            MotorMode::Position => self.kp * (self.target - position) - self.kd * velocity,
            MotorMode::Velocity => self.kd * (self.target - velocity),
            MotorMode::Torque => self.target,
        };
        torque.clamp(-self.max_force, self.max_force)
    }
}

/// Create a phyz joint from a vcad joint definition.
///
/// Returns the phyz Joint and the parent-to-joint spatial transform.
pub fn vcad_joint_to_phyz(joint: &VcadJoint) -> Result<PhyzJoint, PhysicsError> {
    // Convert parent anchor from mm to meters — this becomes the translation
    // in the parent-to-joint transform.
    let anchor = Vec3::new(
        joint.parent_anchor.x / 1000.0,
        joint.parent_anchor.y / 1000.0,
        joint.parent_anchor.z / 1000.0,
    );

    match &joint.kind {
        JointKind::Fixed => {
            let xform = SpatialTransform::new(Mat3::identity(), anchor);
            Ok(PhyzJoint::fixed(xform))
        }
        JointKind::Revolute { axis, limits } => {
            // phyz revolute joints rotate about Z in joint frame.
            // We need to orient the joint frame so that Z aligns with the desired axis.
            let axis_vec = Vec3::new(axis.x, axis.y, axis.z).normalize();
            let rot = rotation_aligning_z_to(axis_vec);
            let xform = SpatialTransform::new(rot, anchor);

            let mut phyz_joint = PhyzJoint::revolute(xform);

            if let Some((lower, upper)) = limits {
                phyz_joint.limits = Some([lower.to_radians(), upper.to_radians()]);
            }

            Ok(phyz_joint)
        }
        JointKind::Slider { axis, limits } => {
            let axis_vec = Vec3::new(axis.x, axis.y, axis.z).normalize();
            let xform = SpatialTransform::new(Mat3::identity(), anchor);

            let mut phyz_joint = PhyzJoint::prismatic(xform, axis_vec);

            // Convert limits from mm to meters
            if let Some((lower, upper)) = limits {
                phyz_joint.limits = Some([*lower / 1000.0, *upper / 1000.0]);
            }

            Ok(phyz_joint)
        }
        JointKind::Cylindrical { axis } => {
            // Approximate as revolute (primary DOF)
            let axis_vec = Vec3::new(axis.x, axis.y, axis.z).normalize();
            let rot = rotation_aligning_z_to(axis_vec);
            let xform = SpatialTransform::new(rot, anchor);
            Ok(PhyzJoint::revolute(xform))
        }
        JointKind::Ball => {
            let xform = SpatialTransform::new(Mat3::identity(), anchor);
            Ok(PhyzJoint::spherical(xform))
        }
    }
}

/// Compute a rotation matrix that maps the Z unit vector to the given axis.
fn rotation_aligning_z_to(target: Vec3) -> Mat3 {
    let z = Vec3::new(0.0, 0.0, 1.0);
    let dot = z.dot(target);

    if dot > 0.9999 {
        return Mat3::identity();
    }
    if dot < -0.9999 {
        // 180° rotation about X
        return Mat3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    }

    let cross = z.cross(target);
    let s = cross.norm();
    let c = dot;
    let vx = phyz::phyz_math::skew(&cross.normalize());

    // Rodrigues: R = I + sin(θ) * [v]× + (1 - cos(θ)) * [v]×²
    Mat3::identity() + vx * s + vx * vx * (1.0 - c)
}

/// Convert joint state from vcad units to physics units.
///
/// - Revolute: degrees → radians
/// - Slider: mm → meters
pub fn convert_state_to_physics(kind: &JointKind, state: f64) -> f64 {
    match kind {
        JointKind::Revolute { .. } | JointKind::Cylindrical { .. } | JointKind::Ball => {
            state.to_radians()
        }
        JointKind::Slider { .. } => state / 1000.0,
        JointKind::Fixed => 0.0,
    }
}

/// Convert joint state from physics units to vcad units.
///
/// - Revolute: radians → degrees
/// - Slider: meters → mm
pub fn convert_state_from_physics(kind: &JointKind, state: f64) -> f64 {
    match kind {
        JointKind::Revolute { .. } | JointKind::Cylindrical { .. } | JointKind::Ball => {
            state.to_degrees()
        }
        JointKind::Slider { .. } => state * 1000.0,
        JointKind::Fixed => 0.0,
    }
}

/// Get the number of DOFs for a vcad joint kind.
pub fn joint_ndof(kind: &JointKind) -> usize {
    match kind {
        JointKind::Fixed => 0,
        JointKind::Revolute { .. } | JointKind::Slider { .. } | JointKind::Cylindrical { .. } => 1,
        JointKind::Ball => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Vec3 as VcadVec3;

    #[test]
    fn test_revolute_joint_conversion() {
        let joint = VcadJoint {
            id: "test".to_string(),
            name: Some("test".to_string()),
            parent_instance_id: Some("parent".to_string()),
            child_instance_id: "child".to_string(),
            parent_anchor: VcadVec3::new(0.0, 0.0, 100.0), // 100mm
            child_anchor: VcadVec3::new(0.0, 0.0, 0.0),
            kind: JointKind::Revolute {
                axis: VcadVec3::new(0.0, 0.0, 1.0),
                limits: Some((-90.0, 90.0)),
            },
            state: 0.0,
        };

        let phyz_joint = vcad_joint_to_phyz(&joint).unwrap();

        // Check that joint was created and has correct limits
        assert!(phyz_joint.limits.is_some());
        let [lower, upper] = phyz_joint.limits.unwrap();
        assert!((lower - (-std::f64::consts::FRAC_PI_2)).abs() < 0.01);
        assert!((upper - std::f64::consts::FRAC_PI_2).abs() < 0.01);
    }

    #[test]
    fn test_state_conversion() {
        let revolute = JointKind::Revolute {
            axis: VcadVec3::new(0.0, 0.0, 1.0),
            limits: None,
        };

        // 90 degrees should become ~1.57 radians
        let physics_state = convert_state_to_physics(&revolute, 90.0);
        assert!((physics_state - std::f64::consts::FRAC_PI_2).abs() < 0.01);

        // And back
        let vcad_state = convert_state_from_physics(&revolute, physics_state);
        assert!((vcad_state - 90.0).abs() < 0.1);
    }

    #[test]
    fn test_motor_target_position() {
        let motor = MotorTarget {
            mode: MotorMode::Position,
            target: 1.0,
            kp: 100.0,
            kd: 10.0,
            max_force: 50.0,
        };
        // At position 0, velocity 0: torque = kp * (1 - 0) - kd * 0 = 100, clamped to 50
        let torque = motor.compute_torque(0.0, 0.0);
        assert!((torque - 50.0).abs() < 0.01);
    }
}
