//! End-to-end checks for the in-process trainer.
//!
//! These run real ARS on a real 23-DOF humanoid, so they are deliberately
//! tiny — a handful of iterations at a short episode length. They are not
//! measuring learning (a few iterations cannot), they are proving the
//! machinery: that a run starts, publishes progress, produces a loadable
//! policy bundle with correct provenance, cancels promptly, and refuses
//! configurations that would waste an hour before failing.

use std::path::{Path, PathBuf};

use vcad_ffi::gym::*;
use vcad_ffi::train::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn g1_document() -> String {
    std::fs::read_to_string(fixtures().join("g1_floating.vcad")).expect("fixture missing")
}

fn last_error() -> String {
    let mut len = 0usize;
    let p = vcad_ffi::vcad_last_error(&mut len);
    if p.is_null() {
        return "<no error>".to_string();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
}

/// A gym spec whose `nominal_height_m` matches the reward's, and whose
/// episodes are short enough to train a few iterations in seconds.
fn gym_spec(max_steps: u32) -> serde_json::Value {
    serde_json::json!({
        "end_effector_ids": ["left_ankle_roll_link_inst", "right_ankle_roll_link_inst"],
        "dt": 1.0 / 1000.0,
        "substeps": 20,
        "max_steps": max_steps,
        "nominal_height_m": 0.78,
        "config": {
            "termination": { "base_height_below": 0.4, "base_tilt_above_deg": 45.0 },
            "base_instance_id": "pelvis_inst"
        }
    })
}

fn reward_spec() -> serde_json::Value {
    serde_json::json!({ "nominal_height_m": 0.78 })
}

fn train_spec(iterations: usize) -> serde_json::Value {
    serde_json::json!({
        "ars": {
            "n_directions": 2, "top_k": 1, "step_size": 0.005,
            "noise_std": 0.05, "iterations": iterations,
            "rollouts_per_eval": 1, "seed": 7
        },
        "policy": "linear",
        "action_scale_deg": 8.0,
        "curriculum_warmup": 0.0,
        "held_out_every": 1,
        "held_out_seeds": 2
    })
}

struct Trainer(*mut VcadTrainer);

impl Trainer {
    fn start(gym: &str, train: &str, reward: &str) -> Self {
        let doc = g1_document();
        let t = vcad_train_start(
            doc.as_ptr(),
            doc.len(),
            gym.as_ptr(),
            gym.len(),
            train.as_ptr(),
            train.len(),
            reward.as_ptr(),
            reward.len(),
        );
        assert!(!t.is_null(), "vcad_train_start failed: {}", last_error());
        Self(t)
    }

    fn poll(&self) -> VcadTrainProgress {
        let mut p = VcadTrainProgress::default();
        assert_eq!(vcad_train_poll(self.0, &mut p), 1);
        p
    }

    /// Block until the run finishes, or panic after `secs`.
    fn wait(&self, secs: u64) -> VcadTrainProgress {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            let p = self.poll();
            if p.finished == 1 {
                return p;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "training did not finish within {secs}s (iteration {}/{})",
                p.iteration,
                p.total_iterations
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn best_bundle(&self) -> Option<String> {
        // The documented two-call protocol: size, then copy.
        let need = vcad_train_best_policy_json(self.0, std::ptr::null_mut(), 0);
        assert_eq!(need, 0, "sizing call must return 0");
        let msg = last_error();
        if msg.contains("no policy has been scored") {
            return None;
        }
        let n: usize = msg
            .split_whitespace()
            .nth(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("could not parse required size from {msg:?}"));
        let mut buf = vec![0u8; n];
        let written = vcad_train_best_policy_json(self.0, buf.as_mut_ptr(), buf.len());
        assert_eq!(written, n, "{}", last_error());
        Some(String::from_utf8(buf).expect("bundle is not UTF-8"))
    }
}

impl Drop for Trainer {
    fn drop(&mut self) {
        vcad_train_free(self.0);
    }
}

#[test]
fn a_short_run_completes_and_produces_a_loadable_bundle() {
    let t = Trainer::start(
        &gym_spec(30).to_string(),
        &train_spec(3).to_string(),
        &reward_spec().to_string(),
    );

    let done = t.wait(180);
    assert_eq!(done.failed, 0, "training failed: {}", {
        let mut len = 0usize;
        let p = vcad_train_error(t.0, &mut len);
        if p.is_null() {
            "<none>".to_string()
        } else {
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
        }
    });
    assert_eq!(done.cancelled, 0);
    assert_eq!(
        done.iteration, 3,
        "all iterations should have been reported"
    );
    assert!(
        done.best_held_out.is_finite(),
        "a completed run must have scored at least one iterate on held-out seeds"
    );

    let bundle = t.best_bundle().expect("no bundle produced");
    let parsed: serde_json::Value = serde_json::from_str(&bundle).unwrap();

    // Provenance is the point of the bundle: without it a policy cannot be
    // told apart from one trained against a different plant.
    assert_eq!(parsed["version"], 1);
    assert!(parsed["document_hash"]
        .as_str()
        .unwrap()
        .starts_with("fnv1a64:"));
    assert_eq!(parsed["env"]["nominal_height_m"], 0.78);
    assert_eq!(parsed["reward"]["nominal_height_m"], 0.78);
    assert_eq!(parsed["env"]["substeps"], 20);
    assert_eq!(parsed["held_out_seeds"], 2);
    assert!(
        parsed["policy"]["weights"].is_array(),
        "policy weights missing"
    );

    // And it must load back through the inference path, dimensionally matched
    // to the env it was trained in — the round trip the app performs.
    let doc = g1_document();
    let spec = gym_spec(30).to_string();
    let gym = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(!gym.is_null(), "{}", last_error());
    let policy = vcad_policy_load_bundle(bundle.as_ptr(), bundle.len(), doc.as_ptr(), doc.len());
    assert!(!policy.is_null(), "bundle did not load: {}", last_error());
    assert_eq!(
        vcad_policy_check(gym, policy),
        1,
        "bundle policy must match its own env: {}",
        last_error()
    );
    assert_eq!(vcad_gym_policy_step(gym, policy), 1, "{}", last_error());
    vcad_policy_free(policy);
    vcad_gym_free(gym);
}

#[test]
fn an_edited_document_marks_a_policy_stale() {
    // The receipt behaviour: the policy still loads (running a stale policy is
    // the user's call) but the drift is reported, so the app can show Stale
    // instead of silently claiming a score that describes a different robot.
    let t = Trainer::start(
        &gym_spec(20).to_string(),
        &train_spec(1).to_string(),
        &reward_spec().to_string(),
    );
    t.wait(120);
    let bundle = t.best_bundle().expect("no bundle");

    // Edit something the IR actually models. `document_hash` is a SEMANTIC
    // hash — it digests the parsed-and-re-serialized Document — so adding a
    // key the schema ignores would (correctly) not move it. Moving the
    // floating base's spawn height changes the plant, which is precisely the
    // class of edit that invalidates a policy.
    let mut doc: serde_json::Value = serde_json::from_str(&g1_document()).unwrap();
    let joints = doc["joints"].as_array_mut().expect("fixture has joints");
    let free = joints
        .iter_mut()
        .find(|j| j["kind"]["type"] == "Free")
        .expect("fixture has a Free base joint");
    free["parentAnchor"]["z"] = serde_json::json!(910.0);
    let edited = serde_json::to_string(&doc).unwrap();
    assert_ne!(
        vcad_ffi::train::document_hash(&vcad_ir::Document::from_json(&edited).unwrap()),
        vcad_ffi::train::document_hash(&vcad_ir::Document::from_json(&g1_document()).unwrap()),
        "the test edit must actually be semantic, or this proves nothing"
    );

    let policy =
        vcad_policy_load_bundle(bundle.as_ptr(), bundle.len(), edited.as_ptr(), edited.len());
    assert!(
        !policy.is_null(),
        "a stale policy must still load — staleness is a judgement, not a load error"
    );
    let e = last_error();
    assert!(e.contains("STALE"), "drift must be reported: {e}");
    vcad_policy_free(policy);

    // Unedited: no staleness reported.
    let original = g1_document();
    let policy = vcad_policy_load_bundle(
        bundle.as_ptr(),
        bundle.len(),
        original.as_ptr(),
        original.len(),
    );
    assert!(!policy.is_null());
    assert!(
        !last_error().contains("STALE"),
        "the unedited document must not be reported stale: {}",
        last_error()
    );
    vcad_policy_free(policy);
}

#[test]
fn cancelling_stops_the_run_promptly() {
    // Enough iterations that it cannot finish on its own inside the timeout,
    // so a pass really does mean cancellation worked.
    let t = Trainer::start(
        &gym_spec(40).to_string(),
        &train_spec(10_000).to_string(),
        &reward_spec().to_string(),
    );
    // Let it get going so we're cancelling a running trainer, not a starting one.
    let start = std::time::Instant::now();
    while t.poll().iteration == 0 && start.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    vcad_train_stop(t.0);
    let p = t.wait(120);
    assert_eq!(p.finished, 1);
    assert_eq!(p.cancelled, 1, "the run must report that it was cancelled");
    assert!(
        p.iteration < 10_000,
        "cancellation must stop early, got {}",
        p.iteration
    );
}

#[test]
fn a_reward_height_that_disagrees_with_the_env_is_refused_before_training() {
    // Catching this at start costs a millisecond; not catching it costs the
    // whole run, because the policy is told it is upright at one height while
    // being rewarded for another and simply never converges.
    let doc = g1_document();
    let gym = gym_spec(30).to_string();
    let train = train_spec(1).to_string();
    let reward = serde_json::json!({ "nominal_height_m": 0.55 }).to_string();
    let t = vcad_train_start(
        doc.as_ptr(),
        doc.len(),
        gym.as_ptr(),
        gym.len(),
        train.as_ptr(),
        train.len(),
        reward.as_ptr(),
        reward.len(),
    );
    assert!(t.is_null(), "mismatched nominal heights must fail closed");
    let e = last_error();
    assert!(e.contains("nominal_height_m"), "{e}");
}

#[test]
fn an_impossible_ars_config_is_refused() {
    let doc = g1_document();
    let gym = gym_spec(30).to_string();
    let reward = reward_spec().to_string();
    // top_k > n_directions: the update would average more directions than were
    // sampled.
    let train = serde_json::json!({
        "ars": { "n_directions": 2, "top_k": 8, "iterations": 1 },
        "policy": "linear"
    })
    .to_string();
    let t = vcad_train_start(
        doc.as_ptr(),
        doc.len(),
        gym.as_ptr(),
        gym.len(),
        train.as_ptr(),
        train.len(),
        reward.as_ptr(),
        reward.len(),
    );
    assert!(t.is_null());
    assert!(last_error().contains("top_k"), "{}", last_error());

    // And an unknown architecture, rather than silently defaulting to one.
    let train = serde_json::json!({ "policy": "transformer" }).to_string();
    let t = vcad_train_start(
        doc.as_ptr(),
        doc.len(),
        gym.as_ptr(),
        gym.len(),
        train.as_ptr(),
        train.len(),
        reward.as_ptr(),
        reward.len(),
    );
    assert!(t.is_null());
    assert!(last_error().contains("transformer"), "{}", last_error());
}

#[test]
fn the_reward_can_be_evaluated_live_against_a_running_gym() {
    let doc = g1_document();
    let spec = gym_spec(400).to_string();
    let gym = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(!gym.is_null(), "{}", last_error());
    let reward = reward_spec().to_string();

    // Before any step there is nothing to score.
    assert_eq!(vcad_gym_reset(gym, 1), 1);
    assert_eq!(vcad_gym_reward(gym, reward.as_ptr(), reward.len()), 0.0);

    // A humanoid still near its spawn height should score close to the alive
    // bonus; after it has collapsed, strictly less.
    let actions = vec![0.0f64; vcad_gym_action_dim(gym)];
    vcad_gym_step(gym, actions.as_ptr(), actions.len(), 1);
    let early = vcad_gym_reward(gym, reward.as_ptr(), reward.len());
    assert!(early.is_finite() && early > 0.9, "early reward {early}");

    let mut late = early;
    for _ in 0..200 {
        if vcad_gym_step(gym, actions.as_ptr(), actions.len(), 1) == 0 {
            break;
        }
        late = vcad_gym_reward(gym, reward.as_ptr(), reward.len());
        if vcad_gym_step_view(gym).done != 0 {
            break;
        }
    }
    assert!(
        late < early,
        "a falling robot must score worse than a standing one ({late} !< {early})"
    );
    vcad_gym_free(gym);
}
