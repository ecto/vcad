//! M3.0 — does the GPU batch integrate the same dynamics as the CPU env?
//!
//! "CPU/GPU parity" has been the stated gate on the whole batched-simulation
//! milestone, and until this file there was nothing measuring it: `vcad-sim`
//! had no test directory at all. phyz has its own GPU-vs-CPU tests, but those
//! check *phyz's* pipeline on *phyz's* models. Nothing checked that a vcad
//! document, built once by `PhysicsWorld::from_document` and handed to both
//! backends, produces the same trajectory.
//!
//! # Why the comparison is at raw phyz state
//!
//! `PhysicsWorld::phyz_state()` and `GpuBatchSimulator::readback_states()` are
//! the same type in the same units. Comparing there isolates the question the
//! gate actually asks — do the two integrators agree — from the separate
//! question of whether the two observation layers agree, which is M3.1's job
//! and involves a genuine unit mismatch (`vcad-sim`'s `Observation` is raw
//! radians/metres; `vcad_kernel_physics`'s is degrees/millimetres). Mixing the
//! two would mean a parity failure could be either a physics divergence or a
//! conversion bug, and the whole point is to tell them apart.
//!
//! # No GPU adapter
//!
//! These skip loudly rather than fail when no adapter is present (headless CI,
//! a container without a device). A silent skip would let the gate report green
//! on a machine that never ran it.

use phyz_model::State;
use vcad_ir::Document;
use vcad_kernel_physics::PhysicsWorld;
use vcad_sim::BatchSimPipeline;

/// The tick both backends must agree to run at. See `BatchSimPipeline::from_document`.
const DT: f64 = 1.0 / 1000.0;

/// The committed floating-base sample: primitives only, so no mesh resolution
/// and no vendored assets. Small enough that a divergence is attributable.
fn floating_arm() -> Document {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/floating-arm.vcad");
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
    serde_json::from_str(&json).expect("fixture is not a valid document")
}

/// Build the GPU pipeline, or `None` when this machine has no usable adapter.
fn gpu_or_skip(doc: &Document, n_envs: usize, what: &str) -> Option<BatchSimPipeline> {
    match BatchSimPipeline::from_document(doc, n_envs, DT) {
        Ok(p) => Some(p),
        Err(e) => {
            // Loud on purpose — see the module docs.
            eprintln!("SKIP {what}: no usable GPU adapter ({e})");
            None
        }
    }
}

/// Worst absolute difference between two flat state vectors, with the index.
///
/// Panics on a non-finite input rather than folding over it. `NaN > acc` is
/// false, so a plain max-fold returns 0.0 for a vector that is entirely NaN —
/// which is exactly how the first version of this harness reported *perfect*
/// GPU/CPU agreement while the GPU was producing NaN. A comparison that cannot
/// distinguish "identical" from "garbage" is worse than no comparison.
fn worst(label: &str, a: &[f64], b: &[f64]) -> (f64, usize) {
    assert_eq!(a.len(), b.len(), "{label}: length mismatch");
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert!(x.is_finite(), "{label}: gpu value {i} is non-finite ({x})");
        assert!(y.is_finite(), "{label}: cpu value {i} is non-finite ({y})");
    }
    a.iter()
        .zip(b.iter())
        .enumerate()
        .map(|(i, (x, y))| ((x - y).abs(), i))
        .fold((0.0, 0), |acc, v| if v.0 > acc.0 { v } else { acc })
}

fn cpu_reference(doc: &Document, dt: f32, steps: usize) -> State {
    let mut world = PhysicsWorld::from_document(doc).expect("cpu world");
    for _ in 0..steps {
        world.step(dt);
    }
    world.phyz_state().clone()
}

