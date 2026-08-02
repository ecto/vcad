//! C ABI for in-process policy training, and for the reward that defines the
//! task.
//!
//! # The reward is data
//!
//! The kernel deliberately computes no task reward, and the reference trainer
//! (`vcad-sim`'s `k1_stand` example) expresses one as a Rust closure. Neither
//! can cross an ABI. So the standing task's reward is reified here as
//! [`RewardSpec`] — a serde struct whose defaults are exactly the weights the
//! bundled K1 policies were trained against.
//!
//! That buys three things that a closure cannot:
//!
//! - The app can *show* the reward terms, and a user can retune them without
//!   a recompile.
//! - A trained policy can record the reward it was trained against, so
//!   "re-scored under a different reward" stops being an invisible mistake.
//! - Training in the app and training from the CLI are the same computation,
//!   because they read the same struct rather than two hand-copied formulas.
//!
//! # Threading
//!
//! Training runs on a worker thread; [`vcad_train_start`] returns immediately.
//! The UI polls [`vcad_train_poll`] for a snapshot and calls
//! [`vcad_train_stop`] to cancel. The worker owns the envs; nothing it touches
//! is shared with the caller except a mutex-guarded progress record and an
//! atomic cancel flag, so polling at 60 Hz costs a mutex acquire and a memcpy
//! of a few dozen bytes.
//!
//! # Held-out selection
//!
//! The trainer's own evaluation return is *not* a trustworthy measure of an
//! iterate on a randomized env — on the K1 standing task it has selected an
//! iterate worth 10.84 over one worth 35.40, precisely because that iterate
//! drew the luckiest evaluation. So this module scores iterates on a fixed
//! held-out seed set on an interval and keeps the best of those, which is what
//! [`vcad_train_best_policy_json`] returns. Getting this wrong doesn't error;
//! it just quietly ships a worse policy, which is why it is done here rather
//! than left to each caller.

use std::ops::ControlFlow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use vcad_ir::Document;
use vcad_kernel_physics::StepResult;
use vcad_sim::rl::{
    actuated_slots, rollout, train_curriculum, ActionSpec, ArsConfig, EnvSpec, IterationLog,
    LinearPolicy, MlpPolicy, Policy,
};

use crate::err::{clear_error, ctx, set_error};
use crate::gym::{GymSpec, VcadGym, VcadPolicy};

// =========================================================================
// Reward
// =========================================================================

/// The standing-balance reward, as data.
///
/// `alive - w_height·(h-h₀)² - w_tilt·tilt² - w_drift·|v|² - w_spin·|ω|² -
/// w_effort·mean((a/effort_scale)²)`, with tilt in radians.
///
/// Defaults are the measured K1 weights. Changing any of them changes the
/// task, not just the score — a policy trained under one set is not comparable
/// to one trained under another, which is why [`PolicyBundle`] records it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RewardSpec {
    /// Constant per-step survival bonus.
    pub alive: f64,
    /// Target base height in meters. Must match the policy's
    /// `nominal_height_m`, or the height term and the feature vector disagree
    /// about what "upright" means.
    pub nominal_height_m: f64,
    /// Weight on squared base-height error.
    pub height: f64,
    /// Weight on squared tilt from upright (radians).
    pub tilt: f64,
    /// Weight on squared base linear speed — penalizes drifting off the spot.
    pub drift: f64,
    /// Weight on squared base angular speed.
    pub spin: f64,
    /// Weight on mean squared normalized action — penalizes thrashing.
    pub effort: f64,
    /// Divisor applied to each action before the effort term (degrees).
    pub effort_scale_deg: f64,
}

impl Default for RewardSpec {
    fn default() -> Self {
        Self {
            alive: 1.0,
            nominal_height_m: 0.5498,
            height: 8.0,
            tilt: 1.5,
            drift: 0.3,
            spin: 0.05,
            effort: 0.1,
            effort_scale_deg: 30.0,
        }
    }
}

impl RewardSpec {
    /// Evaluate one step's reward.
    ///
    /// Reads the **ground-truth** height and tilt from `StepInfo`, never the
    /// (possibly noisy) observation: a reward that inherits sensor noise
    /// rewards lucky sensors. See the asymmetry note on
    /// `vcad_kernel_physics::StepInfo`.
    pub fn eval(&self, r: &StepResult, action: &[f64]) -> f64 {
        let h = r.info.base_height_m.unwrap_or(0.0);
        let tilt = r.info.base_tilt_deg.unwrap_or(90.0).to_radians();
        let v = r.observation.base_velocity.unwrap_or([0.0; 6]);
        let drift = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let spin = v[3] * v[3] + v[4] * v[4] + v[5] * v[5];
        let effort = if action.is_empty() {
            0.0
        } else {
            action
                .iter()
                .map(|a| (a / self.effort_scale_deg).powi(2))
                .sum::<f64>()
                / action.len() as f64
        };
        self.alive
            - self.height * (h - self.nominal_height_m).powi(2)
            - self.tilt * tilt * tilt
            - self.drift * drift
            - self.spin * spin
            - self.effort * effort
    }
}

