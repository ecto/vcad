//! Top-level grader entry point.
//!
//! Loads a task and a candidate `.vcad` file, dispatches each check, and
//! returns a [`RunBlob`]. v0.0 skeleton: every kernel-dependent check
//! returns [`CheckOutcome::NotImplemented`]. Wiring the dispatch to the
//! `vcad-kernel-*` crates is the next step.

use crate::blob::{CheckOutcome, CheckRecord, RunBlob, Summary, SCHEMA_VERSION};
use crate::check::CheckSpec;
use crate::eval::{evaluate_vcad, EvalSnapshot};
use crate::task::Task;
use serde_json::json;
use std::path::Path;
use thiserror::Error;

/// Errors the grader can surface to its caller.
#[derive(Debug, Error)]
pub enum GraderError {
    /// Candidate `.vcad` file could not be read.
    #[error("could not read .vcad: {0}")]
    Io(#[from] std::io::Error),
    /// Candidate `.vcad` is not valid JSON.
    #[error("malformed .vcad: {0}")]
    Vcad(#[from] serde_json::Error),
    /// Task JSON could not be hashed.
    #[error("task hash failed: {0}")]
    HashFailed(String),
}

/// Grade a candidate `.vcad` file against a task.
///
/// `task_json_bytes` is the raw bytes of the task JSON the grader was loaded
/// from — used to compute `task_sha256` for forensic traceability. (We hash
/// the bytes the grader actually saw, not the file at re-read time, to
/// avoid a TOCTOU window.)
pub fn grade(
    task: &Task,
    task_json_bytes: &[u8],
    candidate_vcad: &Path,
) -> Result<RunBlob, GraderError> {
    // Read + sanity-check the candidate.
    let candidate_raw = std::fs::read_to_string(candidate_vcad)?;
    let _candidate_value: serde_json::Value = serde_json::from_str(&candidate_raw)?;

    // Evaluate once up front; every check shares the snapshot.
    let snapshot = evaluate_vcad(&candidate_raw);

    let mut records: Vec<CheckRecord> = Vec::with_capacity(task.checks.len());
    for (n, spec) in task.checks.iter().enumerate() {
        let (outcome, details) = run_check(spec, &snapshot);
        records.push(CheckRecord {
            n,
            r#type: spec.kind().to_string(),
            params: spec.clone(),
            result: outcome,
            details,
        });
    }

    // Anti-cheese + limits enforcement is wired by the harness, which
    // sees token counts, tool-call counts, and wall-clock. The grader
    // proper just receives that as input later. v0.0 reports "not yet
    // checked" — false here so it doesn't force a fail.
    let anti_cheese_violated = false;
    let limits_exceeded: Vec<String> = Vec::new();

    let summary = Summary::from_records(&records, anti_cheese_violated, limits_exceeded);
    let task_sha256 = sha256_hex(task_json_bytes);

    Ok(RunBlob {
        schema_version: SCHEMA_VERSION,
        task_id: task.id.clone(),
        task_sha256,
        checks: records,
        summary,
    })
}

/// Dispatch one check against the prebuilt evaluation snapshot. Checks
/// that haven't been wired yet return [`CheckOutcome::NotImplemented`].
fn run_check(spec: &CheckSpec, snapshot: &EvalSnapshot) -> (CheckOutcome, serde_json::Value) {
    let stub_reason = "skeleton — kernel wiring pending";
    match spec {
        CheckSpec::ValidSolid => check_valid_solid(snapshot),

        CheckSpec::Bbox {
            min,
            max,
            tolerance_mm,
        } => check_bbox(snapshot, *min, *max, *tolerance_mm),

        CheckSpec::MassProps { .. }
        | CheckSpec::HoleCount { .. }
        | CheckSpec::HolePositions { .. }
        | CheckSpec::FilletRadius { .. }
        | CheckSpec::StepRoundtrip { .. }
        | CheckSpec::DrcClean
        | CheckSpec::ErcClean
        | CheckSpec::Dfm { .. }
        | CheckSpec::RefactorInvariant { .. } => (
            CheckOutcome::NotImplemented,
            json!({ "reason": stub_reason }),
        ),

        CheckSpec::BodyValid
        | CheckSpec::FkReaches { .. }
        | CheckSpec::TorqueBudget { .. }
        | CheckSpec::StableDuringRollout { .. }
        | CheckSpec::TaskSuccess { .. } => (
            CheckOutcome::NotImplemented,
            json!({ "reason": stub_reason, "needs": "vcad-gym (phyz + tang)" }),
        ),
    }
}

/// `valid_solid`: candidate evaluates cleanly, has at least one root, and
/// at least one root produces a non-empty solid with positive volume.
fn check_valid_solid(snap: &EvalSnapshot) -> (CheckOutcome, serde_json::Value) {
    if let Some(fatal) = &snap.fatal {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "fatal evaluation error", "error": fatal }),
        );
    }
    if !snap.root_failures.is_empty() {
        // A partially-failed eval is still a fail, but distinguishable from
        // a clean-but-empty doc.
        return (
            CheckOutcome::Fail,
            json!({
                "reason": "one or more roots failed to evaluate",
                "root_failures": snap.root_failures,
                "solids_produced": snap.solids.len(),
            }),
        );
    }
    if snap.solids.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no solids produced", "root_count": snap.root_count }),
        );
    }
    if !snap.has_any_valid_solid() {
        return (
            CheckOutcome::Fail,
            json!({
                "reason": "evaluated but every solid is empty or zero-volume",
                "solids_produced": snap.solids.len(),
            }),
        );
    }
    (
        CheckOutcome::Pass,
        json!({
            "solids_produced": snap.solids.len(),
            "root_count": snap.root_count,
        }),
    )
}

