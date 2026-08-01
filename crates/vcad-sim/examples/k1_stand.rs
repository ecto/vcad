//! Train a standing-balance policy for the Booster K1 humanoid.
//!
//! ```bash
//! # import first:  vcad import-urdf K1_22dof_floating.urdf k1.vcad
//! cargo run --release -p vcad-sim --example k1_stand -- k1.vcad [iterations]
//! ```
//!
//! Writes the trained policy to `k1_stand_policy.json` next to the document.
//!
//! # Diagnostic modes
//!
//! Reach for these *before* tuning the trainer. Each answers a question that
//! decides whether trainer tuning is even the right move:
//!
//! - `K1_TRACE=1` — dump the baseline's fall step by step (height, tilt, foot
//!   heights). Confirms the robot is failing at the task and not at the
//!   physics.
//! - `K1_HANDPD=1` — sweep a hand-written two-gain ankle balance controller,
//!   expressed as a linear policy so it runs the same path a trained one
//!   does. If the hand controller can't stand, the policy class or the task
//!   is the problem and no ARS tuning will fix it.
//! - `<iterations> = 0` — re-score the policy already on disk.
//!
//! Every knob below is an environment variable so a sweep is a shell line
//! rather than a recompile: `K1_ROLLOUTS`, `K1_DIRS`, `K1_TOPK`, `K1_ALPHA`,
//! `K1_NU`, `K1_SEED`, `K1_POLICY` (`linear`|`mlp`), `K1_HIDDEN`,
//! `K1_CURRICULUM_WARMUP`, `K1_ACTION_SCALE`, `K1_SPAWN_Z_MM`, the
//! randomization channels (`K1_POS_PERTURB`, `K1_VEL_PERTURB`, `K1_MASS_PCT`,
//! `K1_GAIN_PCT`, `K1_LATENCY_MAX`), and the PD gains (`K1_KP_ANKLE` etc.).
//!
//! # Measured, on the 22-DOF floating-base K1
//!
//! All figures are the mean over the ten fresh evaluation seeds this example
//! prints last, at 1 kHz physics / 50 Hz control and 400-step episodes. The
//! training-time eval return is *not* comparable to them and overstates by
//! roughly 5× — see [`eval_10`].
//!
//! - Hold-rest-pose baseline: **9.55 over 18.8 steps** with full
//!   randomization, **18.85 over 25.0 steps** with none.
//! - Ablating randomization one channel at a time, the *entire* gap is the
//!   initial joint perturbation: mass alone 18.62, PD gain alone 18.96,
//!   actuator latency alone 17.89 — all indistinguishable from no
//!   randomization at all — while the 2.0°/5.0°-per-second joint kick alone
//!   gives 10.18. The sim2real channels are nearly free here; the shove is
//!   the task.
//! - The K1 at its rest pose is an unstable equilibrium that diverges
//!   exponentially, e-folding about every 90 ms (~4.5 control steps), so a
//!   policy has roughly ten steps of authority before a lean is
//!   unrecoverable.
//! - A hand-written ankle-only strategy inside the linear policy class
//!   reaches 44 steps deterministic — 1.8× the baseline, still nowhere near
//!   the 400-step episode. Stiffening the ankles does not help and makes it
//!   worse (ankle `kp` 100/200/400 → 18.0/16.0/18.0 baseline steps), because
//!   the explicit PD loop is already near its ω·dt stability limit. Ankle
//!   torque authority is therefore *not* the binding constraint.
//!
//! # What was actually holding training back
//!
//! Not the randomization — **the step size**, and then policy capacity. ARS's
//! update is scale-free, so it takes the same-size step when the policy is
//! optimal as when it is useless, and at the old `α = 0.03` it walked out of
//! every solution it found. With randomization off it hit a perfect 400/400
//! on iteration 20 and was at −16 by iteration 29; the update norm sat at
//! ~0.4 the entire time, including while it was solving the task. Three
//! seeds, same shape. See [`vcad_sim::rl::ArsConfig::step_size`].
//!
//! Dropping to `α = 0.005` (now the default here):
//!
//! | task            | α = 0.03                       | α = 0.005                       |
//! |-----------------|--------------------------------|---------------------------------|
//! | deterministic   | 400/400 at iter 20, −16 by 29  | 400/400 at iter 42, still there at 59 |
//! | full randomized | peak 35 at iter 3, ~9 by 20    | 222 at iter 75, 5/10 full episodes |
//!
//! Stacking the curriculum and then the MLP on top of the smaller step
//! solves the task. All figures are ten fresh seeds, full randomization:
//!
//! | configuration                            | held-out | full episodes |
//! |------------------------------------------|----------|---------------|
//! | hold-rest-pose baseline                  | 9.55     | 0/10          |
//! | linear, α = 0.03 (previous defaults)     | 10.84    | 0/10          |
//! | linear, α = 0.005                        | 221.92   | 5/10          |
//! | linear, α = 0.005 + curriculum 0.4       | 280.33   | 7/10          |
//! | **MLP-64, α = 0.005 + curriculum 0.4**   | **386.92** | **10/10**   |
//!
//! The MLP run reaches 400/400 on every held-out seed by iteration 119, with
//! the worst seed scoring 356.29 and terminating for no reason other than
//! running out of episode. It is also *faster per iteration of progress*, not
//! merely better at the end — at matched iterations against the linear policy
//! under an otherwise identical config (same seeds, same curriculum):
//!
//! | iteration | linear | MLP-64 |
//! |-----------|--------|--------|
//! | 10        | 25.99  | 47.11  |
//! | 20        | 84.53  | 210.20 |
//! | 70        | ~180   | 376.30 |
//!
//! So the capacity argument was right: balance switches contact mode, and one
//! gain matrix has to serve every mode at once.
//!
//! One caveat if you compare architectures yourself. The ARS step norm grows
//! with √(parameter count), so at matched `α` the 4950-parameter MLP takes a
//! ~3× larger relative step than the 1188-parameter linear policy — the
//! comparison above is confounded by exactly the thing that turned out to
//! matter most. Scaling `α` down by √(1188/4950) to equalize the step norm
//! was measured too, and is *worse* (307.60): the MLP genuinely wants the
//! larger step, so the confound was real but favoured the other direction.
//!
//! The linear-plus-curriculum run contains the sharpest warning about the
//! trainer's own eval column anywhere here: over iterations 175–199, while
//! held-out sat at 277–280 with 7/10 full episodes, `train-eval` read −1.63,
//! −0.56, −1.02, 0.63, −3.40, −0.15. Three fixed evaluation seeds called the
//! best policy of the session worthless. Selecting on that column picked an
//! iterate worth 109.13 instead of 280.33.
//!
//! So: because ARS wanders back out of good regions even at the smaller step,
//! and because its own eval cannot be trusted to notice, `K1_CURVE=n` scores
//! every n-th iterate on the ten held-out seeds and the run keeps the best of
//! those. Use it for any run you intend to keep.

