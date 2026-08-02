//! Golden-trajectory parity harness for the simulation ABI.
//!
//! # What this defends
//!
//! The native app steps physics through a C boundary and renders the result.
//! Two classes of bug live there and neither one produces an error:
//!
//! 1. **Marshalling drift** — a buffer read at the wrong stride, a unit
//!    conversion applied once too often, a quaternion transposed. The robot
//!    still moves; it just moves wrongly, and "wrongly" is indistinguishable
//!    from "the policy is bad" by eye.
//! 2. **Determinism loss** — a `HashMap` iteration order leaking into the
//!    physics, an uninitialized accumulator. The same seed stops producing the
//!    same rollout, and every A/B measurement downstream becomes noise.
//!
//! So this file pins an actual trajectory: 100 steps of a 23-DOF floating-base
//! humanoid falling under gravity from a held rest pose, recorded to a golden
//! JSON file. The Rust side asserts it reproduces the golden; the Swift side
//! (`GymParityTests`) loads the *same* file through the *same* ABI and asserts
//! the same numbers. A discrepancy localizes immediately: if Rust passes and
//! Swift fails, the bug is in the Swift wrapper, not the kernel.
//!
//! # Regenerating
//!
//! `UPDATE_GOLDEN=1 cargo test -p vcad-ffi --test gym_parity` rewrites it.
//! Do that only when a physics change is *intended*, and say so in the commit
//! — an unexplained golden update is how a regression gets blessed.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use vcad_ffi::gym::*;

/// Where the shared fixture and golden live.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The 23-DOF Unitree G1 with a synthesized floating base, imported from the
/// self-contained (primitive-geometry) URDF fixture so this test needs no
/// vendored meshes and no network.
fn g1_document() -> String {
    std::fs::read_to_string(fixtures().join("g1_floating.vcad"))
        .expect("g1_floating.vcad fixture missing; regenerate with `vcad import-urdf`")
}

/// Spec matching the fixture: feet as end effectors, 1 kHz physics / 50 Hz
/// control, and the stiff-legged gains a humanoid needs to hold itself up.
fn g1_spec() -> String {
    serde_json::json!({
        "end_effector_ids": ["left_ankle_roll_link_inst", "right_ankle_roll_link_inst"],
        "dt": 1.0 / 1000.0,
        "substeps": 20,
        "max_steps": 400,
        "nominal_height_m": 0.78,
        "config": {
            "termination": {
                "base_height_below": 0.4,
                "base_tilt_above_deg": 45.0,
                "terminate_on_joint_limit": false
            },
            "base_instance_id": "pelvis_inst"
        }
    })
    .to_string()
}

