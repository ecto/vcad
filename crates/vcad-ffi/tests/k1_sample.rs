//! The vendored Booster K1 sample: it must render *and* simulate.
//!
//! Both halves matter and they fail independently. A URDF whose meshes are
//! missing still simulates — mass, COM and inertia come from the authored
//! `<inertial>` blocks — but renders nothing, and in the app an assembly whose
//! every instance mesh is empty is indistinguishable from a document with no
//! assembly at all. This is the fixture where that distinction is checked.

use std::path::{Path, PathBuf};

use vcad_ffi::gym::*;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample() -> (String, String) {
    let path = repo().join("examples/k1-floating.vcad");
    let doc = std::fs::read_to_string(&path).expect("k1-floating.vcad missing");
    let dir = path.parent().unwrap().to_string_lossy().into_owned();
    (doc, dir)
}

fn last_error() -> String {
    let mut len = 0usize;
    let p = vcad_ffi::vcad_last_error(&mut len);
    if p.is_null() {
        return "<no error>".into();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
}

/// Every mesh the sample references must exist on disk, by exact name.
///
/// This is the check that would have caught the meshes being absent from the
/// repository. They were dropped by the root `.gitignore`'s `*.stl` rule —
/// which matches `Trunk.STL` on macOS, where git sets `core.ignorecase=true` —
/// so `git add third_party/` skipped all 24 files silently, and the tests below
/// still passed locally because the files were there, just untracked. The
/// sample rendered nothing on every other machine.
///
/// Exact-case comparison on purpose: a case-insensitive filesystem will happily
/// open `trunk.stl` for `Trunk.STL`, so a case mismatch is another bug that
/// only appears on Linux.
#[test]
fn every_referenced_mesh_is_present_by_exact_name() {
    let (doc, dir) = sample();
    let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
    let base = Path::new(&dir);
    let mut checked = 0;
    for node in v["nodes"].as_object().unwrap().values() {
        if node["op"]["type"] != "mesh_import" {
            continue;
        }
        let rel = node["op"]["path"].as_str().unwrap();
        let full = base.join(rel);
        assert!(
            full.is_file(),
            "{rel} is referenced by the sample but not on disk (looked at {}). \
             If it exists locally but not in a fresh clone, it was swallowed by \
             a .gitignore rule — check `git check-ignore -v` on it.",
            full.display()
        );
        // Exact case: readdir the parent and match byte-for-byte.
        let name = full.file_name().unwrap().to_string_lossy().into_owned();
        let parent = full.parent().unwrap();
        let exact = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy() == name);
        assert!(
            exact,
            "{name} resolves only case-insensitively — it will not open on Linux"
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "expected the K1's mesh imports, found {checked}"
    );
}

#[test]
fn the_sample_references_its_meshes_relatively() {
    // An absolute path here resolves on exactly one machine. This is the
    // property that makes the document committable at all.
    let (doc, _) = sample();
    let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
    let mut seen = 0;
    for node in v["nodes"].as_object().unwrap().values() {
        if node["op"]["type"] == "mesh_import" {
            let p = node["op"]["path"].as_str().unwrap();
            assert!(
                !Path::new(p).is_absolute(),
                "mesh path {p:?} is absolute — it will only resolve on the machine \
                 that ran the import. Re-import with --relative-meshes."
            );
            seen += 1;
        }
    }
    assert!(seen >= 20, "expected the K1's mesh imports, found {seen}");
}

#[test]
fn the_sample_renders_real_geometry() {
    let (doc, dir) = sample();
    let scene = vcad_ffi::vcad_scene_from_json_in(doc.as_ptr(), doc.len(), dir.as_ptr(), dir.len());
    assert!(!scene.is_null(), "scene failed to evaluate");
    let n = vcad_ffi::vcad_scene_instance_count(scene);
    assert!(n > 20, "expected the K1's instances, got {n}");

    let mut with_geometry = 0;
    let mut triangles = 0usize;
    for i in 0..n {
        let m = vcad_ffi::vcad_scene_instance_mesh(scene, i);
        if m.indices_len >= 3 {
            with_geometry += 1;
            triangles += m.indices_len / 3;
        }
    }
    // The whole point of vendoring. Without the meshes on disk this is 0, the
    // viewport is empty, and the app hides the Simulate affordance entirely.
    assert!(
        with_geometry >= 20,
        "only {with_geometry}/{n} instances have geometry — the vendored meshes \
         are not resolving (check third_party/booster-k1/meshes)"
    );
    assert!(
        triangles > 50_000,
        "expected the K1's real meshes, got {triangles} triangles — that looks \
         like placeholder geometry"
    );
    vcad_ffi::vcad_scene_free(scene);
}

#[test]
fn the_sample_simulates_and_stands_briefly_before_it_falls() {
    // Uncontrolled, the K1 is an unstable equilibrium: it should hold for a
    // moment and then go down. Standing indefinitely would mean it is welded to
    // the world; falling on step one would mean it spawned below its own
    // termination floor. Both are silent failures that make training
    // meaningless, so the window is asserted from both sides.
    let (doc, dir) = sample();
    let spec = serde_json::json!({
        "end_effector_ids": ["left_foot_link_inst", "right_foot_link_inst"],
        "dt": 1.0 / 1000.0,
        "substeps": 20,
        "max_steps": 400,
        "nominal_height_m": 0.5498,
        "base_dir": dir,
        "config": {
            "base_instance_id": "Trunk_inst",
            "termination": { "base_height_below": 0.42, "base_tilt_above_deg": 35.0 }
        }
    })
    .to_string();

    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(!g.is_null(), "gym failed: {}", last_error());
    assert_eq!(vcad_gym_action_dim(g), 22, "the K1 has 22 actuated joints");
    assert_eq!(vcad_gym_reset(g, 1), 1);

    let hold = vec![0.0f64; vcad_gym_action_dim(g)];
    let mut steps = 0u32;
    for _ in 0..400 {
        assert_eq!(vcad_gym_step(g, hold.as_ptr(), hold.len(), 1), 1);
        let v = vcad_gym_step_view(g);
        steps = v.step;
        if v.done != 0 {
            break;
        }
    }
    assert!(
        steps > 3,
        "the K1 fell on step {steps} — it is spawning below its own termination \
         floor, which makes every rollout measure the spawn rather than the policy"
    );
    assert!(
        steps < 400,
        "the K1 survived a full uncontrolled episode — it is not actually free \
         to fall, so no policy trained here would be learning to balance"
    );
    vcad_gym_free(g);
}

#[test]
fn the_shipped_policy_makes_the_k1_stand() {
    // The end-to-end claim: load the shipped policy through the inference path
    // the app uses, in an env configured the way the app auto-configures it,
    // and the K1 must survive a full episode instead of the ~60 steps it
    // manages uncontrolled.
    //
    // This is the test that would catch a policy/plant mismatch — a changed
    // spawn height, different gains, a renamed joint. All of those produce a
    // robot that falls, not an error.
    let (doc, dir) = sample();
    let policy_json = std::fs::read_to_string(repo().join("examples/k1-stand.vcadpolicy"))
        .expect("k1-stand.vcadpolicy missing");

    let spec = serde_json::json!({
        "end_effector_ids": ["left_foot_link_inst", "right_foot_link_inst"],
        "dt": 1.0 / 1000.0,
        "substeps": 20,
        "max_steps": 400,
        "nominal_height_m": 0.5498,
        "base_dir": dir,
        // The gains the policy trained under. booster_gym's schedule, and the
        // same one the app derives from the joint names.
        "gains": {
            "Left_Hip_Pitch": [200.0, 5.0], "Left_Hip_Roll": [200.0, 5.0],
            "Left_Hip_Yaw": [200.0, 5.0], "Left_Knee_Pitch": [200.0, 5.0],
            "Right_Hip_Pitch": [200.0, 5.0], "Right_Hip_Roll": [200.0, 5.0],
            "Right_Hip_Yaw": [200.0, 5.0], "Right_Knee_Pitch": [200.0, 5.0],
            "Left_Ankle_Pitch": [50.0, 1.0], "Left_Ankle_Roll": [50.0, 1.0],
            "Right_Ankle_Pitch": [50.0, 1.0], "Right_Ankle_Roll": [50.0, 1.0]
        },
        "config": {
            "base_instance_id": "Trunk_inst",
            "termination": { "base_height_below": 0.42, "base_tilt_above_deg": 35.0 }
        }
    })
    .to_string();

    let g = vcad_gym_create(doc.as_ptr(), doc.len(), spec.as_ptr(), spec.len());
    assert!(!g.is_null(), "gym failed: {}", last_error());
    let p = vcad_policy_load(policy_json.as_ptr(), policy_json.len());
    assert!(!p.is_null(), "policy failed to load: {}", last_error());
    assert_eq!(
        vcad_policy_check(g, p),
        1,
        "the shipped policy does not match the shipped robot: {}",
        last_error()
    );

    let mut survived = Vec::new();
    for seed in 1..=3u64 {
        assert_eq!(vcad_gym_reset(g, seed), 1);
        let mut steps = 0;
        for _ in 0..400 {
            assert_eq!(vcad_gym_policy_step(g, p), 1, "{}", last_error());
            let v = vcad_gym_step_view(g);
            steps = v.step;
            if v.done != 0 {
                break;
            }
        }
        survived.push(steps);
    }
    vcad_policy_free(p);
    vcad_gym_free(g);

    // Uncontrolled the K1 manages roughly 60 steps. Anything near that means
    // the policy is not actually driving the robot it was trained on.
    let worst = *survived.iter().min().unwrap();
    assert!(
        worst >= 400,
        "the shipped policy did not hold a full episode: {survived:?} steps \
         (uncontrolled is ~60). The policy and the plant have diverged."
    );
}
