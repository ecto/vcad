//! Derivative-free policy training on top of [`RobotEnv`].
//!
//! Implements ARS (Augmented Random Search, Mania et al. 2018) V2-t: a linear
//! policy over whitened observations, updated from the top-`k` of `n`
//! antithetic finite-difference directions. ARS is the right first trainer for
//! this seam — it needs only env rollouts (no gradients through contact), it
//! parallelizes perfectly across directions, and on locomotion benchmarks it
//! matches PPO-class results with a policy small enough to read.
//!
//! The crate ships the *algorithm* and the observation plumbing; the reward is
//! a caller-supplied closure over [`StepResult`], matching the kernel's stance
//! that no reward DSL belongs in the physics.
//!
//! # ARS does not converge and stay converged
//!
//! The single most useful thing to know about this trainer: its update is
//! scale-free. `α·(r₊−r₋)/(k·σ)` normalizes away the return spread, so the
//! applied step has about the same norm no matter how good the policy already
//! is — there is no built-in annealing as it approaches a solution. Left at
//! too large an `α` it will find a solution and then walk straight back out
//! of it, at the same speed it walked in.
//!
//! On the K1 standing task with randomization off and the old default
//! `α = 0.03`, held-out score reached a perfect 400/400 on iteration 20 and
//! was −16 by iteration 29, with the update norm flat at ~0.4 throughout —
//! including while the policy was solving the task. At `α = 0.005` the same
//! task reaches 400/400 on iteration 42 and is still there 17 iterations
//! later. Three seeds, same story. See [`ArsConfig::step_size`].
//!
//! Two habits follow, and this module is shaped around both: score iterates
//! on held-out seeds from the `on_iteration` callback and *keep the best one*
//! rather than whatever the last iteration happened to leave you with; and
//! watch [`IterationLog::update_norm`] and [`IterationLog::sigma`] when a run
//! climbs and then falls apart.
//!
//! # Evaluating a run on a randomized env
//!
//! One thing to internalize before reading any number this module produces:
//! **[`IterationLog::eval_reward`] is not a measure of policy quality.** It is
//! an average over [`ArsConfig::rollouts_per_eval`] episodes of a
//! domain-randomized env, and with a handful of rollouts its variance across
//! randomization draws swamps the difference between a good policy and a bad
//! one.
//!
//! Measured on the Booster K1 standing task (see the `k1_stand` example), the
//! trainer's eval column and a ten-seed held-out score are not merely noisy
//! versions of each other — they point in *opposite* directions. Held-out
//! score peaked at 35.40 on iteration 3 and decayed to below the
//! do-nothing baseline by iteration 20, while the eval column sat at 30–45
//! throughout and hit its maximum for the whole run (57.16) on the iteration
//! whose held-out score was 10.84.
//!
//! Two consequences are baked into this module's API:
//!
//! - [`train`]'s `on_iteration` callback receives the current policy, so a
//!   caller can score each iterate against seeds the trainer never touches.
//!   That external score is the only trustworthy one.
//! - [`TrainOutcome::best_policy`] selects on the trainer's own eval seeds and
//!   is therefore a trap; see its docs.
//!
//! Antithetic pairs and all directions within an iteration already share a
//! seed set (common random numbers), so the *comparison* between directions is
//! fair. What stays noisy is the comparison *across* iterations, since each
//! iteration draws a fresh seed set — which is what makes the argmax over
//! iterations select for lucky draws.

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use vcad_ir::Document;
use vcad_kernel_physics::{Action, EnvConfig, RobotEnv, StepResult};

/// Per-joint PD gains applied to the env before every rollout.
pub type JointGains = HashMap<String, (f64, f64)>;

/// How a policy output is turned into an env action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Nominal joint targets in degrees, indexed like
    /// [`RobotEnv::actuated_joint_ids`]. The policy outputs a delta from this.
    pub default_pose_deg: Vec<f64>,
    /// Scale (degrees) applied to each raw policy output before adding it to
    /// the default pose. Bounds how far one action can command the joint.
    pub action_scale_deg: f64,
}