/// Read the thread-local last error as a `String`, for assertion messages.
fn last_error() -> String {
    let mut len = 0usize;
    let p = vcad_ffi::vcad_last_error(&mut len);
    if p.is_null() {
        return "<no error>".to_string();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
}

/// RAII wrapper so a failing assertion doesn't leak the env.
struct Gym(*mut VcadGym);

impl Gym {
    fn open(doc: &str, spec: &str) -> Self {
        let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
        assert!(!g.is_null(), "vcad_gym_create failed: {}", last_error());
        Self(g)
    }
    fn as_ptr(&self) -> *mut VcadGym {
        self.0
    }
}

impl Drop for Gym {
    fn drop(&mut self) {
        vcad_gym_free(self.0);
    }
}

/// One recorded step: the quantities a renderer and a reward both read.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
struct Frame {
    step: u32,
    base_height_m: f64,
    base_tilt_deg: f64,
    /// First six joint position slots — the floating base's own DOFs.
    base_dofs: Vec<f64>,
    done: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct Golden {
    /// Human note explaining what this file is, so it isn't mistaken for
    /// generated noise that can be regenerated at will.
    _note: String,
    action_dim: usize,
    obs_dim: usize,
    observation_dim: usize,
    body_count: usize,
    control_dt: f64,
    frames: Vec<Frame>,
}

/// Roll the fixture forward `n` steps holding the rest pose, recording each.
fn record(n: usize) -> Golden {
    let doc = g1_document();
    let spec = g1_spec();
    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();

    let action_dim = vcad_gym_action_dim(g);
    assert!(action_dim > 0, "fixture has no actuated joints");

    assert_eq!(vcad_gym_reset(g, 1), 1, "reset failed: {}", last_error());

    // Zero position targets = hold the rest pose. The zero policy's action,
    // and the baseline every trained policy must beat.
    let actions = vec![0.0f64; action_dim];
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let ok = vcad_gym_step(g, actions.as_ptr(), actions.len(), 1);
        assert_eq!(ok, 1, "step failed: {}", last_error());
        let v = vcad_gym_step_view(g);
        let positions =
            unsafe { std::slice::from_raw_parts(v.joint_positions, v.joint_positions_len) };
        frames.push(Frame {
            step: v.step,
            base_height_m: v.base_height_m,
            base_tilt_deg: v.base_tilt_deg,
            base_dofs: positions.iter().take(6).copied().collect(),
            done: v.done != 0,
        });
        if v.done != 0 {
            break;
        }
    }

    Golden {
        _note: "Golden physics trajectory for the vcad simulation ABI. Regenerate \
                only for an INTENDED physics change: UPDATE_GOLDEN=1 cargo test \
                -p vcad-ffi --test gym_parity. See tests/gym_parity.rs."
            .to_string(),
        action_dim,
        obs_dim: vcad_gym_obs_dim(g),
        observation_dim: vcad_gym_observation_dim(g),
        body_count: vcad_gym_body_count(g),
        control_dt: vcad_gym_control_dt(g),
        frames,
    }
}

#[test]
fn trajectory_matches_the_golden_record() {
    let golden_path = fixtures().join("g1_fall_100.json");
    let got = record(100);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&golden_path, serde_json::to_string_pretty(&got).unwrap()).unwrap();
        eprintln!("wrote {}", golden_path.display());
        return;
    }

    let want: Golden = serde_json::from_str(
        &std::fs::read_to_string(&golden_path)
            .expect("golden missing; run with UPDATE_GOLDEN=1 to create it"),
    )
    .expect("golden is not valid JSON");

    assert_eq!(got.action_dim, want.action_dim, "action dimension changed");
    assert_eq!(got.obs_dim, want.obs_dim, "policy feature count changed");
    assert_eq!(
        got.observation_dim, want.observation_dim,
        "raw observation layout changed"
    );
    assert_eq!(
        got.body_count, want.body_count,
        "simulated body count changed"
    );
    assert!(
        (got.control_dt - want.control_dt).abs() < 1e-12,
        "control period changed: {} vs {}",
        got.control_dt,
        want.control_dt
    );
    assert_eq!(
        got.frames.len(),
        want.frames.len(),
        "episode length changed — the robot now falls at a different rate"
    );

    // Compared to a tolerance, and for two reasons — neither of which is
    // "the simulator is flaky". It is not: `stepping_is_deterministic_for_a_fixed_seed`
    // asserts bit-exactness between two in-memory runs, with no file in the path.
    //
    // 1. JSON is lossy here. `serde_json` serializes an f64 faithfully but its
    //    *parser* does not round-trip subnormal-magnitude values:
    //    -7.510773185222099e-19 is written correctly and reads back as
    //    -7.5107731852221e-19. Several base DOFs sit at 1e-17..1e-19 — that is
    //    numerically zero, carrying no information.
    //
    // 2. The golden is not tied to one build profile or CPU. Optimization
    //    changes floating-point codegen (FMA contraction, reassociation), so a
    //    debug-recorded golden differs from a release run in the last few ulp,
    //    and that difference then amplifies through unstable dynamics — a
    //    falling humanoid is a divergent system, so early rounding grows.
    //
    // Both effects are measured, not assumed, across this 100-step trajectory:
    // same profile as the golden, the worst deviation is 9.8e-17 (that is
    // effect 1 alone); debug golden vs release run, 1.5e-9. Pinning a build
    // profile would only move the problem to the first machine with a
    // different architecture — this repo already has a torture-track baseline
    // that differs between x86_64 and aarch64 for exactly this reason.
    //
    // So the tolerance is sized to *physical* meaning: 1e-8 relative on a
    // 0.78 m base height is 8 nanometres, with ~6× headroom over the observed
    // cross-profile figure. Any real physics regression is orders of magnitude
    // larger, and the assertion below reports the worst deviation it saw, so a
    // drift creeping toward the limit is visible before it trips.
    const TOL: f64 = 1e-8;
    let mut worst = 0.0f64;
    let mut worst_where = String::new();
    let mut note = |a: f64, b: f64, what: String| {
        let rel = (a - b).abs() / (1.0 + a.abs().max(b.abs()));
        if rel > worst {
            worst = rel;
            worst_where = format!("{what}: {a} vs {b}");
        }
    };

    for (i, (a, b)) in got.frames.iter().zip(want.frames.iter()).enumerate() {
        assert_eq!(a.step, b.step, "frame {i}: step index diverged");
        assert_eq!(a.done, b.done, "frame {i}: termination diverged");
        assert_eq!(
            a.base_dofs.len(),
            b.base_dofs.len(),
            "frame {i}: base DOF count changed"
        );
        note(
            a.base_height_m,
            b.base_height_m,
            format!("frame {i} base height"),
        );
        note(
            a.base_tilt_deg,
            b.base_tilt_deg,
            format!("frame {i} base tilt"),
        );
        for (k, (x, y)) in a.base_dofs.iter().zip(b.base_dofs.iter()).enumerate() {
            note(*x, *y, format!("frame {i} base DOF {k}"));
        }
    }

    assert!(
        worst <= TOL,
        "trajectory diverged from the golden by {worst:.3e} (limit {TOL:.0e}) at {worst_where}. \
         A deviation this large is a physics change, not codegen noise — if it is intended, \
         regenerate with UPDATE_GOLDEN=1 and say so in the commit."
    );
    eprintln!("golden trajectory matched; worst relative deviation {worst:.3e}");
}

