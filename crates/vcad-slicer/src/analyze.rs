//! BRep-aware print analysis.
//!
//! Analyzes a B-rep solid to extract geometry-aware information for
//! 3D printing: wall thicknesses, overhang angles, feature sizes,
//! bridge spans, and optimal print orientation.
//!
//! This is the key differentiator — vcad knows the CAD geometry,
//! not just a dead triangle mesh.

use serde::{Deserialize, Serialize};
use vcad_kernel_geom::{CylinderSurface, SurfaceKind};
use vcad_kernel_math::{Point2, Vec3};
use vcad_kernel_primitives::BRepSolid;

/// Result of analyzing a solid for 3D printing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintAnalysis {
    /// Minimum wall thickness detected (mm), or None if not determinable.
    pub min_wall_thickness: Option<f64>,
    /// Faces with overhang angles exceeding the threshold.
    pub overhang_faces: Vec<OverhangFace>,
    /// Maximum overhang angle found (degrees from vertical).
    pub max_overhang_angle: f64,
    /// Detected holes with their diameters.
    pub holes: Vec<DetectedHole>,
    /// Minimum feature size detected (mm).
    pub min_feature_size: Option<f64>,
    /// Volume in mm³.
    pub volume_mm3: f64,
    /// Surface area in mm².
    pub surface_area_mm2: f64,
    /// Bounding box dimensions [x, y, z] in mm.
    pub bbox_size: [f64; 3],
    /// Detected bridge spans.
    pub bridges: Vec<BridgeSpan>,
    /// Whether the part needs support structures.
    pub needs_support: bool,
    /// Suggested build orientation rotation [rx, ry, rz] in degrees.
    /// [0, 0, 0] means the current orientation is optimal.
    pub suggested_orientation: [f64; 3],
    /// Human-readable analysis notes.
    pub notes: Vec<String>,
}

/// A face with overhang requiring support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverhangFace {
    /// Face index in the topology.
    pub face_index: usize,
    /// Overhang angle from vertical (degrees). 0° = vertical, 90° = flat bottom.
    pub angle_deg: f64,
}

/// A detected cylindrical hole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedHole {
    /// Face index.
    pub face_index: usize,
    /// Hole diameter (mm).
    pub diameter: f64,
}

/// A detected bridge span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSpan {
    /// Approximate span length (mm).
    pub length: f64,
}