/// Everything needed to rebuild an identical env for a rollout.
#[derive(Clone)]
pub struct EnvSpec {
    /// Robot document (assembly with a Free base joint for locomotion).
    pub doc: Document,
    /// Instance ids tracked as end effectors (e.g. the feet).
    pub end_effector_ids: Vec<String>,
    /// Physics timestep, seconds.
    pub dt: f32,
    /// Physics substeps per env step.
    pub substeps: u32,
    /// Env config (termination, randomization, observation noise, base id).
    pub config: EnvConfig,
    /// Per-joint PD gains, reapplied on every reset.
    pub gains: JointGains,
    /// Max steps per episode.
    pub max_steps: u32,
}

impl EnvSpec {
    /// Instantiate a fresh env from this spec.
    pub fn build(&self) -> Result<RobotEnv, vcad_kernel_physics::PhysicsError> {
        let mut env = RobotEnv::new_with_config(
            self.doc.clone(),
            self.end_effector_ids.clone(),
            Some(self.dt),
            Some(self.substeps),
            None,
            self.config.clone(),
        )?;
        env.set_max_steps(self.max_steps);
        for (id, (kp, kd)) in &self.gains {
            env.set_joint_gains(id, *kp, *kd);
        }
        Ok(env)
    }
}

/// What ARS needs of a policy: a flat parameter vector it can perturb, the
/// whitening statistics it maintains, and a way to act.
///
/// ARS never differentiates the policy, so this is the whole interface — any
/// architecture that can expose its parameters as one `Vec<f64>` trains
/// unchanged. That is what lets [`MlpPolicy`] drop in beside [`LinearPolicy`]
/// without touching the trainer.
pub trait Policy: Clone + Send + Sync {
    /// Observation feature count.
    fn obs_dim(&self) -> usize;
    /// Flat, contiguous parameter vector — the thing ARS perturbs.
    fn params(&self) -> &[f64];
    /// Mutable view of the same vector. Length must never change.
    fn params_mut(&mut self) -> &mut [f64];
    /// Install refreshed observation whitening statistics.
    fn set_whitening(&mut self, mean: Vec<f64>, std: Vec<f64>);
    /// Joint position targets (degrees) for a feature vector.
    fn act(&self, features: &[f64]) -> Vec<f64>;
}

/// A linear policy `a = W · ẑ` over whitened observation features, with the
/// running feature statistics ARS V2 uses for whitening.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearPolicy {
    /// Row-major `act_dim × obs_dim` weight matrix.
    pub weights: Vec<f64>,
    /// Observation feature count.
    pub obs_dim: usize,
    /// Action dimension (actuated joint count).
    pub act_dim: usize,
    /// Running feature mean used for whitening.
    pub mean: Vec<f64>,
    /// Running feature standard deviation used for whitening.
    pub std: Vec<f64>,
    /// How raw outputs map to joint targets.
    pub action: ActionSpec,
}

impl LinearPolicy {
    /// A zero-initialized policy (ARS starts from the origin, not noise).
    pub fn zeros(obs_dim: usize, act_dim: usize, action: ActionSpec) -> Self {
        Self {
            weights: vec![0.0; obs_dim * act_dim],
            obs_dim,
            act_dim,
            mean: vec![0.0; obs_dim],
            std: vec![1.0; obs_dim],
            action,
        }
    }

    /// Joint position targets (degrees) for a feature vector.
    pub fn act(&self, features: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.act_dim];
        for (a, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (i, f) in features.iter().enumerate() {
                let z = (f - self.mean[i]) / self.std[i].max(1e-6);
                acc += self.weights[a * self.obs_dim + i] * z;
            }
            *o = self.action.default_pose_deg[a]
                + (acc.clamp(-1.0, 1.0)) * self.action.action_scale_deg;
        }
        out
    }
}

impl Policy for LinearPolicy {
    fn obs_dim(&self) -> usize {
        self.obs_dim
    }
    fn params(&self) -> &[f64] {
        &self.weights
    }
    fn params_mut(&mut self) -> &mut [f64] {
        &mut self.weights
    }
    fn set_whitening(&mut self, mean: Vec<f64>, std: Vec<f64>) {
        self.mean = mean;
        self.std = std;
    }
    fn act(&self, features: &[f64]) -> Vec<f64> {
        LinearPolicy::act(self, features)
    }
}

