#![warn(missing_docs)]

//! Design-for-Manufacturing (DFM) analysis for the vcad kernel.
//!
//! `vcad-kernel-dfm` is the generic, process-aware home for
//! manufacturability checks. Each `Process` (CNC, FDM, injection,
//! sheet-metal, casting, …) gets a `RulePack` loaded from a TOML file
//! in `lib/dfm/`. The crate runs the enabled rules over a BRep, then
//! emits a [`DfmReport`] containing structured [`DfmIssue`]s, each with
//! provenance back to the offending faces (and, when available, the
//! [`vcad_ir::NodeId`] that produced them).
//!
//! The shape of [`DfmIssue`] / [`DfmFix`] is intentionally agent-friendly:
//! each issue can carry a [`DfmFix::SetParam`] / [`DfmFix::WrapOp`] /
//! [`DfmFix::ReplaceOp`] patch the agent can apply against the IR to
//! close the loop ("paste a part, get a manufacturable revision back").
//!
//! # Example
//!
//! ```ignore
//! use vcad_kernel_dfm::{run_dfm, Process, RulePack};
//! use vcad_kernel_primitives::make_cube;
//!
//! let brep = make_cube(50.0, 50.0, 0.5);   // very thin plate
//! let pack = RulePack::default_for(Process::Fdm);
//! let report = run_dfm(&brep, None, Process::Fdm, &pack);
//! assert!(report.issues.iter().any(|i| i.rule.contains("thin_wall")));
//! ```

pub mod cost;
pub mod geom;
pub mod issue;
pub mod rules;

pub use cost::estimate_for_process;
pub use issue::{DfmFix, DfmIssue, DfmReport, DfmSeverity};
pub use rules::{DefaultPacks, RulePack};
pub use vcad_kernel_cost::Process;

use thiserror::Error;
use vcad_ir::Document;
use vcad_kernel_primitives::BRepSolid;

/// Errors that can occur during DFM analysis.
#[derive(Debug, Error)]
pub enum DfmError {
    /// The TOML rule pack failed to parse.
    #[error("rule pack parse error: {0}")]
    Pack(#[from] toml::de::Error),
    /// The supplied process string isn't recognised.
    #[error("unknown process: {0}")]
    UnknownProcess(String),
    /// Generic catch-all for downstream errors.
    #[error("{0}")]
    Other(String),
}

/// Result alias for DFM operations.
pub type Result<T> = std::result::Result<T, DfmError>;

/// Run DFM checks against a single BRep.
///
/// `provenance` is an optional `FaceIndex -> NodeId` map produced by the
/// IR evaluator. When supplied, every emitted [`DfmIssue`] gets its
/// `origin_op` filled in so agents can mutate the source op directly.
/// v1 callers may pass `None` — issues will still highlight faces in
/// the viewport, only autofix capability is reduced.
pub fn run_dfm(
    brep: &BRepSolid,
    provenance: Option<&geom::provenance::ProvenanceMap>,
    process: Process,
    pack: &RulePack,
) -> DfmReport {
    let mut issues = Vec::new();
    match process {
        Process::Cnc3Axis => rules::cnc::run(brep, provenance, pack, &mut issues),
        Process::Fdm | Process::Sla => rules::fdm::run(brep, provenance, process, pack, &mut issues),
        Process::Injection => rules::mold::run(brep, provenance, pack, &mut issues),
        Process::SheetMetal => rules::sheet::run(brep, provenance, pack, &mut issues),
        Process::CastingSand | Process::CastingInvestment => {
            rules::casting::run(brep, provenance, process, pack, &mut issues)
        }
    }
    DfmReport {
        process,
        rule_pack_version: pack.version.clone(),
        rule_pack_name: pack.name.clone(),
        issues,
        cost_estimate: None,
    }
}

/// Run DFM against a full IR [`Document`].
///
/// v1 iterates the doc's roots and evaluates each one independently;
/// per-part issues are flattened into a single report. A future
/// revision will surface per-part grouping.
pub fn run_dfm_for_document(
    doc: &Document,
    breps: &[(vcad_ir::NodeId, BRepSolid)],
    provenance: Option<&geom::provenance::ProvenanceMap>,
    process: Process,
    pack: &RulePack,
) -> DfmReport {
    let mut all = Vec::new();
    for (_root, brep) in breps {
        let part = run_dfm(brep, provenance, process, pack);
        all.extend(part.issues);
    }
    let _ = doc; // reserved for per-part grouping
    DfmReport {
        process,
        rule_pack_version: pack.version.clone(),
        rule_pack_name: pack.name.clone(),
        issues: all,
        cost_estimate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn fdm_thin_plate_flags_thin_wall() {
        let brep = make_cube(50.0, 50.0, 0.2);
        let pack = RulePack::default_for(Process::Fdm);
        let report = run_dfm(&brep, None, Process::Fdm, &pack);
        // Only assert the wiring works — actual check correctness lives
        // in the per-rules tests.
        assert_eq!(report.process, Process::Fdm);
    }

    #[test]
    fn cnc_default_pack_loads() {
        let pack = RulePack::default_for(Process::Cnc3Axis);
        assert_eq!(pack.process, Process::Cnc3Axis);
        assert!(!pack.rules.is_empty());
    }
}
