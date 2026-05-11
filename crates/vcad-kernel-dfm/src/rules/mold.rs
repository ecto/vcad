//! Injection-molding checks.
//!
//! `pull_dir` is the mold opening direction; v1 defaults to `+Z` but
//! exposes it through the rule pack so users can fix orientation
//! without rewriting the part.

use vcad_kernel_cost::Process;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;

use crate::geom::{self, provenance::ProvenanceMap};
use crate::issue::{DfmFix, DfmIssue};
use crate::rules::RulePack;

/// Run injection-molding checks.
pub fn run(
    brep: &BRepSolid,
    provenance: Option<&ProvenanceMap>,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
) {
    let process = Process::Injection;
    let pull_dir = pull_direction(pack);

    if let Some(rule) = pack.rule("insufficient_draft") {
        let min_draft = rule.num("min_draft_deg", 1.0);
        for sample in geom::draft::sample(brep, pull_dir) {
            // Skip near-perpendicular faces (those are unrelated to draft).
            if sample.draft_deg.abs() >= 85.0 {
                continue;
            }
            if sample.draft_deg.abs() < min_draft && sample.draft_deg > -90.0 {
                let anchor = geom::face_midpoint_and_normal(brep, sample.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "mold.insufficient_draft",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Draft {:.2}° — below required {:.2}°",
                        sample.draft_deg, min_draft
                    ),
                    anchor,
                    sample.draft_deg.abs(),
                    min_draft,
                    "deg",
                )
                .with_explanation(
                    "Walls without draft scrape against the mold during ejection, \
                     producing scratches or stuck parts. Add at least 1° of taper.",
                )
                .with_faces(vec![sample.face])
                .with_fix(DfmFix::Manual {
                    description: format!("Add at least {:.1}° of draft.", min_draft),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("undercut") {
        for sample in geom::draft::sample(brep, pull_dir) {
            if sample.draft_deg < -0.5 {
                let anchor = geom::face_midpoint_and_normal(brep, sample.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "mold.undercut",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Undercut: face faces away from pull by {:.1}°",
                        -sample.draft_deg
                    ),
                    anchor,
                    -sample.draft_deg,
                    0.0,
                    "deg",
                )
                .with_explanation(
                    "Undercut faces can't be ejected straight out of the mold. \
                     They require a side-action, lifter, or part redesign.",
                )
                .with_faces(vec![sample.face])
                .with_fix(DfmFix::Manual {
                    description:
                        "Add a side-action / lifter, or redesign to eliminate the undercut."
                            .to_string(),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("wall_thickness_uniformity") {
        let max_cv = rule.num("max_thickness_cv", 0.25);
        let samples = geom::thickness::sample_pairs(brep, -0.95);
        let cv = geom::thickness::cv(&samples);
        if cv > max_cv && !samples.is_empty() {
            let anchor = samples[0].anchor;
            issues.push(
                DfmIssue::new(
                    "mold.wall_thickness_uniformity",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Wall thickness CV {:.2} > {:.2} — sink/warp risk",
                        cv, max_cv
                    ),
                    anchor,
                    cv,
                    max_cv,
                    "ratio",
                )
                .with_explanation(
                    "Mixing thin and thick wall sections cools at different rates \
                     and leaves sink marks or warp. Aim for uniform thickness ± 25%.",
                )
                .with_fix(DfmFix::Manual {
                    description: "Even out wall thicknesses; consider coring out thick regions."
                        .to_string(),
                }),
            );
        }
    }
}

fn pull_direction(pack: &RulePack) -> Vec3 {
    let key = pack
        .rule("insufficient_draft")
        .and_then(|r| r.string("pull_dir"))
        .unwrap_or_else(|| "+Z".into());
    parse_axis(&key)
}

pub(crate) fn parse_axis(s: &str) -> Vec3 {
    match s {
        "-Z" => Vec3::new(0.0, 0.0, -1.0),
        "+X" => Vec3::new(1.0, 0.0, 0.0),
        "-X" => Vec3::new(-1.0, 0.0, 0.0),
        "+Y" => Vec3::new(0.0, 1.0, 0.0),
        "-Y" => Vec3::new(0.0, -1.0, 0.0),
        _ => Vec3::new(0.0, 0.0, 1.0),
    }
}