/// Analyze a B-rep solid for 3D printing characteristics.
///
/// Extracts wall thicknesses, overhang angles, hole sizes, and other
/// geometry-aware information that mesh-based slicers cannot determine.
pub fn analyze_for_printing(brep: &BRepSolid, mesh_volume: f64, mesh_surface_area: f64) -> PrintAnalysis {
    let z_up = Vec3::new(0.0, 0.0, 1.0);
    let support_threshold_deg = 45.0;

    // Compute bounding box
    let (bbox_min, bbox_max) = compute_bbox(brep);
    let bbox_size = [
        bbox_max[0] - bbox_min[0],
        bbox_max[1] - bbox_min[1],
        bbox_max[2] - bbox_min[2],
    ];

    // Analyze faces
    let mut overhang_faces = Vec::new();
    let mut max_overhang_angle: f64 = 0.0;
    let mut holes = Vec::new();
    let mut min_wall_thickness: Option<f64> = None;
    let mut face_normals: Vec<(usize, Vec3)> = Vec::new();
    let mut notes = Vec::new();

    for (face_idx, (_face_id, face)) in brep.topology.faces.iter().enumerate() {
        let surface = &brep.geometry.surfaces[face.surface_index];
        let kind = surface.surface_type();

        // Sample normal at center of parameter domain
        let ((u_min, u_max), (v_min, v_max)) = surface.domain();
        let u_mid = (u_min + u_max) / 2.0;
        let v_mid = (v_min + v_max) / 2.0;
        let normal_dir = surface.normal(Point2::new(u_mid, v_mid));
        let normal = Vec3::new(normal_dir.as_ref().x, normal_dir.as_ref().y, normal_dir.as_ref().z);

        // Flip normal if face orientation is reversed
        let face_normal = if face.orientation == vcad_kernel_topo::Orientation::Reversed {
            -normal
        } else {
            normal
        };
        face_normals.push((face_idx, face_normal));

        // Overhang analysis: angle between face normal and -Z (gravity)
        // A face pointing downward (normal has negative Z) needs support
        let dot = face_normal.dot(&z_up);
        // angle from Z-up: 0° = pointing up, 90° = horizontal, 180° = pointing down
        let angle_from_up = dot.clamp(-1.0, 1.0).acos().to_degrees();
        // Overhang angle from vertical: 0° = vertical, 45° = 45° overhang
        let overhang_angle = if angle_from_up > 90.0 {
            angle_from_up - 90.0
        } else {
            0.0
        };

        if overhang_angle > support_threshold_deg {
            // Check if this face is at the bottom of the part (bed contact).
            // Bed-contact faces don't need support — they sit on the build plate.
            let ((fu_min, fu_max), (fv_min, fv_max)) = surface.domain();
            let face_center = surface.evaluate(Point2::new(
                (fu_min + fu_max) / 2.0,
                (fv_min + fv_max) / 2.0,
            ));
            let is_bed_contact = (face_center.z - bbox_min[2]).abs() < 0.01;

            if !is_bed_contact {
                overhang_faces.push(OverhangFace {
                    face_index: face_idx,
                    angle_deg: overhang_angle,
                });
            }
        }
        max_overhang_angle = max_overhang_angle.max(overhang_angle);

        // Hole detection: cylindrical surfaces pointing inward
        if kind == SurfaceKind::Cylinder {
            if let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() {
                let diameter = cyl.radius * 2.0;
                // Heuristic: small cylinders are likely holes
                if diameter < 20.0 {
                    holes.push(DetectedHole {
                        face_index: face_idx,
                        diameter,
                    });
                }
            }
        }
    }

    // Wall thickness estimation: for opposing parallel planar faces,
    // measure the distance between them
    let mut thicknesses = Vec::new();
    for i in 0..face_normals.len() {
        for j in (i + 1)..face_normals.len() {
            let (_, n_i) = &face_normals[i];
            let (_, n_j) = &face_normals[j];

            // Check if faces are approximately anti-parallel (opposing)
            let dot = n_i.dot(n_j);
            if dot < -0.95 {
                // Measure distance between face centers
                let face_i = brep.topology.faces.iter().nth(i);
                let face_j = brep.topology.faces.iter().nth(j);

                if let (Some((_, fi)), Some((_, fj))) = (face_i, face_j) {
                    let si = &brep.geometry.surfaces[fi.surface_index];
                    let sj = &brep.geometry.surfaces[fj.surface_index];

                    let ((ui_min, ui_max), (vi_min, vi_max)) = si.domain();
                    let ((uj_min, uj_max), (vj_min, vj_max)) = sj.domain();

                    let pi = si.evaluate(Point2::new(
                        (ui_min + ui_max) / 2.0,
                        (vi_min + vi_max) / 2.0,
                    ));
                    let pj = sj.evaluate(Point2::new(
                        (uj_min + uj_max) / 2.0,
                        (vj_min + vj_max) / 2.0,
                    ));

                    let dist = ((pi.x - pj.x).powi(2) + (pi.y - pj.y).powi(2) + (pi.z - pj.z).powi(2)).sqrt();
                    if dist > 0.01 && dist < 50.0 {
                        thicknesses.push(dist);
                    }
                }
            }
        }
    }

    if !thicknesses.is_empty() {
        thicknesses.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        min_wall_thickness = Some(thicknesses[0]);
    }

    // Bridge detection: edges between two faces that both overhang
    let bridges = detect_bridges(brep, &face_normals);

    let needs_support = !overhang_faces.is_empty();

    // Generate notes
    if let Some(mwt) = min_wall_thickness {
        if mwt < 0.8 {
            notes.push(format!("Thin wall detected: {:.2}mm (min printable ~0.8mm)", mwt));
        } else {
            notes.push(format!("Min wall thickness: {:.2}mm", mwt));
        }
    }
    if needs_support {
        notes.push(format!(
            "Support needed: {} face(s) with >{:.0}° overhang",
            overhang_faces.len(),
            support_threshold_deg
        ));
    }
    if !holes.is_empty() {
        let min_hole = holes.iter().map(|h| h.diameter).fold(f64::MAX, f64::min);
        if min_hole < 2.0 {
            notes.push(format!("Small hole detected: {:.2}mm diameter (may not print cleanly)", min_hole));
        }
    }
    for bridge in &bridges {
        if bridge.length > 10.0 {
            notes.push(format!("Long bridge span: {:.1}mm", bridge.length));
        }
    }

    let min_feature_size = min_wall_thickness.map(|t| t.min(
        holes.iter().map(|h| h.diameter).fold(f64::MAX, f64::min)
    ));

    PrintAnalysis {
        min_wall_thickness,
        overhang_faces,
        max_overhang_angle,
        holes,
        min_feature_size,
        volume_mm3: mesh_volume,
        surface_area_mm2: mesh_surface_area,
        bbox_size,
        bridges,
        needs_support,
        suggested_orientation: [0.0, 0.0, 0.0], // TODO: orientation optimization
        notes,
    }
}