/// Evaluate a reward spec (JSON) against a gym's most recent step, returning
/// the reward. Writes `nan` and sets the last error on failure.
///
/// Lets the app display a live reward for a hand-driven or policy-driven
/// rollout using the identical formula training uses.
#[no_mangle]
pub extern "C" fn vcad_gym_reward(
    gym: *const VcadGym,
    reward_json: *const u8,
    reward_json_len: usize,
) -> f64 {
    clear_error();
    if gym.is_null() {
        set_error("vcad_gym_reward: null handle");
        return f64::NAN;
    }
    let spec = match parse_or_default::<RewardSpec>("reward spec", reward_json, reward_json_len) {
        Some(s) => s,
        None => return f64::NAN,
    };
    crate::gym::reward_of_last_step(gym, &spec)
}

/// Parse a JSON blob into `T`, or `T::default()` when the pointer is null or
/// empty. Returns `None` (with the last error set) on a parse failure.
fn parse_or_default<T: for<'de> Deserialize<'de> + Default>(
    what: &str,
    json: *const u8,
    json_len: usize,
) -> Option<T> {
    if json.is_null() || json_len == 0 {
        return Some(T::default());
    }
    let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
    let text = ctx(&format!("{what} is not UTF-8"), || {
        std::str::from_utf8(bytes)
    })?;
    ctx(&format!("parse {what}"), || serde_json::from_str::<T>(text))
}

// =========================================================================
// Policy bundle — a policy plus the provenance that makes it reproducible
// =========================================================================

/// A trained policy together with everything needed to know whether it is
/// still valid: the `.vcadpolicy` payload.
///
/// The provenance is not documentation. A policy is only meaningful against
/// the exact plant it trained on, and the failure mode of a mismatch is not an
/// error — it is a robot that almost stands. Recording `dt`, `substeps`,
/// gains, the reward, and a content hash of the document turns "this policy
/// stopped working after I edited the model" from a mystery into a check.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolicyBundle {
    /// The trained policy (a `LinearPolicy` or `MlpPolicy` object).
    pub policy: serde_json::Value,
    /// Which iterate was kept: `last`, `train-eval-best`, or `held-out-best`.
    pub kept: String,
    /// Mean return over the held-out evaluation seeds.
    pub held_out_reward: f64,
    /// How many of the held-out seeds survived a full episode.
    pub held_out_full_episodes: usize,
    /// Number of held-out seeds scored.
    pub held_out_seeds: usize,
    /// The env this was trained against — dt, substeps, gains, randomization.
    pub env: GymSpec,
    /// The reward it was trained against.
    pub reward: RewardSpec,
    /// ARS hyperparameters.
    pub ars: ArsConfig,
    /// Content hash of the document, so an edited model is detectable.
    pub document_hash: String,
    /// Per-iteration training log.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<IterationLog>,
    /// Schema version of this bundle.
    #[serde(default = "one")]
    pub version: u32,
}

fn one() -> u32 {
    1
}

/// A stable content hash of a document, used to detect that a policy's model
/// has been edited under it.
///
/// FNV-1a over the canonical JSON serialization. Not cryptographic — the
/// threat model is "the user scrubbed a parameter", not forgery — but it is
/// deterministic across runs and platforms, which a `DefaultHasher` is
/// explicitly not.
///
/// **Semantic, not textual.** The input is the *parsed and re-serialized*
/// [`Document`], so reformatting, key reordering, and fields the IR does not
/// model all hash identically. That is the property you want: a policy should
/// not be declared stale because the file was pretty-printed. The corollary is
/// that this hash only moves when something the simulator can actually read
/// moves — which is exactly the set of edits that can invalidate a policy.
pub fn document_hash(doc: &Document) -> String {
    let value = serde_json::to_value(doc).unwrap_or(serde_json::Value::Null);
    let mut buf = String::new();
    canonicalize(&value, &mut buf);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in buf.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}")
}

/// Render a JSON value with object keys in sorted order.
///
/// Hashing `serde_json::to_string(doc)` directly is **wrong**, and wrong in the
/// worst way: [`Document`] holds `HashMap` fields (parameters, materials), and
/// Rust's `RandomState` gives each map instance its own iteration order. Two
/// parses of the *same file* therefore serialize to different byte strings, so
/// the hash of a document would differ from itself and every loaded policy
/// would report Stale at random — an alarm that fires constantly and is
/// therefore ignored, which is worse than no alarm.
///
/// Sorting keys here makes the digest depend on content alone. It is also
/// independent of whether `serde_json`'s `preserve_order` feature is on, which
/// silently changes `Map` from a sorted `BTreeMap` to an insertion-ordered
/// `IndexMap` and would otherwise reintroduce the bug from a dependency edit.
fn canonicalize(v: &serde_json::Value, out: &mut String) {
    use std::fmt::Write;
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{k:?}:");
                canonicalize(&map[*k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonicalize(item, out);
            }
            out.push(']');
        }
        other => {
            let _ = write!(out, "{other}");
        }
    }
}