#[test]
fn gpu_matches_cpu_over_a_short_horizon() {
    // The gate, stated correctly. See `f32_divergence_grows_and_bounds_the_horizon`
    // for why this is 50 steps and not 400.
    let doc = floating_arm();
    let steps = 50;

    let Some(mut gpu) = gpu_or_skip(&doc, 1, "gpu_matches_cpu_over_a_short_horizon") else {
        return;
    };
    gpu.batch_reset();
    let zero = vec![0.0f64; gpu.action_dim()];
    for _ in 0..steps {
        gpu.batch_step(&zero).expect("gpu step");
    }
    let got = gpu.batch_observe();
    let cpu = cpu_reference(&doc, DT as f32, steps);

    let (dq, iq) = worst("q", &got[0].joint_positions, cpu.q.as_slice());
    let (dv, iv) = worst("v", &got[0].joint_velocities, cpu.v.as_slice());
    eprintln!("{steps} steps: worst dq {dq:.3e} (dof {iq}), dv {dv:.3e} (dof {iv})");

    // 1e-5 sits an order above the f32 epsilon (1.2e-7) the GPU seeds and
    // several orders below anything physical. It is not a bit-parity claim —
    // that is unavailable, see below.
    assert!(
        dq < 1e-5 && dv < 1e-4,
        "GPU and CPU disagree beyond f32 seeding after {steps} steps: \
         dq {dq:.3e} at dof {iq}, dv {dv:.3e} at dof {iv}"
    );
}

#[test]
fn f32_divergence_grows_and_bounds_the_horizon() {
    // The single most important fact about the batch path, and the one that
    // sets what M3 can promise: **the GPU is f32 and the CPU is f64.**
    // phyz-gpu's shaders are 211 f32s and zero f64s — WGSL has no practical
    // f64 — and `readback_states` widens on the way out. So bit-parity is not
    // achievable, and "do the trajectories match" is the wrong exit criterion
    // for the milestone.
    //
    // What actually happens, measured on the floating-arm sample in free fall
    // (an undamped chain, so the worst case — nothing removes energy from a
    // perturbation):
    //
    //     step  50   dq 1.6e-7   <- f32 epsilon, the seed
    //     step 100   dq 5.3e-7
    //     step 150   dq 1.7e-6
    //     step 200   dq 1.5e-5
    //     step 250   dq 2.4e-4
    //     step 300   dq 7.8e-3
    //     step 400   dq 2.1e0    <- no longer the same trajectory
    //
    // Roughly an order of magnitude per 50 steps. The CPU's elbow stays at
    // exactly zero throughout, which is right — in free fall there is no
    // relative torque — so this is entirely f32 noise being amplified, not a
    // modelling difference.
    //
    // Consequences worth stating plainly:
    //   - A 400-step episode will NOT match between backends. Any M3 milestone
    //     phrased as trajectory agreement over an episode is unachievable.
    //   - A policy must therefore be judged by *behaviour* on each backend
    //     (does it stand, what does it score) rather than by matching states.
    //   - This sample is the pessimistic case. A robot under PD servos has
    //     damping, which removes the amplification; the K1 is the case that
    //     matters and should be measured separately.
    let doc = floating_arm();
    let Some(mut gpu) = gpu_or_skip(&doc, 1, "f32_divergence_grows") else {
        return;
    };
    let mut world = PhysicsWorld::from_document(&doc).expect("cpu world");
    gpu.batch_reset();
    let zero = vec![0.0f64; gpu.action_dim()];

    let mut at_50 = 0.0;
    let mut at_300 = 0.0;
    for s in 1..=300 {
        gpu.batch_step(&zero).expect("gpu step");
        world.step(DT as f32);
        if s == 50 || s == 300 {
            let g = gpu.batch_observe();
            let d = g[0]
                .joint_positions
                .iter()
                .zip(world.phyz_state().q.as_slice())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f64, f64::max);
            if s == 50 {
                at_50 = d;
            } else {
                at_300 = d;
            }
        }
    }
    eprintln!("f32 divergence: {at_50:.3e} at 50 steps -> {at_300:.3e} at 300");

    assert!(
        at_50 < 1e-5,
        "the seed should still be f32-sized at 50 steps"
    );
    // Pinned as a *fact*, not a wish: if this ever stops growing, the GPU has
    // gained f64 or the model has gained damping, and the horizon guidance
    // above needs revisiting rather than quietly being wrong.
    assert!(
        at_300 > at_50 * 100.0,
        "f32 divergence is expected to amplify on an undamped chain \
         ({at_50:.3e} -> {at_300:.3e}); if it no longer does, re-measure the \
         horizon this milestone is planned around"
    );
}

