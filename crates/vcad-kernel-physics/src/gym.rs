//! Gym-style interface for reinforcement learning.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use vcad_ir::Document;

use crate::error::PhysicsError;
use crate::world::{GroundConfig, PhysicsWorld};

/// Observation from the robot environment.
///
/// Joint vectors are indexed by [`RobotEnv::joint_ids`] order, which is the
/// document's `joints` array order (deterministic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Joint positions (radians for revolute, meters for prismatic).
    pub joint_positions: Vec<f64>,
    /// Joint velocities (rad/s for revolute, m/s for prismatic).
    pub joint_velocities: Vec<f64>,
    /// End effector poses as [x, y, z, qw, qx, qy, qz] in meters.
    pub end_effector_poses: Vec<[f64; 7]>,
    /// Base pose as [x, y, z, qw, qx, qy, qz] (meters / unit quaternion) of
    /// the base instance (config `base_instance_id`, defaulting to the
    /// document's ground instance). None when the base instance is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_pose: Option<[f64; 7]>,
    /// Base velocity as [vx, vy, vz, wx, wy, wz] (m/s of the base origin,
    /// then rad/s), world frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_velocity: Option<[f64; 6]>,
    // TODO(contact): per-foot contact state (in-contact flag + normal force).
    // Ground-plane contact now exists (`PhysicsWorld::ground_contacts`), but
    // the per-body manifold isn't surfaced through the observation yet.
}

impl Observation {
    /// Create a zero observation with the given dimensions.
    pub fn zeros(num_joints: usize, num_end_effectors: usize) -> Self {
        Self {
            joint_positions: vec![0.0; num_joints],
            joint_velocities: vec![0.0; num_joints],
            end_effector_poses: vec![[0.0; 7]; num_end_effectors],
            base_pose: None,
            base_velocity: None,
        }
    }
}

/// Inclusive `[min, max]` sampling range for domain randomization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Range {
    /// Lower bound (inclusive).
    pub min: f64,
    /// Upper bound (inclusive).
    pub max: f64,
}

impl Range {
    fn sample(&self, rng: &mut StdRng) -> f64 {
        self.min + rng.gen::<f64>() * (self.max - self.min)
    }
}

/// Seeded domain randomization applied on every [`RobotEnv::reset`].
///
/// All fields are optional; an unset field applies no randomization for that
/// quantity. Samples are drawn from a `StdRng` seeded from the env seed plus
/// an episode counter, so a given `(seed, episode)` pair is reproducible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DomainRandomization {
    /// Per-link multiplicative mass scale (e.g. `{min: 0.9, max: 1.1}` for
    /// ±10%). Sampled independently per non-ground instance; scales the whole
    /// spatial inertia.
    pub mass_scale: Option<Range>,
    /// Per-joint multiplicative scale on dry friction loss and viscous
    /// damping. (Joint friction only — see the contact TODO on
    /// [`PhysicsWorld::scale_joint_friction`].)
    pub friction_scale: Option<Range>,
    /// Global multiplicative scale on the PD motor gains (kp, kd), sampled
    /// once per episode.
    pub pd_gain_scale: Option<Range>,
    /// Actuator latency in physics substeps, sampled uniformly from the
    /// inclusive `[min, max]` integer range once per episode. An action
    /// passed to `step` takes effect after that many substeps (spilling into
    /// the next env step if it exceeds `substeps`). Booster's official K1
    /// config randomizes 2–8 sim steps.
    pub action_latency_steps: Option<[u32; 2]>,
    /// Uniform ± perturbation of each actuated joint's initial position
    /// (degrees for revolute, mm for prismatic).
    pub joint_pos_perturb: Option<f64>,
    /// Uniform ± perturbation of each actuated joint's initial velocity
    /// (deg/s or mm/s).
    pub joint_vel_perturb: Option<f64>,
}

/// Zero-mean gaussian noise added to observations returned by `step`/`reset`.
///
/// [`RobotEnv::observe`] stays noise-free (it reads the true state); the
/// noisy view is what a policy trains against.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservationNoise {
    /// Std-dev on joint positions (degrees / mm).
    pub joint_pos_std: f64,
    /// Std-dev on joint velocities (deg/s / mm/s).
    pub joint_vel_std: f64,
    /// Std-dev on base position (meters).
    pub base_pos_std: f64,
    /// Std-dev on base orientation (radians, small-angle perturbation).
    pub base_rot_std: f64,
    /// Std-dev on base linear/angular velocity (m/s and rad/s).
    pub base_vel_std: f64,
}

