//! Solve report types (serialized to JSON for WASM/MCP consumers).

use serde::{Deserialize, Serialize};

/// A measured value for one driven (reference) dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrivenValue {
    /// Constraint id.
    pub id: String,
    /// Measured value (mm or degrees, per the constraint kind).
    pub value: f64,
}

/// Residual magnitude of one constraint against current geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConstraintResidual {
    /// Constraint id.
    pub id: String,
    /// |error| in the constraint's natural units (mm or degrees; 0 = holds
    /// exactly). Driven dimensions report drift from their stored value.
    pub residual: f64,
    /// Whether the constraint is a driven (reference) dimension.
    pub driven: bool,
}

/// Per-plane solve group outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupReport {
    /// The node this group solved (PCB board or sketch node).
    pub node: u64,
    /// Solver status string (`converged`, `maxIterations`, …).
    pub status: String,
    /// Whether this group converged.
    pub converged: bool,
    /// Levenberg-Marquardt iterations used.
    pub iterations: usize,
    /// Final residual norm.
    pub residual_norm: f64,
    /// Remaining degrees of freedom (negative = over-constrained).
    pub dof: i64,
    /// Number of driving constraints lowered in this group.
    pub constraint_count: usize,
}

/// Aggregate result of a design-constraint solve or check pass.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignSolveReport {
    /// Whether every solve group converged (vacuously true with none).
    pub converged: bool,
    /// Per-group outcomes.
    pub groups: Vec<GroupReport>,
    /// Footprint references whose position or rotation changed.
    pub moved_footprints: Vec<String>,
    /// Outline vertex indices that moved, as `node:index` strings.
    pub moved_vertices: Vec<String>,
    /// Sketch nodes whose segments moved.
    pub moved_sketches: Vec<u64>,
    /// Measured values for driven dimensions (and, on check passes, for
    /// every dimensional constraint).
    pub driven_values: Vec<DrivenValue>,
    /// Per-constraint residuals against the final geometry (0 = holds).
    pub residuals: Vec<ConstraintResidual>,
    /// Constraints that could not be lowered (bad refs, lost part anchors,
    /// unresolvable formulas), as human-readable messages. The rest of the
    /// system still solves.
    pub errors: Vec<String>,
    /// Non-fatal caveats (e.g. pad anchor combined with free rotation).
    pub warnings: Vec<String>,
}
