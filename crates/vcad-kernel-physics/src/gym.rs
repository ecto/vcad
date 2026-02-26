//! Gym-style interface for reinforcement learning.

use serde::{Deserialize, Serialize};
use vcad_ir::Document;

use crate::error::PhysicsError;
use crate::world::PhysicsWorld;

/// Observation from the robot environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Joint positions (radians for revolute, meters for prismatic).
    pub joint_positions: Vec<f64>,
    /// Joint velocities (rad/s for revolute, m/s for prismatic).
    pub joint_velocities: Vec<f64>,
    /// End effector poses as [x, y, z, qw, qx, qy, qz] in meters.
    pub end_effector_poses: Vec<[f64; 7]>,
}

impl Observation {
    /// Create a zero observation with the given dimensions.
    pub fn zeros(num_joints: usize, num_end_effectors: usize) -> Self {
        Self {
            joint_positions: vec![0.0; num_joints],
            joint_velocities: vec![0.0; num_joints],
            end_effector_poses: vec![[0.0; 7]; num_end_effectors],
        }
    }
}

/// Action to apply to the robot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    /// Torque/force commands for each joint (Nm or N).
    Torque(Vec<f64>),
    /// Position targets for each joint (degrees or mm).
    PositionTarget(Vec<f64>),
    /// Velocity targets for each joint (deg/s or mm/s).
    VelocityTarget(Vec<f64>),
}

/// Robot environment for RL training.
pub struct RobotEnv {
    /// The physics world.
    world: PhysicsWorld,
    /// Joint IDs in order.
    joint_ids: Vec<String>,
    /// End effector instance IDs.
    end_effector_ids: Vec<String>,
    /// Simulation timestep.
    dt: f32,
    /// Number of physics steps per environment step.
    substeps: u32,
    /// Maximum episode length.
    max_steps: u32,
    /// Current step count.
    current_step: u32,
    /// Initial document for reset.
    initial_doc: Document,
    /// Random seed.
    seed: u64,
}

impl RobotEnv {
    /// Create a new robot environment from a vcad Document.
    ///
    /// # Arguments
    ///
    /// * `doc` - The vcad Document describing the robot
    /// * `end_effector_ids` - Instance IDs to track as end effectors
    /// * `dt` - Base simulation timestep in seconds (default: 1/240)
    /// * `substeps` - Number of physics steps per environment step (default: 4)
    pub fn new(
        doc: Document,
        end_effector_ids: Vec<String>,
        dt: Option<f32>,
        substeps: Option<u32>,
    ) -> Result<Self, PhysicsError> {
        let world = PhysicsWorld::from_document(&doc)?;
        let joint_ids = world.joint_ids();

        Ok(Self {
            world,
            joint_ids,
            end_effector_ids,
            dt: dt.unwrap_or(1.0 / 240.0),
            substeps: substeps.unwrap_or(4),
            max_steps: 1000,
            current_step: 0,
            initial_doc: doc,
            seed: 0,
        })
    }

    /// Reset the environment to initial state.
    ///
    /// Returns the initial observation.
    pub fn reset(&mut self) -> Observation {
        // Recreate physics world from initial document
        self.world =
            PhysicsWorld::from_document(&self.initial_doc).expect("Failed to reset physics world");
        self.joint_ids = self.world.joint_ids();
        self.current_step = 0;

        self.observe()
    }

    /// Step the environment with an action.
    ///
    /// Returns (observation, reward, done).
    pub fn step(&mut self, action: Action) -> (Observation, f64, bool) {
        // Apply action
        self.apply_action(&action);

        // Step physics multiple times
        for _ in 0..self.substeps {
            self.world.step(self.dt);
        }

        self.current_step += 1;

        // Get observation
        let obs = self.observe();

        // Compute reward (placeholder - should be customized per task)
        let reward = self.compute_reward(&obs);

        // Check termination
        let done = self.current_step >= self.max_steps || self.is_terminated(&obs);

        (obs, reward, done)
    }

    /// Get current observation without stepping.
    pub fn observe(&self) -> Observation {
        let joint_states = self.world.get_joint_states();

        let mut positions = Vec::with_capacity(self.joint_ids.len());
        let mut velocities = Vec::with_capacity(self.joint_ids.len());

        for joint_id in &self.joint_ids {
            if let Some(state) = joint_states.get(joint_id) {
                // Values are already in vcad units (degrees/mm) from get_joint_states()
                // which calls convert_state_from_physics() internally
                positions.push(state.position);
                velocities.push(state.velocity);
            } else {
                positions.push(0.0);
                velocities.push(0.0);
            }
        }

        let mut end_effector_poses = Vec::with_capacity(self.end_effector_ids.len());
        for ee_id in &self.end_effector_ids {
            if let Some((pos, quat)) = self.world.get_instance_pose(ee_id) {
                end_effector_poses
                    .push([pos[0], pos[1], pos[2], quat[0], quat[1], quat[2], quat[3]]);
            } else {
                end_effector_poses.push([0.0; 7]);
            }
        }

        Observation {
            joint_positions: positions,
            joint_velocities: velocities,
            end_effector_poses,
        }
    }

    /// Set the random seed.
    pub fn seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// Set the maximum episode length.
    pub fn set_max_steps(&mut self, max_steps: u32) {
        self.max_steps = max_steps;
    }

    /// Get the number of joints (action dimension for position/velocity control).
    pub fn num_joints(&self) -> usize {
        self.joint_ids.len()
    }