impl ObservationNoise {
    fn is_zero(&self) -> bool {
        self.joint_pos_std == 0.0
            && self.joint_vel_std == 0.0
            && self.base_pos_std == 0.0
            && self.base_rot_std == 0.0
            && self.base_vel_std == 0.0
    }
}

/// Configurable termination conditions checked after every step.
///
/// When set, these replace the legacy end-effector-below-ground check.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminationConfig {
    /// Terminate when the base origin's world z drops below this (meters).
    pub base_height_below: Option<f64>,
    /// Terminate when the base tilts more than this many degrees from
    /// upright (angle between the base frame's +Z axis and world +Z).
    pub base_tilt_above_deg: Option<f64>,
    /// Terminate when any joint reaches (or passes) a limit.
    pub terminate_on_joint_limit: bool,
}

/// Full env configuration accepted by [`RobotEnv::new_with_config`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvConfig {
    /// Domain randomization applied on every reset.
    pub randomization: Option<DomainRandomization>,
    /// Gaussian observation noise on `step`/`reset` observations.
    pub observation_noise: Option<ObservationNoise>,
    /// Termination conditions.
    pub termination: Option<TerminationConfig>,
    /// Instance id treated as the robot base for base pose/velocity
    /// observations and termination checks. Defaults to the document's
    /// ground instance.
    pub base_instance_id: Option<String>,
}

/// Per-step diagnostics returned alongside the observation — everything a
/// client-side reward needs, without baking a reward DSL into the kernel.
///
/// **Every quantity here is the simulator's true state, never the noisy view.**
/// [`ObservationNoise`] perturbs only [`StepResult::observation`], so with base
/// noise configured `info.base_height_m` deliberately will *not* equal
/// `observation.base_pose[2]`. That asymmetry is the point: the policy trains
/// against noisy sensors while the reward and the termination decision are
/// computed from ground truth (the usual privileged-information / asymmetric
/// actor-critic split). Reward from `StepInfo`, observe from `observation`, and
/// don't cross-reference the two expecting agreement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInfo {
    /// Steps taken since the last reset (this step included).
    pub step: u32,
    /// Episode ended because `max_steps` was reached.
    pub truncated: bool,
    /// Episode ended because a termination condition fired.
    pub terminated: bool,
    /// Which condition fired (e.g. "base_height", "base_tilt",
    /// "joint_limit", "end_effector_below_ground").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,
    /// True base origin height above world z=0 (meters), when a base is
    /// known. Noise-free — see the note on [`StepInfo`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_height_m: Option<f64>,
    /// True base tilt from upright (degrees), when a base is known.
    /// Noise-free — see the note on [`StepInfo`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_tilt_deg: Option<f64>,
    /// Ids of joints currently at/past a limit.
    pub joint_limit_violations: Vec<String>,
    /// The episode's sampled actuator latency, in physics substeps.
    pub action_latency_substeps: u32,
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
    /// Env configuration (randomization / noise / termination).
    config: EnvConfig,
    /// Resolved base instance id (config override or document ground).
    base_instance_id: Option<String>,
    /// Episode counter, folded into the per-reset RNG stream.
    episode: u64,
    /// RNG for the current episode (observation noise draws).
    rng: StdRng,
    /// The episode's sampled actuator latency in physics substeps.
    latency_substeps: u32,
    /// Delay line: (substeps until active, action). Motors persist once
    /// applied, so the previous action keeps acting until its successor
    /// clears the delay line.
    pending_actions: Vec<(u32, Action)>,
    /// Ground-plane contact configuration, reapplied on every reset.
    ground: GroundConfig,
}

