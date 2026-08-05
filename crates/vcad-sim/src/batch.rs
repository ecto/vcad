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
    /// Single-DOF joints in document order: (joint id, q offset, v offset,
    /// authored effort limit). The PD action interface indexes against this.
    servo_joints: Vec<(String, usize, usize, Option<f64>)>,
    /// Number of servoed DOFs once [`Self::enable_pd`] has run.
    pd_dofs: usize,
    /// Timestep the GPU captured at construction.
    dt: f64,
}

impl BatchSimPipeline {
    /// Create a batch simulation pipeline from a vcad Document.
    ///
    /// Builds the phyz Model from the assembly, then initializes `n_envs`
    /// parallel GPU environments stepping at `dt` seconds.
    ///
    /// **`dt` is required, deliberately.** `GpuBatchSimulator` reads
    /// `model.dt` once at construction and steps at it forever, while the CPU
    /// env overrides `model.dt` per `PhysicsWorld::step(dt)` call. With no
    /// argument here the batch silently ran at the model default of 1/240 s
    /// while the gym ran at 1/1000 — a 4.17x mismatch that is not a small
    /// error but a different plant. Measured on the floating-arm sample, the
    /// GPU diverged to non-finite state by step 185 while the CPU was stable;
    /// the K1's leg gains need the 1 kHz tick and come apart at 200 Hz.
    ///
    /// A default would have hidden that again, so there isn't one.
    pub fn from_document(doc: &Document, n_envs: usize, dt: f64) -> Result<Self, SimError> {
        // One model builder for the whole stack: `PhysicsWorld::from_document`
        // is what the CPU gym env runs, with authored inertials, collider
        // masses, joint frames and limits. The GPU batch inherits it verbatim
        // — an earlier version of this pipeline re-derived the model here with
        // density-guessed box inertias, which silently trained against the
        // wrong robot.
        let world = vcad_kernel_physics::PhysicsWorld::from_document(doc)?;
        let mut model = world.model().clone();
        // Stamp the timestep into the model before the GPU captures it.
        model.dt = dt;
        let initial_state = world.phyz_state().clone();
        let nv = model.nv;

        // Single-DOF joints in document order — the servo-able set, matching
        // the CPU gym env's actuated_joint_ids minus multi-DOF joints (which
        // have no scalar position target; the K1's floating base is the
        // canonical example).
        let servo_joints = world
            .joint_ids()
            .into_iter()
            .filter_map(|id| {
                let (q, v, ndof, effort) = world.joint_addressing(&id)?;
                (ndof == 1).then_some((id, q, v, effort))
            })
            .collect();

        let gpu_sim = GpuBatchSimulator::new(model, n_envs).map_err(SimError::Gpu)?;

        Ok(Self {
            gpu_sim,
            n_envs,
            nv,
            initial_state,
            servo_joints,
            pd_dofs: 0,
            dt,
        })
    }

    /// Enable GPU PD position servos on every single-DOF joint.
    ///
    /// `gains` supplies per-joint `(kp, kd)` by joint id; joints not in the
    /// map fall back to `default_gains`. The torque clamp is the joint's
    /// authored effort limit when it has one, else the CPU servo's `kp·π`
    /// fallback — the same law `RobotEnv` runs, minus its inertia-scaled
    /// defaults (a batched caller supplies real gains; guessing them
    /// per-joint on the GPU would hide the difference from the CPU env).
    ///
    /// After this, [`Self::set_position_targets`] is the action interface.
    pub fn enable_pd(
        &mut self,
        gains: &std::collections::HashMap<String, (f64, f64)>,
        default_gains: (f64, f64),
    ) -> Result<(), SimError> {
        let dofs: Vec<phyz_gpu::PdDof> = self
            .servo_joints
            .iter()
            .map(|(id, q, v, effort)| {
                let (kp, kd) = gains.get(id).copied().unwrap_or(default_gains);
                phyz_gpu::PdDof {
                    q_index: *q,
                    v_index: *v,
                    kp,
                    kd,
                    max_force: effort.unwrap_or((kp * std::f64::consts::PI).max(1e-12)),
                }
            })
            .collect();
        self.gpu_sim
            .enable_pd_control(&dofs)
            .map_err(SimError::Gpu)?;
        self.pd_dofs = dofs.len();
        Ok(())
    }

    /// The servoed joints' q offsets in the flat state, in
    /// [`Self::servo_joint_ids`] order — for reading joint angles out of
    /// [`Self::batch_observe`] without re-deriving the model's layout.
    pub fn servo_q_offsets(&self) -> Vec<usize> {
        self.servo_joints.iter().map(|(_, q, ..)| *q).collect()
    }

    /// Ids of the servoed joints, in the order
    /// [`Self::set_position_targets`] expects its per-env target vectors.
    pub fn servo_joint_ids(&self) -> Vec<&str> {
        self.servo_joints
            .iter()
            .map(|(id, ..)| id.as_str())
            .collect()
    }

    /// Upload per-environment position targets (radians / meters) for the
    /// PD servos and step all environments, without host readback.
    ///
    /// The RL rollout hot path: PD torque computation happens on the GPU, so
    /// nothing crosses the bus but the targets themselves.
    pub fn batch_step_targets(&mut self, targets: &[Vec<f64>]) -> Result<(), SimError> {
        if self.pd_dofs == 0 {
            return Err(SimError::Gpu(
                "PD control not enabled — call enable_pd first".into(),
            ));
        }
        if targets.len() != self.n_envs {
            return Err(SimError::ActionMismatch {
                expected: self.n_envs,
                got: targets.len(),
            });
        }
        if let Some(bad) = targets.iter().find(|t| t.len() != self.pd_dofs) {
            return Err(SimError::ActionMismatch {
                expected: self.pd_dofs,
                got: bad.len(),
            });
        }
        self.gpu_sim
            .set_position_targets(targets)
            .map_err(SimError::Gpu)?;
        self.gpu_sim.step();
        Ok(())
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

    /// The timestep every environment steps at, in seconds.
    pub fn dt(&self) -> f64 {
        self.dt
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
