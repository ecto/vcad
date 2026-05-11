//! Sheet-metal checks.
//!
//! v1 ships a minimal version that flags small cylindrical features as
//! holes that may be too close to a bend (without yet using
//! `vcad-kernel-sheet`'s feature recognizer). The real implementation
//! consumes the sheet-metal feature tree; that's deferred to a
//! follow-up alongside the deep geom samplers.

use vcad_kernel_cost::Process;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;

use crate::geom::{self, provenance::ProvenanceMap};
use crate::issue::{DfmFix, DfmIssue};
use crate::rules::RulePack;

/// Run sheet-metal checks.
pub fn run(
    brep: &BRepSolid,
    provenance: Option<&ProvenanceMap>,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
) {
    let process = Process::SheetMetal;

    if let Some(rule) = pack.rule("small_hole") {
        let min_diameter = rule.num("min_diameter_mm", 1.0);
        for cyl in geom::radii::cylinders(brep) {
            let diameter = cyl.radius_mm * 2.0;
            if diameter < min_diameter {
                let anchor = geom::face_midpoint_and_normal(brep, cyl.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "sheet.small_hole",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Hole Ø{:.2} mm below sheet-metal minimum Ø{:.2} mm",
                        diameter, min_diameter
                    ),
                    anchor,
                    diameter,
                    min_diameter,
                    "mm",
                )
                .with_explanation(
                    "Punched holes smaller than the sheet thickness deform the \
                     metal around them and shorten the punch life. Drilled holes \
                     are safer but slower."
                        .into(),
                )
                .with_faces(vec![cyl.face])
                .with_fix(DfmFix::Manual {
                    description: format!("Enlarge to Ø{:.2} mm or switch to drilled.", min_diameter),
                });
                if let Some(node) = provenance.and_then(|p| p.get(cyl.face)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("thin_blank") {
        let min_thickness = rule.num("min_thickness_mm", 0.5);
        let samples = geom::thickness::sample_pairs(brep, -0.95);
        if let Some(min_sample) = samples
            .iter()
            .min_by(|a, b| a.thickness_mm.partial_cmp(&b.thickness_mm).unwrap())
        {
            if min_sample.thickness_mm < min_thickness {
                let mut issue = DfmIssue::new(
                    "sheet.thin_blank",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Sheet thickness {:.2} mm below minimum {:.2} mm",
                        min_sample.thickness_mm, min_thickness
                    ),
                    min_sample.anchor,
                    min_sample.thickness_mm,
                    min_thickness,
                    "mm",
                )
                .with_explanation(
                    "Sheet metal thinner than the supplier's minimum stock won't \
                     bend repeatably and tears at punched edges."
                        .into(),
                )
                .with_faces(vec![min_sample.face_a, min_sample.face_b])
                .with_fix(DfmFix::Manual {
                    description: format!("Use ≥ {:.2} mm sheet stock.", min_thickness),
                });
                if let Some(node) = provenance.and_then(|p| p.get(min_sample.face_a)) {
                    issue = issue.with_origin(node);
                }
                issues.push(issue);
            }
        }
    }
}
