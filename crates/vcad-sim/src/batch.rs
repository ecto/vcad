//! GPU batch simulation pipeline.

use phyz_gpu::GpuBatchSimulator;
use phyz_model::{Model, State};
use vcad_ir::Document;

use crate::error::SimError;
use crate::{Observation, StepResult};

/// GPU batch simulation pipeline running N parallel environments.
///
/// Uses `phyz_gpu::GpuBatchSimulator` for GPU-accelerated Featherstone ABA
/// across multiple independent environments simultaneously.
pub struct BatchSimPipeline {
    gpu_sim: GpuBatchSimulator,
    n_envs: usize,
    nv: usize,
    initial_state: State,
}

impl BatchSimPipeline {
    /// Create a batch simulation pipeline from a vcad Document.
    ///
    /// Builds the phyz Model from the assembly, then initializes `n_envs`
    /// parallel GPU environments.
    pub fn from_document(doc: &Document, n_envs: usize) -> Result<Self, SimError> {
        let model = Self::build_model(doc)?;
        let initial_state = model.default_state();
        let nv = model.nv;

        let gpu_sim = GpuBatchSimulator::new(model, n_envs)
            .map_err(SimError::Gpu)?;

        Ok(Self {
            gpu_sim,
            n_envs,
            nv,
            initial_state,
        })
    }

    /// Step all environments with per-environment actions.
    ///
    /// `actions` is a flat slice of length `n_envs * nv`, where each
    /// contiguous block of `nv` values is the action for one environment.
    pub fn batch_step(&mut self, actions: &[f64]) -> Result<Vec<StepResult>, SimError> {
        let expected = self.n_envs * self.nv;
        if actions.len() != expected {
            return Err(SimError::ActionMismatch {
                expected,
                got: actions.len(),
            });
        }

        // Set controls for each env
        let ctrls: Vec<Vec<f64>> = actions
            .chunks(self.nv)
            .map(|c| c.to_vec())
            .collect();
        self.gpu_sim.set_controls(&ctrls);

        // Step GPU simulation
        self.gpu_sim.step();

        // Readback states
        let states = self.gpu_sim.readback_states();

        let mut results = Vec::with_capacity(self.n_envs);
        for state in &states {
            results.push(StepResult {
                joint_positions: state.q.as_slice().to_vec(),
                joint_velocities: state.v.as_slice().to_vec(),
                done: false,
            });
        }

        Ok(results)
    }

    /// Observe all environments without stepping.
    pub fn batch_observe(&self) -> Vec<Observation> {
        let states = self.gpu_sim.readback_states();

        let mut observations = Vec::with_capacity(self.n_envs);
        for state in &states {
            observations.push(Observation {
                joint_positions: state.q.as_slice().to_vec(),
                joint_velocities: state.v.as_slice().to_vec(),
            });
        }

        observations
    }

    /// Reset all environments to the initial state.
    pub fn batch_reset(&mut self) {
        let states: Vec<State> = (0..self.n_envs)
            .map(|_| self.initial_state.clone())
            .collect();
        self.gpu_sim.load_states(&states);
    }

    /// Enable ground-plane contact detection.
    ///
    /// Objects will be repelled from the ground at `height` via penalty forces.
    pub fn enable_ground_contact(
        &mut self,
        height: f64,
        stiffness: f64,
        damping: f64,
        friction: f64,
    ) -> Result<(), SimError> {
        self.gpu_sim
            .enable_ground_contact(height, stiffness, damping, friction)
            .map_err(SimError::Gpu)?;
        Ok(())
    }

    /// Get the number of parallel environments.
    pub fn n_envs(&self) -> usize {
        self.n_envs
    }

    /// Get the number of action dimensions per environment.
    pub fn action_dim(&self) -> usize {
        self.nv
    }

