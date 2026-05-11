//! CNC 3-axis milling checks.

use vcad_kernel_cost::Process;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;

use crate::geom::{self, provenance::ProvenanceMap};
use crate::issue::{DfmFix, DfmIssue};
use crate::rules::RulePack;

/// Run CNC checks.
pub fn run(
    brep: &BRepSolid,
    provenance: Option<&ProvenanceMap>,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
) {
    let process = Process::Cnc3Axis;

    if let Some(rule) = pack.rule("internal_radius_too_small") {
        let min_radius = rule.num("min_internal_radius_mm", 3.0);
        for cyl in geom::radii::cylinders(brep) {
            if cyl.radius_mm < min_radius {
                let anchor = geom::face_midpoint_and_normal(brep, cyl.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "cnc.internal_radius_too_small",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Cylindrical feature R{:.2} mm — smaller than min cutter R{:.2} mm",
                        cyl.radius_mm, min_radius
                    ),
                    anchor,
                    cyl.radius_mm,
                    min_radius,
                    "mm",
                )
                .with_explanation(
                    "Internal radii smaller than the cutter radius can't be machined. \
                     Raise the fillet, switch to EDM, or accept a corner-relief slot."
                        .into(),
                )
                .with_faces(vec![cyl.face]);
                if let Some(node) = provenance.and_then(|p| p.get(cyl.face)) {
                    issue = issue.with_origin(node);
                    issue = issue.with_fix(DfmFix::SetParam {
                        node,
                        path: "radius".into(),
                        value: serde_json::json!(min_radius),
                    });
                } else {
                    issue = issue.with_fix(DfmFix::Manual {
                        description: format!("Raise this radius to ≥ {:.2} mm.", min_radius),
                    });
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("thin_wall") {
        let min_wall = rule.num("min_wall_mm", 1.0);
        for sample in geom::thickness::sample_pairs(brep, -0.95) {
            if sample.thickness_mm < min_wall {
                let mut issue = DfmIssue::new(
                    "cnc.thin_wall",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Wall {:.2} mm — below CNC minimum {:.2} mm",
                        sample.thickness_mm, min_wall
                    ),
                    sample.anchor,
                    sample.thickness_mm,
                    min_wall,
                    "mm",
                )
                .with_explanation(
                    "Thin walls chatter and deflect under cutter load. Thicken \
                     the wall, switch to sheet metal, or accept the workholding cost."
                        .into(),
                )
                .with_faces(vec![sample.face_a, sample.face_b])
                .with_fix(DfmFix::Manual {
                    description: format!("Thicken to at least {:.2} mm.", min_wall),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face_a)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("tool_inaccessibility") {
        // v1 hardcoded to +Z; multi-axis ships in the follow-up.
        let threshold = rule.num("min_dot", 0.0);
        for sample in geom::accessibility::sample(brep, Vec3::new(0.0, 0.0, 1.0)) {
            if sample.dot_with_axis < threshold {
                let anchor = geom::face_midpoint_and_normal(brep, sample.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "cnc.tool_inaccessibility",
                    rule.severity_enum(),
                    process,
                    "Face is not reachable by a +Z tool axis (needs 3+2 or 5-axis)".into(),
                    anchor,
                    sample.dot_with_axis,
                    threshold,
                    "dot",
                )
                .with_explanation(
                    "On a 3-axis mill the spindle only reaches features visible \
                     from +Z. Flip the part for a second setup, switch to 3+2, \
                     or use a 5-axis machine."
                        .into(),
                )
                .with_faces(vec![sample.face])
                .with_fix(DfmFix::Manual {
                    description: "Plan a second setup or switch to a 5-axis machine.".into(),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }
}
