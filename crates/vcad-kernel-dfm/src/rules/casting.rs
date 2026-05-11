//! Casting checks (sand / investment).
//!
//! Casting reuses the draft + thickness samplers and adds hot-spot
//! detection: any wall thickness much larger than the part-wide
//! average is at risk of shrinkage cavities and needs a riser.

use vcad_kernel_cost::Process;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;

use crate::geom::{self, provenance::ProvenanceMap};
use crate::issue::{DfmFix, DfmIssue};
use crate::rules::{mold::parse_axis, RulePack};

/// Run casting checks.
pub fn run(
    brep: &BRepSolid,
    provenance: Option<&ProvenanceMap>,
    process: Process,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
) {
    debug_assert!(matches!(
        process,
        Process::CastingSand | Process::CastingInvestment
    ));

    let pull_dir = pack
        .rule("insufficient_draft")
        .and_then(|r| r.string("pull_dir"))
        .map(|s| parse_axis(&s))
        .unwrap_or_else(|| vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0));

    if let Some(rule) = pack.rule("insufficient_draft") {
        let min_draft = rule.num("min_draft_deg", 2.0);
        for sample in geom::draft::sample(brep, pull_dir) {
            if sample.draft_deg.abs() >= 85.0 {
                continue;
            }
            if sample.draft_deg.abs() < min_draft && sample.draft_deg > -90.0 {
                let anchor = geom::face_midpoint_and_normal(brep, sample.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "casting.insufficient_draft",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Draft {:.1}° — castings need ≥ {:.1}°",
                        sample.draft_deg.abs(),
                        min_draft
                    ),
                    anchor,
                    sample.draft_deg.abs(),
                    min_draft,
                    "deg",
                )
                .with_explanation(
                    "Sand and investment patterns need more draft than injection \
                     because the moldwall is fragile. 2–3° is typical."
                        .into(),
                )
                .with_faces(vec![sample.face])
                .with_fix(DfmFix::Manual {
                    description: format!("Add at least {:.1}° draft along the pull axis.", min_draft),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("min_section_thickness") {
        let min_section = rule.num("min_section_mm", 3.0);
        for sample in geom::thickness::sample_pairs(brep, -0.95) {
            if sample.thickness_mm < min_section {
                let mut issue = DfmIssue::new(
                    "casting.min_section_thickness",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Section {:.2} mm — casting needs ≥ {:.2} mm",
                        sample.thickness_mm, min_section
                    ),
                    sample.anchor,
                    sample.thickness_mm,
                    min_section,
                    "mm",
                )
                .with_explanation(
                    "Thin sections freeze before the metal can fill them, leaving \
                     misruns and cold shuts."
                        .into(),
                )
                .with_faces(vec![sample.face_a, sample.face_b])
                .with_fix(DfmFix::Manual {
                    description: format!("Thicken section to ≥ {:.2} mm.", min_section),
                });
                if let Some(node) = provenance.and_then(|p| p.get(sample.face_a)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("hot_spot") {
        let max_ratio = rule.num("max_thickness_ratio", 2.0);
        let samples = geom::thickness::sample_pairs(brep, -0.95);
        if samples.len() >= 2 {
            let nominal = samples
                .iter()
                .map(|s| s.thickness_mm)
                .sum::<f64>()
                / samples.len() as f64;
            if let Some(thickest) = samples
                .iter()
                .max_by(|a, b| a.thickness_mm.partial_cmp(&b.thickness_mm).unwrap())
            {
                let ratio = thickest.thickness_mm / nominal.max(1e-6);
                if ratio > max_ratio {
                    issues.push(
                        DfmIssue::new(
                            "casting.hot_spot",
                            rule.severity_enum(),
                            process,
                            format!(
                                "Hot spot: section {:.2}× nominal thickness (shrinkage risk)",
                                ratio
                            ),
                            thickest.anchor,
                            ratio,
                            max_ratio,
                            "ratio",
                        )
                        .with_explanation(
                            "Isolated thick sections freeze last and shrink onto \
                             themselves, leaving voids. Core them out or add a riser."
                                .into(),
                        )
                        .with_faces(vec![thickest.face_a, thickest.face_b])
                        .with_fix(DfmFix::Manual {
                            description: "Core out the thick section, or add a riser/feeder.".into(),
                        }),
                    );
                }
            }
        }
    }
}