/// A one-hidden-layer `tanh` policy over the same whitened features.
///
/// Balance switches contact mode — double support, single support, toe-off —
/// and a linear map has to serve every mode with one gain matrix. This is the
/// smallest thing with enough capacity to gate on the mode.
///
/// The first layer is initialized *random*, not zero, unlike the linear
/// policy. ARS at an all-zero MLP is nearly stationary: with `W1 = 0` the
/// hidden activations are zero, so a perturbation of `W2` has no effect at
/// all and only the second-order `W1×W2` term moves the return. Random `W1`
/// with `W2 = 0` starts the policy at the same do-nothing action as the
/// linear one (so the two are comparable from step 0) while giving `W2` a
/// non-degenerate basis to act on immediately. Both layers train.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpPolicy {
    /// Flat parameters: `W1 (hidden × obs)`, `b1 (hidden)`, `W2 (act ×
    /// hidden)`, `b2 (act)`, concatenated in that order.
    pub params: Vec<f64>,
    /// Observation feature count.
    pub obs_dim: usize,
    /// Hidden unit count.
    pub hidden: usize,
    /// Action dimension.
    pub act_dim: usize,
    /// Running feature mean used for whitening.
    pub mean: Vec<f64>,
    /// Running feature standard deviation used for whitening.
    pub std: Vec<f64>,
    /// How raw outputs map to joint targets.
    pub action: ActionSpec,
}

impl MlpPolicy {
    /// A policy with random first-layer features and a zero output layer.
    ///
    /// `seed` fixes the first-layer draw so two runs being compared share an
    /// initialization and differ only in what is under test.
    pub fn new(
        obs_dim: usize,
        hidden: usize,
        act_dim: usize,
        action: ActionSpec,
        seed: u64,
    ) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        let scale = 1.0 / (obs_dim as f64).sqrt();
        let mut params = vec![0.0; hidden * obs_dim + hidden + act_dim * hidden + act_dim];
        for w in params.iter_mut().take(hidden * obs_dim) {
            *w = rng.gen_range(-1.0..1.0) * scale;
        }
        Self {
            params,
            obs_dim,
            hidden,
            act_dim,
            mean: vec![0.0; obs_dim],
            std: vec![1.0; obs_dim],
            action,
        }
    }
}

impl Policy for MlpPolicy {
    fn obs_dim(&self) -> usize {
        self.obs_dim
    }
    fn params(&self) -> &[f64] {
        &self.params
    }
    fn params_mut(&mut self) -> &mut [f64] {
        &mut self.params
    }
    fn set_whitening(&mut self, mean: Vec<f64>, std: Vec<f64>) {
        self.mean = mean;
        self.std = std;
    }
    fn act(&self, features: &[f64]) -> Vec<f64> {
        let (n_w1, n_b1) = (self.hidden * self.obs_dim, self.hidden);
        let (w1, rest) = self.params.split_at(n_w1);
        let (b1, rest) = rest.split_at(n_b1);
        let (w2, b2) = rest.split_at(self.act_dim * self.hidden);

        let mut h = vec![0.0; self.hidden];
        for (j, hj) in h.iter_mut().enumerate() {
            let mut acc = b1[j];
            for (i, f) in features.iter().enumerate() {
                let z = (f - self.mean[i]) / self.std[i].max(1e-6);
                acc += w1[j * self.obs_dim + i] * z;
            }
            *hj = acc.tanh();
        }

        let mut out = vec![0.0; self.act_dim];
        for (a, o) in out.iter_mut().enumerate() {
            let mut acc = b2[a];
            for (j, hj) in h.iter().enumerate() {
                acc += w2[a * self.hidden + j] * hj;
            }
            *o = self.action.default_pose_deg[a]
                + acc.clamp(-1.0, 1.0) * self.action.action_scale_deg;
        }
        out
    }
}

/// Online mean/variance accumulator (Welford) for observation whitening.
#[derive(Debug, Clone, Default)]
struct RunningStats {
    n: f64,
    mean: Vec<f64>,
    m2: Vec<f64>,
}

impl RunningStats {
    fn new(dim: usize) -> Self {
        Self {
            n: 0.0,
            mean: vec![0.0; dim],
            m2: vec![0.0; dim],
        }
    }

