//! Check vocabulary — every grader check the schema currently supports.
//!
//! Each variant carries its parameters. Dispatch (in `grader.rs`) matches on
//! this enum to call the appropriate validator.

use serde::{Deserialize, Serialize};

/// One grader check. Tagged on the `type` field per the JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum CheckSpec {
    /// Output is a valid closed manifold solid.
    ValidSolid,

    /// Output bounding box matches expected min/max within tolerance.
    Bbox {
        min: [f64; 3],
        max: [f64; 3],
        tolerance_mm: f64,
    },

    /// Mass properties match (any subset of volume / area / COM).
    MassProps {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        volume_mm3: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface_area_mm2: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        center_of_mass: Option<[f64; 3]>,
        tolerance_pct: f64,
    },

    /// N cylindrical through-features of a given diameter.
    HoleCount {
        diameter_mm: f64,
        expected: u32,
        diameter_tolerance_mm: f64,
    },

    /// Specific hole positions, given diameter.
    HolePositions {
        diameter_mm: f64,
        positions: Vec<[f64; 3]>,
        tolerance_mm: f64,
    },

    /// Detected fillet/chamfer with target radius on a named edge class.
    FilletRadius {
        edge_class: String,
        radius_mm: f64,
        tolerance_mm: f64,
    },

    /// Export to STEP, re-import, mass-props match within tolerance.
    StepRoundtrip { tolerance_pct: f64 },

    /// ECAD design-rule check passes clean.
    DrcClean,

    /// ECAD electrical-rule check passes clean.
    ErcClean,

    /// DFM rule set passes (e.g. min_wall, draft, no_undercut).
    Dfm { rules: Vec<String> },

    /// Refactor task: untouched parts have unchanged mass-props.
    RefactorInvariant {
        untouched_parts: Vec<String>,
        tolerance_pct: f64,
    },

    /// Suite C: body assembly is valid (closed solids, joints connect, no
    /// inter-penetration at rest).
    BodyValid,

    /// Suite C: forward kinematics reaches target under joint limits.
    FkReaches { target: [f64; 3], tolerance_m: f64 },

    /// Suite C: per-joint torque budget covers gravity + payload.
    TorqueBudget { payload_kg: f64, safety_factor: f64 },

    /// Suite C: COM stays inside support polygon during a rollout.
    StableDuringRollout { rollout: String, min_margin_mm: f64 },

    /// Suite C: gym rollout completes the named task.
    TaskSuccess {
        task: String,
        params: serde_json::Value,
    },
}

impl CheckSpec {
    /// Short, stable identifier for the variant. Used in run blobs and logs.
    pub fn kind(&self) -> &'static str {
        match self {
            CheckSpec::ValidSolid => "valid_solid",
            CheckSpec::Bbox { .. } => "bbox",
            CheckSpec::MassProps { .. } => "mass_props",
            CheckSpec::HoleCount { .. } => "hole_count",
            CheckSpec::HolePositions { .. } => "hole_positions",
            CheckSpec::FilletRadius { .. } => "fillet_radius",
            CheckSpec::StepRoundtrip { .. } => "step_roundtrip",
            CheckSpec::DrcClean => "drc_clean",
            CheckSpec::ErcClean => "erc_clean",
            CheckSpec::Dfm { .. } => "dfm",
            CheckSpec::RefactorInvariant { .. } => "refactor_invariant",
            CheckSpec::BodyValid => "body_valid",
            CheckSpec::FkReaches { .. } => "fk_reaches",
            CheckSpec::TorqueBudget { .. } => "torque_budget",
            CheckSpec::StableDuringRollout { .. } => "stable_during_rollout",
            CheckSpec::TaskSuccess { .. } => "task_success",
        }
    }

    /// True for checks that require Suite C (mech) physics simulation.
    pub fn is_suite_c(&self) -> bool {
        matches!(
            self,
            CheckSpec::BodyValid
                | CheckSpec::FkReaches { .. }
                | CheckSpec::TorqueBudget { .. }
                | CheckSpec::StableDuringRollout { .. }
                | CheckSpec::TaskSuccess { .. }
        )
    }
}