/// Compute the document hash for a `.vcad` JSON blob, writing it as a
/// hex string into `out` (which must hold at least 24 bytes).
///
/// Returns the number of bytes written, or 0 on failure.
#[no_mangle]
pub extern "C" fn vcad_document_hash(
    doc_json: *const u8,
    doc_json_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    clear_error();
    if doc_json.is_null() || out.is_null() {
        set_error("vcad_document_hash: null pointer");
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(doc_json, doc_json_len) };
    let Some(text) = ctx("document is not UTF-8", || std::str::from_utf8(bytes)) else {
        return 0;
    };
    let Some(doc) = ctx("parse document", || Document::from_json(text)) else {
        return 0;
    };
    let h = document_hash(&doc);
    if out_cap < h.len() {
        set_error(format!(
            "vcad_document_hash: need {} bytes, got {out_cap}",
            h.len()
        ));
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(h.as_ptr(), out, h.len()) };
    h.len()
}

// =========================================================================
// Trainer
// =========================================================================

/// Live training progress, copied out under a mutex on every poll.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VcadTrainProgress {
    /// Iterations completed.
    pub iteration: u32,
    /// Total iterations requested.
    pub total_iterations: u32,
    /// Mean return over this iteration's perturbed rollouts.
    pub mean_reward: f64,
    /// Return of the unperturbed policy on the trainer's own eval seeds —
    /// **not** a trustworthy measure of the iterate; see the module docs.
    pub eval_reward: f64,
    /// Steps survived by that evaluation rollout.
    pub eval_steps: u32,
    /// Spread of the top-k returns. Collapsing toward zero one or two
    /// iterations before the return falls apart is the classic ARS failure,
    /// so it is surfaced rather than buried in a log.
    pub sigma: f64,
    /// Norm of the parameter update applied this iteration.
    pub update_norm: f64,
    /// Effective step size after any decay schedule.
    pub step_size: f64,
    /// Best held-out mean return seen so far.
    pub best_held_out: f64,
    /// Full episodes on the held-out seeds at that best.
    pub best_held_out_full: u32,
    /// Iteration the best held-out score came from.
    pub best_iteration: u32,
    /// 1 while the worker is running.
    pub running: u8,
    /// 1 once the worker has finished (completed or cancelled).
    pub finished: u8,
    /// 1 when the run ended in an error; read `vcad_train_error` for it.
    pub failed: u8,
    /// 1 when the run was cancelled by `vcad_train_stop`.
    pub cancelled: u8,
}

/// Shared state between the worker thread and the polling UI.
#[derive(Default)]
struct Shared {
    progress: Mutex<VcadTrainProgress>,
    /// Best-by-held-out policy so far, serialized. Swapped wholesale so a
    /// poller never sees a half-written policy.
    best_bundle: Mutex<Option<String>>,
    error: Mutex<Option<String>>,
    cancel: AtomicBool,
}