use std::collections::HashMap;

use vcad_ir::Document;
use vcad_kernel_physics::{DomainRandomization, EnvConfig, Range, TerminationConfig};
use vcad_sim::rl::{
    actuated_slots, ActionSpec, ArsConfig, EnvSpec, LinearPolicy, MlpPolicy, Policy, TrainOutcome,
};

/// Booster K1 trunk height (m) once it has settled on the ground with the
/// legs at their URDF rest pose — measured, not assumed.
const NOMINAL_H: f64 = 0.5498;

/// booster_gym-style PD gains: stiff hips/knees, soft ankles, soft upper body.
///
/// These need the 1 kHz physics tick below. They assume Isaac's implicit
/// actuator integration; at the more natural-looking dt = 5 ms this
/// semi-implicit loop puts the light joints past the explicit stability limit
/// (ω·dt ≈ 0.7) and the robot shakes itself apart inside 0.2 s. The kernel's
/// own inertia-scaled defaults (ω = 20 rad/s) are stable at any tick but far
/// too soft to hold 20 kg up: the K1 sinks through its knees instead of
/// balancing, and the policy learns to fight its own springs.
fn gains_for(joint: &str) -> (f64, f64) {
    if joint.contains("Hip") || joint.contains("Knee") {
        (env_f64("K1_KP_LEG", 200.0), env_f64("K1_KD_LEG", 5.0))
    } else if joint.contains("Ankle") {
        (env_f64("K1_KP_ANKLE", 50.0), env_f64("K1_KD_ANKLE", 1.0))
    } else {
        (env_f64("K1_KP_ARM", 40.0), env_f64("K1_KD_ARM", 1.0))
    }
}