    fn push(&mut self, x: &[f64]) {
        self.n += 1.0;
        for ((xi, mean), m2) in x.iter().zip(self.mean.iter_mut()).zip(self.m2.iter_mut()) {
            let d = xi - *mean;
            *mean += d / self.n;
            *m2 += d * (xi - *mean);
        }
    }

    fn merge(&mut self, other: &RunningStats) {
        if other.n == 0.0 {
            return;
        }
        if self.n == 0.0 {
            *self = other.clone();
            return;
        }
        let n = self.n + other.n;
        for i in 0..self.mean.len() {
            let d = other.mean[i] - self.mean[i];
            let mean = self.mean[i] + d * other.n / n;
            self.m2[i] += other.m2[i] + d * d * self.n * other.n / n;
            self.mean[i] = mean;
        }
        self.n = n;
    }

    fn std(&self) -> Vec<f64> {
        self.m2
            .iter()
            .map(|m2| (m2 / self.n.max(1.0)).sqrt().max(1e-3))
            .collect()
    }
}

/// ARS hyperparameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArsConfig {
    /// Number of antithetic directions sampled per iteration.
    pub n_directions: usize,
    /// How many best-performing directions contribute to the update.
    pub top_k: usize,
    /// Step size α.
    ///
    /// This matters more than it looks, because the ARS update is
    /// *scale-free*: `α·(r₊−r₋)/(k·σ)` divides out the return spread, so the
    /// applied step has roughly the same norm whether the policy is terrible
    /// or optimal. It does not anneal on its own as the policy converges the
    /// way a gradient method's does. Too large an α therefore does not show
    /// up as failure to learn — it shows up as *inability to stay* anywhere
    /// good.
    ///
    /// Measured on the K1 standing task with the previous default of 0.03,
    /// with domain randomization off so the signal is clean. Held-out score
    /// per iteration, and the norm of the applied update:
    ///
    /// | seed | reaches 400/400 | then                        | ‖ΔW‖    |
    /// |------|-----------------|-----------------------------|---------|
    /// | 7    | iteration 20    | −16 by iteration 29         | 0.39–0.47 |
    /// | 23   | iteration 8     | 87 by iteration 24          | 0.36–0.42 |
    /// | 11   | never (peak 88) | wanders 35–87               | 0.31–0.46 |
    ///
    /// The step norm is flat across every one of those regimes, including
    /// while the policy is solving the task perfectly. ARS walks out of the
    /// solution basin at the same speed it walked in.
    ///
    /// At 0.005 the same task reaches 400/400 on iteration 42 and is still
    /// there 17 iterations later. If a run of yours climbs and then falls
    /// apart, reach for this before anything else, and log
    /// [`IterationLog::update_norm`] to see the step that is doing it.
    pub step_size: f64,
    /// Exploration noise ν.
    pub noise_std: f64,
    /// Training iterations.
    pub iterations: usize,
    /// Rollouts averaged per policy evaluation — both when scoring a
    /// direction and when evaluating the current iterate.
    ///
    /// On a randomized env a single rollout measures the *episode's*
    /// randomization draw at least as much as the policy: selecting the
    /// best-scoring iterate from single rollouts reliably picks a lucky draw
    /// (measured: an iterate scoring 116 on its own eval seed averaged 7.9
    /// across ten fresh ones, no better than doing nothing). The same seed
    /// set is shared by the ± pair of every direction, so the comparison is
    /// a common-random-numbers one and the draw cancels out of the
    /// difference.
    pub rollouts_per_eval: usize,
    /// RNG seed.
    pub seed: u64,
}

impl Default for ArsConfig {
    fn default() -> Self {
        Self {
            n_directions: 16,
            top_k: 8,
            // Deliberately small: a step that finds a solution faster but
            // cannot stay in it is worth less than a slower one that can.
            // See the measurements on `step_size`.
            step_size: 0.005,
            noise_std: 0.03,
            iterations: 200,
            // One rollout per evaluation measures the randomization draw as
            // much as the policy; only sound on a deterministic env.
            rollouts_per_eval: 3,
            seed: 0,
        }
    }
}