/// `bbox`: aggregate AABB across all evaluated solids must match the spec
/// within `tolerance_mm` on every face (six bounds total).
fn check_bbox(
    snap: &EvalSnapshot,
    spec_min: [f64; 3],
    spec_max: [f64; 3],
    tolerance_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let (actual_min, actual_max) = match snap.aggregate_bbox() {
        Some(b) => b,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no valid solid to measure" }),
            );
        }
    };

    let dev_min: [f64; 3] = [
        actual_min[0] - spec_min[0],
        actual_min[1] - spec_min[1],
        actual_min[2] - spec_min[2],
    ];
    let dev_max: [f64; 3] = [
        actual_max[0] - spec_max[0],
        actual_max[1] - spec_max[1],
        actual_max[2] - spec_max[2],
    ];

    let max_abs_dev = dev_min
        .iter()
        .chain(dev_max.iter())
        .fold(0.0_f64, |acc, d| acc.max(d.abs()));

    let outcome = if max_abs_dev <= tolerance_mm {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };

    (
        outcome,
        json!({
            "actual_min": actual_min,
            "actual_max": actual_max,
            "deviation_min": dev_min,
            "deviation_max": dev_max,
            "max_abs_deviation_mm": max_abs_dev,
            "tolerance_mm": tolerance_mm,
        }),
    )
}

/// SHA-256 hex of arbitrary bytes. Tiny pure-Rust implementation so we
/// don't pull in a crypto crate just for this.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut s = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

// Minimal in-tree SHA-256 to avoid a crypto dep in v0.0. Replace with
// a vetted crate (`sha2`) once the grader gains real wiring.
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// A 25mm cube .vcad as a JSON string — used by valid_solid + bbox +
    /// mass_props tests below.
    fn cube_vcad(size: f64) -> String {
        format!(
            r#"{{"version":"0.1","nodes":{{"1":{{"id":1,"name":"cube","op":{{"type":"Cube","size":{{"x":{s},"y":{s},"z":{s}}}}}}}}},"materials":{{}},"part_materials":{{}},"roots":[{{"root":1,"material":"default"}}]}}"#,
            s = size
        )
    }

    fn task_with(checks: Vec<CheckSpec>) -> (Task, Vec<u8>) {
        let task = Task {
            id: "test-1".into(),
            suite: crate::task::Suite::A,
            tier: "A1".into(),
            title: "t".into(),
            prompt: "p".into(),
            inputs: vec![],
            checks,
            anti_cheese: Default::default(),
            limits: Default::default(),
            pass_k: 5,
            tags: vec![],
        };
        let bytes = serde_json::to_vec(&task).unwrap();
        (task, bytes)
    }

    fn write_tmp_vcad(name: &str, contents: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn unwired_checks_still_return_not_implemented() {
        // DRC is still unwired; this catches accidental dispatch leaks.
        let (task, task_bytes) = task_with(vec![CheckSpec::DrcClean]);
        let tmp = write_tmp_vcad("mecheval-unwired.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.checks.len(), 1);
        assert_eq!(blob.checks[0].result, CheckOutcome::NotImplemented);
    }

    #[test]
    fn valid_solid_passes_for_a_real_cube() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad("mecheval-valid-cube.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Pass);
        assert!(blob.summary.passed);
    }

    #[test]
    fn valid_solid_fails_for_empty_doc() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad(
            "mecheval-empty-doc.vcad",
            r#"{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}"#,
        );
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
        assert!(!blob.summary.passed);
    }

    #[test]
    fn bbox_passes_when_within_tolerance() {
        // Cube primitive's corner is at origin per the IR convention (a
        // 10mm cube spans 0..10 on each axis).
        let (task, task_bytes) = task_with(vec![CheckSpec::Bbox {
            min: [0.0, 0.0, 0.0],
            max: [10.0, 10.0, 10.0],
            tolerance_mm: 0.05,
        }]);
        let tmp = write_tmp_vcad("mecheval-bbox-pass.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Pass, "{:?}", blob.checks[0].details);
    }

    #[test]
    fn bbox_fails_when_off() {
        // Same 10mm cube, but spec says 50mm — should fail with deviations.
        let (task, task_bytes) = task_with(vec![CheckSpec::Bbox {
            min: [0.0, 0.0, 0.0],
            max: [50.0, 50.0, 50.0],
            tolerance_mm: 0.1,
        }]);
        let tmp = write_tmp_vcad("mecheval-bbox-fail.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
        let dev = blob.checks[0].details["max_abs_deviation_mm"].as_f64().unwrap();
        assert!(dev > 30.0, "expected large deviation, got {}", dev);
    }

    #[test]
    fn task_sha256_has_64_hex_chars() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad("mecheval-hash-len.vcad", &cube_vcad(5.0));
        let blob = grade(&task, &task_bytes, &tmp).expect("grade");
        assert_eq!(blob.task_sha256.len(), 64);
    }
}