#[test]
fn stepping_is_deterministic_for_a_fixed_seed() {
    // Two independent envs, same seed, same actions: the trajectories must be
    // **bit**-identical. This is what makes every downstream A/B measurement
    // mean something, and it is asserted here rather than against the golden
    // file precisely because there is no JSON in this path to blunt it.
    let a = record(40);
    let b = record(40);
    assert_eq!(a.frames, b.frames, "two runs at seed 1 diverged");
}

#[test]
fn the_humanoid_actually_falls_when_it_is_not_controlled() {
    // Guards against the failure mode where the env silently has no floating
    // base: height reads a constant, nothing ever terminates, and the harness
    // reports a confident full-length episode while measuring nothing.
    let g = record(400);
    let first = g.frames.first().expect("no frames");
    let last = g.frames.last().expect("no frames");
    assert!(
        last.base_height_m < first.base_height_m - 0.05,
        "an uncontrolled humanoid must fall: started {:.3} m, ended {:.3} m",
        first.base_height_m,
        last.base_height_m
    );
    assert!(
        last.done,
        "the episode must terminate once the base drops below the floor"
    );
}

#[test]
fn body_transforms_are_millimeters_and_track_the_base() {
    let doc = g1_document();
    let spec = g1_spec();
    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();
    assert_eq!(vcad_gym_reset(g, 1), 1);

    let n = vcad_gym_body_count(g);
    assert!(n > 0);
    let mut buf = vec![0.0f64; n * 16];
    let written = vcad_gym_body_transforms(g, buf.as_mut_ptr(), buf.len());
    assert_eq!(written, n, "{}", last_error());

    // The pelvis spawns at 780 mm. If the ABI ever forgets the meters→mm
    // conversion this reads 0.78 and the whole robot renders as a speck at the
    // origin — a bug that looks like "the mesh failed to load".
    let mut found = false;
    for i in 0..n {
        let mut len = 0usize;
        let p = vcad_gym_body_id(g, i, &mut len);
        let id = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) });
        if id == "pelvis_inst" {
            let z = buf[i * 16 + 14];
            assert!(
                (700.0..900.0).contains(&z),
                "pelvis z should be ~780 mm, got {z} — meters/mm conversion is wrong"
            );
            assert_eq!(buf[i * 16 + 15], 1.0, "homogeneous row must be [0,0,0,1]");
            found = true;
        }
    }
    assert!(found, "pelvis_inst not among the simulated bodies");

    // An undersized buffer must refuse rather than write out of bounds.
    let mut tiny = vec![0.0f64; 4];
    assert_eq!(
        vcad_gym_body_transforms(g, tiny.as_mut_ptr(), tiny.len()),
        0
    );
    assert!(last_error().contains("need"), "{}", last_error());
}