#[test]
fn every_env_in_a_batch_is_identical_without_randomization() {
    // `batch_reset` puts every env in the same state and nothing perturbs
    // them, so all N must stay bit-identical. When they don't, the batch is
    // either sharing state across envs or reading past an env's stride — both
    // of which look like "training is noisy" rather than like a bug.
    //
    // This is also the measurement that makes M3.3 concrete: until per-env
    // randomization exists, N envs cost N times the compute for exactly one
    // env's worth of information, and this test is what proves it.
    let doc = floating_arm();
    let n = 8;
    let Some(mut gpu) = gpu_or_skip(&doc, n, "every_env_in_a_batch_is_identical") else {
        return;
    };
    gpu.batch_reset();
    let zero = vec![0.0f64; gpu.action_dim() * n];
    for _ in 0..100 {
        gpu.batch_step(&zero).expect("gpu step");
    }
    let obs = gpu.batch_observe();
    assert_eq!(obs.len(), n);
    for i in 1..n {
        assert_eq!(
            obs[i].joint_positions, obs[0].joint_positions,
            "env {i} drifted from env 0 with no randomization applied"
        );
    }
    eprintln!("{n} envs identical after 100 steps (as expected until M3.3 adds per-env seeds)");
}

#[test]
fn the_batch_reports_the_same_model_the_cpu_built() {
    // Cheap structural guard. The batch used to re-derive its own model with
    // density-guessed inertias, which trained against a different robot than
    // the one being edited; it now shares `PhysicsWorld::from_document`. This
    // pins that they still agree on shape, so a regression shows up as a
    // dimension mismatch rather than as a slow drift in the numbers above.
    let doc = floating_arm();
    let world = PhysicsWorld::from_document(&doc).expect("cpu world");
    let Some(gpu) = gpu_or_skip(&doc, 1, "the_batch_reports_the_same_model") else {
        return;
    };
    assert_eq!(
        gpu.action_dim(),
        world.phyz_state().v.len(),
        "the batch's action width must be the model's velocity DOF count"
    );
    assert_eq!(gpu.n_envs(), 1);

    // Every servoed joint must be one the CPU also knows about, addressed the
    // same way — the PD interface indexes against this list.
    for (id, q) in gpu.servo_joint_ids().iter().zip(gpu.servo_q_offsets()) {
        let (cpu_q, _, ndof, _) = world
            .joint_addressing(id)
            .unwrap_or_else(|| panic!("gpu servoes {id:?}, which the CPU model does not have"));
        assert_eq!(ndof, 1, "{id} is servoed but is not single-DOF");
        assert_eq!(cpu_q, q, "{id} has a different q offset on the two paths");
    }
}

// ---------------------------------------------------------------------------
// M3.1 — do the two backends produce the same *policy input*?
// ---------------------------------------------------------------------------

/// Build a CPU env over the sample, matching what a batch decoder needs.
///
/// `base_instance_id` is set explicitly to the free joint's child. Left to
/// default it resolves to the document's *ground* instance — the world — whose
/// pose is a constant and whose velocity is zero. Every base-derived feature
/// then reads its fallback: projected gravity comes back as the literal
/// `[0, 0, -1]` default and the base velocity as zeros. Both backends agree
/// perfectly on that, and it means nothing. This is the same trap as a
/// fixed-base document passing a floating-base check.
fn cpu_env(doc: &Document) -> vcad_kernel_physics::RobotEnv {
    vcad_kernel_physics::RobotEnv::new_with_config(
        doc.clone(),
        vec!["end_effector_inst".to_string()],
        Some(DT as f32),
        Some(1),
        None,
        vcad_kernel_physics::EnvConfig {
            base_instance_id: Some("base_link_inst".to_string()),
            ..Default::default()
        },
    )
    .expect("cpu env")
}

