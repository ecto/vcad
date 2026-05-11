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

    /// DFM rule set passes for the named manufacturing process. The
    /// optional `rules` field is informational (legacy hint about which
    /// rules a task author cared about); the grader uses the process's
    /// default rule pack from `vcad-kernel-dfm` and fails on any issue
    /// at or above `max_severity` (default "error"). `process` defaults
    /// to "fdm" — sensible for 3D-printed plastic F-suite accessories.
    Dfm {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rules: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        process: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_severity: Option<String>,
    },

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

    /// Suite F: minimum separation between the candidate accessory and the
    /// host. `host` names a `kind` from `task.inputs` (typically `host_geometry`).
    MateClearance {
        host: String,
        min_mm: f64,
        max_mm: f64,
    },

    /// Suite F: volume of (accessory ∩ host). Should be near zero for slip
    /// fits, small and positive for press fits.
    InterferenceVolume { host: String, max_mm3: f64 },

    /// Suite F: surface area of accessory within `epsilon_mm` of the host.
    ContactArea {
        host: String,
        epsilon_mm: f64,
        min_mm2: f64,
    },

    /// Suite F: cap on the accessory's overall bounding-box extents (mm).
    Envelope { max_mm: [f64; 3] },

    /// Suite F: assemble host + accessory, apply gravity for `duration_sec`,
    /// pass if relative drift stays below `max_drift_mm`. Backed by phyz.
    GravityHold {
        host: String,
        host_mass_kg: f64,
        gravity_dir: [f64; 3],
        duration_sec: f64,
        max_drift_mm: f64,
    },

    /// Suite F: pull the accessory away from the host with `force_n`
    /// (newtons) along `direction`; pass if drift stays under threshold.
    /// Backed by phyz.
    PullForce {
        host: String,
        force_n: f64,
        direction: [f64; 3],
        duration_sec: f64,
        max_drift_mm: f64,
    },

    /// Suite F: geometric form-lock test. Translate the accessory by
    /// `displacement_mm` along `direction` (sampled at intermediate
    /// poses) and measure the peak interference volume with the host.
    /// Pass if peak interference exceeds the baseline (as-designed
    /// pose) by at least `min_interference_gain_mm3`.
    ///
    /// Intuition: a snap-fit cap or a C-clip, when pulled, would have
    /// to push deeper into the host's retention feature before clearing
    /// it — that's the form-lock signature. A freely-sliding part
    /// (press fit, simple plug) shows no interference gain.
    ///
    /// This is the deterministic alternative to `pull_force` while
    /// phyz's penalty-contact stability matures.
    PullRetentionGeometric {
        host: String,
        direction: [f64; 3],
        displacement_mm: f64,
        min_interference_gain_mm3: f64,
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
            CheckSpec::MateClearance { .. } => "mate_clearance",
            CheckSpec::InterferenceVolume { .. } => "interference_volume",
            CheckSpec::ContactArea { .. } => "contact_area",
            CheckSpec::Envelope { .. } => "envelope",
            CheckSpec::GravityHold { .. } => "gravity_hold",
            CheckSpec::PullForce { .. } => "pull_force",
            CheckSpec::PullRetentionGeometric { .. } => "pull_retention_geometric",
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

    /// True for checks that require a Suite F host geometry to be loaded.
    pub fn is_suite_f(&self) -> bool {
        matches!(
            self,
            CheckSpec::MateClearance { .. }
                | CheckSpec::InterferenceVolume { .. }
                | CheckSpec::ContactArea { .. }
                | CheckSpec::GravityHold { .. }
                | CheckSpec::PullForce { .. }
                | CheckSpec::PullRetentionGeometric { .. }
        )
    }
}