/// Opaque handle to a running (or finished) training run.
pub struct VcadTrainer {
    shared: Arc<Shared>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Trainer configuration, as JSON.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrainSpec {
    /// ARS hyperparameters.
    pub ars: ArsConfig,
    /// Policy architecture: `"mlp"` or `"linear"`.
    pub policy: String,
    /// Hidden units when `policy == "mlp"`.
    pub hidden: usize,
    /// Degrees one action may move a joint from the default pose.
    pub action_scale_deg: f64,
    /// Random seed for the MLP's first-layer initialization.
    pub init_seed: u64,
    /// Curriculum warmup: fraction of training over which the randomization
    /// level ramps 0 → 1. `0` disables the curriculum (full randomization
    /// from iteration 0).
    ///
    /// Measured on the K1 standing task, this is worth ~60 points of held-out
    /// return on a linear policy (221.92 → 280.33) for one number.
    pub curriculum_warmup: f64,
    /// Score every `n`-th iterate on the held-out seeds and keep the best.
    /// `0` disables held-out selection (and with it the only reliable way to
    /// know which iterate was good).
    pub held_out_every: usize,
    /// Number of held-out evaluation seeds, disjoint from training's.
    pub held_out_seeds: usize,
}

impl Default for TrainSpec {
    fn default() -> Self {
        Self {
            ars: ArsConfig {
                n_directions: 12,
                top_k: 6,
                step_size: 0.005,
                step_size_final: None,
                noise_std: 0.05,
                iterations: 150,
                rollouts_per_eval: 3,
                seed: 7,
            },
            policy: "mlp".to_string(),
            hidden: 64,
            action_scale_deg: 8.0,
            init_seed: 0,
            curriculum_warmup: 0.4,
            held_out_every: 5,
            held_out_seeds: 10,
        }
    }
}

/// Scale every randomization channel in a config by `level` in `[0, 1]`.
///
/// The curriculum is one scalar because every channel should widen together:
/// a schedule that ramps them independently has no defensible ordering and
/// makes "which channel is the run stuck on" unanswerable.
fn config_at(base: &vcad_kernel_physics::EnvConfig, level: f64) -> vcad_kernel_physics::EnvConfig {
    use vcad_kernel_physics::Range;
    let mut c = base.clone();
    let level = level.clamp(0.0, 1.0);
    if let Some(r) = c.randomization.as_mut() {
        // Multiplicative channels are centred on 1.0, so scaling the
        // *half-width* toward 0 collapses them to "no randomization".
        let lerp_range = |r: Range| Range {
            min: 1.0 + (r.min - 1.0) * level,
            max: 1.0 + (r.max - 1.0) * level,
        };
        r.mass_scale = r.mass_scale.map(lerp_range);
        r.friction_scale = r.friction_scale.map(lerp_range);
        r.pd_gain_scale = r.pd_gain_scale.map(lerp_range);
        // Additive channels scale straight to zero.
        r.joint_pos_perturb = r.joint_pos_perturb.map(|v| v * level);
        r.joint_vel_perturb = r.joint_vel_perturb.map(|v| v * level);
        r.action_latency_steps = r.action_latency_steps.map(|[lo, hi]| {
            let hi = (hi as f64 * level).round() as u32;
            [lo.min(hi), hi]
        });
    }
    c
}

/// Mean held-out return and full-episode count for a policy.
fn score_held_out<P: Policy>(
    spec: &EnvSpec,
    policy: &P,
    slots: &[usize],
    nominal_h: f64,
    reward: &RewardSpec,
    seeds: usize,
) -> Result<(f64, usize), vcad_kernel_physics::PhysicsError> {
    let mut env = spec.build()?;
    let f = |r: &StepResult, a: &[f64]| reward.eval(r, a);
    let mut total = 0.0;
    let mut full = 0usize;
    for s in 1..=seeds as u64 {
        let stats = rollout(&mut env, policy, slots, nominal_h, s, &f, None);
        total += stats.reward;
        if stats.steps == spec.max_steps {
            full += 1;
        }
    }
    Ok((total / seeds.max(1) as f64, full))
}

/// Run the ARS loop, publishing progress and best-so-far into `shared`.
///
/// Generic over the policy type so the linear and MLP paths are literally the
/// same code — the divergence between "which architecture" and "what the
/// trainer does" is exactly where a hand-duplicated trainer rots.
#[allow(clippy::too_many_arguments)]
fn run_training<P: Policy + Serialize + 'static>(
    shared: &Arc<Shared>,
    env_spec: EnvSpec,
    policy: P,
    tcfg: &TrainSpec,
    reward: RewardSpec,
    gym_spec: GymSpec,
    doc_hash: String,
) -> Result<(), String> {
    let nominal_h = reward.nominal_height_m;
    let target = env_spec.clone();
    let probe = target.build().map_err(|e| e.to_string())?;
    let slots = actuated_slots(&probe);
    drop(probe);

    let warmup = tcfg.curriculum_warmup;
    let base_config = env_spec.config.clone();
    let spec_at = |progress: f64| {
        let level = if warmup > 0.0 {
            (progress / warmup).min(1.0)
        } else {
            1.0
        };
        let mut s = env_spec.clone();
        s.config = config_at(&base_config, level);
        s
    };

    let reward_fn = {
        let reward = reward.clone();
        move |r: &StepResult, a: &[f64]| reward.eval(r, a)
    };

    let total = tcfg.ars.iterations as u32;
    {
        let mut p = shared.progress.lock().unwrap();
        p.total_iterations = total;
        p.running = 1;
        p.best_held_out = f64::NEG_INFINITY;
    }

    let mut best: Option<(f64, usize, u32)> = None;
    let mut iteration_err: Option<String> = None;

    let outcome = train_curriculum(
        spec_at,
        policy,
        &tcfg.ars,
        nominal_h,
        reward_fn,
        |log: &IterationLog, pol: &P| {
            // Breaking out is what actually stops the run. Returning early
            // without breaking would only skip the bookkeeping while ARS kept
            // spending rollouts — a "Stop" button that stops nothing.
            if shared.cancel.load(Ordering::Relaxed) || iteration_err.is_some() {
                return ControlFlow::Break(());
            }

            // Held-out scoring on an interval. This is the only measurement
            // the run may be judged by — see the module docs.
            let mut scored: Option<(f64, usize)> = None;
            if tcfg.held_out_every > 0 && log.iteration.is_multiple_of(tcfg.held_out_every) {
                match score_held_out(
                    &target,
                    pol,
                    &slots,
                    nominal_h,
                    &reward,
                    tcfg.held_out_seeds,
                ) {
                    Ok(s) => scored = Some(s),
                    Err(e) => {
                        iteration_err = Some(format!("held-out evaluation failed: {e}"));
                        return ControlFlow::Break(());
                    }
                }
            }

            if let Some((mean, full)) = scored {
                let better = best.map(|(b, _, _)| mean > b).unwrap_or(true);
                if better {
                    best = Some((mean, full, log.iteration as u32));
                    let bundle = PolicyBundle {
                        policy: serde_json::to_value(pol).unwrap_or(serde_json::Value::Null),
                        kept: "held-out-best".to_string(),
                        held_out_reward: mean,
                        held_out_full_episodes: full,
                        held_out_seeds: tcfg.held_out_seeds,
                        env: gym_spec.clone(),
                        reward: reward.clone(),
                        ars: tcfg.ars.clone(),
                        document_hash: doc_hash.clone(),
                        log: Vec::new(),
                        version: 1,
                    };
                    if let Ok(text) = serde_json::to_string(&bundle) {
                        *shared.best_bundle.lock().unwrap() = Some(text);
                    }
                }
            }

            let mut p = shared.progress.lock().unwrap();
            p.iteration = log.iteration as u32 + 1;
            p.mean_reward = log.mean_reward;
            p.eval_reward = log.eval_reward;
            p.eval_steps = log.eval_steps;
            p.sigma = log.sigma;
            p.update_norm = log.update_norm;
            p.step_size = log.step_size;
            if let Some((mean, full, it)) = best {
                p.best_held_out = mean;
                p.best_held_out_full = full as u32;
                p.best_iteration = it;
            }
            ControlFlow::Continue(())
        },
    )
    .map_err(|e| e.to_string())?;

    if let Some(e) = iteration_err {
        return Err(e);
    }

    // Score the final iterate too, so a run whose last iterations were its
    // best isn't discarded because the interval missed them.
    if tcfg.held_out_every > 0 && !shared.cancel.load(Ordering::Relaxed) {
        let (mean, full) = score_held_out(
            &target,
            &outcome.policy,
            &slots,
            nominal_h,
            &reward,
            tcfg.held_out_seeds,
        )
        .map_err(|e| e.to_string())?;
        if best.map(|(b, _, _)| mean > b).unwrap_or(true) {
            let bundle = PolicyBundle {
                policy: serde_json::to_value(&outcome.policy).unwrap_or(serde_json::Value::Null),
                kept: "last".to_string(),
                held_out_reward: mean,
                held_out_full_episodes: full,
                held_out_seeds: tcfg.held_out_seeds,
                env: gym_spec.clone(),
                reward: reward.clone(),
                ars: tcfg.ars.clone(),
                document_hash: doc_hash.clone(),
                log: outcome.log.clone(),
                version: 1,
            };
            if let Ok(text) = serde_json::to_string(&bundle) {
                *shared.best_bundle.lock().unwrap() = Some(text);
            }
            let mut p = shared.progress.lock().unwrap();
            p.best_held_out = mean;
            p.best_held_out_full = full as u32;
            p.best_iteration = total;
        }
    }

    Ok(())
}

