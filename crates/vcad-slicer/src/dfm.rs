//! Design for Manufacturing (DFM) printability checks.
//!
//! As of v1 DFM, this module is a back-compat adapter over the
//! generic [`vcad_kernel_dfm`] crate. New callers should use
//! `vcad_kernel_dfm::run_dfm` directly — it covers FDM along with
//! CNC, injection, sheet metal, and casting, and returns the richer
//! [`DfmIssue`](vcad_kernel_dfm::DfmIssue) payload that the agent
//! loop and the inline app annotations consume.
//!
//! The legacy `DfmResult` / `DfmWarning` shape kept here makes the
//! existing `vcad-kernel-wasm::checkPrintability` JS binding work
//! without churn — it converts a shared `DfmReport` into the older
//! flat warning list, and adds the build-volume check the generic
//! crate doesn't model.

use serde::{Deserialize, Serialize};
use vcad_kernel_dfm::{
    run_dfm, DfmFix, DfmReport, DfmSeverity as KernelSeverity, Process, RulePack,
};
use vcad_kernel_primitives::BRepSolid;

use crate::smart_defaults::PrinterParams;

/// Severity of a DFM warning (legacy shape; mirrors
/// `vcad_kernel_dfm::DfmSeverity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DfmSeverity {
    /// Will not print at all.
    Error,
    /// Will print but with poor quality.
    Warning,
    /// Informational suggestion.
    Info,
}

impl From<KernelSeverity> for DfmSeverity {
    fn from(s: KernelSeverity) -> Self {
        match s {
            KernelSeverity::Error => Self::Error,
            KernelSeverity::Warning => Self::Warning,
            KernelSeverity::Info => Self::Info,
        }
    }
}

/// A DFM warning with face/edge references for highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DfmWarning {
    /// Severity level.
    pub severity: DfmSeverity,
    /// Warning type tag.
    pub kind: String,
    /// Human-readable description.
    pub message: String,
    /// Affected face indices (for viewport highlighting).
    pub face_indices: Vec<usize>,
    /// Suggested fix.
    pub suggestion: Option<String>,
}

/// Result of DFM analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DfmResult {
    /// All warnings found.
    pub warnings: Vec<DfmWarning>,
    /// Overall printability score (0-100, 100 = no issues).
    pub score: u32,
}

/// Check a solid for printability issues against a printer profile.
///
/// Internally builds an FDM [`RulePack`] from the printer params (so
/// `nozzle_diameter` drives `min_wall_mm` and `min_diameter_mm`) then
/// delegates to `vcad_kernel_dfm::run_dfm`. The build-volume check is
/// the one rule the generic crate doesn't yet model — added here on
/// top of the converted warning list.
pub fn check_printability(brep: &BRepSolid, params: &PrinterParams) -> DfmResult {
    let pack = build_pack_from_params(params);
    let report = run_dfm(brep, None, Process::Fdm, &pack);
    let mut warnings: Vec<DfmWarning> =
        report.issues.iter().map(issue_to_warning).collect();

    // Build-volume check (not in vcad-kernel-dfm yet — printer-specific).
    let (lo, hi) = brep_bbox(brep);
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    if size[0] > params.bed_x || size[1] > params.bed_y || size[2] > params.bed_z {
        warnings.push(DfmWarning {
            severity: DfmSeverity::Error,
            kind: "exceeds_build_volume".into(),
            message: format!(
                "Part ({:.1} x {:.1} x {:.1}mm) exceeds build volume ({:.0} x {:.0} x {:.0}mm)",
                size[0], size[1], size[2], params.bed_x, params.bed_y, params.bed_z
            ),
            face_indices: Vec::new(),
            suggestion: Some("Scale down or choose a larger printer".into()),
        });
    }

    DfmResult {
        score: score_from(&report, &warnings),
        warnings,
    }
}

fn build_pack_from_params(params: &PrinterParams) -> RulePack {
    let mut pack = RulePack::default_for(Process::Fdm);
    let nozzle = params.nozzle_diameter;
    let min_line_width = nozzle * 0.8;
    if let Some(rule) = pack.rules.get_mut("thin_wall") {
        rule.params
            .insert("min_wall_mm".into(), toml::Value::Float(min_line_width));
    }
    if let Some(rule) = pack.rules.get_mut("small_hole") {
        rule.params
            .insert("min_diameter_mm".into(), toml::Value::Float(nozzle * 2.0));
    }
    pack
}

fn issue_to_warning(issue: &vcad_kernel_dfm::DfmIssue) -> DfmWarning {
    let kind = issue
        .rule
        .split_once('.')
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_else(|| issue.rule.clone());
    let suggestion = match &issue.suggested_fix {
        Some(DfmFix::Manual { description }) => Some(description.clone()),
        Some(DfmFix::SetParam { path, value, .. }) => Some(format!("Set {path} = {value}")),
        Some(_) => Some("Apply suggested fix via dfm_apply_fix".into()),
        None => None,
    };
    DfmWarning {
        severity: issue.severity.into(),
        kind,
        message: issue.message.clone(),
        face_indices: issue.face_indices.clone(),
        suggestion,
    }
}

fn score_from(report: &DfmReport, extra_warnings: &[DfmWarning]) -> u32 {
    let mut errors = report.error_count();
    let mut warns = report.warning_count();
    for w in extra_warnings {
        match w.severity {
            DfmSeverity::Error => errors += 1,
            DfmSeverity::Warning => warns += 1,
            _ => {}
        }
    }
    100u32.saturating_sub((errors * 30 + warns * 10) as u32)
}

fn brep_bbox(brep: &BRepSolid) -> ([f64; 3], [f64; 3]) {
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for (_id, v) in &brep.topology.vertices {
        min[0] = min[0].min(v.point.x);
        min[1] = min[1].min(v.point.y);
        min[2] = min[2].min(v.point.z);
        max[0] = max[0].max(v.point.x);
        max[1] = max[1].max(v.point.y);
        max[2] = max[2].max(v.point.z);
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    fn a1_mini_params() -> PrinterParams {
        PrinterParams {
            nozzle_diameter: 0.4,
            bed_x: 180.0,
            bed_y: 180.0,
            bed_z: 180.0,
        }
    }

    #[test]
    fn test_cube_no_errors() {
        let brep = make_cube(20.0, 20.0, 10.0);
        let result = check_printability(&brep, &a1_mini_params());
        let errors: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.severity == DfmSeverity::Error)
            .collect();
        assert!(errors.is_empty());
        assert!(result.score >= 70);
    }

    #[test]
    fn test_exceeds_build_volume() {
        let brep = make_cube(200.0, 200.0, 200.0);
        let result = check_printability(&brep, &a1_mini_params());
        let volume_errors: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.kind == "exceeds_build_volume")
            .collect();
        assert!(!volume_errors.is_empty());
    }
}