#[test]
fn the_two_backends_produce_the_same_policy_features() {
    // State-level parity is necessary but not sufficient. What a policy
    // actually consumes is `rl::features`, and between the raw batch state and
    // that vector sit a unit system (radians/metres vs degrees/millimetres),
    // an angular-first free-joint layout, a body-to-world rotation of the base
    // velocity, and a quaternion built from exponential coordinates. Any one
    // of those done differently on the two paths gives a policy that works on
    // one backend and twitches on the other, with nothing raising an error.
    //
    // This asserts the thing that matters: identical feature vectors, from the
    // identical state, through both paths.
    let doc = floating_arm();
    let Some(mut gpu) = gpu_or_skip(&doc, 1, "the_two_backends_produce_the_same_policy_features")
    else {
        return;
    };
    let mut decoder = cpu_env(&doc);
    let mut cpu = cpu_env(&doc);

    let slots = vcad_sim::rl::actuated_slots(&cpu);
    let nominal_h = 0.0;

    let cpu_obs = cpu.reset_with_seed(1);
    gpu.batch_reset();

    let f_cpu = vcad_sim::rl::features(&cpu_obs, &slots, nominal_h);
    let gpu_obs = gpu.batch_observe_gym(&mut decoder).expect("decode");
    let f_gpu = vcad_sim::rl::features(&gpu_obs[0], &slots, nominal_h);

    assert_eq!(
        f_cpu.len(),
        f_gpu.len(),
        "the two backends disagree about the policy's input width"
    );
    let (d, i) = worst("features at reset", &f_gpu, &f_cpu);
    eprintln!(
        "features at reset: {} elements, worst delta {d:.3e} (element {i})",
        f_cpu.len()
    );

    // At reset both describe the same state, so the only difference is the
    // f32 round-trip through the GPU buffers — about 1e-7 relative on values
    // of order 1.
    assert!(
        d < 1e-5,
        "policy features differ at reset by {d:.3e} at element {i}. \
         The batch path is feeding a policy something the CPU path would not."
    );
}

#[test]
fn the_feature_vector_is_not_accidentally_all_zeros() {
    // The failure this guards is the one that looks like success: if the
    // decoder never loaded the state, or the free-joint slots were read at the
    // wrong offsets, both feature vectors could agree *and* be meaningless.
    // A robot that has fallen for a while has a non-trivial gravity vector and
    // a non-zero base velocity; assert the vector actually carries signal.
    let doc = floating_arm();
    let Some(mut gpu) = gpu_or_skip(&doc, 1, "the_feature_vector_is_not_all_zeros") else {
        return;
    };
    let mut decoder = cpu_env(&doc);
    let slots = vcad_sim::rl::actuated_slots(&decoder);

    gpu.batch_reset();
    let zero = vec![0.0f64; gpu.action_dim()];
    for _ in 0..50 {
        gpu.batch_step(&zero).expect("gpu step");
    }
    let obs = gpu.batch_observe_gym(&mut decoder).expect("decode");
    let f = vcad_sim::rl::features(&obs[0], &slots, 0.0);

    // Feature layout: [projected gravity (3), base lin vel (3), base ang vel
    // (3), height - nominal (1), joint angles, joint vels, contacts].
    let g_mag = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
    assert!(
        (g_mag - 1.0).abs() < 1e-3,
        "projected gravity should be a unit vector, got {g_mag:.4} — the base \
         orientation is not being decoded"
    );
    let falling = f[5];
    assert!(
        falling < -0.1,
        "after 50 steps of free fall the base should be moving downward, got \
         vz = {falling:.4} — the base velocity is not being decoded"
    );
    eprintln!("features carry signal: |gravity| = {g_mag:.4}, vz = {falling:.4}");
}