#[test]
fn a_wrong_length_action_is_rejected_rather_than_padded() {
    let doc = g1_document();
    let spec = g1_spec();
    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();
    let short = vec![0.0f64; 3];
    assert_eq!(vcad_gym_step(g, short.as_ptr(), short.len(), 1), 0);
    let e = last_error();
    assert!(e.contains("action values"), "{e}");
}

#[test]
fn an_unknown_end_effector_is_refused_at_construction() {
    // Left to run, this would give a foot that is permanently airborne: zero pose, never
    // in contact. A policy trained against it learns nothing about contact and
    // the returns look fine.
    let doc = g1_document();
    let spec = serde_json::json!({
        "end_effector_ids": ["no_such_foot_inst"],
        "config": { "base_instance_id": "pelvis_inst" }
    })
    .to_string();
    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(g.is_null(), "an unknown end effector must fail closed");
    let e = last_error();
    assert!(e.contains("no_such_foot_inst"), "{e}");
}

#[test]
fn unknown_pd_gain_joints_are_refused() {
    let doc = g1_document();
    let spec = serde_json::json!({
        "end_effector_ids": [],
        "gains": { "not_a_joint": [200.0, 5.0] },
        "config": { "base_instance_id": "pelvis_inst" }
    })
    .to_string();
    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(g.is_null(), "gains for an unknown joint must fail closed");
    assert!(last_error().contains("not_a_joint"), "{}", last_error());
}

#[test]
fn a_fixed_base_document_is_refused_by_default() {
    // The exact mistake that silently bills an afternoon of training to a
    // document that was never standing up.
    // The SAME humanoid, imported without `--floating-base`: identical
    // geometry and joints, no 6-DOF root. Using the same robot isolates the
    // floating base as the only variable.
    let doc = std::fs::read_to_string(fixtures().join("g1_fixed.vcad"))
        .expect("g1_fixed.vcad fixture missing; regenerate with `vcad import-urdf`");
    let spec = serde_json::json!({ "end_effector_ids": [] }).to_string();
    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(g.is_null(), "a fixed-base document must fail closed");
    let e = last_error();
    assert!(e.contains("floating base"), "{e}");

    // ...and can be opted into explicitly, for a genuinely fixed-base task.
    let spec = serde_json::json!({
        "end_effector_ids": [],
        "require_floating_base": false
    })
    .to_string();
    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(!g.is_null(), "opt-out must work: {}", last_error());
    vcad_gym_free(g);
}

#[test]
fn a_zero_policy_reproduces_the_hold_rest_pose_trajectory() {
    // The zero policy's action is exactly the default pose, so driving the env
    // through the policy path must give the same trajectory as commanding zero
    // targets directly. If it doesn't, the feature/act/step chain has drifted
    // from what training does — the single highest-consequence bug in the
    // inference path, because it degrades a policy silently.
    let doc = g1_document();
    let spec = g1_spec();

    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();
    let policy = vcad_policy_zeros(g, 8.0);
    assert!(!policy.is_null());
    assert_eq!(vcad_policy_check(g, policy), 1, "{}", last_error());
    assert_eq!(vcad_gym_reset(g, 1), 1);
    let mut via_policy = Vec::new();
    for _ in 0..60 {
        assert_eq!(vcad_gym_policy_step(g, policy), 1, "{}", last_error());
        let v = vcad_gym_step_view(g);
        via_policy.push(v.base_height_m);
        if v.done != 0 {
            break;
        }
    }
    vcad_policy_free(policy);

    let direct: Vec<f64> = record(60).frames.iter().map(|f| f.base_height_m).collect();
    assert_eq!(
        via_policy.len(),
        direct.len(),
        "policy path and direct path ended at different steps"
    );
    for (i, (a, b)) in via_policy.iter().zip(direct.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-12,
            "step {i}: policy path {a} != direct path {b}"
        );
    }
}