/// One rollout's outcome.
#[derive(Debug, Clone)]
pub struct RolloutStats {
    /// Undiscounted return.
    pub reward: f64,
    /// Steps survived.
    pub steps: u32,
    /// Why the episode ended, when it terminated early.
    pub termination_reason: Option<String>,
}

/// Per-iteration training log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationLog {
    /// Iteration index (0-based).
    pub iteration: usize,
    /// Mean return over all perturbed rollouts this iteration.
    pub mean_reward: f64,
    /// Best perturbed return this iteration.
    pub max_reward: f64,
    /// Return of the (unperturbed) current policy, evaluated each iteration.
    pub eval_reward: f64,
    /// Steps survived by the evaluation rollout.
    pub eval_steps: u32,
    /// Spread of the top-`k` returns this iteration — the denominator of the
    /// ARS update.
    ///
    /// Worth logging because it is the failure mode: as the policy converges,
    /// every rollout returns nearly the same thing, this collapses toward
    /// zero, and the update `α·(r₊−r₋)/(k·σ)` blows up. A run that reaches a
    /// good policy and then falls apart will show `sigma` cratering one or
    /// two iterations before the return does.
    pub sigma: f64,
    /// Norm of the parameter update actually applied this iteration.
    pub update_norm: f64,
}

/// Build the policy feature vector from an env observation.
///
/// Layout (all SI-ish and roughly unit-scaled before whitening):
/// `[projected gravity (3), base linear vel (3), base angular vel (3),
///   base height − nominal (1), joint angles rad (n), joint velocities rad/s (n)]`
pub fn features(
    obs: &vcad_kernel_physics::Observation,
    slots: &[usize],
    nominal_h: f64,
) -> Vec<f64> {
    let mut f = Vec::with_capacity(10 + 2 * slots.len());
    // Gravity in the base frame: the classic tilt observation. q = [w,x,y,z].
    let (g, h) = match obs.base_pose {
        Some([_, _, z, qw, qx, qy, qz]) => {
            // Third row of R(q)ᵀ · (0,0,-1) = -(R⁻¹ e_z).
            let gx = -2.0 * (qx * qz - qw * qy);
            let gy = -2.0 * (qy * qz + qw * qx);
            let gz = -(1.0 - 2.0 * (qx * qx + qy * qy));
            ([gx, gy, gz], z)
        }
        None => ([0.0, 0.0, -1.0], nominal_h),
    };
    f.extend_from_slice(&g);
    let v = obs.base_velocity.unwrap_or([0.0; 6]);
    f.extend_from_slice(&v[0..3]);
    f.extend_from_slice(&v[3..6]);
    f.push(h - nominal_h);
    for &s in slots {
        f.push(obs.joint_positions[s].to_radians());
    }
    for &s in slots {
        f.push(obs.joint_velocities[s].to_radians());
    }
    f
}

/// Flattened observation slot index of each actuated joint's first DOF.
///
/// The observation concatenates every joint's DOFs in document order —
/// including the 6 slots of a floating base — so a policy that wants "the 22
/// actuated angles" has to walk the slot table rather than index directly.
pub fn actuated_slots(env: &RobotEnv) -> Vec<usize> {
    let counts = env.joint_slot_counts();
    let ids = env.joint_ids();
    let actuated: std::collections::HashSet<&str> = env
        .actuated_joint_ids()
        .iter()
        .map(|s| s.as_str())
        .collect();
    let mut slots = Vec::new();
    let mut cursor = 0usize;
    for (id, n) in ids.iter().zip(counts.iter()) {
        if actuated.contains(id.as_str()) {
            slots.push(cursor);
        }
        cursor += n;
    }
    slots
}

/// Run one episode with `policy`, returning its return and stats.
///
/// `reward` sees each step's full [`StepResult`] plus the commanded action, so
/// task shaping stays entirely on the caller's side.
pub fn rollout<P, R>(
    env: &mut RobotEnv,
    policy: &P,
    slots: &[usize],
    nominal_h: f64,
    seed: u64,
    reward: &R,
    stats: Option<&mut RunningStatsHandle>,
) -> RolloutStats
where
    P: Policy,
    R: Fn(&StepResult, &[f64]) -> f64 + Sync,
{
    let mut obs = env.reset_with_seed(seed);
    let mut total = 0.0;
    let mut steps = 0;
    let mut reason = None;
    let mut collector = stats;
    for _ in 0..env.max_steps() {
        let f = features(&obs, slots, nominal_h);
        if let Some(c) = collector.as_deref_mut() {
            c.0.push(&f);
        }
        let targets = policy.act(&f);
        let result = env.step_full(Action::PositionTarget(targets.clone()));
        total += reward(&result, &targets);
        steps += 1;
        obs = result.observation;
        if result.done {
            reason = result.info.termination_reason.clone();
            break;
        }
    }
    RolloutStats {
        reward: total,
        steps,
        termination_reason: reason,
    }
}

