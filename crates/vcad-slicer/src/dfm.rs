//! Design for Manufacturing (DFM) printability checks.
//!
//! Analyzes a solid against a printer profile to find issues that will
//! cause print failures or poor quality. Returns face/edge IDs so the
//! UI can highlight problem areas in the viewport.

use serde::{Deserialize, Serialize};
use vcad_kernel_geom::{CylinderSurface, SurfaceKind};
use vcad_kernel_math::Point2;
use vcad_kernel_primitives::BRepSolid;

use crate::smart_defaults::PrinterParams;

/// Severity of a DFM warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DfmSeverity {
    /// Will not print at all.
    Error,
    /// Will print but with poor quality.
    Warning,
    /// Informational suggestion.
    Info,
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
pub fn check_printability(brep: &BRepSolid, params: &PrinterParams) -> DfmResult {
    let mut warnings = Vec::new();
    let nozzle = params.nozzle_diameter;
    let min_line_width = nozzle * 0.8;

    // Check build volume
    let (bbox_min, bbox_max) = compute_bbox(brep);
    let size = [
        bbox_max[0] - bbox_min[0],
        bbox_max[1] - bbox_min[1],
        bbox_max[2] - bbox_min[2],
    ];

    if size[0] > params.bed_x || size[1] > params.bed_y || size[2] > params.bed_z {
        warnings.push(DfmWarning {
            severity: DfmSeverity::Error,
            kind: "exceeds_build_volume".into(),
            message: format!(
                "Part ({:.1} x {:.1} x {:.1}mm) exceeds build volume ({:.0} x {:.0} x {:.0}mm)",
                size[0], size[1], size[2],
                params.bed_x, params.bed_y, params.bed_z
            ),
            face_indices: Vec::new(),
            suggestion: Some("Scale down or choose a larger printer".into()),
        });
    }

    // Analyze each face
    let z_up = vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0);
    let mut thin_wall_faces = Vec::new();
    let mut steep_overhang_faces = Vec::new();
    let mut small_hole_faces = Vec::new();

    let face_entries: Vec<_> = brep.topology.faces.iter().collect();

    for (face_idx, (_face_id, face)) in face_entries.iter().enumerate() {
        let surface = &brep.geometry.surfaces[face.surface_index];
        let kind = surface.surface_type();

        // Sample normal
        let ((u_min, u_max), (v_min, v_max)) = surface.domain();
        let normal_dir = surface.normal(Point2::new(
            (u_min + u_max) / 2.0,
            (v_min + v_max) / 2.0,
        ));
        let normal = vcad_kernel_math::Vec3::new(
            normal_dir.as_ref().x,
            normal_dir.as_ref().y,
            normal_dir.as_ref().z,
        );
        let face_normal = if face.orientation == vcad_kernel_topo::Orientation::Reversed {
            -normal
        } else {
            normal
        };

        // Steep overhang check (>60° from vertical is problematic even with support)
        let dot = face_normal.dot(&z_up);
        let angle_from_up = dot.clamp(-1.0, 1.0).acos().to_degrees();
        if angle_from_up > 150.0 {
            // Nearly flat bottom face — needs support
            steep_overhang_faces.push(face_idx);
        }

        // Small hole check
        if kind == SurfaceKind::Cylinder {
            if let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() {
                let diameter = cyl.radius * 2.0;
                if diameter < nozzle * 2.0 {
                    small_hole_faces.push((face_idx, diameter));
                }
            }
        }
    }

    // Wall thickness: check opposing parallel faces
    for i in 0..face_entries.len() {
        for j in (i + 1)..face_entries.len() {
            let (_, fi) = face_entries[i];
            let (_, fj) = face_entries[j];
            let si = &brep.geometry.surfaces[fi.surface_index];
            let sj = &brep.geometry.surfaces[fj.surface_index];

            let ((ui_min, ui_max), (vi_min, vi_max)) = si.domain();
            let ((uj_min, uj_max), (vj_min, vj_max)) = sj.domain();

            let ni = si.normal(Point2::new((ui_min + ui_max) / 2.0, (vi_min + vi_max) / 2.0));
            let nj = sj.normal(Point2::new((uj_min + uj_max) / 2.0, (vj_min + vj_max) / 2.0));

            let ni_vec = vcad_kernel_math::Vec3::new(ni.as_ref().x, ni.as_ref().y, ni.as_ref().z);
            let nj_vec = vcad_kernel_math::Vec3::new(nj.as_ref().x, nj.as_ref().y, nj.as_ref().z);

            // Account for face orientation
            let ni_oriented = if fi.orientation == vcad_kernel_topo::Orientation::Reversed { -ni_vec } else { ni_vec };
            let nj_oriented = if fj.orientation == vcad_kernel_topo::Orientation::Reversed { -nj_vec } else { nj_vec };

            if ni_oriented.dot(&nj_oriented) < -0.95 {
                let pi = si.evaluate(Point2::new((ui_min + ui_max) / 2.0, (vi_min + vi_max) / 2.0));
                let pj = sj.evaluate(Point2::new((uj_min + uj_max) / 2.0, (vj_min + vj_max) / 2.0));
                let dist = ((pi.x - pj.x).powi(2) + (pi.y - pj.y).powi(2) + (pi.z - pj.z).powi(2)).sqrt();

                if dist > 0.01 && dist < min_line_width {
                    thin_wall_faces.push(i);
                    thin_wall_faces.push(j);
                }
            }
        }
    }

    // Emit warnings
    if !thin_wall_faces.is_empty() {
        thin_wall_faces.sort();
        thin_wall_faces.dedup();
        warnings.push(DfmWarning {
            severity: DfmSeverity::Error,
            kind: "thin_wall".into(),
            message: format!(
                "Wall too thin to print ({} face(s) below {:.2}mm min)",
                thin_wall_faces.len(), min_line_width
            ),
            face_indices: thin_wall_faces,
            suggestion: Some(format!("Increase wall thickness to at least {:.2}mm", min_line_width)),
        });
    }

    if !steep_overhang_faces.is_empty() {
        warnings.push(DfmWarning {
            severity: DfmSeverity::Warning,
            kind: "steep_overhang".into(),
            message: format!(
                "{} face(s) with steep overhang (>60°) — needs support",
                steep_overhang_faces.len()
            ),
            face_indices: steep_overhang_faces,
            suggestion: Some("Enable support structures or reorient part".into()),
        });
    }

    for (face_idx, diameter) in &small_hole_faces {
        warnings.push(DfmWarning {
            severity: DfmSeverity::Warning,
            kind: "small_hole".into(),
            message: format!("Small hole ({:.2}mm) may close during printing", diameter),
            face_indices: vec![*face_idx],
            suggestion: Some(format!("Enlarge hole to at least {:.2}mm", nozzle * 2.0)),
        });
    }

    // Compute score
    let error_count = warnings.iter().filter(|w| w.severity == DfmSeverity::Error).count();
    let warning_count = warnings.iter().filter(|w| w.severity == DfmSeverity::Warning).count();
    let score = 100u32.saturating_sub((error_count * 30 + warning_count * 10) as u32);

    DfmResult { warnings, score }
}

fn compute_bbox(brep: &BRepSolid) -> ([f64; 3], [f64; 3]) {
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

        let errors: Vec<_> = result.warnings.iter().filter(|w| w.severity == DfmSeverity::Error).collect();
        assert!(errors.is_empty());
        assert!(result.score >= 70);
    }

    #[test]
    fn test_exceeds_build_volume() {
        let brep = make_cube(200.0, 200.0, 200.0);
        let result = check_printability(&brep, &a1_mini_params());

        let volume_errors: Vec<_> = result.warnings.iter()
            .filter(|w| w.kind == "exceeds_build_volume")
            .collect();
        assert!(!volume_errors.is_empty());
    }
}
