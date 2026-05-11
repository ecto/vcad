//! Task JSON parsing.
//!
//! See `mecheval/tasks/SCHEMA.md` for the schema.

use crate::check::CheckSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Top-level task definition. One JSON file per task; filename matches `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Stable ID, matches filename. Convention: `<tier>-<short-name>-<NN>`.
    pub id: String,
    /// Which suite (A, B, or C).
    pub suite: Suite,
    /// Tier label, e.g. `A1`, `B-boolean`, `C-reacher`.
    pub tier: String,
    /// Short human label.
    pub title: String,
    /// Natural-language spec given to the agent.
    pub prompt: String,
    /// Optional starter files. Each entry is either a bare path string
    /// (legacy form) or a structured object describing kind / visibility /
    /// view metadata. See `mecheval/tasks/SCHEMA.md` ("Structured inputs").
    #[serde(default)]
    pub inputs: Vec<TaskInput>,
    /// Ordered list of grader checks. All must pass for the task to score.
    pub checks: Vec<CheckSpec>,
    /// Per-task structural constraints.
    #[serde(default)]
    pub anti_cheese: AntiCheese,
    /// Hard ceilings on cost.
    #[serde(default)]
    pub limits: Limits,
    /// Number of attempts aggregated for `pass^k`.
    #[serde(default = "default_pass_k")]
    pub pass_k: u32,
    /// Free-form filtering tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Top-level suite designator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Suite {
    /// CAD authoring (A1–A6).
    A,
    /// Kernel-only (no AI).
    B,
    /// Mech (the headline).
    C,
    /// Fit — accessory designed against a host (F1–F4).
    F,
}

/// One entry in `Task.inputs`. Either a bare path (legacy starter-`.vcad`
/// case) or a structured object with `kind` / `agent_visible` metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskInput {
    /// Bare path, relative to the task file's directory.
    Path(String),
    /// Structured input — kind-tagged, with explicit agent visibility.
    Structured(StructuredInput),
}

/// A structured input. The grader only inspects `kind`, `agent_visible`,
/// `path`, and `frame` (when present); everything else is preserved in
/// `extra` so task authors can attach arbitrary metadata (view labels,
/// fiducials, captions) without grader churn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredInput {
    /// What this input represents. Conventional values: `reference_image`,
    /// `host_geometry`, `known_dimensions`.
    pub kind: String,
    /// Whether the agent should see this input. `false` items are
    /// stripped by the harness before invoking the solver.
    pub agent_visible: bool,
    /// Optional path, relative to the task file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional placement frame (used by `host_geometry`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<InputFrame>,
    /// All other fields preserved as-is.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Placement frame for an input (e.g. where the host sits in the assembly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputFrame {
    /// World-space origin in mm.
    pub origin: [f64; 3],
    /// Declared "up/main" axis as a unit vector.
    pub axis: [f64; 3],
}

impl Task {
    /// Find the first input whose `kind` matches and is `agent_visible: false`.
    /// Used by graders to locate non-agent-visible reference geometry
    /// (host objects, golden parts).
    pub fn private_input(&self, kind: &str) -> Option<&StructuredInput> {
        self.inputs.iter().find_map(|i| match i {
            TaskInput::Structured(s) if s.kind == kind && !s.agent_visible => Some(s),
            _ => None,
        })
    }
}

/// Per-task structural anti-cheese rules. All optional; defaults are unset.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AntiCheese {
    /// Minimum number of distinct rigid bodies in a Suite-C submission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rigid_bodies: Option<u32>,
    /// Minimum number of actuated joints in a Suite-C submission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_actuated_joints: Option<u32>,
    /// Maximum number of disjoint solids in a Suite-A output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_solid_count: Option<u32>,
    /// Total mass ceiling in kg (Suite C).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_mass_kg: Option<f64>,
    /// Per-joint torque ceiling in N·m (Suite C).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint_torque_ceiling_nm: Option<f64>,
    /// Required link names (Suite C).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_links: Vec<String>,
    /// Suite F: cap on accessory bbox ∩ host bbox as % of host bbox volume.
    /// Stops "wrap the whole host" cheese.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_overlap_with_host_envelope_pct: Option<f64>,
    /// Suite F: floor on accessory volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_accessory_volume_mm3: Option<f64>,
    /// Suite F: whether the accessory bbox must contain the host centroid.
    /// `Some(true)` for stand/cradle tasks; `Some(false)` for cap/clip
    /// tasks. `None` means no constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub must_enclose_host_centroid: Option<bool>,
}

/// Cost ceilings per attempt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum input + output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Maximum end-to-end wall-clock in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wallclock_sec: Option<f64>,
    /// Maximum number of MCP tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
}

fn default_pass_k() -> u32 {
    5
}

/// Load and parse a task JSON file. Returns an error if the file cannot be
/// read or the JSON does not match the schema.
pub fn load_task(path: &Path) -> anyhow::Result<Task> {
    let raw = std::fs::read_to_string(path)?;
    let task: Task = serde_json::from_str(&raw)?;
    if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
        if filename != task.id {
            anyhow::bail!(
                "task filename {:?} does not match task id {:?}",
                filename,
                task.id
            );
        }
    }
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tasks_dir() -> PathBuf {
        // Cargo runs tests with CWD = crate root (mecheval/graders/).
        PathBuf::from("../tasks")
    }

    #[test]
    fn loads_a1_plate() {
        let path = tasks_dir().join("a1-plate-01.json");
        let task = load_task(&path).expect("a1-plate-01 should parse");
        assert_eq!(task.id, "a1-plate-01");
        assert_eq!(task.suite, Suite::A);
        assert_eq!(task.tier, "A1");
        assert!(!task.checks.is_empty());
    }

    #[test]
    fn loads_all_seed_tasks() {
        let dir = std::fs::read_dir(tasks_dir()).expect("tasks dir");
        let mut count = 0;
        for entry in dir {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                load_task(&path).unwrap_or_else(|e| {
                    panic!("failed to load {:?}: {}", path, e);
                });
                count += 1;
            }
        }
        assert!(count >= 1, "expected at least one task in tasks/");
    }

    #[test]
    fn filename_must_match_id() {
        let json = r#"{
            "id": "a1-mismatch",
            "suite": "A",
            "tier": "A1",
            "title": "x",
            "prompt": "x",
            "checks": []
        }"#;
        let tmp = std::env::temp_dir().join("a1-different.json");
        std::fs::write(&tmp, json).unwrap();
        assert!(load_task(&tmp).is_err());
    }
}
