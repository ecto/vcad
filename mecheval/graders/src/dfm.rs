//! `dfm` grader check: run `vcad-kernel-dfm`'s process-specific rule
//! pack against the candidate's BRep(s) and pass if no issues fail the
//! severity threshold.
//!
//! The Dfm check spec accepts:
//! - `process`: one of `"fdm"`, `"sla"`, `"cnc"` / `"cnc3axis"`,
//!   `"injection"`, `"sheet_metal"`, `"casting_sand"`,
//!   `"casting_investment"`. Defaults to `"fdm"`.
//! - `max_severity`: highest severity that counts as a pass. `"error"`
//!   (default) means only Error-severity issues fail; `"warning"` fails
//!   on Errors or Warnings; `"info"` fails on any issue.
//! - `rules`: informational, ignored by the grader (process rule pack
//!   drives the actual check).

use crate::blob::CheckOutcome;
use crate::eval::EvalSnapshot;
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use vcad_kernel_dfm::{run_dfm, DfmSeverity, Process, RulePack};

/// Parse a process name (case-insensitive, snake/kebab tolerant).
fn parse_process(name: &str) -> Option<Process> {
    let key = name.trim().to_ascii_lowercase();
    let normalized = key.replace('-', "_");
    match normalized.as_str() {
        "fdm" => Some(Process::Fdm),
        "sla" => Some(Process::Sla),
        "cnc" | "cnc3axis" | "cnc_3axis" | "cnc_3_axis" => Some(Process::Cnc3Axis),
        "injection" | "injection_molding" => Some(Process::Injection),
        "sheet_metal" | "sheetmetal" => Some(Process::SheetMetal),
        "casting_sand" | "sand_casting" => Some(Process::CastingSand),
        "casting_investment" | "investment_casting" => Some(Process::CastingInvestment),
        _ => None,
    }
}

/// Parse the severity threshold. "error" → fail only on Errors;
/// "warning" → fail on Errors or Warnings; "info" → fail on any issue.
fn severity_threshold(name: Option<&str>) -> DfmSeverity {
    match name.unwrap_or("error").trim().to_ascii_lowercase().as_str() {
        "info" => DfmSeverity::Info,
        "warning" => DfmSeverity::Warning,
        _ => DfmSeverity::Error,
    }
}

fn severity_rank(s: DfmSeverity) -> u32 {
    match s {
        DfmSeverity::Error => 2,
        DfmSeverity::Warning => 1,
        DfmSeverity::Info => 0,
    }
}

/// Run the dfm check.
pub fn check_dfm(
    snapshot: &EvalSnapshot,
    process_name: Option<&str>,
    max_severity_name: Option<&str>,
    legacy_rules: &[String],
) -> (CheckOutcome, serde_json::Value) {
    let process_str = process_name.unwrap_or("fdm");
    let process = match parse_process(process_str) {
        Some(p) => p,
        None => {
            return (
                CheckOutcome::Fail,
                json!({
                    "reason": format!("unknown process: {}", process_str),
                    "hint": "supported: fdm, sla, cnc, injection, sheet_metal, casting_sand, casting_investment",
                }),
            );
        }
    };
    let threshold = severity_threshold(max_severity_name);

    if snapshot.solids.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no valid solid to analyse" }),
        );
    }

    let pack = RulePack::default_for(process);
    let mut all_issues = Vec::new();
    let mut analysed = 0_usize;
    let mut skipped_mesh_only = 0_usize;

    for solid in &snapshot.solids {
        let brep = match solid.as_brep() {
            Some(b) => b,
            None => {
                skipped_mesh_only += 1;
                continue;
            }
        };
        let report = match catch_unwind(AssertUnwindSafe(|| run_dfm(brep, None, process, &pack))) {
            Ok(r) => r,
            Err(_) => {
                return (
                    CheckOutcome::Fail,
                    json!({ "reason": "dfm rule pack panicked" }),
                );
            }
        };
        all_issues.extend(report.issues);
        analysed += 1;
    }

    let threshold_rank = severity_rank(threshold);
    let failing: Vec<_> = all_issues
        .iter()
        .filter(|i| severity_rank(i.severity) >= threshold_rank)
        .collect();

    let mut by_rule: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for i in &all_issues {
        *by_rule.entry(i.rule.clone()).or_insert(0) += 1;
    }

    let pass = failing.is_empty() && analysed > 0;
    (
        if pass {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        json!({
            "process": format!("{:?}", process).to_lowercase(),
            "max_severity": format!("{:?}", threshold).to_lowercase(),
            "rule_pack_version": pack.version,
            "analysed_solid_count": analysed,
            "skipped_mesh_only_count": skipped_mesh_only,
            "total_issues": all_issues.len(),
            "failing_issue_count": failing.len(),
            "issues_by_rule": by_rule,
            "failing_issues": failing.iter().take(10).map(|i| json!({
                "rule": i.rule,
                "severity": format!("{:?}", i.severity).to_lowercase(),
                "message": i.message,
                "measured": i.measured,
                "limit": i.limit,
                "units": i.units,
            })).collect::<Vec<_>>(),
            "legacy_rules_hint": legacy_rules,
        }),
    )
}
