//! Train a standing-balance policy for the Booster K1 humanoid.
//!
//! ```bash
//! # import first:  vcad import-urdf K1_22dof_floating.urdf k1.vcad
//! cargo run --release -p vcad-sim --example k1_stand -- k1.vcad [iterations]
//! ```
//!
//! Writes the trained policy to `k1_stand_policy.json` next to the document.

use std::collections::HashMap;

use vcad_ir::Document;
use vcad_kernel_physics::{Action, DomainRandomization, EnvConfig, Range, TerminationConfig};
use vcad_sim::rl::{actuated_slots, ActionSpec, ArsConfig, EnvSpec, LinearPolicy, TrainOutcome};

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
        (200.0, 5.0)
    } else if joint.contains("Ankle") {
        (50.0, 1.0)
    } else {
        (40.0, 1.0)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: k1_stand <doc.vcad> [iterations]");
    let iterations: usize = args.next().map_or(150, |s| s.parse().unwrap());

    let doc: Document = serde_json::from_str(&std::fs::read_to_string(&path)?)?;

    // Sim2real-shaped randomization, in the spirit of Booster's own K1 config
    // (2–8 substeps of actuator latency). Without it every episode is
    // bit-identical and "the policy survives 400 steps" says nothing about
    // whether it survives a robot.
    let config = EnvConfig {
        randomization: Some(DomainRandomization {
            mass_scale: Some(Range { min: 0.9, max: 1.1 }),
            pd_gain_scale: Some(Range { min: 0.8, max: 1.2 }),
            action_latency_steps: Some([2, 8]),
            joint_pos_perturb: Some(2.0),
            joint_vel_perturb: Some(5.0),
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
    let obs_dim = vcad_sim::rl::feature_dim(&probe, &slots);
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

    let policy = LinearPolicy::zeros(
        obs_dim,
        act_dim,
        ActionSpec {
            // The URDF rest pose is the standing pose.
            default_pose_deg: vec![0.0; act_dim],
            action_scale_deg: 8.0,
        },
    );

    let cfg = ArsConfig {
        n_directions: 12,
        top_k: 6,
        step_size: 0.03,
        noise_std: 0.05,
        iterations,
        episode_steps: spec.max_steps,
        // Three draws per evaluation: enough to stop the ranking chasing
        // lucky randomization, cheap enough to keep the run under 20 min.
        rollouts_per_eval: 3,
        seed: 7,
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

    // `iterations = 0` re-evaluates the policy already on disk instead of
    // retraining — the cheap way to re-check a run you just paid for.
    let saved = std::path::Path::new(&path).with_file_name("k1_stand_policy.json");
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
        return Ok(());
    }

    let t0 = std::time::Instant::now();
    let TrainOutcome {
        policy: _final,
        best_policy: policy,
        best_eval_reward,
        log,
    } = vcad_sim::rl::train(&spec, policy, &cfg, NOMINAL_H, reward, |e| {
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
    println!(
        "trained in {:?} — keeping the best iterate (eval {best_eval_reward:.2})",
        t0.elapsed()
    );

    // Final report: baseline (hold the rest pose) vs the learned policy.
    let mut env = spec.build()?;
    let mut obs = env.reset_with_seed(1);
    let (mut baseline_steps, mut baseline_r) = (0u32, 0.0);
    for _ in 0..spec.max_steps {
        let targets = vec![0.0; act_dim];
        let r = env.step_full(Action::PositionTarget(targets.clone()));
        baseline_r += reward(&r, &targets);
        baseline_steps += 1;
        obs = r.observation;
        if r.done {
            break;
        }
    }
    let _ = obs;

    // Evaluate over a spread of seeds, not one. Each reset re-samples the
    // episode's actuator latency, so a single-seed number hides brittleness.
    let mut env = spec.build()?;
    let evals: Vec<_> = (1..=10u64)
        .map(|s| vcad_sim::rl::rollout(&mut env, &policy, &slots, NOMINAL_H, s, &reward, None))
        .collect();
    let survived = evals.iter().filter(|e| e.steps == spec.max_steps).count();
    let mean_r = evals.iter().map(|e| e.reward).sum::<f64>() / evals.len() as f64;
    let worst = evals
        .iter()
        .min_by(|a, b| a.reward.total_cmp(&b.reward))
        .unwrap();
    println!(
        "\nhold-rest-pose baseline: {:>7.2} over {:>3} steps\n\
         learned policy (10 seeds): mean {:>7.2}, full episode on {survived}/10, \
         worst {:>7.2} over {} steps ({:?})",
        baseline_r, baseline_steps, mean_r, worst.reward, worst.steps, worst.termination_reason
    );

    let out = saved;
    std::fs::write(
        &out,
        serde_json::to_string_pretty(&serde_json::json!({
            "policy": policy,
            "log": log,
            "config": cfg,
        }))?,
    )?;
    println!("policy → {}", out.display());

    Ok(())
}