    /// Build a phyz Model from a vcad Document.
    fn build_model(doc: &Document) -> Result<Model, SimError> {
        use phyz::phyz_math::{Mat3, SpatialInertia, SpatialTransform, Vec3};
        use phyz_model::ModelBuilder;
        use std::collections::{HashMap, HashSet};

        let instances = doc.instances.as_ref().ok_or(SimError::NoAssembly)?;
        let joints = doc.joints.as_ref().ok_or(SimError::NoAssembly)?;
        let part_defs = doc.part_defs.as_ref().ok_or(SimError::NoAssembly)?;
        let ground_id = doc
            .ground_instance_id
            .as_ref()
            .ok_or(SimError::NoAssembly)?;

        let mut builder = ModelBuilder::new()
            .gravity(Vec3::new(0.0, 0.0, -9.81))
            .dt(1.0 / 240.0);

        let mut instance_to_body: HashMap<String, usize> = HashMap::new();
        let mut body_count = 0usize;

        // Compute box inertia from a primitive node
        let compute_inertia = |node_id: u64| -> (f64, SpatialInertia) {
            let node = doc.nodes.get(&node_id);
            let (dx, dy, dz) = match node.map(|n| &n.op) {
                Some(vcad_ir::CsgOp::Cube { size }) => {
                    (size.x / 1000.0, size.y / 1000.0, size.z / 1000.0)
                }
                Some(vcad_ir::CsgOp::Cylinder { radius, height, .. }) => {
                    let d = 2.0 * radius / 1000.0;
                    (d, d, height / 1000.0)
                }
                Some(vcad_ir::CsgOp::Sphere { radius, .. }) => {
                    let d = 2.0 * radius / 1000.0;
                    (d, d, d)
                }
                _ => (0.01, 0.01, 0.01),
            };
            let density = 1000.0; // kg/m³
            let mass = density * dx * dy * dz;
            let ixx = mass / 12.0 * (dy * dy + dz * dz);
            let iyy = mass / 12.0 * (dx * dx + dz * dz);
            let izz = mass / 12.0 * (dx * dx + dy * dy);
            let inertia_mat = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
            (mass, SpatialInertia::new(mass, Vec3::zeros(), inertia_mat))
        };

        let instance_transform = |inst: &vcad_ir::Instance| -> SpatialTransform {
            inst.transform
                .as_ref()
                .map(|t| {
                    let translation = Vec3::new(
                        t.translation.x / 1000.0,
                        t.translation.y / 1000.0,
                        t.translation.z / 1000.0,
                    );
                    SpatialTransform::new(Mat3::identity(), translation)
                })
                .unwrap_or(SpatialTransform::identity())
        };

        // 1. Ground body (fixed)
        let ground_inst = instances
            .iter()
            .find(|i| i.id == *ground_id)
            .ok_or(SimError::NoAssembly)?;
        {
            let part = part_defs.get(&ground_inst.part_def_id).ok_or(SimError::NoAssembly)?;
            let (_, inertia) = compute_inertia(part.root);
            let xform = instance_transform(ground_inst);
            builder = builder.add_fixed_body(&ground_inst.id, -1, xform, inertia);
            instance_to_body.insert(ground_inst.id.clone(), body_count);
            body_count += 1;
        }

        // 2. BFS from ground through joints
        let mut queue = vec![ground_id.clone()];
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(ground_id.clone());

        while let Some(parent_id) = queue.pop() {
            for joint in joints {
                let joint_parent = joint.parent_instance_id.as_deref().unwrap_or(ground_id);
                if joint_parent != parent_id || visited.contains(&joint.child_instance_id) {
                    continue;
                }

                let child_inst = instances
                    .iter()
                    .find(|i| i.id == joint.child_instance_id)
                    .ok_or(SimError::NoAssembly)?;

                let parent_body_idx = *instance_to_body
                    .get(&parent_id)
                    .ok_or(SimError::NoAssembly)?;

                let part = part_defs
                    .get(&child_inst.part_def_id)
                    .ok_or(SimError::NoAssembly)?;
                let (_, inertia) = compute_inertia(part.root);

                let phyz_joint = vcad_kernel_physics::joints::vcad_joint_to_phyz(joint)?;

                builder = builder.add_body(
                    &child_inst.id,
                    parent_body_idx as i32,
                    phyz_joint,
                    inertia,
                );

                instance_to_body.insert(child_inst.id.clone(), body_count);
                body_count += 1;
                visited.insert(child_inst.id.clone());
                queue.push(child_inst.id.clone());
            }
        }

        Ok(builder.build())
    }
}