#[test]
fn a_policy_from_a_different_robot_is_refused() {
    let doc = g1_document();
    let spec = g1_spec();
    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();

    // A policy sized for a 4-feature, 2-action robot.
    let mismatched = serde_json::json!({
        "weights": vec![0.0; 8],
        "obs_dim": 4,
        "act_dim": 2,
        "mean": [0.0, 0.0, 0.0, 0.0],
        "std": [1.0, 1.0, 1.0, 1.0],
        "action": { "default_pose_deg": [0.0, 0.0], "action_scale_deg": 8.0 }
    })
    .to_string();
    let p = vcad_policy_load(mismatched.as_ptr(), mismatched.len());
    assert!(!p.is_null(), "policy should load: {}", last_error());
    assert_eq!(
        vcad_policy_check(g, p),
        0,
        "a policy for a different robot must be refused"
    );
    let e = last_error();
    assert!(e.contains("features"), "{e}");
    // And stepping with it must refuse rather than produce motion.
    assert_eq!(vcad_gym_policy_step(g, p), 0);
    vcad_policy_free(p);
}

#[test]
fn a_shove_changes_the_trajectory_and_a_fixed_base_refuses_one() {
    let doc = g1_document();
    let spec = g1_spec();
    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();
    let action = vec![0.0f64; vcad_gym_action_dim(g)];

    assert_eq!(vcad_gym_reset(g, 1), 1);
    for _ in 0..5 {
        vcad_gym_step(g, action.as_ptr(), action.len(), 1);
    }
    // 1 m/s sideways is a real shove, not a rounding error.
    assert_eq!(
        vcad_gym_nudge_base(g, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        1,
        "{}",
        last_error()
    );
    vcad_gym_step(g, action.as_ptr(), action.len(), 1);
    let shoved = vcad_gym_step_view(g).base_height_m;

    let unshoved = record(6).frames.last().unwrap().base_height_m;
    assert!(
        (shoved - unshoved).abs() > 1e-9,
        "a 1 m/s shove must change the trajectory (got {shoved} vs {unshoved})"
    );
}

/// Silences the unused-import warning for `c_void` on builds where no test
/// happens to need it; keeping the import documents that these handles are
/// opaque pointers on the Swift side.
#[allow(dead_code)]
fn _opaque(_: *mut c_void) {}

#[test]
fn scene_binding_delivers_simulated_poses_in_scene_order() {
    // The render seam, end to end — and the only place the two index spaces
    // meet. The physics world orders bodies by sorted instance id; the scene
    // orders instances by document order. They are *different orderings of the
    // same set*, so a renderer that indexed one with the other would draw every
    // limb attached to the wrong link: a robot that moves plausibly and is
    // completely wrong. Nothing else in this file exercises that mapping.
    let doc = g1_document();
    let spec = g1_spec();

    let scene = vcad_ffi::vcad_scene_from_json(doc.as_ptr(), doc.len());
    assert!(!scene.is_null(), "scene failed to evaluate");
    let instance_count = vcad_ffi::vcad_scene_instance_count(scene);
    assert!(instance_count > 0, "fixture must be an assembly");

    let gym = Gym::open(&doc, &spec);
    let g = gym.as_ptr();

    let matched = vcad_gym_bind_scene(g, scene);
    assert_eq!(
        matched, instance_count,
        "every scene instance in this fixture has a physics body"
    );
    assert_eq!(vcad_gym_scene_binding_len(g), instance_count);

    // Seed with the authored transforms, exactly as the app does, so an
    // instance the simulation doesn't own keeps its pose.
    let mut authored = vec![0.0f64; instance_count * 16];
    for i in 0..instance_count {
        let mut m = [0.0f64; 16];
        assert_eq!(
            vcad_ffi::vcad_scene_instance_transform(scene, i, m.as_mut_ptr()),
            1
        );
        authored[i * 16..i * 16 + 16].copy_from_slice(&m);
    }

    assert_eq!(vcad_gym_reset(g, 1), 1);
    let mut buf = authored.clone();
    assert_eq!(
        vcad_gym_scene_transforms(g, buf.as_mut_ptr(), buf.len()),
        instance_count,
        "{}",
        last_error()
    );

    // Locate the pelvis by SCENE index, then check that slot carries the
    // pelvis's simulated pose. If the two orderings were confused this would
    // read some other link's height.
    let pelvis_scene_index = (0..instance_count)
        .find(|&i| {
            let mut len = 0usize;
            let p = vcad_ffi::vcad_scene_instance_id(scene, i, &mut len);
            !p.is_null()
                && std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, len) }).unwrap()
                    == "pelvis_inst"
        })
        .expect("pelvis_inst not in the scene");

    let z = buf[pelvis_scene_index * 16 + 14];
    assert!(
        (700.0..900.0).contains(&z),
        "pelvis should be near its 780 mm spawn in SCENE order, got {z} mm at index \
         {pelvis_scene_index} — the body/scene index mapping is wrong"
    );

    // Stepping must actually move the transforms, or the render seam is
    // publishing a frozen pose while the simulation advances behind it.
    let actions = vec![0.0f64; vcad_gym_action_dim(g)];
    for _ in 0..25 {
        assert_eq!(vcad_gym_step(g, actions.as_ptr(), actions.len(), 1), 1);
    }
    let mut after = authored.clone();
    assert_eq!(
        vcad_gym_scene_transforms(g, after.as_mut_ptr(), after.len()),
        instance_count
    );
    let z_after = after[pelvis_scene_index * 16 + 14];
    assert!(
        z_after < z - 1.0,
        "the pelvis must fall: {z} mm -> {z_after} mm"
    );

    // Every transform must stay a well-formed affine 4x4.
    for i in 0..instance_count {
        assert_eq!(after[i * 16 + 15], 1.0, "instance {i}: bad homogeneous row");
        for k in [3, 7, 11] {
            assert_eq!(after[i * 16 + k], 0.0, "instance {i}: bad column {k}");
        }
    }

    vcad_ffi::vcad_scene_free(scene);
}

