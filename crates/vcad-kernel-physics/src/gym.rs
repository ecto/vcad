//! Gym-style interface for reinforcement learning.

use serde::{Deserialize, Serialize};
use vcad_ir::Document;

use crate::error::PhysicsError;
use crate::world::{GroundConfig, PhysicsWorld};

/// Observation from the robot environment.
///
/// Joint vectors are flattened in [`RobotEnv::joint_ids`] order (the
/// document's `joints` array order, deterministic), with each joint
/// contributing `max(1, ndof)` consecutive entries:
/// - Fixed: 1 entry, always `0.0`
/// - Revolute / Slider / Cylindrical: 1 entry (degrees or mm; deg/s or mm/s)
/// - Ball: 3 entries — rotation exp-coords in degrees; angular velocity in
///   deg/s
/// - Free (floating base): 6 entries — positions
///   `[x, y, z (mm), rx, ry, rz (exp-coords, degrees)]` and velocities
///   `[wx, wy, wz (deg/s), vx, vy, vz (body-frame mm/s)]`. Note the swapped
///   rotation/translation order between positions and velocities (phyz's
///   Featherstone free-joint convention).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Flattened joint positions (degrees for rotational DOFs, mm for
    /// translational DOFs) — see the struct docs for the per-kind layout.
    pub joint_positions: Vec<f64>,
    /// Flattened joint velocities (deg/s for rotational DOFs, mm/s for
    /// translational DOFs) — see the struct docs for the per-kind layout.
    pub joint_velocities: Vec<f64>,
    /// End effector poses as [x, y, z, qw, qx, qy, qz] in meters.
    pub end_effector_poses: Vec<[f64; 7]>,
}

