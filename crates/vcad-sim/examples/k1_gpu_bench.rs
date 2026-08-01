//! Bring-up + throughput benchmark for the GPU batch path on the Booster K1.
//!
//! ```bash
//! cargo run --release -p vcad-sim --example k1_gpu_bench -- k1.vcad
//! ```
//!
//! Measures three things, in order of what they tell you:
//!
//! 1. **Does the K1 construct and step on the GPU at all** — 22 revolute DOF
//!    plus a 7-position/6-velocity Free base through the batched ABA shader.
//!    A wrong answer here is a correctness bug, not a performance number.
//! 2. **CPU reference throughput** — one `RobotEnv` stepped exactly the way
//!    ARS rollouts step it (20 substeps at 1 kHz per control step). This is
//!    the number the GPU has to beat *per environment × environment count*.
//! 3. **GPU batch throughput** across environment counts, with and without
//!    per-step host readback. The gap between those two columns is the
//!    argument for the zero-copy tensor contract
//!    (`phyz_gpu::GpuBatchSimulator::interop`): once observations stay on
//!    device, only the submit column matters.
//!
//! What this deliberately does **not** measure yet: contact — the GPU
//! penalty pipeline needs collision geometry the model doesn't carry yet.
//! PD actuation landed (`BatchSimPipeline::enable_pd`) and is sanity-checked
//! at the bottom: all 22 servoed K1 joints track a 0.3 rad command while the
//! base free-falls.

use vcad_ir::Document;
use vcad_sim::BatchSimPipeline;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: k1_gpu_bench <doc.vcad>");
    let mut doc: Document = serde_json::from_str(&std::fs::read_to_string(&path)?)?;

    // Raise the floating base like k1_stand does — import anchors it at z=0.
    for j in doc.joints.iter_mut().flat_map(|js| js.iter_mut()) {
        if matches!(j.kind, vcad_ir::JointKind::Free) {
            j.parent_anchor.z = 549.8;
        }
    }

    // --- CPU reference: one env, the exact ARS rollout configuration. ---
    let mut env = vcad_kernel_physics::RobotEnv::new(
        doc.clone(),
        vec![],
        Some(1.0 / 1000.0),
        Some(20),
        None,
    )?;
    env.set_max_steps(u32::MAX);
    let act_dim = env.action_dim();
    let cpu_steps = 200u32;
    let t0 = std::time::Instant::now();
    for _ in 0..cpu_steps {
        env.step(vcad_kernel_physics::Action::Torque(vec![0.0; act_dim]));
    }
    let cpu_dt = t0.elapsed();
    let cpu_rate = cpu_steps as f64 / cpu_dt.as_secs_f64();
    println!(
        "CPU RobotEnv (1 env, 20 substeps/step): {cpu_rate:.0} env-steps/s \
         ({:.0} substeps/s)",
        cpu_rate * 20.0
    );

    // --- GPU batch: same document, growing environment counts. ---
    // The GPU pipeline steps a single physics tick per call (no substep
    // batching yet), so its numbers are *substeps*/s — compare against the
    // CPU substep column, and divide by 20 for control-step terms.
    println!(
        "\n{:>7}  {:>16}  {:>16}  {:>10}",
        "envs", "submit substeps/s", "readback substeps/s", "vs CPU"
    );
    for n_envs in [64usize, 256, 1024, 4096] {
        let mut batch = match BatchSimPipeline::from_document(&doc, n_envs) {
            Ok(b) => b,
            Err(e) => {
                println!("{n_envs:>7}  construction failed: {e}");
                continue;
            }
        };
        let nv = batch.action_dim();
        let actions = vec![0.0f64; n_envs * nv];
        let iters = 200usize;

        batch.batch_reset();
        // Warmup: shader compilation and first-submit costs land here.
        for _ in 0..10 {
            batch.batch_step_submit(&actions)?;
        }
        let _ = batch.batch_observe(); // sync

        let t0 = std::time::Instant::now();
        for _ in 0..iters {
            batch.batch_step_submit(&actions)?;
        }
        let _ = batch.batch_observe(); // block until the queue drains
        let submit_rate = (n_envs * iters) as f64 / t0.elapsed().as_secs_f64();

        batch.batch_reset();
        let t0 = std::time::Instant::now();
        for _ in 0..iters {
            let _ = batch.batch_step(&actions)?;
        }
        let readback_rate = (n_envs * iters) as f64 / t0.elapsed().as_secs_f64();

        println!(
            "{n_envs:>7}  {submit_rate:>16.0}  {readback_rate:>16.0}  {:>9.1}x",
            submit_rate / (cpu_rate * 20.0)
        );
    }

    // --- Sanity: gravity acts. A zero-torque K1 must be falling. ---
    let mut batch = BatchSimPipeline::from_document(&doc, 4)?;
    batch.batch_reset();
    let z0 = batch.batch_observe()[0].joint_positions[2];
    let nv = batch.action_dim();
    for _ in 0..100 {
        batch.batch_step_submit(&vec![0.0; 4 * nv])?;
    }
    let z1 = batch.batch_observe()[0].joint_positions[2];
    println!(
        "\nsanity: base z {z0:.3} -> {z1:.3} after 100 ticks of free fall \
         ({})",
        if z1 < z0 - 1e-3 {
            "falling — gravity OK"
        } else {
            "NOT falling — GPU path is broken, numbers above are meaningless"
        }
    );

    // --- PD sanity: servos track a commanded pose while the base
    // free-falls. ---
    //
    // Zero targets would prove nothing (in uniform free fall the joints stay
    // at rest with or without servos), so command 0.3 rad on every servoed
    // joint and require the tracking error to shrink. A sign error diverges;
    // wrong DOF indexing leaves some joint untouched at 0.3 rad of error.
    // (No contact geometry yet, so the robot falls forever; joint tracking
    // is the thing under test.)
    let mut batch = BatchSimPipeline::from_document(&doc, 4)?;
    let gains: std::collections::HashMap<String, (f64, f64)> = batch
        .servo_joint_ids()
        .iter()
        .map(|id| {
            let g = if id.contains("Hip") || id.contains("Knee") {
                (200.0, 5.0)
            } else if id.contains("Ankle") {
                (50.0, 1.0)
            } else {
                (40.0, 1.0)
            };
            (id.to_string(), g)
        })
        .collect();
    batch.enable_pd(&gains, (40.0, 1.0))?;
    let n_servo = batch.servo_joint_ids().len();
    batch.batch_reset();
    let cmd = 0.3f64;
    for _ in 0..200 {
        batch.batch_step_targets(&vec![vec![cmd; n_servo]; 4])?;
    }
    let obs = batch.batch_observe();
    // Worst tracking error over the servoed slots, at their true offsets:
    let servo_offsets = batch.servo_q_offsets();
    let worst = servo_offsets
        .iter()
        .map(|&s| (obs[0].joint_positions[s] - cmd).abs())
        .fold(0.0f64, f64::max);
    println!(
        "PD sanity: worst |q - {cmd}| after 200 ticks commanding {cmd} rad: \
         {worst:.4} rad ({})",
        if worst < 0.15 {
            "tracking — PD pass OK"
        } else {
            "NOT tracking — PD pass broken"
        }
    );
    Ok(())
}
