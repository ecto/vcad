//! Run blob — the immutable per-attempt result format.
//!
//! See `mecheval/runs/SCHEMA.md` for the full schema. This module only
//! captures the subset the grader itself produces (the harness fills in
//! model identity, prompt, and tool-call trace before persisting).

use crate::check::CheckSpec;
use serde::{Deserialize, Serialize};

/// Schema version for run blobs. Bump on breaking changes.
pub const SCHEMA_VERSION: u32 = 0;

/// One check's result, recorded in the run blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRecord {
    /// Index in the task's `checks` array.
    pub n: usize,
    /// Check kind, e.g. `"bbox"`.
    pub r#type: String,
    /// Original parameters from the task spec.
    pub params: CheckSpec,
    /// Pass / fail / not-implemented / error.
    pub result: CheckOutcome,
    /// Free-form structured details (actual values, deviation, etc.).
    pub details: serde_json::Value,
}

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// Check ran and the candidate passed.
    Pass,
    /// Check ran and the candidate failed.
    Fail,
    /// Check is not implemented yet (skeleton state). Counts as a fail
    /// when computing `summary.passed`, but is reported separately so the
    /// leaderboard can distinguish "broken submission" from "grader gap".
    NotImplemented,
    /// Check could not run (e.g. file missing, kernel panic). Counts as a
    /// fail. Details carry the error string.
    Error,
}

/// Aggregate of one attempt's check outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    /// True iff all checks passed and no anti-cheese / limits violated.
    pub passed: bool,
    /// Number of checks with `Pass`.
    pub checks_passed: usize,
    /// Total number of checks.
    pub checks_total: usize,
    /// `checks_passed / checks_total` as a 0..=1 score.
    pub score: f64,
    /// True iff any anti-cheese rule was violated.
    pub anti_cheese_violated: bool,
    /// Names of any limits that were exceeded.
    pub limits_exceeded: Vec<String>,
}

/// Partial run blob produced by the grader. The harness wraps this with
/// model identity, prompt, trace, and timestamps to form the final blob
/// committed to `mecheval/runs/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunBlob {
    /// Run blob schema version.
    pub schema_version: u32,
    /// Stable task ID (e.g. `a1-plate-01`).
    pub task_id: String,
    /// SHA-256 of the task JSON the grader was run against.
    pub task_sha256: String,
    /// Per-check records, in task order.
    pub checks: Vec<CheckRecord>,
    /// Aggregate.
    pub summary: Summary,
}

impl Summary {
    /// Compute the aggregate from per-check outcomes plus the anti-cheese /
    /// limits flags. Anti-cheese violation or any limit exceeded forces
    /// `passed = false`, even if every check happens to pass.
    pub fn from_records(
        records: &[CheckRecord],
        anti_cheese_violated: bool,
        limits_exceeded: Vec<String>,
    ) -> Self {
        let total = records.len();
        let passed = records
            .iter()
            .filter(|r| r.result == CheckOutcome::Pass)
            .count();
        let score = if total == 0 {
            0.0
        } else {
            passed as f64 / total as f64
        };
        let all_pass = total > 0 && passed == total;
        Summary {
            passed: all_pass && !anti_cheese_violated && limits_exceeded.is_empty(),
            checks_passed: passed,
            checks_total: total,
            score,
            anti_cheese_violated,
            limits_exceeded,
        }
    }
}