    /// Get the observation dimension.
    pub fn observation_dim(&self) -> usize {
        self.joint_ids.len() * 2 + self.end_effector_ids.len() * 7
    }

    /// Get the action dimension (for torque control).
    pub fn action_dim(&self) -> usize {
        self.joint_ids.len()
    }

    fn apply_action(&mut self, action: &Action) {
        match action {
            Action::Torque(torques) => {
                for (i, joint_id) in self.joint_ids.iter().enumerate() {
                    if let Some(&torque) = torques.get(i) {
                        self.world.apply_joint_torque(joint_id, torque);
                    }
                }
            }
            Action::PositionTarget(targets) => {
                for (i, joint_id) in self.joint_ids.iter().enumerate() {
                    if let Some(&target) = targets.get(i) {
                        self.world.set_joint_position(joint_id, target);
                    }
                }
            }
            Action::VelocityTarget(targets) => {
                for (i, joint_id) in self.joint_ids.iter().enumerate() {
                    if let Some(&target) = targets.get(i) {
                        self.world.set_joint_velocity(joint_id, target);
                    }
                }
            }
        }
    }

    fn compute_reward(&self, _obs: &Observation) -> f64 {
        // Placeholder reward - should be customized per task
        // Common rewards:
        // - Distance to goal
        // - Energy penalty
        // - Smoothness penalty
        // - Success bonus
        0.0
    }

    fn is_terminated(&self, obs: &Observation) -> bool {
        // Check for invalid states (e.g., robot fell over)
        // Placeholder - should be customized per task
        for pose in &obs.end_effector_poses {
            // Check if end effector is below ground
            if pose[2] < -1.0 {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_ir::{Instance, Joint, JointKind, PartDef, Vec3};

    fn create_simple_robot() -> Document {
        let mut doc = Document::new();

        // Add geometry nodes
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("base".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("link1".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(20.0, 20.0, 100.0),
                },
            },
        );
        doc.nodes.insert(
            3,
            vcad_ir::Node {
                id: 3,
                name: Some("link2".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(20.0, 20.0, 100.0),
                },
            },
        );

        // Part definitions
        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".to_string(),
            PartDef {
                id: "base".to_string(),
                name: Some("Base".to_string()),
                root: 1,
                default_material: None,
            },
        );
        part_defs.insert(
            "link1".to_string(),
            PartDef {
                id: "link1".to_string(),
                name: Some("Link 1".to_string()),
                root: 2,
                default_material: None,
            },
        );
        part_defs.insert(
            "link2".to_string(),
            PartDef {
                id: "link2".to_string(),
                name: Some("Link 2".to_string()),
                root: 3,
                default_material: None,
            },
        );
        doc.part_defs = Some(part_defs);

        // Instances
        doc.instances = Some(vec![
            Instance {
                id: "base_inst".to_string(),
                part_def_id: "base".to_string(),
                name: Some("Base".to_string()),
                transform: None,
                material: None,
            },
            Instance {
                id: "link1_inst".to_string(),
                part_def_id: "link1".to_string(),
                name: Some("Link 1".to_string()),
                transform: None,
                material: None,
            },
            Instance {
                id: "link2_inst".to_string(),
                part_def_id: "link2".to_string(),
                name: Some("Link 2".to_string()),
                transform: None,
                material: None,
            },
        ]);

        // Joints
        doc.joints = Some(vec![
            Joint {
                id: "joint1".to_string(),
                name: Some("Joint 1".to_string()),
                parent_instance_id: Some("base_inst".to_string()),
                child_instance_id: "link1_inst".to_string(),
                parent_anchor: Vec3::new(0.0, 0.0, 25.0),
                child_anchor: Vec3::new(0.0, 0.0, -50.0),
                kind: JointKind::Revolute {
                    axis: Vec3::new(0.0, 1.0, 0.0),
                    limits: Some((-90.0, 90.0)),
                },
                state: 0.0,
            },
            Joint {
                id: "joint2".to_string(),
                name: Some("Joint 2".to_string()),
                parent_instance_id: Some("link1_inst".to_string()),
                child_instance_id: "link2_inst".to_string(),
                parent_anchor: Vec3::new(0.0, 0.0, 50.0),
                child_anchor: Vec3::new(0.0, 0.0, -50.0),
                kind: JointKind::Revolute {
                    axis: Vec3::new(0.0, 1.0, 0.0),
                    limits: Some((-90.0, 90.0)),
                },
                state: 0.0,
            },
        ]);

        doc.ground_instance_id = Some("base_inst".to_string());

        doc
    }

    #[test]
    fn test_env_creation() {
        let doc = create_simple_robot();
        let env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None).unwrap();

        assert_eq!(env.num_joints(), 2);
        assert_eq!(env.action_dim(), 2);
    }

    #[test]
    fn test_env_reset() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None).unwrap();

        let obs = env.reset();
        assert_eq!(obs.joint_positions.len(), 2);
        assert_eq!(obs.joint_velocities.len(), 2);
        assert_eq!(obs.end_effector_poses.len(), 1);
    }

    #[test]
    fn test_env_step() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None).unwrap();

        env.reset();

        // Step with position target
        let action = Action::PositionTarget(vec![45.0, 30.0]);
        let (obs, reward, done) = env.step(action);

        assert_eq!(obs.joint_positions.len(), 2);
        assert!(!done); // Should not be done after 1 step
    }
}