/// Read an `f64` knob from the environment, falling back to `default`.
///
/// Every hyperparameter that a sweep wants to move lives behind one of these:
/// a run is then a shell line, not a recompile, and the committed defaults
/// stay the ones that were actually measured.
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Read a `usize` knob from the environment.
fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Ten fresh evaluation seeds, disjoint from anything training touches.
///
/// This is the *only* number any change may be judged by. ARS's own
/// training-time eval return selects on its seed set, and on this env that
/// selection reliably picks a lucky randomization draw rather than a better
/// policy (measured: an iterate scoring 116.34 on its own eval seeds averaged
/// 7.93 here).
fn eval_10<P, R>(
    spec: &EnvSpec,
    policy: &P,
    slots: &[usize],
    reward: &R,
) -> Result<(f64, f64, usize, vcad_sim::rl::RolloutStats), vcad_kernel_physics::PhysicsError>
where
    P: Policy,
    R: Fn(&vcad_kernel_physics::StepResult, &[f64]) -> f64 + Sync,
{
    let mut env = spec.build()?;
    let evals: Vec<_> = (1..=10u64)
        .map(|s| vcad_sim::rl::rollout(&mut env, policy, slots, NOMINAL_H, s, reward, None))
        .collect();
    let mean_r = evals.iter().map(|e| e.reward).sum::<f64>() / evals.len() as f64;
    let mean_steps = evals.iter().map(|e| e.steps as f64).sum::<f64>() / evals.len() as f64;
    let full = evals.iter().filter(|e| e.steps == spec.max_steps).count();
    let worst = evals
        .iter()
        .min_by(|a, b| a.reward.total_cmp(&b.reward))
        .unwrap()
        .clone();
    Ok((mean_r, mean_steps, full, worst))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: k1_stand <doc.vcad> [iterations]");
    let iterations: usize = args.next().map_or(150, |s| s.parse().unwrap());

    let mut doc: Document = serde_json::from_str(&std::fs::read_to_string(&path)?)?;

    // Raise the floating base to standing height.
    //
    // `import-urdf` anchors the Free base joint at the world origin, so a
    // freshly imported K1 spawns with its trunk at z = 0 — a metre below the
    // 0.42 m termination floor. Every episode then terminates on step 1 and
    // the whole task silently degenerates. This was a manual doc-prep step
    // before; leaving it manual is how a run gets billed to a document that
    // was never standing up.
    let spawn_z_mm = env_f64("K1_SPAWN_Z_MM", NOMINAL_H * 1000.0);
    let raised = doc
        .joints
        .iter_mut()
        .flat_map(|js| js.iter_mut())
        .find(|j| matches!(j.kind, vcad_ir::JointKind::Free))
        .map(|j| {
            j.parent_anchor.z = spawn_z_mm;
        })
        .is_some();
    if !raised {
        return Err("document has no Free base joint — this example trains a \
                    floating-base standing task and needs one (import \
                    K1_22dof_floating.urdf, not the fixed-base K1_22dof.urdf)"
            .into());
    }

    // Sim2real-shaped randomization, in the spirit of Booster's own K1 config
    // (2–8 substeps of actuator latency). Without it every episode is
    // bit-identical and "the policy survives 400 steps" says nothing about
    // whether it survives a robot.
    // Each randomization channel is separately switchable, so "the randomized
    // task is unsolved" can be decomposed into *which* channel costs what
    // rather than treated as one lump. A channel set to 0 is disabled.
    let pos_perturb = env_f64("K1_POS_PERTURB", 2.0);
    let vel_perturb = env_f64("K1_VEL_PERTURB", 5.0);
    let mass_pct = env_f64("K1_MASS_PCT", 0.1);
    let gain_pct = env_f64("K1_GAIN_PCT", 0.2);
    let latency_max = env_usize("K1_LATENCY_MAX", 8);
    let opt = |v: f64| (v > 0.0).then_some(v);

    // Randomization at a curriculum `level` in [0, 1]: 0 is the deterministic
    // env, 1 is the full sim2real task. Every channel scales together, so a
    // schedule is one scalar.
    let config_at = move |level: f64| EnvConfig {
        randomization: Some(DomainRandomization {
            mass_scale: opt(mass_pct * level).map(|p| Range {
                min: 1.0 - p,
                max: 1.0 + p,
            }),
            pd_gain_scale: opt(gain_pct * level).map(|p| Range {
                min: 1.0 - p,
                max: 1.0 + p,
            }),
            action_latency_steps: opt(latency_max as f64 * level)
                .map(|m| [2.min(m as u32), m as u32]),
            joint_pos_perturb: opt(pos_perturb * level),
            joint_vel_perturb: opt(vel_perturb * level),
            ..DomainRandomization::default()
        }),
        termination: Some(TerminationConfig {
            base_height_below: Some(0.42),
            base_tilt_above_deg: Some(35.0),
            terminate_on_joint_limit: false,
        }),
        base_instance_id: Some("Trunk_inst".to_string()),
        ..EnvConfig::default()
    };
    let config = config_at(1.0);

    let mut spec = EnvSpec {
        doc,
        end_effector_ids: vec!["left_foot_link_inst".into(), "right_foot_link_inst".into()],
        // 1 kHz physics, 50 Hz control — the ratio the stiff gains need.
        dt: 1.0 / 1000.0,
        substeps: 20,
        config,
        gains: HashMap::new(),
        max_steps: 400,
    };
    let probe = spec.build()?;
    spec.gains = probe
        .actuated_joint_ids()
        .iter()
        .map(|id| (id.clone(), gains_for(id)))
        .collect();
    let slots = actuated_slots(&probe);
    let act_dim = probe.action_dim();
    let obs_dim = 10 + 2 * slots.len();
    drop(probe);

    // Reward: stay alive, hold the nominal height and an upright trunk, don't
    // drift, and don't thrash the actuators.
    let reward = move |r: &vcad_kernel_physics::StepResult, action: &[f64]| -> f64 {
        let h = r.info.base_height_m.unwrap_or(0.0);
        let tilt = r.info.base_tilt_deg.unwrap_or(90.0).to_radians();
        let v = r.observation.base_velocity.unwrap_or([0.0; 6]);
        let drift = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let spin = v[3] * v[3] + v[4] * v[4] + v[5] * v[5];
        let effort = action.iter().map(|a| (a / 30.0).powi(2)).sum::<f64>() / action.len() as f64;
        1.0 - 8.0 * (h - NOMINAL_H).powi(2)
            - 1.5 * tilt * tilt
            - 0.3 * drift
            - 0.05 * spin
            - 0.1 * effort
    };

    let cfg = ArsConfig {
        n_directions: env_usize("K1_DIRS", 12),
        top_k: env_usize("K1_TOPK", 6),
        step_size: env_f64("K1_ALPHA", 0.005),
        noise_std: env_f64("K1_NU", 0.05),
        iterations,
        // Draws averaged per evaluation. Three is not enough on this env —
        // see the sweep in the module docs.
        rollouts_per_eval: env_usize("K1_ROLLOUTS", 3),
        seed: env_usize("K1_SEED", 7) as u64,
    };

    println!(
        "K1: {act_dim} actuated joints, {obs_dim} policy features, {} steps/episode",
        spec.max_steps
    );
    println!(
        "ARS: {} dirs, top {}, α={}, ν={}",
        cfg.n_directions, cfg.top_k, cfg.step_size, cfg.noise_std
    );
    println!(
        "      {} rollouts averaged per evaluation",
        cfg.rollouts_per_eval
    );

    // The hold-rest-pose baseline is exactly a zero policy (zero weights ⇒
    // the action is the default pose), so it goes through the identical
    // rollout and evaluation path as anything learned. Measuring it on one
    // seed while measuring the policy on ten is how a wash gets reported as
    // an improvement.
    // Fail closed on a document that has no floating base. Without one,
    // `base_pose` is None for every step, so the height term reads exactly
    // nominal, the tilt term reads a constant, and *no termination condition
    // can ever fire* — the run reports a confident 400/400 survival and a
    // stable reward while measuring nothing at all. This bit me on a
    // same-named fixed-base document; it should not be possible to bill an
    // afternoon of training to that mistake twice.
    {
        let mut env = spec.build()?;
        let obs = env.reset_with_seed(1);
        if obs.base_pose.is_none() {
            return Err(format!(
                "document has no floating base reachable from base_instance_id {:?} — \
                 every termination check is disabled and the returns below would be \
                 meaningless. Import with `vcad import-urdf K1_22dof_floating.urdf`.",
                spec.config.base_instance_id
            )
            .into());
        }
    }

    let baseline = LinearPolicy::zeros(
        obs_dim,
        act_dim,
        ActionSpec {
            default_pose_deg: vec![0.0; act_dim],
            action_scale_deg: env_f64("K1_ACTION_SCALE", 8.0),
        },
    );
    let (b_r, b_steps, b_full, _) = eval_10(&spec, &baseline, &slots, &reward)?;
    println!(
        "hold-rest-pose baseline (10 seeds): mean {b_r:>7.2} over {b_steps:>5.1} steps, \
         full episode on {b_full}/10"
    );

    // Output path is overridable, because a sweep whose arms all write
    // "k1_stand_policy.json" silently leaves you holding whichever arm
    // finished last rather than the one you wanted to keep.
    let saved = match std::env::var("K1_OUT") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => std::path::Path::new(&path).with_file_name("k1_stand_policy.json"),
    };

    // `K1_REPLAY=<seed>` rolls the saved policy out on one seed and writes a
    // `.vcad` per frame to `K1_REPLAY_DIR`, ready for `vcad-render` + ffmpeg.
    //
    // Getting the *base* pose into a document takes a small surgery. Document
    // FK renders a `Free` joint at its zero pose on purpose — a 6-DOF pose
    // does not fit in a joint's scalar `state`, and the live pose belongs to
    // the physics, not the document. But FK seeds *root* instances from their
    // own `transform`. So dropping the world joint promotes the trunk to a
    // root and lets its transform carry the full floating-base pose, which
    // the rest of the tree then hangs off correctly.
    if let Ok(seed) = std::env::var("K1_REPLAY") {
        let seed: u64 = seed.parse().unwrap_or(1);
        let dir = std::env::var("K1_REPLAY_DIR").unwrap_or_else(|_| "k1_replay".into());
        std::fs::create_dir_all(&dir)?;
        let every = env_usize("K1_REPLAY_EVERY", 2).max(1);

        let blob: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&saved)?)?;
        let pol = &blob["policy"];
        // The saved policy is whichever architecture trained it; `hidden`
        // is the discriminator.
        let mlp: Option<MlpPolicy> = pol
            .get("hidden")
            .is_some()
            .then(|| serde_json::from_value(pol.clone()))
            .transpose()?;
        let lin: Option<LinearPolicy> = match &mlp {
            Some(_) => None,
            None => Some(serde_json::from_value(pol.clone())?),
        };
        println!(
            "replay: {} policy, seed {seed} → {dir}/",
            if mlp.is_some() { "mlp" } else { "linear" }
        );

        let mut env = spec.build()?;
        let mut obs = env.reset_with_seed(seed);
        let joint_ids = env.actuated_joint_ids().to_vec();
        let mut frame = 0usize;
        let mut total = 0.0;
        for i in 0..spec.max_steps {
            if (i as usize).is_multiple_of(every) {
                write_frame(&spec.doc, &obs, &slots, &joint_ids, &dir, frame)?;
                frame += 1;
            }
            let f = vcad_sim::rl::features(&obs, &slots, NOMINAL_H);
            let targets = match (&mlp, &lin) {
                (Some(m), _) => Policy::act(m, &f),
                (_, Some(l)) => Policy::act(l, &f),
                _ => unreachable!(),
            };
            let r = env.step_full(vcad_kernel_physics::Action::PositionTarget(targets.clone()));
            total += reward(&r, &targets);
            obs = r.observation;
            if r.done {
                println!("terminated at step {i}: {:?}", r.info.termination_reason);
                break;
            }
        }
        println!("replay: {frame} frames, return {total:.2}");
        return Ok(());
    }

    // `K1_HANDPD=1` sweeps a hand-written ankle balance controller,
    // *expressed as a linear policy* so it runs through the exact path a
    // trained policy does.
    //
    // This is the load-bearing experiment behind everything else here. If a
    // two-gain hand controller inside the linear policy class can hold the
    // K1 up on the randomized task, then the policy class is adequate and a
    // failure to learn is the trainer's fault — worth tuning. If it cannot,
    // no amount of ARS tuning will help and the task, the gains, or the
    // observation are what need to change. Answering this costs a minute;
    // guessing wrong costs a day of training runs.
    //
    // Feature layout (see `rl::features`): 0..3 projected gravity, 3..6 base
    // linear vel, 6..9 base angular vel, 9 height error, 10.. joint angles.
    // Ankle-pitch actions are driven from gravity-x (lean) and ω_y (lean
    // rate) — the textbook ankle strategy.
    if env_usize("K1_HANDPD", 0) > 0 {
        const L_ANKLE_PITCH: usize = 14;
        const R_ANKLE_PITCH: usize = 20;
        const GX: usize = 0;
        const WY: usize = 7;
        println!("hand ankle-PD sweep (kp on gravity-x, kd on ω_y), 10 seeds each:");
        println!("  sign    kp     kd      mean    steps  full");
        let mut best = (f64::NEG_INFINITY, 0.0, 0.0, 0.0);
        for sign in [1.0f64, -1.0] {
            for kp in [0.0, 2.0, 5.0, 10.0, 20.0, 40.0] {
                for kd in [0.0, 0.2, 0.5, 1.0, 2.0] {
                    let mut p = baseline.clone();
                    for a in [L_ANKLE_PITCH, R_ANKLE_PITCH] {
                        p.weights[a * obs_dim + GX] = sign * kp;
                        p.weights[a * obs_dim + WY] = sign * kd;
                    }
                    let (r, st, full, _) = eval_10(&spec, &p, &slots, &reward)?;
                    println!(
                        "  {sign:>4}  {kp:>5.1}  {kd:>5.2}  {r:>8.2}  {st:>6.1}  {full:>3}/10"
                    );
                    if r > best.0 {
                        best = (r, sign, kp, kd);
                    }
                }
            }
        }
        println!(
            "best: mean {:.2} at sign {} kp {} kd {} (baseline {b_r:.2})",
            best.0, best.1, best.2, best.3
        );
        return Ok(());
    }

    // `K1_TRACE=1` dumps the baseline's fall step by step. Before tuning a
    // trainer it is worth confirming the robot is failing at the task rather
    // than at the physics — a foot that never registers contact produces a
    // task no policy can solve and a training curve that looks merely hard.
    if env_usize("K1_TRACE", 0) > 0 {
        let mut env = spec.build()?;
        let mut obs = env.reset_with_seed(1);
        println!("step  height   tilt°   vz       Lfoot_z  Rfoot_z");
        for i in 0..spec.max_steps {
            let f = vcad_sim::rl::features(&obs, &slots, NOMINAL_H);
            let r = env.step_full(vcad_kernel_physics::Action::PositionTarget(
                baseline.act(&f),
            ));
            let h = r.info.base_height_m.unwrap_or(f64::NAN);
            let tilt = r.info.base_tilt_deg.unwrap_or(f64::NAN);
            let vz = r.observation.base_velocity.unwrap_or([0.0; 6])[2];
            let foot = |k: usize| {
                r.observation
                    .end_effector_poses
                    .get(k)
                    .map_or(f64::NAN, |p| p[2])
            };
            if i % 2 == 0 || r.done {
                println!(
                    "{i:>4}  {h:>6.3}  {tilt:>6.2}  {vz:>7.3}  {:>7.4}  {:>7.4}",
                    foot(0),
                    foot(1)
                );
            }
            obs = r.observation;
            if r.done {
                println!("terminated: {:?}", r.info.termination_reason);
                break;
            }
        }
        return Ok(());
    }

    // `iterations = 0` re-evaluates the policy already on disk instead of
    // retraining — the cheap way to re-check a run you just paid for.
    if iterations == 0 {
        let blob: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&saved)?)?;
        let policy: LinearPolicy = serde_json::from_value(blob["policy"].clone())?;
        let mut env = spec.build()?;
        for s in 1..=10u64 {
            let e = vcad_sim::rl::rollout(&mut env, &policy, &slots, NOMINAL_H, s, &reward, None);
            println!(
                "seed {s:>2}: {:>7.2} over {:>3}/{} steps ({:?})",
                e.reward, e.steps, spec.max_steps, e.termination_reason
            );
        }
        let (r, st, full, _) = eval_10(&spec, &policy, &slots, &reward)?;
        println!("saved policy (10 seeds): mean {r:>7.2} over {st:>5.1} steps, full {full}/10");
        return Ok(());
    }

    // Curriculum: hold randomization at `level` = progress/warmup, capped at
    // 1, so the task starts deterministic (which this trainer solves) and
    // reaches the full sim2real ranges by the `warmup` fraction of training,
    // leaving the remainder to consolidate on the real task. `warmup = 0`
    // disables it and every iteration runs at full randomization.
    let warmup = env_f64("K1_CURRICULUM_WARMUP", 0.4);
    let spec_at = |progress: f64| {
        let level = if warmup > 0.0 {
            (progress / warmup).min(1.0)
        } else {
            1.0
        };
        let mut s = spec.clone();
        s.config = config_at(level);
        s
    };
    if warmup > 0.0 {
        println!(
            "curriculum: randomization ramps 0 → full over the first {:.0}% of training",
            warmup * 100.0
        );
    }

    let arch = std::env::var("K1_POLICY").unwrap_or_else(|_| "mlp".into());
    let action = ActionSpec {
        // The URDF rest pose is the standing pose.
        default_pose_deg: vec![0.0; act_dim],
        action_scale_deg: env_f64("K1_ACTION_SCALE", 8.0),
    };
    match arch.as_str() {
        "linear" => train_and_report(
            &spec_at,
            LinearPolicy::zeros(obs_dim, act_dim, action),
            &cfg,
            &slots,
            &reward,
            iterations,
            &saved,
            (b_r, b_steps, b_full),
        ),
        "mlp" => {
            let hidden = env_usize("K1_HIDDEN", 64);
            println!("policy: 1-hidden-layer tanh MLP, {hidden} units");
            train_and_report(
                &spec_at,
                MlpPolicy::new(obs_dim, hidden, act_dim, action, cfg.seed),
                &cfg,
                &slots,
                &reward,
                iterations,
                &saved,
                (b_r, b_steps, b_full),
            )
        }
        other => {
            Err(format!("unknown K1_POLICY {other:?} (expected \"linear\" or \"mlp\")").into())
        }
    }
}