/// Opaque handle over the whitening accumulator, so rollouts can feed it
/// without exposing Welford internals.
#[derive(Debug, Clone)]
pub struct RunningStatsHandle(RunningStats);

impl RunningStatsHandle {
    /// A fresh accumulator over `dim` features.
    pub fn new(dim: usize) -> Self {
        Self(RunningStats::new(dim))
    }
}

/// Result of a training run.
pub struct TrainOutcome<P> {
    /// The policy as of the final iteration.
    pub policy: P,
    /// The iterate that scored highest on the trainer's *own* evaluation
    /// seeds.
    ///
    /// **This is not the best policy, and on a randomized env it is not even
    /// correlated with being good.** Measured on the K1 standing task, per
    /// iteration, against ten held-out seeds: the iterate this field selects
    /// scored 10.84 while the iterate three updates earlier scored 35.40, and
    /// the selection was made precisely *because* that iterate drew the
    /// luckiest evaluation (57.16, the highest training-eval of the run). The
    /// argmax over a noisy score selects for noise, and it selects harder the
    /// longer training runs.
    ///
    /// Use [`Self::policy`], or better, do the selection yourself against
    /// held-out seeds from the `on_iteration` callback, which is handed each
    /// iterate for exactly this purpose. This field is retained only so a
    /// caller can measure the gap.
    pub best_policy: P,
    /// Evaluation return of [`Self::best_policy`] on the trainer's own seeds
    /// — the quantity that does the overfitting described there.
    pub best_eval_reward: f64,
    /// Per-iteration log.
    pub log: Vec<IterationLog>,
}

/// Train a linear policy with ARS.
///
/// `reward` is called once per env step. `on_iteration` is invoked after each
/// update with the log entry *and the updated policy*, so a caller can score
/// it on held-out seeds or checkpoint it. On a randomized env that hook is
/// not optional bookkeeping: the trainer's own eval return is not a
/// trustworthy measure of the iterate, so without an external one there is no
/// way to know which iteration was actually best.
pub fn train<P, R, F>(
    spec: &EnvSpec,
    policy: P,
    cfg: &ArsConfig,
    nominal_h: f64,
    reward: R,
    on_iteration: F,
) -> Result<TrainOutcome<P>, vcad_kernel_physics::PhysicsError>
where
    P: Policy,
    R: Fn(&StepResult, &[f64]) -> f64 + Sync + Send,
    F: FnMut(&IterationLog, &P),
{
    train_curriculum(
        |_| spec.clone(),
        policy,
        cfg,
        nominal_h,
        reward,
        on_iteration,
    )
}