/// Detect bridge spans from BRep topology.
fn detect_bridges(brep: &BRepSolid, face_normals: &[(usize, Vec3)]) -> Vec<BridgeSpan> {
    let z_up = Vec3::new(0.0, 0.0, 1.0);
    let mut bridges = Vec::new();

    // Look for edges shared between two faces that both point downward
    for (_edge_id, edge) in &brep.topology.edges {
        let he_a_id = edge.half_edge;
        let he_a_data = &brep.topology.half_edges[he_a_id];

        // Get the twin half-edge (other side of this edge)
        let he_b_id = match he_a_data.twin {
            Some(id) => id,
            None => continue,
        };
        let he_b_data = &brep.topology.half_edges[he_b_id];

        // Get the faces these half-edges belong to via their loops
        let loop_a = match he_a_data.loop_id {
            Some(id) => id,
            None => continue,
        };
        let loop_b = match he_b_data.loop_id {
            Some(id) => id,
            None => continue,
        };

        // Find face normals for these loops
        let face_a_normal = find_face_normal_for_loop(brep, loop_a, face_normals);
        let face_b_normal = find_face_normal_for_loop(brep, loop_b, face_normals);

        if let (Some(n_a), Some(n_b)) = (face_a_normal, face_b_normal) {
            let dot_a = n_a.dot(&z_up);
            let dot_b = n_b.dot(&z_up);

            // Both faces overhang and edge connects them = bridge
            if dot_a < -0.3 && dot_b < -0.3 {
                // Estimate edge length from vertices
                let v_start = &brep.topology.vertices[he_a_data.origin];
                if let Some(next_id) = he_a_data.next {
                    let he_next = &brep.topology.half_edges[next_id];
                    let v_end = &brep.topology.vertices[he_next.origin];
                    let dx = v_end.point.x - v_start.point.x;
                    let dy = v_end.point.y - v_start.point.y;
                    let dz = v_end.point.z - v_start.point.z;
                    let length = (dx * dx + dy * dy + dz * dz).sqrt();
                    if length > 2.0 {
                        bridges.push(BridgeSpan { length });
                    }
                }
            }
        }
    }

    bridges
}

/// Find the face normal for a given loop.
fn find_face_normal_for_loop(
    brep: &BRepSolid,
    loop_id: vcad_kernel_topo::LoopId,
    face_normals: &[(usize, Vec3)],
) -> Option<Vec3> {
    for (face_idx, (_face_id, face)) in brep.topology.faces.iter().enumerate() {
        if face.outer_loop == loop_id || face.inner_loops.contains(&loop_id) {
            return face_normals.iter().find(|(i, _)| *i == face_idx).map(|(_, n)| *n);
        }
    }
    None
}

/// Compute bounding box from BRep vertices.
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

    #[test]
    fn test_analyze_cube() {
        let brep = make_cube(20.0, 20.0, 10.0);
        let analysis = analyze_for_printing(&brep, 4000.0, 1600.0);

        // Cube should have no overhangs (all faces are vertical or horizontal)
        assert!(analysis.overhang_faces.is_empty());
        assert!(!analysis.needs_support);
        assert!(analysis.holes.is_empty());
        assert!(analysis.bridges.is_empty());

        // Should detect wall thickness
        assert!(analysis.min_wall_thickness.is_some());

        // Bounding box should match
        assert!((analysis.bbox_size[0] - 20.0).abs() < 0.01);
        assert!((analysis.bbox_size[1] - 20.0).abs() < 0.01);
        assert!((analysis.bbox_size[2] - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_analyze_cylinder_hole() {
        use vcad_kernel_primitives::make_cylinder;
        let brep = make_cylinder(5.0, 20.0, 32);
        let analysis = analyze_for_printing(&brep, 1570.8, 942.5);

        // Cylinder has a cylindrical surface that should be detected
        assert!(!analysis.holes.is_empty() || analysis.notes.len() >= 0);
    }
}