/// Start a training run. Returns a handle immediately; the run proceeds on a
/// worker thread. Returns null and sets the last error if the env or specs
/// don't build.
///
/// `gym_spec_json` uses the same [`GymSpec`] shape as [`crate::gym::vcad_gym_create`],
/// so an env the app is already simulating and the env it trains in cannot
/// disagree. `train_spec_json` and `reward_json` may be null for defaults.
#[no_mangle]
pub extern "C" fn vcad_train_start(
    doc_json: *const u8,
    doc_json_len: usize,
    gym_spec_json: *const u8,
    gym_spec_json_len: usize,
    train_spec_json: *const u8,
    train_spec_json_len: usize,
    reward_json: *const u8,
    reward_json_len: usize,
) -> *mut VcadTrainer {
    clear_error();
    if doc_json.is_null() {
        set_error("vcad_train_start: null document");
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(doc_json, doc_json_len) };
        let Some(text) = ctx("document is not UTF-8", || std::str::from_utf8(bytes)) else {
            return ptr::null_mut();
        };
        let Some(mut doc) = ctx("parse document", || Document::from_json(text)) else {
            return ptr::null_mut();
        };
        let Some(gym_spec) =
            parse_or_default::<GymSpec>("gym spec", gym_spec_json, gym_spec_json_len)
        else {
            return ptr::null_mut();
        };
        let Some(tspec) =
            parse_or_default::<TrainSpec>("train spec", train_spec_json, train_spec_json_len)
        else {
            return ptr::null_mut();
        };
        let Some(reward) =
            parse_or_default::<RewardSpec>("reward spec", reward_json, reward_json_len)
        else {
            return ptr::null_mut();
        };

        if tspec.ars.top_k > tspec.ars.n_directions || tspec.ars.top_k == 0 {
            set_error(format!(
                "invalid ARS config: top_k={} must be in 1..={}",
                tspec.ars.top_k, tspec.ars.n_directions
            ));
            return ptr::null_mut();
        }
        // A reward whose target height disagrees with the feature vector's
        // means the policy is told it is upright at one height and rewarded
        // for another. Catch it before spending an hour of compute.
        if (reward.nominal_height_m - gym_spec.nominal_height_m).abs() > 1e-9 {
            set_error(format!(
                "reward.nominal_height_m ({}) != env nominal_height_m ({}) — the reward \
                 and the policy's own height feature would disagree about upright",
                reward.nominal_height_m, gym_spec.nominal_height_m
            ));
            return ptr::null_mut();
        }

        if let Some(z) = gym_spec.spawn_z_mm {
            if !crate::gym::raise_base(&mut doc, z) {
                set_error("spec sets spawn_z_mm but the document has no Free base joint");
                return ptr::null_mut();
            }
        }
        let doc_hash = document_hash(&doc);

        let env_spec = EnvSpec {
            doc,
            end_effector_ids: gym_spec.end_effector_ids.clone(),
            dt: gym_spec.dt,
            substeps: gym_spec.substeps,
            config: gym_spec.config.clone(),
            gains: gym_spec
                .gains
                .iter()
                .map(|(k, [kp, kd])| (k.clone(), (*kp, *kd)))
                .collect(),
            max_steps: gym_spec.max_steps,
        };

        // Build once up front so a malformed env fails here — with a message
        // the user sees — rather than on the worker thread after the UI has
        // already reported that training started.
        let Some(probe) = ctx("build training env", || env_spec.build()) else {
            return ptr::null_mut();
        };
        let slots = actuated_slots(&probe);
        let act_dim = probe.action_dim();
        let obs_dim = vcad_sim::rl::feature_dim(&probe, &slots);
        drop(probe);

        let action = ActionSpec {
            default_pose_deg: vec![0.0; act_dim],
            action_scale_deg: tspec.action_scale_deg,
        };
        let is_mlp = match tspec.policy.as_str() {
            "mlp" => true,
            "linear" => false,
            other => {
                set_error(format!(
                    "unknown policy architecture {other:?} (mlp|linear)"
                ));
                return ptr::null_mut();
            }
        };

        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("vcad-train".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    if is_mlp {
                        run_training(
                            &worker_shared,
                            env_spec,
                            MlpPolicy::new(obs_dim, tspec.hidden, act_dim, action, tspec.init_seed),
                            &tspec,
                            reward,
                            gym_spec,
                            doc_hash,
                        )
                    } else {
                        run_training(
                            &worker_shared,
                            env_spec,
                            LinearPolicy::zeros(obs_dim, act_dim, action),
                            &tspec,
                            reward,
                            gym_spec,
                            doc_hash,
                        )
                    }
                }));
                let mut p = worker_shared.progress.lock().unwrap();
                p.running = 0;
                p.finished = 1;
                p.cancelled = worker_shared.cancel.load(Ordering::Relaxed) as u8;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        p.failed = 1;
                        *worker_shared.error.lock().unwrap() = Some(e);
                    }
                    Err(_) => {
                        p.failed = 1;
                        *worker_shared.error.lock().unwrap() =
                            Some("training panicked".to_string());
                    }
                }
            });

        match handle {
            Ok(h) => Box::into_raw(Box::new(VcadTrainer {
                shared,
                handle: Some(h),
            })),
            Err(e) => {
                set_error(format!("could not spawn training thread: {e}"));
                ptr::null_mut()
            }
        }
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_train_start: panic");
        ptr::null_mut()
    })
}