/// Train `policy` and report it against the baseline on the ten fresh seeds.
///
/// Generic over the architecture so the linear and MLP arms are the *same*
/// experiment — identical env, identical seeds, identical reporting — and a
/// difference between them can only come from the policy.
#[allow(clippy::too_many_arguments)]
fn train_and_report<P, R, S>(
    spec_at: &S,
    policy: P,
    cfg: &ArsConfig,
    slots: &[usize],
    reward: &R,
    iterations: usize,
    saved: &std::path::Path,
    baseline: (f64, f64, usize),
) -> Result<(), Box<dyn std::error::Error>>
where
    P: vcad_sim::rl::Policy + serde::Serialize,
    R: Fn(&vcad_kernel_physics::StepResult, &[f64]) -> f64 + Sync + Send,
    S: Fn(f64) -> EnvSpec + Sync,
{
    let (b_r, b_steps, b_full) = baseline;
    // Everything is reported against the *full* task, never the curriculum's
    // current easier version of it.
    let spec = &spec_at(1.0);

    // Score the iterate on the ten held-out seeds every n iterations
    // (`K1_CURVE=0` disables). On by default: at ~10 rollouts per probe
    // against ~72 per training iteration it costs a couple of percent, and
    // it is the only honest learning curve there is:
    // the trainer's own eval column can sit flat, or climb, while held-out
    // performance collapses — which is exactly what it does here.
    let curve = env_usize("K1_CURVE", 10);
    let mut best_held_out = (f64::NEG_INFINITY, 0usize);
    // ...and keep the iterate that earned it. ARS does not converge and stay
    // converged here: it walks out of a solution at the same rate it walked
    // in (see `ArsConfig::step_size`). Holding on to the best *honestly
    // scored* iterate makes that wandering harmless instead of fatal.
    let mut best_held_out_policy: Option<P> = None;
    let t0 = std::time::Instant::now();
    let TrainOutcome {
        policy: last,
        best_policy: best,
        best_eval_reward,
        log,
    } = vcad_sim::rl::train_curriculum(spec_at, policy, cfg, NOMINAL_H, reward, |e, p| {
        if curve > 0 && (e.iteration % curve == 0 || e.iteration + 1 == iterations) {
            if let Ok((r, st, full, _)) = eval_10(spec, p, slots, reward) {
                if r > best_held_out.0 {
                    best_held_out = (r, e.iteration);
                    best_held_out_policy = Some(p.clone());
                }
                println!(
                    "iter {:>4}  train-eval {:>8.2}  sigma {:>8.3}  |dW| {:>7.4}  |  HELD-OUT {:>8.2} over {:>5.1} steps, full {full}/10",
                    e.iteration, e.eval_reward, e.sigma, e.update_norm, r, st
                );
            }
            return;
        }
        if e.iteration % 5 == 0 || e.iteration + 1 == iterations {
            println!(
                "iter {:>4}  mean {:>8.2}  max {:>8.2}  eval {:>8.2}  survived {:>4}/{}",
                e.iteration,
                e.mean_reward,
                e.max_reward,
                e.eval_reward,
                e.eval_steps,
                spec.max_steps
            );
        }
    })?;
    println!("trained in {:?}", t0.elapsed());
    if curve > 0 {
        println!(
            "best held-out score during training: {:.2} at iteration {}",
            best_held_out.0, best_held_out.1
        );
    }

    // Final report. Both the *best* iterate (selected on ARS's own eval
    // seeds) and the *last* iterate are scored on the ten fresh seeds,
    // because "best" is a selection made on a handful of draws and is itself
    // a quantity that can overfit. If the two disagree wildly, the selection
    // is the thing that is broken, not the policy.
    let (mean_r, mean_steps, survived, worst) = eval_10(spec, &best, slots, reward)?;
    let (l_r, l_steps, l_full, _) = eval_10(spec, &last, slots, reward)?;
    println!(
        "\nhold-rest-pose baseline (10 seeds): mean {b_r:>7.2} over {b_steps:>5.1} steps, \
         full {b_full}/10\n\
         best iterate  (10 seeds): mean {mean_r:>7.2} over {mean_steps:>5.1} steps, \
         full {survived}/10, worst {:>7.2} over {} steps ({:?})\n\
         last iterate  (10 seeds): mean {l_r:>7.2} over {l_steps:>5.1} steps, full {l_full}/10\n\
         (training-time eval of the best iterate was {best_eval_reward:.2} — \
         compare that gap, not the number)",
        worst.reward, worst.steps, worst.termination_reason
    );

    // Ship whichever iterate actually scored best on held-out seeds, and say
    // which. Defaulting to the trainer's own "best" is what produced a policy
    // no better than doing nothing; and because ARS walks back out of a
    // solution (see `ArsConfig::step_size`), the peak is usually neither the
    // last iterate nor the one the trainer liked.
    let (mut kept, mut kept_name, mut kept_score) = if l_r > mean_r {
        (&last, "last", l_r)
    } else {
        (&best, "train-eval-best", mean_r)
    };
    if let Some(p) = best_held_out_policy.as_ref() {
        if best_held_out.0 > kept_score {
            kept = p;
            kept_name = "held-out-best";
            kept_score = best_held_out.0;
        }
    }
    println!("keeping the {kept_name} iterate (held-out {kept_score:.2})");
    std::fs::write(
        saved,
        serde_json::to_string_pretty(&serde_json::json!({
            "policy": kept,
            "kept": kept_name,
            "log": log,
            "config": cfg,
        }))?,
    )?;
    println!("policy → {}", saved.display());

    Ok(())
}

