//! [`DfmIssue`] / [`DfmFix`] / [`DfmReport`] — the structured payload an
//! agent (or the app) consumes to act on a manufacturability problem.

use serde::{Deserialize, Serialize};
use vcad_ir::NodeId;
use vcad_kernel_cost::{CostEstimate, Process};
use vcad_kernel_math::Point3;

/// Severity of a single DFM issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DfmSeverity {
    /// Part will not manufacture at all.
    Error,
    /// Part will manufacture, but with poor quality / high risk.
    Warning,
    /// Informational — surface to the user but doesn't block.
    Info,
}

/// A patch an agent or user can apply to mutate the IR back into a
/// manufacturable state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DfmFix {
    /// Mutate a numeric parameter on an existing op
    /// (e.g. raise the radius on Fillet5 from 0.5 mm to 3 mm).
    SetParam {
        /// Target node in the IR DAG.
        node: NodeId,
        /// JSON-path style key into the op (e.g. `"radius"`, `"size.x"`).
        path: String,
        /// New value.
        value: serde_json::Value,
    },
    /// Wrap a node in a new op (e.g. add a draft / fillet feature).
    WrapOp {
        /// Target node to wrap.
        node: NodeId,
        /// Description of the wrapping op (CsgOp shape stored as JSON to
        /// avoid a cyclic dependency through serde).
        op_json: serde_json::Value,
    },
    /// Replace a node entirely.
    ReplaceOp {
        /// Target node.
        node: NodeId,
        /// New op JSON.
        op_json: serde_json::Value,
    },
    /// No automatic fix — surface a description to the user.
    Manual {
        /// Human-readable instructions.
        description: String,
    },
}

/// A single DFM finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DfmIssue {
    /// Stable id derived from `(rule, anchor, units)`.
    pub id: String,
    /// Rule identifier, e.g. `"cnc.internal_radius_too_small"`.
    pub rule: String,
    /// Severity tier.
    pub severity: DfmSeverity,
    /// Process this issue belongs to.
    pub process: Process,
    /// Short human-readable summary.
    pub message: String,
    /// Long-form explanation of why this matters.
    pub explanation: String,
    /// Offending BRep face indices (matches `BRepSolid.topology.faces`
    /// iteration order for cross-language stability).
    pub face_indices: Vec<usize>,
    /// Offending BRep edge indices.
    pub edge_indices: Vec<usize>,
    /// World-space anchor point for floating annotations.
    pub anchor: [f64; 3],
    /// Measured value (e.g. wall thickness in mm, draft angle in deg).
    pub measured: f64,
    /// Rule limit / threshold the value violated.
    pub limit: f64,
    /// Units string (`"mm"`, `"deg"`, `"ratio"`).
    pub units: String,
    /// IR node that produced the offending geometry (when known).
    pub origin_op: Option<NodeId>,
    /// Suggested fix the agent or user can apply.
    pub suggested_fix: Option<DfmFix>,
}

impl DfmIssue {
    /// Construct an issue with a stable id derived from the rule + anchor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule: impl Into<String>,
        severity: DfmSeverity,
        process: Process,
        message: impl Into<String>,
        anchor: Point3,
        measured: f64,
        limit: f64,
        units: impl Into<String>,
    ) -> Self {
        let rule = rule.into();
        let units = units.into();
        let id = stable_id(&rule, &anchor);
        Self {
            id,
            rule,
            severity,
            process,
            message: message.into(),
            explanation: String::new(),
            face_indices: Vec::new(),
            edge_indices: Vec::new(),
            anchor: [anchor.x, anchor.y, anchor.z],
            measured,
            limit,
            units,
            origin_op: None,
            suggested_fix: None,
        }
    }

    /// Attach a long-form explanation.
    pub fn with_explanation(mut self, e: impl Into<String>) -> Self {
        self.explanation = e.into();
        self
    }

    /// Attach offending face indices.
    pub fn with_faces(mut self, faces: Vec<usize>) -> Self {
        self.face_indices = faces;
        self
    }

    /// Attach offending edge indices.
    pub fn with_edges(mut self, edges: Vec<usize>) -> Self {
        self.edge_indices = edges;
        self
    }

    /// Attach a fix suggestion.
    pub fn with_fix(mut self, fix: DfmFix) -> Self {
        self.suggested_fix = Some(fix);
        self
    }

    /// Attach IR provenance.
    pub fn with_origin(mut self, node: NodeId) -> Self {
        self.origin_op = Some(node);
        self
    }
}

fn stable_id(rule: &str, anchor: &Point3) -> String {
    // Tiny, stable hash without pulling in extra crates. Format:
    // <rule>:<x>:<y>:<z> quantized to 0.01 mm.
    let q = |v: f64| (v * 100.0).round() as i64;
    format!("{}:{}:{}:{}", rule, q(anchor.x), q(anchor.y), q(anchor.z))
}

/// Pass/fail verdict for one rule of a ruleset that reports per rule
/// (the hobby-mill ruleset does; the per-process packs only emit issues).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    /// Rule identifier, e.g. `"hobby_mill.r1_internal_corner_radius"`.
    pub rule: String,
    /// Short label, e.g. `"R1 internal corner radius"`.
    pub label: String,
    /// Whether the rule passed.
    pub passed: bool,
    /// Number of located violations (issues carry the examples).
    pub violation_count: usize,
    /// One-line summary of what was measured.
    pub summary: String,
    /// Suggested affordances ("add corner relief Ø2.2 at 4 corners …").
    pub affordances: Vec<String>,
}

/// Full DFM report for a single check run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DfmReport {
    /// The process this report describes.
    pub process: Process,
    /// Rule pack name (e.g. "Standard 3-axis aluminum").
    pub rule_pack_name: String,
    /// Rule pack version string (defaults to `"1"`).
    pub rule_pack_version: String,
    /// All issues found in this run.
    pub issues: Vec<DfmIssue>,
    /// Optional cost estimate.
    pub cost_estimate: Option<CostEstimate>,
    /// Per-rule verdicts (empty for packs that only emit issues).
    #[serde(default)]
    pub rule_results: Vec<RuleResult>,
}

impl DfmReport {
    /// Number of `Error`-severity issues.
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == DfmSeverity::Error)
            .count()
    }

    /// Number of `Warning`-severity issues.
    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == DfmSeverity::Warning)
            .count()
    }

    /// A 0..=100 "manufacturability" score: 100 minus 30/error and
    /// 10/warning, saturated at 0. Useful for top-line UI display.
    pub fn score(&self) -> u32 {
        let errs = self.error_count();
        let warns = self.warning_count();
        100u32.saturating_sub((errs * 30 + warns * 10) as u32)
    }
}