/// Copy the current progress snapshot into `out`. Returns 1 on success.
#[no_mangle]
pub extern "C" fn vcad_train_poll(trainer: *const VcadTrainer, out: *mut VcadTrainProgress) -> u8 {
    if trainer.is_null() || out.is_null() {
        return 0;
    }
    let t: &VcadTrainer = unsafe { &*trainer };
    let p = *t.shared.progress.lock().unwrap();
    unsafe { *out = p };
    1
}

/// Request cancellation. The worker stops at the next iteration boundary;
/// poll `finished` to know when it has actually exited.
#[no_mangle]
pub extern "C" fn vcad_train_stop(trainer: *mut VcadTrainer) {
    if trainer.is_null() {
        return;
    }
    let t: &mut VcadTrainer = unsafe { &mut *trainer };
    t.shared.cancel.store(true, Ordering::Relaxed);
}

/// Copy the best-so-far policy bundle (`.vcadpolicy` JSON) into `out`.
///
/// Returns the number of bytes written, or 0 when no policy has been scored
/// yet or `out_cap` is too small. Call with `out = null, out_cap = 0` to
/// query the required size — it is reported through
/// [`crate::vcad_last_error`] and returned as 0, so size first, then copy.
#[no_mangle]
pub extern "C" fn vcad_train_best_policy_json(
    trainer: *const VcadTrainer,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    clear_error();
    if trainer.is_null() {
        return 0;
    }
    let t: &VcadTrainer = unsafe { &*trainer };
    let guard = t.shared.best_bundle.lock().unwrap();
    let Some(text) = guard.as_ref() else {
        set_error("no policy has been scored yet");
        return 0;
    };
    if out.is_null() || out_cap < text.len() {
        set_error(format!("policy bundle needs {} bytes", text.len()));
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) };
    text.len()
}

/// Borrow the training run's error message, or null. Valid until the trainer
/// is freed.
#[no_mangle]
pub extern "C" fn vcad_train_error(trainer: *const VcadTrainer, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    if trainer.is_null() {
        return ptr::null();
    }
    let t: &VcadTrainer = unsafe { &*trainer };
    let guard = t.shared.error.lock().unwrap();
    match guard.as_ref() {
        Some(msg) => {
            if !out_len.is_null() {
                unsafe { *out_len = msg.len() };
            }
            // The message is written once by the worker before it sets
            // `finished`, and never replaced, so the pointer stays valid for
            // the trainer's lifetime.
            msg.as_ptr()
        }
        None => ptr::null(),
    }
}

/// Cancel and join the worker, then free the trainer. Null is a no-op.
///
/// This blocks until the worker exits — deliberately. Detaching would let the
/// worker keep stepping physics against freed state.
#[no_mangle]
pub extern "C" fn vcad_train_free(trainer: *mut VcadTrainer) {
    if trainer.is_null() {
        return;
    }
    let mut t = unsafe { Box::from_raw(trainer) };
    t.shared.cancel.store(true, Ordering::Relaxed);
    if let Some(h) = t.handle.take() {
        let _ = h.join();
    }
}

