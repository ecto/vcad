//! GPU batch simulation pipeline.

use phyz_gpu::GpuBatchSimulator;
use phyz_model::State;
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
        // One model builder for the whole stack: `PhysicsWorld::from_document`
        // is what the CPU gym env runs, with authored inertials, collider
        // masses, joint frames and limits. The GPU batch inherits it verbatim
        // — an earlier version of this pipeline re-derived the model here with
        // density-guessed box inertias, which silently trained against the
        // wrong robot.
        let world = vcad_kernel_physics::PhysicsWorld::from_document(doc)?;
        let model = world.model().clone();
        let initial_state = world.phyz_state().clone();
        let nv = model.nv;

        let gpu_sim = GpuBatchSimulator::new(model, n_envs).map_err(SimError::Gpu)?;

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
        let ctrls: Vec<Vec<f64>> = actions.chunks(self.nv).map(|c| c.to_vec()).collect();
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

    /// Step all environments without reading state back to the host.
    ///
    /// The throughput path: `batch_step` pays a GPU→CPU readback every call,
    /// which dominates once the physics itself is cheap. A rollout loop that
    /// only needs observations every k-th control step (or never — see
    /// `phyz_gpu::GpuBatchSimulator::interop` for the zero-copy tensor
    /// contract) submits with this and reads back explicitly via
    /// [`Self::batch_observe`] when it wants eyes.
    pub fn batch_step_submit(&mut self, actions: &[f64]) -> Result<(), SimError> {
        let expected = self.n_envs * self.nv;
        if actions.len() != expected {
            return Err(SimError::ActionMismatch {
                expected,
                got: actions.len(),
            });
        }
        let ctrls: Vec<Vec<f64>> = actions.chunks(self.nv).map(|c| c.to_vec()).collect();
        self.gpu_sim.set_controls(&ctrls);
        self.gpu_sim.step();
        Ok(())
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
}
