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
    pub step_size: f64,
    /// Exploration noise ν.
    pub noise_std: f64,
    /// Training iterations.
    pub iterations: usize,
    /// Max env steps per rollout.
    pub episode_steps: u32,
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
            step_size: 0.02,
            noise_std: 0.03,
            iterations: 200,
            episode_steps: 500,
            rollouts_per_eval: 1,
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
pub fn rollout<R>(
    env: &mut RobotEnv,
    policy: &LinearPolicy,
    slots: &[usize],
    nominal_h: f64,
    seed: u64,
    reward: &R,
    stats: Option<&mut RunningStatsHandle>,
) -> RolloutStats
where
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
pub struct TrainOutcome {
    /// The policy as of the final iteration.
    ///
    /// Prefer [`Self::best_policy`] for anything you intend to *use*: ARS's
    /// last iterate is a random draw from a noisy walk, and on a randomized
    /// env it is routinely far worse than the best iterate seen.
    pub policy: LinearPolicy,
    /// The iterate with the highest evaluation return, and that return.
    pub best_policy: LinearPolicy,
    /// Evaluation return of [`Self::best_policy`].
    pub best_eval_reward: f64,
    /// Per-iteration log.
    pub log: Vec<IterationLog>,
}

/// Train a linear policy with ARS.
///
/// `reward` is called once per env step. `on_iteration` is invoked after each
/// update with the log entry, for progress printing.
pub fn train<R, F>(
    spec: &EnvSpec,
    mut policy: LinearPolicy,
    cfg: &ArsConfig,
    nominal_h: f64,
    reward: R,
    mut on_iteration: F,
) -> Result<TrainOutcome, vcad_kernel_physics::PhysicsError>
where
    R: Fn(&StepResult, &[f64]) -> f64 + Sync + Send,
    F: FnMut(&IterationLog),
{
    let probe = spec.build()?;
    let slots = actuated_slots(&probe);
    drop(probe);

    let mut rng = StdRng::seed_from_u64(cfg.seed);
    let mut stats = RunningStats::new(policy.obs_dim);
    let mut log = Vec::with_capacity(cfg.iterations);
    let mut best = (f64::NEG_INFINITY, policy.clone());

    for it in 0..cfg.iterations {
        // Sample δ directions up front so the parallel section is pure.
        let deltas: Vec<Vec<f64>> = (0..cfg.n_directions)
            .map(|_| {
                (0..policy.weights.len())
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect()
            })
            .collect();
        let base_seed = rng.gen::<u64>();

        let results: Vec<Result<(f64, f64, RunningStats), vcad_kernel_physics::PhysicsError>> =
            deltas
                .par_iter()
                .enumerate()
                .map(|(d, delta)| {
                    let mut env = spec.build()?;
                    let mut local = RunningStatsHandle::new(policy.obs_dim);
                    let _ = d;
                    let mut score = |sign: f64| -> f64 {
                        let mut p = policy.clone();
                        for (w, dw) in p.weights.iter_mut().zip(delta.iter()) {
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

        for (plus, minus, i) in top {
            let coeff = cfg.step_size * (plus - minus) / (cfg.top_k as f64 * sigma);
            for (w, dw) in policy.weights.iter_mut().zip(deltas[*i].iter()) {
                *w += coeff * dw;
            }
        }

        // Refresh whitening from everything seen so far.
        policy.mean = stats.mean.clone();
        policy.std = stats.std();

        // Evaluate the updated policy on a *fixed* held-out seed set, the
        // same one every iteration, averaged. Fixed so the curve is
        // comparable across iterations; averaged so it measures the policy
        // rather than the draw.
        let mut env = spec.build()?;
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
        };
        on_iteration(&entry);
        log.push(entry);
    }

    Ok(TrainOutcome {
        policy,
        best_policy: best.1,
        best_eval_reward: best.0,
        log,
    })
}