/// Load a `.vcadpolicy` bundle's policy into an inference handle, checking it
/// against `gym` and against the document it was trained on.
///
/// `document_json` may be null to skip the model-drift check. When supplied
/// and the hash differs, this still returns the policy (the app decides
/// whether to run a stale policy — that is a receipt-level judgement, not a
/// load error) but records the drift in [`crate::vcad_last_error`], so a
/// caller that surfaces it gets the Stale state for free.
#[no_mangle]
pub extern "C" fn vcad_policy_load_bundle(
    bundle_json: *const u8,
    bundle_json_len: usize,
    document_json: *const u8,
    document_json_len: usize,
) -> *mut VcadPolicy {
    clear_error();
    if bundle_json.is_null() {
        set_error("vcad_policy_load_bundle: null bundle");
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(bundle_json, bundle_json_len) };
        let Some(text) = ctx("bundle is not UTF-8", || std::str::from_utf8(bytes)) else {
            return ptr::null_mut();
        };
        let Some(bundle) = ctx("parse policy bundle", || {
            serde_json::from_str::<PolicyBundle>(text)
        }) else {
            return ptr::null_mut();
        };

        let drift = if document_json.is_null() {
            None
        } else {
            let db = unsafe { std::slice::from_raw_parts(document_json, document_json_len) };
            std::str::from_utf8(db)
                .ok()
                .and_then(|t| Document::from_json(t).ok())
                .map(|d| document_hash(&d))
                .filter(|h| *h != bundle.document_hash)
        };

        let policy_text = match serde_json::to_string(&bundle.policy) {
            Ok(t) => t,
            Err(e) => {
                set_error(format!("bundle policy is not serializable: {e}"));
                return ptr::null_mut();
            }
        };
        let handle = crate::gym::vcad_policy_load(policy_text.as_ptr(), policy_text.len());
        if !handle.is_null() {
            if let Some(now) = drift {
                set_error(format!(
                    "policy is STALE: trained against document {} but this document \
                     hashes to {}. Its held-out score of {:.2} does not describe this model.",
                    bundle.document_hash, now, bundle.held_out_reward
                ));
            }
        }
        handle
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_policy_load_bundle: panic");
        ptr::null_mut()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_physics::{DomainRandomization, Observation, Range, StepInfo};

    fn step_result(height: f64, tilt_deg: f64, vel: [f64; 6]) -> StepResult {
        let mut obs = Observation::zeros(0, 0);
        obs.base_velocity = Some(vel);
        StepResult {
            observation: obs,
            reward: 0.0,
            done: false,
            info: StepInfo {
                step: 1,
                truncated: false,
                terminated: false,
                termination_reason: None,
                base_height_m: Some(height),
                base_tilt_deg: Some(tilt_deg),
                joint_limit_violations: Vec::new(),
                action_latency_substeps: 0,
            },
        }
    }

    #[test]
    fn perfect_standing_earns_exactly_the_alive_bonus() {
        let spec = RewardSpec::default();
        let r = spec.eval(
            &step_result(spec.nominal_height_m, 0.0, [0.0; 6]),
            &[0.0, 0.0],
        );
        assert!((r - 1.0).abs() < 1e-12, "got {r}");
    }

    #[test]
    fn reward_matches_the_k1_reference_formula() {
        // The weights and the arithmetic are transcribed from the trainer the
        // bundled policies came from; this pins them against a drift that
        // would silently redefine the task.
        let spec = RewardSpec::default();
        let (h, tilt_deg) = (0.50, 10.0);
        let vel = [0.1, -0.2, 0.05, 0.3, 0.0, -0.1];
        let action = [4.0, -8.0, 12.0];
        let got = spec.eval(&step_result(h, tilt_deg, vel), &action);

        let tilt = tilt_deg.to_radians();
        let drift = 0.1f64 * 0.1 + 0.2 * 0.2 + 0.05 * 0.05;
        let spin = 0.3f64 * 0.3 + 0.0 + 0.1 * 0.1;
        let effort =
            ((4.0f64 / 30.0).powi(2) + (-8.0f64 / 30.0).powi(2) + (12.0f64 / 30.0).powi(2)) / 3.0;
        let want = 1.0
            - 8.0 * (h - 0.5498f64).powi(2)
            - 1.5 * tilt * tilt
            - 0.3 * drift
            - 0.05 * spin
            - 0.1 * effort;
        assert!((got - want).abs() < 1e-12, "got {got}, want {want}");
    }

    #[test]
    fn falling_is_penalized_relative_to_standing() {
        let spec = RewardSpec::default();
        let standing = spec.eval(&step_result(spec.nominal_height_m, 0.0, [0.0; 6]), &[0.0]);
        let falling = spec.eval(
            &step_result(0.30, 40.0, [0.5, 0.0, -1.2, 0.0, 0.0, 0.0]),
            &[0.0],
        );
        assert!(
            falling < standing,
            "falling {falling} !< standing {standing}"
        );
    }

    #[test]
    fn an_empty_action_contributes_no_effort_penalty() {
        // Guards the `action.len()` divisor against a division by zero on a
        // document with no actuated joints.
        let spec = RewardSpec::default();
        let r = spec.eval(&step_result(spec.nominal_height_m, 0.0, [0.0; 6]), &[]);
        assert!(r.is_finite() && (r - 1.0).abs() < 1e-12);
    }

    #[test]
    fn curriculum_level_zero_disables_every_randomization_channel() {
        let base = vcad_kernel_physics::EnvConfig {
            randomization: Some(DomainRandomization {
                mass_scale: Some(Range { min: 0.9, max: 1.1 }),
                pd_gain_scale: Some(Range { min: 0.8, max: 1.2 }),
                joint_pos_perturb: Some(2.0),
                joint_vel_perturb: Some(5.0),
                action_latency_steps: Some([2, 8]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let off = config_at(&base, 0.0);
        let r = off.randomization.unwrap();
        let m = r.mass_scale.unwrap();
        assert_eq!((m.min, m.max), (1.0, 1.0));
        let g = r.pd_gain_scale.unwrap();
        assert_eq!((g.min, g.max), (1.0, 1.0));
        assert_eq!(r.joint_pos_perturb, Some(0.0));
        assert_eq!(r.joint_vel_perturb, Some(0.0));
        assert_eq!(r.action_latency_steps, Some([0, 0]));
    }

    #[test]
    fn curriculum_level_one_is_the_identity() {
        let base = vcad_kernel_physics::EnvConfig {
            randomization: Some(DomainRandomization {
                mass_scale: Some(Range { min: 0.9, max: 1.1 }),
                joint_pos_perturb: Some(2.0),
                action_latency_steps: Some([2, 8]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let full = config_at(&base, 1.0);
        let r = full.randomization.unwrap();
        let m = r.mass_scale.unwrap();
        assert!((m.min - 0.9).abs() < 1e-12 && (m.max - 1.1).abs() < 1e-12);
        assert_eq!(r.joint_pos_perturb, Some(2.0));
        assert_eq!(r.action_latency_steps, Some([2, 8]));
    }

    #[test]
    fn curriculum_half_level_narrows_multiplicative_channels_around_one() {
        let base = vcad_kernel_physics::EnvConfig {
            randomization: Some(DomainRandomization {
                mass_scale: Some(Range { min: 0.8, max: 1.2 }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let half = config_at(&base, 0.5);
        let m = half.randomization.unwrap().mass_scale.unwrap();
        assert!((m.min - 0.9).abs() < 1e-12, "{m:?}");
        assert!((m.max - 1.1).abs() < 1e-12, "{m:?}");
    }

    #[test]
    fn document_hash_is_independent_of_hashmap_iteration_order() {
        // The bug this pins: `Document` holds `HashMap` fields, and each map
        // instance gets its own `RandomState` ordering. Hashing a plain
        // `to_string` made a document hash differently from *itself* whenever
        // the two parses happened to order their maps differently, so every
        // loaded policy reported Stale at random.
        //
        // Building the same logical document by inserting keys in opposite
        // orders reproduces exactly that: the maps compare equal, iterate
        // differently, and must still hash the same.
        let names = ["femur", "tibia", "torso", "pelvis", "ankle", "shoulder"];
        let mut forward = Document::default();
        for (i, n) in names.iter().enumerate() {
            forward
                .parameters
                .insert(n.to_string(), vcad_ir::Parameter::literal(i as f64));
        }
        let mut backward = Document::default();
        for (i, n) in names.iter().enumerate().rev() {
            backward
                .parameters
                .insert(n.to_string(), vcad_ir::Parameter::literal(i as f64));
        }
        assert_eq!(
            document_hash(&forward),
            document_hash(&backward),
            "insertion order must not change the hash"
        );
    }

    #[test]
    fn canonicalize_sorts_object_keys_at_every_depth() {
        let v = serde_json::json!({ "b": 1, "a": { "z": [3, 2], "y": null } });
        let mut s = String::new();
        canonicalize(&v, &mut s);
        assert_eq!(s, r#"{"a":{"y":null,"z":[3,2]},"b":1}"#);
    }

    #[test]
    fn document_hash_is_stable_and_sensitive() {
        let a = Document::default();
        let mut b = Document::default();
        assert_eq!(document_hash(&a), document_hash(&a));
        b.parameters
            .insert("femur".to_string(), vcad_ir::Parameter::literal(120.0));
        assert_ne!(
            document_hash(&a),
            document_hash(&b),
            "an edited document must hash differently or drift is undetectable"
        );
    }

    #[test]
    fn train_spec_defaults_are_the_measured_k1_configuration() {
        let s: TrainSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(s.policy, "mlp");
        assert_eq!(s.hidden, 64);
        assert!((s.ars.step_size - 0.005).abs() < 1e-12);
        assert!((s.curriculum_warmup - 0.4).abs() < 1e-12);
        assert_eq!(s.held_out_every, 5);
        assert_eq!(s.held_out_seeds, 10);
    }

    #[test]
    fn specs_reject_unknown_fields() {
        assert!(serde_json::from_str::<TrainSpec>(r#"{"hiden": 64}"#).is_err());
        assert!(serde_json::from_str::<RewardSpec>(r#"{"tiltt": 1.0}"#).is_err());
    }

    #[test]
    fn null_trainer_handles_are_inert() {
        let mut p = VcadTrainProgress::default();
        assert_eq!(vcad_train_poll(ptr::null(), &mut p), 0);
        vcad_train_stop(ptr::null_mut());
        vcad_train_free(ptr::null_mut());
        assert_eq!(
            vcad_train_best_policy_json(ptr::null(), ptr::null_mut(), 0),
            0
        );
        let mut len = 0usize;
        assert!(vcad_train_error(ptr::null(), &mut len).is_null());
    }
}