#[test]
fn gains_on_a_non_actuated_joint_are_refused_not_panicked() {
    // Found by pressing Simulate on a robot with a Fixed joint. Installing PD
    // gains on a zero-DOF joint reached a kernel probe that indexed one past
    // the end of the control vector and panicked — caught by catch_unwind, so
    // it surfaced only as an opaque "panic in kernel".
    //
    // Two defences, both exercised here: the kernel no longer panics (there is
    // nothing to measure on a jointless DOF, so it returns the neutral value),
    // and this ABI refuses the gain up front with a message that names the
    // actual problem instead of accepting it and hoping.
    let doc = g1_document();
    let spec = serde_json::json!({
        "end_effector_ids": [],
        "gains": { "waist_yaw_fixed_joint": [200.0, 5.0] },
        "config": { "base_instance_id": "pelvis_inst" }
    })
    .to_string();
    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());

    // The fixture may or may not name that joint; either refusal is correct,
    // but it must be a *described* refusal and never a panic.
    assert!(
        g.is_null(),
        "gains on a non-actuated joint must fail closed"
    );
    let e = last_error();
    assert!(
        !e.contains("panic"),
        "must be a described refusal, not a panic: {e}"
    );
    assert!(
        e.contains("actuated") || e.contains("unknown joint"),
        "message should name the problem: {e}"
    );
}
