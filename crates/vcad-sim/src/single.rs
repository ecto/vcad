//! CPU single-environment simulation pipeline.

use vcad_ir::Document;
use vcad_kernel_physics::PhysicsWorld;

use crate::error::SimError;
use crate::{Observation, StepResult};

/// CPU single-environment simulation pipeline.
///
/// Wraps `PhysicsWorld` from vcad-kernel-physics with a simplified
/// step/observe/reset API suitable for RL training loops.
pub struct SimPipeline {
    world: PhysicsWorld,
    doc: Document,
    nv: usize,
}

impl SimPipeline {
    /// Create a simulation pipeline from a vcad Document.
    ///
    /// The document must contain assembly data (instances, joints, ground).
    pub fn from_document(doc: &Document) -> Result<Self, SimError> {
        let world = PhysicsWorld::from_document(doc)?;
        let nv = world.joint_ids().len();
        Ok(Self {
            world,
            doc: doc.clone(),
            nv,
        })
    }

    /// Step the simulation with the given actions (torques).
    ///
    /// Actions are applied as direct torques to joints in alphabetical order
    /// by joint ID. Returns the resulting state.
    pub fn step(&mut self, actions: &[f64], dt: f32) -> Result<StepResult, SimError> {
        if actions.len() != self.nv {
            return Err(SimError::ActionMismatch {
                expected: self.nv,
                got: actions.len(),
            });
        }

        // Apply torques to joints
        let mut joint_ids: Vec<String> = self.world.joint_ids();
        joint_ids.sort();
        for (i, jid) in joint_ids.iter().enumerate() {
            self.world.apply_joint_torque(jid, actions[i]);
        }

        // Step physics
        self.world.step(dt);

        // Read back state
        let states = self.world.get_joint_states();
        let mut positions = Vec::with_capacity(self.nv);
        let mut velocities = Vec::with_capacity(self.nv);
        for jid in &joint_ids {
            if let Some(s) = states.get(jid) {
                positions.push(s.position);
                velocities.push(s.velocity);
            }
        }

        Ok(StepResult {
            joint_positions: positions,
            joint_velocities: velocities,
            done: false,
        })
    }

    /// Observe the current simulation state without stepping.
    pub fn observe(&self) -> Observation {
        let states = self.world.get_joint_states();
        let mut joint_ids: Vec<String> = self.world.joint_ids();
        joint_ids.sort();

        let mut positions = Vec::with_capacity(self.nv);
        let mut velocities = Vec::with_capacity(self.nv);
        for jid in &joint_ids {
            if let Some(s) = states.get(jid) {
                positions.push(s.position);
                velocities.push(s.velocity);
            }
        }

        Observation {
            joint_positions: positions,
            joint_velocities: velocities,
        }
    }

    /// Reset the simulation to its initial state.
    pub fn reset(&mut self) -> Result<(), SimError> {
        self.world = PhysicsWorld::from_document(&self.doc)?;
        Ok(())
    }

    /// Get the number of action dimensions (joint DOFs).
    pub fn action_dim(&self) -> usize {
        self.nv
    }

    /// Get the sorted list of joint IDs.
    pub fn joint_ids(&self) -> Vec<String> {
        let mut ids = self.world.joint_ids();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vcad_ir::{CsgOp, Instance, Joint, JointKind, Node, PartDef, Vec3};

    fn test_document() -> Document {
        let mut doc = Document::new();

        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("base_geom".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            Node {
                id: 2,
                name: Some("arm_geom".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(20.0, 20.0, 100.0),
                },
            },
        );

        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".into(),
            PartDef {
                id: "base".into(),
                name: Some("Base".into()),
                root: 1,
                default_material: None,
            },
        );
        part_defs.insert(
            "arm".into(),
            PartDef {
                id: "arm".into(),
                name: Some("Arm".into()),
                root: 2,
                default_material: None,
            },
        );
        doc.part_defs = Some(part_defs);

        doc.instances = Some(vec![
            Instance {
                id: "base_inst".into(),
                part_def_id: "base".into(),
                name: Some("Base".into()),
                transform: None,
                material: None,
            },
            Instance {
                id: "arm_inst".into(),
                part_def_id: "arm".into(),
                name: Some("Arm".into()),
                transform: None,
                material: None,
            },
        ]);

        doc.joints = Some(vec![Joint {
            id: "shoulder".into(),
            name: Some("Shoulder".into()),
            parent_instance_id: Some("base_inst".into()),
            child_instance_id: "arm_inst".into(),
            parent_anchor: Vec3::new(0.0, 0.0, 25.0),
            child_anchor: Vec3::new(0.0, 0.0, -50.0),
            kind: JointKind::Revolute {
                axis: Vec3::new(0.0, 1.0, 0.0),
                limits: Some((-90.0, 90.0)),
            },
            state: 0.0,
        }]);

        doc.ground_instance_id = Some("base_inst".into());
        doc
    }

    #[test]
    fn test_sim_pipeline_from_document() {
        let doc = test_document();
        let pipeline = SimPipeline::from_document(&doc).unwrap();
        assert_eq!(pipeline.action_dim(), 1);
        assert_eq!(pipeline.joint_ids(), vec!["shoulder"]);
    }

    #[test]
    fn test_sim_pipeline_step() {
        let doc = test_document();
        let mut pipeline = SimPipeline::from_document(&doc).unwrap();

        // Step with zero torque
        let result = pipeline.step(&[0.0], 1.0 / 240.0).unwrap();
        assert_eq!(result.joint_positions.len(), 1);
        assert_eq!(result.joint_velocities.len(), 1);
        assert!(!result.done);
    }

    #[test]
    fn test_sim_pipeline_observe() {
        let doc = test_document();
        let pipeline = SimPipeline::from_document(&doc).unwrap();

        let obs = pipeline.observe();
        assert_eq!(obs.joint_positions.len(), 1);
        assert_eq!(obs.joint_velocities.len(), 1);
    }

    #[test]
    fn test_sim_pipeline_reset() {
        let doc = test_document();
        let mut pipeline = SimPipeline::from_document(&doc).unwrap();

        // Step to change state
        pipeline.step(&[10.0], 1.0 / 60.0).unwrap();
        let _obs_after_step = pipeline.observe();

        // Reset
        pipeline.reset().unwrap();
        let obs_after_reset = pipeline.observe();

        // After reset, position should be back to initial (0)
        assert!((obs_after_reset.joint_positions[0]).abs() < 1e-6);
    }

    #[test]
    fn test_sim_pipeline_action_mismatch() {
        let doc = test_document();
        let mut pipeline = SimPipeline::from_document(&doc).unwrap();

        // Wrong number of actions
        let result = pipeline.step(&[0.0, 0.0], 1.0 / 60.0);
        assert!(result.is_err());
    }
}