impl Observation {
    /// Create a zero observation with the given dimensions.
    ///
    /// `joint_slots` is the flattened DOF count — the value
    /// [`RobotEnv::observation_dim`] computes, not the number of joints. A
    /// Free joint contributes 6 slots and a Ball joint 3, so passing a joint
    /// count would under-size the vectors for any multi-DOF joint.
    pub fn zeros(joint_slots: usize, num_end_effectors: usize) -> Self {
        Self {
            joint_positions: vec![0.0; joint_slots],
            joint_velocities: vec![0.0; joint_slots],
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
    /// Joint IDs with at least one dof (document order) — Fixed joints are
    /// excluded. Action vectors index against this list.
    actuated_joint_ids: Vec<String>,
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
    /// Ground-plane contact configuration, reapplied on every reset.
    ground: GroundConfig,
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
    /// * `ground` - Ground-plane contact config. `None` enables the default
    ///   ground: plane at z = 0, friction 0.8, inelastic. Pass
    ///   `Some(GroundConfig::disabled())` for the old contact-free dynamics.
    pub fn new(
        doc: Document,
        end_effector_ids: Vec<String>,
        dt: Option<f32>,
        substeps: Option<u32>,
        ground: Option<GroundConfig>,
    ) -> Result<Self, PhysicsError> {
        let ground = ground.unwrap_or_default();
        let mut world = PhysicsWorld::from_document(&doc)?;
        world.set_ground(ground);
        let joint_ids = world.joint_ids();
        let actuated_joint_ids = world.actuated_joint_ids();

        Ok(Self {
            world,
            joint_ids,
            actuated_joint_ids,
            end_effector_ids,
            dt: dt.unwrap_or(1.0 / 240.0),
            substeps: substeps.unwrap_or(4),
            max_steps: 1000,
            current_step: 0,
            initial_doc: doc,
            seed: 0,
            ground,
        })
    }

    /// Reset the environment to initial state.
    ///
    /// Returns the initial observation.
    pub fn reset(&mut self) -> Observation {
        // Rebuild from `initial_doc`, which was already validated via `?` in
        // `new()` and is never mutated afterwards — so this expect should be
        // unreachable. A failure here would indicate an internal invariant
        // violation (e.g. a non-deterministic bug in PhysicsWorld::from_document).
        self.world = PhysicsWorld::from_document(&self.initial_doc)
            .expect("gym reset: PhysicsWorld::from_document failed on a doc that was valid at construction — this should be unreachable");
        self.world.set_ground(self.ground);
        self.joint_ids = self.world.joint_ids();
        self.actuated_joint_ids = self.world.actuated_joint_ids();
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
        let mut positions = Vec::new();
        let mut velocities = Vec::new();

        for joint_id in &self.joint_ids {
            // Each joint contributes max(1, ndof) entries: Fixed joints keep
            // their historical single zero slot; multi-DOF joints (Ball,
            // Free) contribute one entry per DOF, in vcad units — see the
            // `Observation` docs for the exact per-kind layout.
            match self.world.get_joint_dofs(joint_id) {
                Some((q, v)) if !q.is_empty() => {
                    positions.extend(q);
                    velocities.extend(v);
                }
                _ => {
                    positions.push(0.0);
                    velocities.push(0.0);
                }
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

    /// Joint ids in observation order (document order).
    ///
    /// `Observation::joint_positions[i]` / `joint_velocities[i]` index against
    /// this list. Action vectors index against [`Self::actuated_joint_ids`],
    /// which drops zero-dof (Fixed) joints.
    pub fn joint_ids(&self) -> &[String] {
        &self.joint_ids
    }

    /// Actuated joint ids in action order (document order, Fixed joints excluded).
    pub fn actuated_joint_ids(&self) -> &[String] {
        &self.actuated_joint_ids
    }

    /// Get the number of joints (observation joint count, including Fixed joints).
    pub fn num_joints(&self) -> usize {
        self.joint_ids.len()
    }

    /// Get the observation dimension.
    ///
    /// Each joint contributes `max(1, ndof)` position slots plus the same
    /// number of velocity slots (Fixed = 1 zero slot, Ball = 3, Free = 6).
    pub fn observation_dim(&self) -> usize {
        let joint_slots: usize = self
            .joint_ids
            .iter()
            .map(|id| self.world.joint_dof_count(id).max(1))
            .sum();
        joint_slots * 2 + self.end_effector_ids.len() * 7
    }

    /// Get the action dimension: one entry per actuated (non-Fixed) joint.
    pub fn action_dim(&self) -> usize {
        self.actuated_joint_ids.len()
    }

    fn apply_action(&mut self, action: &Action) {
        match action {
            Action::Torque(torques) => {
                for (i, joint_id) in self.actuated_joint_ids.iter().enumerate() {
                    if let Some(&torque) = torques.get(i) {
                        self.world.apply_joint_torque(joint_id, torque);
                    }
                }
            }
            Action::PositionTarget(targets) => {
                for (i, joint_id) in self.actuated_joint_ids.iter().enumerate() {
                    if let Some(&target) = targets.get(i) {
                        self.world.set_joint_position(joint_id, target);
                    }
                }
            }
            Action::VelocityTarget(targets) => {
                for (i, joint_id) in self.actuated_joint_ids.iter().enumerate() {
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
                inertial: None,
            },
        );
        part_defs.insert(
            "link1".to_string(),
            PartDef {
                id: "link1".to_string(),
                name: Some("Link 1".to_string()),
                root: 2,
                default_material: None,
                inertial: None,
            },
        );
        part_defs.insert(
            "link2".to_string(),
            PartDef {
                id: "link2".to_string(),
                name: Some("Link 2".to_string()),
                root: 3,
                default_material: None,
                inertial: None,
            },
        );
        doc.part_defs = Some(part_defs);

        // Instances
        doc.instances = Some(vec![
            Instance {
                id: "base_inst".to_string(),
                part_def_id: "base".to_string(),
                name: Some("Base".to_string()),
                tags: Vec::new(),
                transform: None,
                material: None,
            },
            Instance {
                id: "link1_inst".to_string(),
                part_def_id: "link1".to_string(),
                name: Some("Link 1".to_string()),
                tags: Vec::new(),
                transform: None,
                material: None,
            },
            Instance {
                id: "link2_inst".to_string(),
                part_def_id: "link2".to_string(),
                name: Some("Link 2".to_string()),
                tags: Vec::new(),
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

    /// Three-link chain whose `joints` array is declared in REVERSE of the
    /// BFS discovery order (leaf joint first). Each joint carries a distinct
    /// initial state so a permuted observation ordering is detectable.
    fn create_three_joint_robot_reversed() -> Document {
        let mut doc = Document::new();

        for (node_id, name) in [(1, "base"), (2, "link1"), (3, "link2"), (4, "link3")] {
            doc.nodes.insert(
                node_id,
                vcad_ir::Node {
                    id: node_id,
                    name: Some(name.to_string()),
                    op: vcad_ir::CsgOp::Cube {
                        size: Vec3::new(20.0, 20.0, 100.0),
                    },
                },
            );
        }

        let mut part_defs = HashMap::new();
        for (root, name) in [(1, "base"), (2, "link1"), (3, "link2"), (4, "link3")] {
            part_defs.insert(
                name.to_string(),
                PartDef {
                    id: name.to_string(),
                    name: None,
                    root,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);

        doc.instances = Some(
            ["base", "link1", "link2", "link3"]
                .iter()
                .map(|name| Instance {
                    id: format!("{name}_inst"),
                    part_def_id: name.to_string(),
                    name: None,
                    tags: Vec::new(),
                    transform: None,
                    material: None,
                })
                .collect(),
        );

        let make_joint = |id: &str, parent: &str, child: &str, state: f64| Joint {
            id: id.to_string(),
            name: None,
            parent_instance_id: Some(format!("{parent}_inst")),
            child_instance_id: format!("{child}_inst"),
            parent_anchor: Vec3::new(0.0, 0.0, 50.0),
            child_anchor: Vec3::new(0.0, 0.0, -50.0),
            kind: JointKind::Revolute {
                axis: Vec3::new(0.0, 1.0, 0.0),
                limits: Some((-90.0, 90.0)),
            },
            state,
        };

        // Leaf-most joint first: doc order is the opposite of the order the
        // world builder's BFS from ground discovers them in.
        doc.joints = Some(vec![
            make_joint("joint3", "link2", "link3", 30.0),
            make_joint("joint2", "link1", "link2", 20.0),
            make_joint("joint1", "base", "link1", 10.0),
        ]);

        doc.ground_instance_id = Some("base_inst".to_string());
        doc
    }

    #[test]
    fn joint_observation_order_matches_document_order() {
        let doc = create_three_joint_robot_reversed();
        let env = RobotEnv::new(doc, vec!["link3_inst".to_string()], None, None, None).unwrap();

        // The contract: joint_ids() is doc.joints order, not BFS or HashMap
        // order. Before joint_order landed this permuted run-to-run.
        assert_eq!(env.joint_ids(), ["joint3", "joint2", "joint1"]);

        // Each observation slot must carry the state of the joint at the
        // same index of joint_ids(): 30/20/10 degrees, not any permutation.
        let obs = env.observe();
        assert_eq!(obs.joint_positions.len(), 3);
        assert!(
            (obs.joint_positions[0] - 30.0).abs() < 1e-6,
            "slot 0 should be joint3 (30 deg), got {}",
            obs.joint_positions[0]
        );
        assert!(
            (obs.joint_positions[1] - 20.0).abs() < 1e-6,
            "slot 1 should be joint2 (20 deg), got {}",
            obs.joint_positions[1]
        );
        assert!(
            (obs.joint_positions[2] - 10.0).abs() < 1e-6,
            "slot 2 should be joint1 (10 deg), got {}",
            obs.joint_positions[2]
        );

        // reset() rebuilds the world from the initial doc — ordering must
        // survive the round trip.
        let mut env = env;
        let obs = env.reset();
        assert_eq!(env.joint_ids(), ["joint3", "joint2", "joint1"]);
        assert!((obs.joint_positions[0] - 30.0).abs() < 1e-6);
        assert!((obs.joint_positions[2] - 10.0).abs() < 1e-6);
    }

    /// Repro for the record_simulation kernel trap: base —revolute→ arm
    /// —fixed→ tip. The fixed joint has zero dof, so its q/v offsets point
    /// past the end of the state vectors; installing a motor on it indexed
    /// out of bounds and trapped (unreachable in wasm).
    fn create_robot_with_fixed_joint() -> Document {
        let mut doc = Document::new();

        for (node_id, name, size) in [
            (1, "base", Vec3::new(80.0, 80.0, 30.0)),
            (2, "arm", Vec3::new(80.0, 20.0, 20.0)),
            (3, "tip", Vec3::new(24.0, 24.0, 24.0)),
        ] {
            doc.nodes.insert(
                node_id,
                vcad_ir::Node {
                    id: node_id,
                    name: Some(name.to_string()),
                    op: vcad_ir::CsgOp::Cube { size },
                },
            );
        }

        let mut part_defs = HashMap::new();
        for (root, name) in [(1, "base"), (2, "arm"), (3, "tip")] {
            part_defs.insert(
                name.to_string(),
                PartDef {
                    id: name.to_string(),
                    name: None,
                    root,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);

        doc.instances = Some(
            ["base", "arm", "tip"]
                .iter()
                .map(|name| Instance {
                    id: format!("{name}_inst"),
                    part_def_id: name.to_string(),
                    name: None,
                    tags: Vec::new(),
                    transform: None,
                    material: None,
                })
                .collect(),
        );

        doc.joints = Some(vec![
            Joint {
                id: "swing".to_string(),
                name: None,
                parent_instance_id: Some("base_inst".to_string()),
                child_instance_id: "arm_inst".to_string(),
                parent_anchor: Vec3::new(0.0, 0.0, 30.0),
                child_anchor: Vec3::new(0.0, 10.0, 0.0),
                kind: JointKind::Revolute {
                    axis: Vec3::new(0.0, 0.0, 1.0),
                    limits: Some((-180.0, 180.0)),
                },
                state: 0.0,
            },
            Joint {
                id: "f-tip".to_string(),
                name: None,
                parent_instance_id: Some("arm_inst".to_string()),
                child_instance_id: "tip_inst".to_string(),
                parent_anchor: Vec3::new(80.0, 10.0, 10.0),
                child_anchor: Vec3::new(0.0, 0.0, 0.0),
                kind: JointKind::Fixed,
                state: 0.0,
            },
        ]);

        doc.ground_instance_id = Some("base_inst".to_string());
        doc
    }

    #[test]
    fn fixed_joint_rollout_completes_and_tip_tracks_arm() {
        let doc = create_robot_with_fixed_joint();
        // Track both the fixed child and its parent so we can assert the weld.
        let mut env = RobotEnv::new(
            doc,
            vec!["tip_inst".to_string(), "arm_inst".to_string()],
            None,
            None,
            None,
        )
        .unwrap();

        // Fixed joints appear in observations but contribute zero actuated dof.
        assert_eq!(env.num_joints(), 2);
        assert_eq!(env.action_dim(), 1);
        assert_eq!(env.actuated_joint_ids(), ["swing"]);

        let obs0 = env.reset();
        let rel0 = {
            let [tip, arm] = [obs0.end_effector_poses[0], obs0.end_effector_poses[1]];
            [tip[0] - arm[0], tip[1] - arm[1], tip[2] - arm[2]]
        };
        let dist0 = (rel0[0].powi(2) + rel0[1].powi(2) + rel0[2].powi(2)).sqrt();
        assert!(dist0 > 1e-3, "tip should be offset from arm origin");

        // Short torque-driven rollout — installing any motor on the fixed
        // joint trapped out-of-bounds before zero-dof joints were excluded
        // from the action path. (A small constant torque gives smooth motion;
        // the default PD position gains oscillate on an arm this light.)
        let mut max_abs_pos: f64 = 0.0;
        let mut last = obs0;
        for _ in 0..60 {
            let (obs, _, _) = env.step(Action::Torque(vec![1e-4]));
            max_abs_pos = max_abs_pos.max(obs.joint_positions[0].abs());

            // The fixed joint reads zero and the tip rides the arm at every
            // step: tip-to-arm distance is invariant under the weld.
            assert!(obs.joint_positions[1].abs() < 1e-9);
            let [tip, arm] = [obs.end_effector_poses[0], obs.end_effector_poses[1]];
            let rel = [tip[0] - arm[0], tip[1] - arm[1], tip[2] - arm[2]];
            let dist = (rel[0].powi(2) + rel[1].powi(2) + rel[2].powi(2)).sqrt();
            assert!(
                (dist - dist0).abs() < 1e-6,
                "fixed child should stay welded to its parent: |tip-arm| went {dist0} -> {dist}"
            );
            last = obs;
        }

        // The arm actually swung at some point during the rollout.
        assert!(
            max_abs_pos > 1.0,
            "revolute joint should have moved, max |pos| = {max_abs_pos} deg"
        );
        assert!(last.joint_positions.len() == 2);

        // Position and velocity actions must not trap either.
        env.step(Action::PositionTarget(vec![60.0]));
        env.step(Action::VelocityTarget(vec![10.0]));
    }

    /// A free 100 mm cube dropped from 1 m plus the mandatory (fixed, and
    /// therefore contact-exempt) ground instance parked off to the side.
    fn create_drop_test_doc() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("anchor".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(20.0, 20.0, 20.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("crate".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(100.0, 100.0, 100.0),
                },
            },
        );
        let mut part_defs = HashMap::new();
        for (root, name) in [(1, "anchor"), (2, "crate")] {
            part_defs.insert(
                name.to_string(),
                PartDef {
                    id: name.to_string(),
                    name: None,
                    root,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![
            Instance {
                id: "anchor_inst".to_string(),
                part_def_id: "anchor".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                transform: Some(vcad_ir::Transform3D {
                    translation: Vec3::new(1000.0, 0.0, 0.0),
                    ..Default::default()
                }),
                material: None,
            },
            Instance {
                id: "crate_inst".to_string(),
                part_def_id: "crate".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                // Cube spans z ∈ [0, 100] mm in its own frame; lift it 1 m.
                transform: Some(vcad_ir::Transform3D {
                    translation: Vec3::new(0.0, 0.0, 1000.0),
                    ..Default::default()
                }),
                material: None,
            },
        ]);
        doc.joints = Some(std::vec::Vec::new());
        doc.ground_instance_id = Some("anchor_inst".to_string());
        doc
    }

    /// The M0 acceptance test: a box dropped from 1 m must land on the
    /// ground plane and come to rest — not tunnel through the world.
    #[test]
    fn free_box_drop_lands_and_rests() {
        let doc = create_drop_test_doc();
        // Default ground: on at z = 0, friction 0.8, inelastic.
        let mut env = RobotEnv::new(doc, vec!["crate_inst".to_string()], None, None, None).unwrap();
        env.set_max_steps(100_000);

        let mut min_z = f64::INFINITY;
        let mut last = env.observe();
        assert!((last.end_effector_poses[0][2] - 1.0).abs() < 1e-6);

        // 480 env steps × 4 substeps at 1/240 s = 8 s of sim time; the fall
        // itself takes ~0.45 s.
        for _ in 0..480 {
            let (obs, _, _) = env.step(Action::Torque(vec![]));
            let z = obs.end_effector_poses[0][2];
            assert!(z.is_finite(), "box pose went non-finite");
            min_z = min_z.min(z);
            last = obs;
        }

        // Never tunneled: the body origin (cube bottom face) may dip a
        // little below the plane while the solve catches it, but must not
        // pass through.
        assert!(
            min_z > -0.05,
            "box tunneled through the ground: min z = {min_z} m"
        );
        // At rest ON the floor: origin back within a couple of cm of z = 0
        // (it started at 1 m, so this also proves it actually fell).
        assert!(
            last.end_effector_poses[0][2].abs() < 0.03,
            "box did not come to rest on the floor: final z = {} m",
            last.end_effector_poses[0][2]
        );
    }

    /// The same drop with the ground disabled must fall straight through —
    /// proving the previous test exercises contact rather than some other
    /// floor the dynamics grew.
    #[test]
    fn free_box_drop_without_ground_falls_forever() {
        let doc = create_drop_test_doc();
        let mut env = RobotEnv::new(
            doc,
            vec!["crate_inst".to_string()],
            None,
            None,
            Some(crate::world::GroundConfig::disabled()),
        )
        .unwrap();
        env.set_max_steps(100_000);
        let mut last_z = 1.0;
        for _ in 0..480 {
            let (obs, _, _) = env.step(Action::Torque(vec![]));
            last_z = obs.end_effector_poses[0][2];
        }
        assert!(
            last_z < -1.0,
            "with ground disabled the box should keep falling, final z = {last_z} m"
        );
    }

    /// Fixed base at height with a long pendulum arm: under gravity the arm
    /// swings down and must come to rest ON the floor instead of swinging
    /// through it. Also a stability check — a PD position hold is engaged
    /// while the arm rests against the plane, and nothing may diverge.
    fn create_pendulum_over_floor() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: None,
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(60.0, 60.0, 60.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: None,
                op: vcad_ir::CsgOp::Cube {
                    size: Vec3::new(20.0, 20.0, 300.0),
                },
            },
        );
        let mut part_defs = HashMap::new();
        for (root, name) in [(1, "post"), (2, "arm")] {
            part_defs.insert(
                name.to_string(),
                PartDef {
                    id: name.to_string(),
                    name: None,
                    root,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![
            Instance {
                id: "post_inst".to_string(),
                part_def_id: "post".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                // Post top face at z = 200 mm.
                transform: Some(vcad_ir::Transform3D {
                    translation: Vec3::new(0.0, 0.0, 140.0),
                    ..Default::default()
                }),
                material: None,
            },
            Instance {
                id: "arm_inst".to_string(),
                part_def_id: "arm".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                transform: None,
                material: None,
            },
        ]);
        // Pivot at the post top; the arm's own frame spans z ∈ [0, 300] mm
        // and hangs from its top end — a 300 mm pendulum from a 200 mm-high
        // pivot swings well below z = 0.
        doc.joints = Some(vec![Joint {
            id: "pivot".to_string(),
            name: None,
            parent_instance_id: Some("post_inst".to_string()),
            child_instance_id: "arm_inst".to_string(),
            parent_anchor: Vec3::new(0.0, 0.0, 60.0),
            child_anchor: Vec3::new(10.0, 10.0, 300.0),
            kind: JointKind::Revolute {
                axis: Vec3::new(0.0, 1.0, 0.0),
                limits: None,
            },
            // Start horizontal so it has to swing down into the floor.
            state: 90.0,
        }]);
        doc.ground_instance_id = Some("post_inst".to_string());
        doc
    }

    #[test]
    fn pendulum_rests_on_floor_instead_of_swinging_through() {
        let doc = create_pendulum_over_floor();
        let mut env = RobotEnv::new(doc, vec!["arm_inst".to_string()], None, None, None).unwrap();
        env.set_max_steps(100_000);

        // Phase 1: passive swing under gravity. Track the arm's lowest
        // pose-origin z (the origin is the arm's own frame corner; its
        // lowest mesh point is what actually touches, so allow the ~20 mm
        // arm cross-section plus contact slop).
        let mut min_z = f64::INFINITY;
        let mut last = env.observe();
        for _ in 0..600 {
            let (obs, _, _) = env.step(Action::Torque(vec![0.0]));
            assert!(obs.joint_positions[0].is_finite());
            min_z = min_z.min(obs.end_effector_poses[0][2]);
            last = obs;
        }
        // A frictionless-through-floor swing would carry the origin to
        // roughly -(300 + 200) mm below the pivot at the bottom of the arc.
        assert!(
            min_z > -0.06,
            "arm swung through the floor: min origin z = {min_z} m"
        );
        // It ended up resting against the plane (not oscillating wildly).
        assert!(
            last.joint_velocities[0].abs() < 20.0,
            "arm still moving fast after 10 s: {} deg/s",
            last.joint_velocities[0]
        );

        // Phase 2: engage a PD hold at the resting pose while in contact —
        // the explicit integrator must stay finite and the arm must stay
        // above the floor.
        let hold = last.joint_positions[0];
        for _ in 0..240 {
            let (obs, _, _) = env.step(Action::PositionTarget(vec![hold]));
            assert!(
                obs.joint_positions[0].is_finite() && obs.joint_velocities[0].is_finite(),
                "PD hold in contact diverged"
            );
            assert!(obs.end_effector_poses[0][2] > -0.06);
        }
    }

    #[test]
    fn test_env_creation() {
        let doc = create_simple_robot();
        let env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None, None).unwrap();

        assert_eq!(env.num_joints(), 2);
        assert_eq!(env.action_dim(), 2);
    }

    #[test]
    fn test_env_reset() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None, None).unwrap();

        let obs = env.reset();
        assert_eq!(obs.joint_positions.len(), 2);
        assert_eq!(obs.joint_velocities.len(), 2);
        assert_eq!(obs.end_effector_poses.len(), 1);
    }

    #[test]
    fn test_env_step() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(doc, vec!["link2_inst".to_string()], None, None, None).unwrap();

        env.reset();

        // Step with position target
        let action = Action::PositionTarget(vec![45.0, 30.0]);
        let (obs, _reward, done) = env.step(action);

        assert_eq!(obs.joint_positions.len(), 2);
        assert!(!done); // Should not be done after 1 step
    }
}