/// Result of a single env step: observation plus reward, done, and the
/// [`StepInfo`] diagnostics map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Observation after the step (noisy when observation noise is set).
    pub observation: Observation,
    /// Reward. The kernel computes no task reward (always 0.0) — compute
    /// rewards client-side from the observation and [`StepInfo`].
    pub reward: f64,
    /// True when the episode ended (terminated or truncated).
    pub done: bool,
    /// Per-step diagnostics.
    pub info: StepInfo,
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
        Self::new_with_config(
            doc,
            end_effector_ids,
            dt,
            substeps,
            ground,
            EnvConfig::default(),
        )
    }

    /// Create a new robot environment with an explicit [`EnvConfig`]
    /// (domain randomization, observation noise, termination conditions).
    pub fn new_with_config(
        doc: Document,
        end_effector_ids: Vec<String>,
        dt: Option<f32>,
        substeps: Option<u32>,
        ground: Option<GroundConfig>,
        config: EnvConfig,
    ) -> Result<Self, PhysicsError> {
        let ground = ground.unwrap_or_default();
        let mut world = PhysicsWorld::from_document(&doc)?;
        world.set_ground(ground);
        let joint_ids = world.joint_ids();
        let actuated_joint_ids = world.actuated_joint_ids();
        let base_instance_id = config
            .base_instance_id
            .clone()
            .or_else(|| doc.ground_instance_id.clone());

        let mut env = Self {
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
            config,
            base_instance_id,
            episode: 0,
            rng: StdRng::seed_from_u64(0),
            latency_substeps: 0,
            pending_actions: Vec::new(),
            ground,
        };
        // Apply episode-0 randomization to the freshly built world so the
        // very first rollout (before any explicit reset) is randomized too.
        // Unlike `reset`, this doesn't clear `pending_actions` first — it is
        // `Vec::new()` here. A future from-snapshot constructor that
        // pre-populates state would have to clear it, or episode-0 actions
        // would inherit a stale delay line.
        env.apply_episode_randomization();
        Ok(env)
    }

    /// Reset the environment to initial state, applying a fresh draw of
    /// domain randomization (if configured).
    ///
    /// Returns the initial observation (noisy when observation noise is set).
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
        self.episode = self.episode.wrapping_add(1);
        self.pending_actions.clear();

        self.apply_episode_randomization();

        let obs = self.observe();
        self.noisify(obs)
    }

    /// Reset with a new seed: replaces the stored seed, rewinds the episode
    /// counter (so the next randomization draw is episode 0 of the new
    /// stream), then resets.
    pub fn reset_with_seed(&mut self, seed: u64) -> Observation {
        self.seed = seed;
        // reset() pre-increments; start the new stream at episode 0.
        self.episode = u64::MAX;
        self.reset()
    }

    /// Re-seed the per-episode RNG and sample + apply this episode's domain
    /// randomization to the freshly (re)built world.
    fn apply_episode_randomization(&mut self) {
        // Distinct, deterministic stream per (seed, episode).
        self.rng =
            StdRng::seed_from_u64(self.seed ^ self.episode.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        self.latency_substeps = 0;

        let Some(dr) = self.config.randomization.clone() else {
            return;
        };

        if let Some(range) = dr.mass_scale {
            let ground = self.initial_doc.ground_instance_id.clone();
            for inst_id in self.world.instance_ids() {
                if ground.as_deref() == Some(inst_id.as_str()) {
                    continue;
                }
                let scale = range.sample(&mut self.rng);
                self.world.scale_instance_mass(&inst_id, scale);
            }
        }
        if let Some(range) = dr.friction_scale {
            for joint_id in self.actuated_joint_ids.clone() {
                let scale = range.sample(&mut self.rng);
                self.world.scale_joint_friction(&joint_id, scale);
            }
        }
        if let Some(range) = dr.pd_gain_scale {
            let scale = range.sample(&mut self.rng);
            self.world.set_gain_scale(scale);
        }
        if let Some([lo, hi]) = dr.action_latency_steps {
            let (lo, hi) = (lo.min(hi), lo.max(hi));
            // Width in u64: a full-width `[0, u32::MAX]` config would overflow
            // `hi - lo + 1` in u32 (panic in debug, then a `% 0` divide-by-zero
            // in release). The config is caller-supplied JSON, so it has to
            // survive absurd input. Same draw count and modulus as before for
            // any sane range, so seeded streams are unchanged.
            let span = (hi as u64) - (lo as u64) + 1;
            self.latency_substeps = lo.saturating_add((self.rng.gen::<u64>() % span) as u32);
        }
        if dr.joint_pos_perturb.is_some() || dr.joint_vel_perturb.is_some() {
            let dp = dr.joint_pos_perturb.unwrap_or(0.0);
            let dv = dr.joint_vel_perturb.unwrap_or(0.0);
            for joint_id in self.actuated_joint_ids.clone() {
                let dpos = (self.rng.gen::<f64>() * 2.0 - 1.0) * dp;
                let dvel = (self.rng.gen::<f64>() * 2.0 - 1.0) * dv;
                self.world.perturb_joint_state(&joint_id, dpos, dvel);
            }
            self.world.refresh_kinematics();
        }
    }

    /// Step the environment with an action.
    ///
    /// Returns (observation, reward, done). See [`Self::step_full`] for the
    /// variant that also returns [`StepInfo`].
    pub fn step(&mut self, action: Action) -> (Observation, f64, bool) {
        let result = self.step_full(action);
        (result.observation, result.reward, result.done)
    }

    /// Step the environment with an action, returning the full
    /// [`StepResult`] including per-step diagnostics.
    ///
    /// The action enters a delay line of the episode's sampled actuator
    /// latency (in physics substeps); until it clears, the previous motor
    /// targets keep acting.
    pub fn step_full(&mut self, action: Action) -> StepResult {
        self.pending_actions.push((self.latency_substeps, action));

        for _ in 0..self.substeps {
            // Apply, in FIFO order, every queued action whose delay elapsed.
            let mut i = 0;
            while i < self.pending_actions.len() {
                if self.pending_actions[i].0 == 0 {
                    let (_, a) = self.pending_actions.remove(i);
                    self.apply_action(&a);
                } else {
                    i += 1;
                }
            }
            self.world.step(self.dt);
            for pending in &mut self.pending_actions {
                pending.0 = pending.0.saturating_sub(1);
            }
        }

        self.current_step += 1;

        let obs = self.observe();
        let reward = self.compute_reward(&obs);

        let (terminated, termination_reason, joint_limit_violations) = self.check_termination(&obs);
        let truncated = self.current_step >= self.max_steps;

        // Both the termination decision above and these diagnostics read the
        // clean `obs`; only the returned observation gets noise. Deliberate —
        // an episode must not end because a sensor blipped, and the reward
        // signal must not inherit the policy's sensor noise. Consequence:
        // `info.base_height_m != observation.base_pose[2]` under base noise.
        let (base_height_m, base_tilt_deg) = match obs.base_pose {
            Some(pose) => (Some(pose[2]), Some(tilt_from_upright_deg(&pose))),
            None => (None, None),
        };

        let info = StepInfo {
            step: self.current_step,
            truncated,
            terminated,
            termination_reason,
            base_height_m,
            base_tilt_deg,
            joint_limit_violations,
            action_latency_substeps: self.latency_substeps,
        };

        StepResult {
            observation: self.noisify(obs),
            reward,
            done: terminated || truncated,
            info,
        }
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

        let (base_pose, base_velocity) = match self.base_instance_id.as_deref() {
            Some(base_id) => (
                self.world.get_instance_pose(base_id).map(|(pos, quat)| {
                    [pos[0], pos[1], pos[2], quat[0], quat[1], quat[2], quat[3]]
                }),
                self.world.get_instance_velocity(base_id),
            ),
            None => (None, None),
        };

        Observation {
            joint_positions: positions,
            joint_velocities: velocities,
            end_effector_poses,
            base_pose,
            base_velocity,
        }
    }

    /// Apply configured gaussian observation noise, consuming RNG draws from
    /// the current episode's stream. Identity when no noise is configured.
    fn noisify(&mut self, mut obs: Observation) -> Observation {
        let Some(noise) = self.config.observation_noise.clone() else {
            return obs;
        };
        if noise.is_zero() {
            return obs;
        }
        for p in &mut obs.joint_positions {
            *p += gaussian(&mut self.rng) * noise.joint_pos_std;
        }
        for v in &mut obs.joint_velocities {
            *v += gaussian(&mut self.rng) * noise.joint_vel_std;
        }
        if let Some(pose) = obs.base_pose.as_mut() {
            for p in pose.iter_mut().take(3) {
                *p += gaussian(&mut self.rng) * noise.base_pos_std;
            }
            if noise.base_rot_std > 0.0 {
                // Small-angle perturbation: compose with a noise quaternion
                // built from three gaussian axis-angle components.
                let (dx, dy, dz) = (
                    gaussian(&mut self.rng) * noise.base_rot_std,
                    gaussian(&mut self.rng) * noise.base_rot_std,
                    gaussian(&mut self.rng) * noise.base_rot_std,
                );
                let dq = normalize4([1.0, dx * 0.5, dy * 0.5, dz * 0.5]);
                let q = [pose[3], pose[4], pose[5], pose[6]];
                let qn = quat_mul(dq, q);
                pose[3] = qn[0];
                pose[4] = qn[1];
                pose[5] = qn[2];
                pose[6] = qn[3];
            }
        }
        if let Some(vel) = obs.base_velocity.as_mut() {
            for v in vel.iter_mut() {
                *v += gaussian(&mut self.rng) * noise.base_vel_std;
            }
        }
        obs
    }

    /// Evaluate termination conditions against a (noise-free) observation.
    ///
    /// Returns (terminated, reason, joints currently at/past a limit). The
    /// limit list is reported regardless of whether limits terminate.
    fn check_termination(&self, obs: &Observation) -> (bool, Option<String>, Vec<String>) {
        const LIMIT_EPS: f64 = 1e-6;
        let mut violations = Vec::new();
        for (i, joint_id) in self.joint_ids.iter().enumerate() {
            if let Some((lo, hi)) = self.world.joint_limits_vcad(joint_id) {
                let q = obs.joint_positions[i];
                if q <= lo + LIMIT_EPS || q >= hi - LIMIT_EPS {
                    violations.push(joint_id.clone());
                }
            }
        }

        let Some(term) = self.config.termination.as_ref() else {
            // Legacy behavior: terminate when an end effector falls below
            // ground.
            return (self.is_terminated(obs), None, violations);
        };

        if let Some(pose) = &obs.base_pose {
            if let Some(min_z) = term.base_height_below {
                if pose[2] < min_z {
                    return (true, Some("base_height".to_string()), violations);
                }
            }
            if let Some(max_tilt) = term.base_tilt_above_deg {
                if tilt_from_upright_deg(pose) > max_tilt {
                    return (true, Some("base_tilt".to_string()), violations);
                }
            }
        }
        if term.terminate_on_joint_limit && !violations.is_empty() {
            return (true, Some("joint_limit".to_string()), violations);
        }
        (false, None, violations)
    }

    /// Set the random seed. Takes effect from the next [`Self::reset`].
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
    pub fn observation_dim(&self) -> usize {
        self.joint_ids.len() * 2 + self.end_effector_ids.len() * 7
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
        // Rewards are deliberately computed client-side: the observation plus
        // StepInfo (base pose/velocity, height, tilt, limit violations) carry
        // everything a task reward needs. The kernel always returns 0.0.
        0.0
    }

    /// Legacy termination check, used only when no [`TerminationConfig`] is
    /// set: an end effector more than a meter below ground.
    fn is_terminated(&self, obs: &Observation) -> bool {
        for pose in &obs.end_effector_poses {
            if pose[2] < -1.0 {
                return true;
            }
        }
        false
    }
}

