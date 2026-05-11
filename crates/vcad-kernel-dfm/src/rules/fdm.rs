//! FDM / SLA 3D-printing checks.

use vcad_kernel_cost::Process;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;

use crate::geom::{self, provenance::ProvenanceMap};
use crate::issue::{DfmFix, DfmIssue};
use crate::rules::RulePack;

/// Run the enabled rules in `pack` against `brep`, appending issues.
pub fn run(
    brep: &BRepSolid,
    provenance: Option<&ProvenanceMap>,
    process: Process,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
) {
    if let Some(rule) = pack.rule("thin_wall") {
        let min_wall = rule.num("min_wall_mm", 0.8);
        for sample in geom::thickness::sample_pairs(brep, -0.95) {
            if sample.thickness_mm < min_wall {
                let mut issue = DfmIssue::new(
                    "fdm.thin_wall",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Wall {:.2} mm — below printable minimum of {:.2} mm",
                        sample.thickness_mm, min_wall
                    ),
                    sample.anchor,
                    sample.thickness_mm,
                    min_wall,
                    "mm",
                )
                .with_explanation(
                    "FDM walls thinner than ~2 line widths can't bond cleanly and \
                     either fail to print or come out as loose strings."
                        .into(),
                )
                .with_faces(vec![sample.face_a, sample.face_b]);
                if let Some(node) = provenance.and_then(|p| p.get(sample.face_a)) {
                    issue = issue.with_origin(node);
                }
                issue = issue.with_fix(DfmFix::Manual {
                    description: format!(
                        "Increase wall thickness to at least {:.2} mm.",
                        min_wall
                    ),
                });
                issues.push(issue);
            }
        }
    }

    if let Some(rule) = pack.rule("steep_overhang") {
        let max_overhang_deg = rule.num("max_overhang_deg", 135.0);
        for sample in geom::overhang::sample(brep, max_overhang_deg) {
            let support_note = if sample.support_column_mm > 0.0 {
                format!(
                    " (support column ≈ {:.1} mm)",
                    sample.support_column_mm
                )
            } else {
                String::new()
            };
            let mut issue = DfmIssue::new(
                "fdm.steep_overhang",
                rule.severity_enum(),
                process,
                format!(
                    "Face leans {:.0}° from +Z — supports required{}",
                    sample.angle_from_up_deg, support_note
                ),
                sample.anchor,
                sample.angle_from_up_deg,
                max_overhang_deg,
                "deg",
            )
            .with_explanation(
                "Faces angled more than 45° below horizontal need support material \
                 to print without sagging. Re-orient the part or accept the support cost."
                    .into(),
            )
            .with_faces(vec![sample.face]);
            if let Some(node) = provenance.and_then(|p| p.get(sample.face)) {
                issue = issue.with_origin(node);
            }
            issue = issue.with_fix(DfmFix::Manual {
                description: "Re-orient the part so this face is closer to vertical, \
                              or enable supports in the slicer."
                    .into(),
            });
            issues.push(issue);
        }
    }

    if let Some(rule) = pack.rule("small_hole") {
        let min_diameter = rule.num("min_diameter_mm", 0.8);
        for cyl in geom::radii::cylinders(brep) {
            let diameter = cyl.radius_mm * 2.0;
            if diameter < min_diameter {
                let anchor = geom::face_midpoint_and_normal(brep, cyl.face)
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
                let mut issue = DfmIssue::new(
                    "fdm.small_hole",
                    rule.severity_enum(),
                    process,
                    format!(
                        "Hole Ø{:.2} mm may close during printing (min Ø{:.2} mm)",
                        diameter, min_diameter
                    ),
                    anchor,
                    diameter,
                    min_diameter,
                    "mm",
                )
                .with_explanation(
                    "Holes below ~2× nozzle width often close up from extrusion \
                     spread. Enlarge or post-drill."
                        .into(),
                )
                .with_faces(vec![cyl.face]);
                if let Some(node) = provenance.and_then(|p| p.get(cyl.face)) {
                    issue = issue.with_origin(node);
                }
                issue = issue.with_fix(DfmFix::Manual {
                    description: format!("Enlarge hole to Ø{:.2} mm minimum.", min_diameter),
                });
                issues.push(issue);
            }
        }
    }
}