/// Write one replay frame as a standalone `.vcad`.
///
/// Drops the `Free` world joint so the trunk becomes a root instance whose
/// `transform` carries the live floating-base pose, then stamps each actuated
/// joint's angle into its `state`. Document FK reproduces the rest.
fn write_frame(
    doc: &Document,
    obs: &vcad_kernel_physics::Observation,
    slots: &[usize],
    joint_ids: &[String],
    dir: &str,
    frame: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut d = doc.clone();
    let pose = obs
        .base_pose
        .unwrap_or([0.0, 0.0, NOMINAL_H, 1.0, 0.0, 0.0, 0.0]);
    let (x, y, z) = (pose[0], pose[1], pose[2]);
    let (qw, qx, qy, qz) = (pose[3], pose[4], pose[5], pose[6]);

    // Quaternion → the Rz·Ry·Rx Euler degrees `vcad_eval::kinematics` expects.
    let m20 = -2.0 * (qx * qz - qw * qy);
    let m21 = 2.0 * (qy * qz + qw * qx);
    let m22 = 1.0 - 2.0 * (qx * qx + qy * qy);
    let m00 = 1.0 - 2.0 * (qy * qy + qz * qz);
    let m10 = 2.0 * (qx * qy + qw * qz);
    let sy = -m20;
    let cy = (m00 * m00 + m10 * m10).sqrt();
    let (rx, ry, rz) = if cy > 1e-6 {
        (m21.atan2(m22), sy.atan2(cy), m10.atan2(m00))
    } else {
        (0.0, sy.atan2(cy), 0.0)
    };
    let deg = 180.0 / std::f64::consts::PI;

    let base_id = d
        .joints
        .iter()
        .flat_map(|js| js.iter())
        .find(|j| matches!(j.kind, vcad_ir::JointKind::Free))
        .map(|j| j.child_instance_id.clone())
        .ok_or("replay needs a Free base joint")?;

    if let Some(js) = d.joints.as_mut() {
        js.retain(|j| !matches!(j.kind, vcad_ir::JointKind::Free));
        for j in js.iter_mut() {
            if let Some(a) = joint_ids.iter().position(|id| *id == j.id) {
                j.state = obs.joint_positions[slots[a]];
            }
        }
    }
    if let Some(insts) = d.instances.as_mut() {
        for inst in insts.iter_mut() {
            if inst.id == base_id {
                inst.transform = Some(vcad_ir::Transform3D {
                    // Physics is metres, documents are millimetres.
                    translation: vcad_ir::Vec3::new(x * 1000.0, y * 1000.0, z * 1000.0),
                    rotation: vcad_ir::Vec3::new(rx * deg, ry * deg, rz * deg),
                    scale: vcad_ir::Vec3::new(1.0, 1.0, 1.0),
                });
            }
        }
    }
    d.ground_instance_id = None;
    std::fs::write(
        format!("{dir}/frame_{frame:04}.vcad"),
        serde_json::to_string(&d)?,
    )?;
    Ok(())
}