/// Standard-normal draw via Box–Muller (avoids a rand_distr dependency).
fn gaussian(rng: &mut StdRng) -> f64 {
    let u1 = rng.gen::<f64>().max(1e-300);
    let u2 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Normalize a wxyz quaternion.
fn normalize4(q: [f64; 4]) -> [f64; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n < 1e-12 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
}

/// Hamilton product of two wxyz quaternions.
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// Tilt from upright (degrees) of a `[x, y, z, qw, qx, qy, qz]` pose: the
/// angle between the frame's +Z axis and world +Z. For a unit quaternion,
/// `R·ez · ez = 1 − 2(qx² + qy²)`.
fn tilt_from_upright_deg(pose: &[f64; 7]) -> f64 {
    let (qx, qy) = (pose[4], pose[5]);
    let cos_tilt = (1.0 - 2.0 * (qx * qx + qy * qy)).clamp(-1.0, 1.0);
    cos_tilt.acos().to_degrees()
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

    fn randomized_config() -> EnvConfig {
        EnvConfig {
            randomization: Some(DomainRandomization {
                mass_scale: Some(Range { min: 0.9, max: 1.1 }),
                friction_scale: Some(Range { min: 0.5, max: 2.0 }),
                pd_gain_scale: Some(Range { min: 0.8, max: 1.2 }),
                action_latency_steps: Some([2, 8]),
                joint_pos_perturb: Some(5.0),
                joint_vel_perturb: Some(1.0),
            }),
            observation_noise: Some(ObservationNoise {
                joint_pos_std: 0.1,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Observation noise and domain randomization share one `StdRng`, so a
    /// step consumes draws that would otherwise feed the next episode's
    /// randomization. `apply_episode_randomization` re-seeds from
    /// `(seed, episode)` on every reset, which wipes that carry-over — but
    /// only as long as the re-seed stays. Pin it: envs that took different
    /// numbers of steps must still reproduce each other after re-seeding.
    ///
    /// To confirm this test still bites, delete the re-seed outright. Do
    /// *not* gate it on `episode == 0`: `reset_with_seed` sets `episode` to
    /// `u64::MAX` and `reset` pre-increments to 0, so that guard still
    /// re-seeds on every call and the test passes against code you think you
    /// broke.
    #[test]
    fn step_count_does_not_leak_into_the_next_episode() {
        let build = || {
            let mut cfg = randomized_config();
            // Noise on, so stepping definitely consumes RNG draws.
            cfg.observation_noise = Some(ObservationNoise {
                joint_pos_std: 0.5,
                base_pos_std: 0.5,
                ..Default::default()
            });
            RobotEnv::new_with_config(
                create_simple_robot(),
                vec!["link2_inst".to_string()],
                None,
                None,
                Some(GroundConfig::disabled()),
                cfg,
            )
            .unwrap()
        };

        let mut busy = build();
        busy.reset_with_seed(5);
        for _ in 0..7 {
            busy.step(Action::Torque(vec![0.01, 0.01]));
        }

        let mut idle = build();
        idle.reset_with_seed(5);

        // Both re-seed the same stream; the 7 intervening steps must not
        // shift `busy`'s next episode.
        let a = busy.reset_with_seed(99);
        let b = idle.reset_with_seed(99);
        assert_eq!(
            a.joint_positions, b.joint_positions,
            "step count leaked into the next episode's randomization"
        );

        let (sa, _, _) = busy.step(Action::Torque(vec![0.01, 0.01]));
        let (sb, _, _) = idle.step(Action::Torque(vec![0.01, 0.01]));
        assert_eq!(
            sa.joint_positions, sb.joint_positions,
            "post-reset rollouts diverged despite identical seeds"
        );
    }

    /// `StepInfo` reports ground truth while the returned observation is
    /// noisy. The divergence is the contract, not a bug: reward and
    /// termination must not inherit sensor noise. Pinned so a later "make
    /// these agree" refactor has to break a test to do it.
    #[test]
    fn step_info_reports_truth_while_observation_is_noisy() {
        let doc = create_simple_robot();
        let cfg = EnvConfig {
            observation_noise: Some(ObservationNoise {
                base_pos_std: 5.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut env = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            cfg,
        )
        .unwrap();
        env.reset_with_seed(11);
        let result = env.step_full(Action::Torque(vec![0.0, 0.0]));

        let truth = env.observe().base_pose.expect("base pose")[2];
        let reported = result.info.base_height_m.expect("base height");
        let noisy = result.observation.base_pose.expect("noisy base pose")[2];

        // info tracks the simulator, not the sensor.
        assert!(
            (reported - truth).abs() < 1e-9,
            "StepInfo height {reported} should equal true height {truth}"
        );
        // ...and the caller-visible observation is perturbed away from it.
        assert!(
            (noisy - reported).abs() > 1e-6,
            "observation height {noisy} should differ from true {reported} under 5m base noise"
        );
    }

    /// `action_latency_steps` is caller-supplied JSON, so an absurd but
    /// well-typed range must not panic. A full-width `[0, u32::MAX]` overflows
    /// a u32 `hi - lo + 1` (debug panic; `% 0` divide-by-zero in release).
    /// Note the overflow needs a *wide* range, not `lo == hi == u32::MAX`,
    /// where the width is simply 1.
    #[test]
    fn full_width_latency_range_does_not_overflow() {
        let doc = create_simple_robot();
        let cfg = EnvConfig {
            randomization: Some(DomainRandomization {
                action_latency_steps: Some([0, u32::MAX]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut env = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            cfg,
        )
        .unwrap();
        env.reset_with_seed(3);
        let result = env.step_full(Action::Torque(vec![0.0, 0.0]));
        // Any draw in range is acceptable; not panicking is the assertion.
        assert!(result.info.action_latency_substeps <= u32::MAX);

        // The degenerate range Choji named is a width of 1, always exact.
        let doc2 = create_simple_robot();
        let cfg2 = EnvConfig {
            randomization: Some(DomainRandomization {
                action_latency_steps: Some([u32::MAX, u32::MAX]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut env2 = RobotEnv::new_with_config(
            doc2,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            cfg2,
        )
        .unwrap();
        env2.reset_with_seed(3);
        assert_eq!(
            env2.step_full(Action::Torque(vec![0.0, 0.0]))
                .info
                .action_latency_substeps,
            u32::MAX
        );
    }

    /// Randomization must be applied to a *pristine* world each episode.
    /// `scale_instance_mass` / `scale_joint_friction` are multiplicative, so if
    /// `reset` ever stopped rebuilding from `initial_doc`, successive episodes
    /// would compound their scales and drift unboundedly. Re-seeding the same
    /// stream after intervening episodes must reproduce the same rollout.
    #[test]
    fn randomization_does_not_compound_across_episodes() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            randomized_config(),
        )
        .unwrap();

        env.reset_with_seed(7);
        let (first, _, _) = env.step(Action::Torque(vec![0.02, 0.02]));

        // Burn several unrelated episodes; each re-applies a fresh mass /
        // friction / gain draw to the rebuilt world.
        for s in 0..5 {
            env.reset_with_seed(100 + s);
            env.step(Action::Torque(vec![0.02, 0.02]));
        }

        env.reset_with_seed(7);
        let (again, _, _) = env.step(Action::Torque(vec![0.02, 0.02]));
        assert_eq!(
            first.joint_positions, again.joint_positions,
            "episode 0 of seed 7 drifted after intervening episodes — \
             randomization is compounding instead of starting from initial_doc"
        );
    }

    #[test]
    fn seeded_reset_is_reproducible() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new_with_config(
            doc.clone(),
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            randomized_config(),
        )
        .unwrap();
        let mut env2 = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            randomized_config(),
        )
        .unwrap();

        let a = env.reset_with_seed(42);
        let b = env2.reset_with_seed(42);
        assert_eq!(a.joint_positions, b.joint_positions);
        assert_eq!(a.joint_velocities, b.joint_velocities);

        // Same seed, same episode → identical rollouts too (randomized mass,
        // gains, latency all resampled identically).
        let (sa, _, _) = env.step(Action::Torque(vec![0.01, 0.01]));
        let (sb, _, _) = env2.step(Action::Torque(vec![0.01, 0.01]));
        assert_eq!(sa.joint_positions, sb.joint_positions);

        // A different seed draws a different initial state (pos perturb ±5°
        // makes a collision vanishingly unlikely).
        let c = env.reset_with_seed(43);
        let a2 = env2.reset_with_seed(42);
        assert_ne!(c.joint_positions, a2.joint_positions);
    }

    #[test]
    fn action_latency_delays_effect() {
        let doc = create_simple_robot();
        let latency_cfg = EnvConfig {
            randomization: Some(DomainRandomization {
                // Fixed 8-substep latency = 2 env steps at substeps=4.
                action_latency_steps: Some([8, 8]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut delayed = RobotEnv::new_with_config(
            doc.clone(),
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            latency_cfg,
        )
        .unwrap();
        let mut immediate = RobotEnv::new(
            doc.clone(),
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
        )
        .unwrap();
        // Reference: no torque at all, so the first step is gravity-only.
        let mut passive = RobotEnv::new(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
        )
        .unwrap();
        delayed.reset();
        immediate.reset();
        passive.reset();

        let torque = Action::Torque(vec![0.5, 0.0]);
        let (obs_d, _, _) = delayed.step(torque.clone());
        let (obs_i, _, _) = immediate.step(torque.clone());
        let (obs_p, _, _) = passive.step(Action::Torque(vec![0.0, 0.0]));
        // During the latency window the delayed env evolves exactly like the
        // passive one (gravity only), while the immediate env already feels
        // the torque.
        assert_eq!(obs_d.joint_velocities, obs_p.joint_velocities);
        assert_ne!(obs_i.joint_velocities, obs_p.joint_velocities);

        // After the delay elapses the action lands and the trajectories split.
        let result = delayed.step_full(torque.clone());
        assert_eq!(result.info.action_latency_substeps, 8);
        let (obs_d2, _, _) = delayed.step(torque);
        let (obs_p2, _, _) = {
            passive.step(Action::Torque(vec![0.0, 0.0]));
            passive.step(Action::Torque(vec![0.0, 0.0]))
        };
        assert_ne!(obs_d2.joint_velocities, obs_p2.joint_velocities);
    }

    #[test]
    fn observation_noise_perturbs_step_but_not_observe() {
        let doc = create_simple_robot();
        let cfg = EnvConfig {
            observation_noise: Some(ObservationNoise {
                joint_pos_std: 0.5,
                joint_vel_std: 0.5,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut env = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            cfg,
        )
        .unwrap();
        let noisy = env.reset_with_seed(7);
        let clean = env.observe();
        assert_ne!(noisy.joint_positions, clean.joint_positions);
        let (stepped, _, _) = env.step(Action::Torque(vec![0.0, 0.0]));
        assert_ne!(stepped.joint_positions, env.observe().joint_positions);
    }

    #[test]
    fn base_state_and_step_info_are_reported() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
        )
        .unwrap();
        let obs = env.reset();
        // Default base is the ground instance: present, at rest.
        let pose = obs.base_pose.expect("base pose should be reported");
        let vel = obs.base_velocity.expect("base velocity should be reported");
        assert!(vel.iter().all(|v| v.abs() < 1e-12));

        let result = env.step_full(Action::Torque(vec![0.01, 0.01]));
        assert_eq!(result.info.step, 1);
        assert!(!result.info.truncated);
        assert!(!result.info.terminated);
        assert_eq!(result.info.base_height_m, Some(pose[2]));
        assert!(result.info.base_tilt_deg.unwrap() < 1e-6);
    }

    #[test]
    fn configurable_termination_fires_on_base_height() {
        let doc = create_simple_robot();
        let cfg = EnvConfig {
            termination: Some(TerminationConfig {
                // Ground base sits near z=0 — an absurdly high floor makes
                // the condition fire on the first step, proving the plumbing.
                base_height_below: Some(10.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut env = RobotEnv::new_with_config(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
            cfg,
        )
        .unwrap();
        env.reset();
        let result = env.step_full(Action::Torque(vec![0.0, 0.0]));
        assert!(result.done);
        assert!(result.info.terminated);
        assert_eq!(
            result.info.termination_reason.as_deref(),
            Some("base_height")
        );
    }

    #[test]
    fn joint_limit_violations_reported_in_info() {
        let doc = create_simple_robot();
        let mut env = RobotEnv::new(
            doc,
            vec!["link2_inst".to_string()],
            None,
            None,
            Some(GroundConfig::disabled()),
        )
        .unwrap();
        env.reset();
        // Drive joint1 hard into its +90° limit; the world hard-clamps there.
        let mut last = None;
        for _ in 0..300 {
            last = Some(env.step_full(Action::PositionTarget(vec![90.0, 0.0])));
        }
        let info = last.unwrap().info;
        assert!(
            info.joint_limit_violations.contains(&"joint1".to_string()),
            "expected joint1 at its limit, got {:?}",
            info.joint_limit_violations
        );
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