/// Train with an env that *changes over training*.
///
/// `spec_at` is called with training progress in `[0, 1]` and returns the env
/// to collect rollouts in at that point — the hook for a domain-randomization
/// curriculum, where the ranges start narrow and widen toward the real task.
///
/// Evaluation always runs at `spec_at(1.0)`, the full target task, never at
/// the current curriculum level. An eval that eases off with the curriculum
/// would show a rising curve made entirely of the task getting easier.
pub fn train_curriculum<P, R, F, S>(
    spec_at: S,
    mut policy: P,
    cfg: &ArsConfig,
    nominal_h: f64,
    reward: R,
    mut on_iteration: F,
) -> Result<TrainOutcome<P>, vcad_kernel_physics::PhysicsError>
where
    P: Policy,
    R: Fn(&StepResult, &[f64]) -> f64 + Sync + Send,
    F: FnMut(&IterationLog, &P),
    S: Fn(f64) -> EnvSpec + Sync,
{
    let target_spec = spec_at(1.0);
    let probe = target_spec.build()?;
    let slots = actuated_slots(&probe);
    drop(probe);

    let mut rng = StdRng::seed_from_u64(cfg.seed);
    let obs_dim = policy.obs_dim();
    let n_params = policy.params().len();
    let mut stats = RunningStats::new(obs_dim);
    let mut log = Vec::with_capacity(cfg.iterations);
    let mut best = (f64::NEG_INFINITY, policy.clone());

    for it in 0..cfg.iterations {
        // Sample δ directions up front so the parallel section is pure.
        let deltas: Vec<Vec<f64>> = (0..cfg.n_directions)
            .map(|_| (0..n_params).map(|_| rng.gen_range(-1.0..1.0)).collect())
            .collect();
        let base_seed = rng.gen::<u64>();
        // The curriculum level this iteration collects rollouts at.
        let progress = if cfg.iterations > 1 {
            it as f64 / (cfg.iterations - 1) as f64
        } else {
            1.0
        };
        let iter_spec = spec_at(progress);

        let results: Vec<Result<(f64, f64, RunningStats), vcad_kernel_physics::PhysicsError>> =
            deltas
                .par_iter()
                .enumerate()
                .map(|(d, delta)| {
                    let mut env = iter_spec.build()?;
                    let mut local = RunningStatsHandle::new(obs_dim);
                    let _ = d;
                    let mut score = |sign: f64| -> f64 {
                        let mut p = policy.clone();
                        for (w, dw) in p.params_mut().iter_mut().zip(delta.iter()) {
                            *w += sign * cfg.noise_std * dw;
                        }
                        // Same seed set for +δ and −δ (and for every
                        // direction this iteration): common random numbers.
                        let k = cfg.rollouts_per_eval.max(1);
                        (0..k)
                            .map(|j| {
                                rollout(
                                    &mut env,
                                    &p,
                                    &slots,
                                    nominal_h,
                                    base_seed.wrapping_add(j as u64),
                                    &reward,
                                    Some(&mut local),
                                )
                                .reward
                            })
                            .sum::<f64>()
                            / k as f64
                    };
                    let plus = score(1.0);
                    let minus = score(-1.0);
                    Ok((plus, minus, local.0))
                })
                .collect();

        let mut scored: Vec<(f64, f64, usize)> = Vec::with_capacity(cfg.n_directions);
        for (i, r) in results.into_iter().enumerate() {
            let (plus, minus, local) = r?;
            stats.merge(&local);
            scored.push((plus, minus, i));
        }

        // ARS V1-t: rank directions by max(r+, r-), keep the top k.
        scored.sort_by(|a, b| b.0.max(b.1).total_cmp(&a.0.max(a.1)));
        let top = &scored[..cfg.top_k.min(scored.len())];
        let rewards: Vec<f64> = top.iter().flat_map(|(p, m, _)| [*p, *m]).collect();
        let mean_r = rewards.iter().sum::<f64>() / rewards.len() as f64;
        let sigma = (rewards.iter().map(|r| (r - mean_r).powi(2)).sum::<f64>()
            / rewards.len() as f64)
            .sqrt()
            .max(1e-6);

        let before: Vec<f64> = policy.params().to_vec();
        for (plus, minus, i) in top {
            let coeff = cfg.step_size * (plus - minus) / (cfg.top_k as f64 * sigma);
            for (w, dw) in policy.params_mut().iter_mut().zip(deltas[*i].iter()) {
                *w += coeff * dw;
            }
        }
        let update_norm = policy
            .params()
            .iter()
            .zip(before.iter())
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>()
            .sqrt();

        // Refresh whitening from everything seen so far.
        policy.set_whitening(stats.mean.clone(), stats.std());

        // Evaluate the updated policy on a *fixed* held-out seed set, the
        // same one every iteration, averaged. Fixed so the curve is
        // comparable across iterations; averaged so it measures the policy
        // rather than the draw.
        let mut env = target_spec.build()?;
        let k = cfg.rollouts_per_eval.max(1);
        let evals: Vec<RolloutStats> = (0..k)
            .map(|j| {
                rollout(
                    &mut env,
                    &policy,
                    &slots,
                    nominal_h,
                    u64::MAX - j as u64,
                    &reward,
                    None,
                )
            })
            .collect();
        let eval = RolloutStats {
            reward: evals.iter().map(|e| e.reward).sum::<f64>() / k as f64,
            steps: (evals.iter().map(|e| e.steps as f64).sum::<f64>() / k as f64) as u32,
            termination_reason: evals[0].termination_reason.clone(),
        };
        let _ = it;

        if eval.reward > best.0 {
            best = (eval.reward, policy.clone());
        }

        let all: Vec<f64> = scored.iter().flat_map(|(p, m, _)| [*p, *m]).collect();
        let entry = IterationLog {
            iteration: it,
            mean_reward: all.iter().sum::<f64>() / all.len() as f64,
            max_reward: all.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            eval_reward: eval.reward,
            eval_steps: eval.steps,
            sigma,
            update_norm,
        };
        on_iteration(&entry, &policy);
        log.push(entry);
    }

    Ok(TrainOutcome {
        policy,
        best_policy: best.1,
        best_eval_reward: best.0,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(act_dim: usize) -> ActionSpec {
        ActionSpec {
            default_pose_deg: vec![0.0; act_dim],
            action_scale_deg: 8.0,
        }
    }

    /// Both architectures start at the default pose, so a run that swaps one
    /// for the other is comparable from iteration 0 rather than starting the
    /// MLP somewhere else on the reward surface.
    #[test]
    fn both_policies_start_at_the_default_pose() {
        let (obs, act) = (54, 22);
        let f = vec![0.3; obs];
        let lin = LinearPolicy::zeros(obs, act, spec(act));
        let mlp = MlpPolicy::new(obs, 16, act, spec(act), 7);
        assert_eq!(Policy::act(&lin, &f), vec![0.0; act]);
        assert_eq!(Policy::act(&mlp, &f), vec![0.0; act]);
    }

    /// The MLP's first layer must be non-degenerate at init. If it were zero
    /// (the obvious choice, and the one the linear policy uses) the hidden
    /// activations would be zero, every second-layer perturbation would have
    /// exactly zero effect on the return, and ARS would sit still burning
    /// rollouts while looking merely slow to converge.
    #[test]
    fn mlp_output_layer_has_a_nonzero_basis_to_act_on() {
        let (obs, hidden, act) = (54, 16, 22);
        let f: Vec<f64> = (0..obs).map(|i| (i as f64 * 0.1).sin()).collect();
        let mut mlp = MlpPolicy::new(obs, hidden, act, spec(act), 7);

        // Perturb only the second layer, exactly as ARS would.
        let w2_start = hidden * obs + hidden;
        for w in mlp.params[w2_start..w2_start + act * hidden].iter_mut() {
            *w = 0.01;
        }
        let out = Policy::act(&mlp, &f);
        assert!(
            out.iter().any(|&o| o.abs() > 1e-9),
            "second-layer perturbation had no effect — first layer is degenerate"
        );
    }

    /// Actions stay inside `default ± action_scale_deg` however large the
    /// features or weights get: the env is handed joint targets, and an
    /// unclamped policy would command past the joint limits.
    #[test]
    fn mlp_action_is_bounded_by_the_action_scale() {
        let (obs, act) = (54, 22);
        let f = vec![1e6; obs];
        let mut mlp = MlpPolicy::new(obs, 8, act, spec(act), 1);
        for w in mlp.params.iter_mut() {
            *w = 1e3;
        }
        for o in Policy::act(&mlp, &f) {
            assert!(o.abs() <= 8.0 + 1e-9, "action {o} escaped the action scale");
        }
    }

    /// `params_mut` must alias the same storage `act` reads, or ARS would
    /// perturb a copy and every direction would score identically.
    #[test]
    fn param_views_alias_the_live_parameters() {
        let (obs, act) = (12, 4);
        let mut mlp = MlpPolicy::new(obs, 5, act, spec(act), 3);
        let n = mlp.params().len();
        let before = Policy::act(&mlp, &vec![0.5; obs]);
        for w in mlp.params_mut().iter_mut().skip(n - act * 5 - act) {
            *w += 0.5;
        }
        assert_ne!(before, Policy::act(&mlp, &vec![0.5; obs]));
    }
}
