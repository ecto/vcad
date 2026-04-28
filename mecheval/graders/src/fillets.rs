//! Fillet / chamfer detection.
//!
//! v0.0 scope: a fillet manifests as a cylindrical surface whose axis
//! does NOT align with Z (Z-aligned cylinders are most likely hole walls
//! or upright body cylinders, picked up by `holes` instead). For fillets
//! between two flat faces meeting at a straight edge, the blend surface
//! is a cylinder whose axis runs along the original edge.
//!
//! Known limits:
//! - Toroidal fillets (where the original edge is curved) are NOT yet
//!   detected; we'd need to walk torus surfaces and match the minor
//!   radius. Future work.
//! - The check spec carries an `edge_class` string ("inside_corner",
//!   "outside_corner", etc.). v0.0 ignores it — any non-Z cylinder of
//!   the right radius counts.
//! - We don't yet require the cylinder to be tangent to two adjacent
//!   non-cylindrical faces, which is what would make this rigorously
//!   "this is a fillet, not a stray cylindrical body". For the seed
//!   corpus the diameter filter is enough; we'll tighten when a task
//!   needs to differentiate.

use crate::eval::EvalSnapshot;
use vcad_kernel_geom::{CylinderSurface, SurfaceKind};

/// One detected fillet candidate, in kernel-space mm.
#[derive(Debug, Clone, Copy)]
pub struct Fillet {
    /// Cylinder radius (= fillet radius for a true fillet).
    pub radius: f64,
    /// Axis direction (unit), in kernel space.
    pub axis: [f64; 3],
    /// A point on the axis (cylinder.center).
    pub axis_point: [f64; 3],
}

/// Cosine threshold below which an axis is considered "not Z-aligned".
const Z_AXIS_COS_TOL: f64 = 0.99;

/// Find non-Z-axis cylindrical surfaces whose radius matches
/// `target_radius_mm` within `tolerance_mm`. De-duplicated by axis line
/// + radius within tolerance.
pub fn find_non_z_cylinders(
    snap: &EvalSnapshot,
    target_radius_mm: f64,
    tolerance_mm: f64,
) -> Vec<Fillet> {
    let mut out: Vec<Fillet> = Vec::new();
    for solid in &snap.solids {
        let Some(brep) = solid.as_brep() else {
            continue;
        };
        for (_face_id, face) in &brep.topology.faces {
            let Some(surface) = brep.geometry.surfaces.get(face.surface_index) else {
                continue;
            };
            if surface.surface_type() != SurfaceKind::Cylinder {
                continue;
            }
            let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() else {
                continue;
            };
            let axis = cyl.axis.as_ref();
            if axis.z.abs() >= Z_AXIS_COS_TOL {
                continue; // Z-aligned — let `holes` handle it.
            }
            if (cyl.radius - target_radius_mm).abs() > tolerance_mm {
                continue;
            }
            let cand = Fillet {
                radius: cyl.radius,
                axis: [axis.x, axis.y, axis.z],
                axis_point: [cyl.center.x, cyl.center.y, cyl.center.z],
            };
            if !out.iter().any(|f| same_fillet(f, &cand, tolerance_mm)) {
                out.push(cand);
            }
        }
    }
    out
}

fn same_fillet(a: &Fillet, b: &Fillet, tol: f64) -> bool {
    if (a.radius - b.radius).abs() > tol {
        return false;
    }
    // Axes parallel (sign-invariant): |a · b| close to 1.
    let dot = a.axis[0] * b.axis[0] + a.axis[1] * b.axis[1] + a.axis[2] * b.axis[2];
    if dot.abs() < 0.99 {
        return false;
    }
    // Axis points within tolerance, projected perpendicular to the axis.
    let dx = a.axis_point[0] - b.axis_point[0];
    let dy = a.axis_point[1] - b.axis_point[1];
    let dz = a.axis_point[2] - b.axis_point[2];
    let along = dx * a.axis[0] + dy * a.axis[1] + dz * a.axis[2];
    let perp_x = dx - along * a.axis[0];
    let perp_y = dy - along * a.axis[1];
    let perp_z = dz - along * a.axis[2];
    let perp_dist = (perp_x * perp_x + perp_y * perp_y + perp_z * perp_z).sqrt();
    perp_dist <= tol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluate_vcad;

    /// Hand-author a tiny .vcad with one Y-axis cylinder of radius 5
    /// (a stand-in for what a true fillet between two flat faces would
    /// produce). Exercises the non-Z filter + radius match.
    fn cyl_along_y_vcad() -> String {
        // vcad's Cylinder primitive is Z-axis; rotate -90° around X to put
        // the axis along Y. Rotate angles are degrees in the IR.
        r#"{
            "version": "0.1",
            "nodes": {
                "1": {"id": 1, "name": "cyl", "op": {"type": "Cylinder", "radius": 5.0, "height": 30.0, "segments": 32}},
                "2": {"id": 2, "name": "rot", "op": {"type": "Rotate", "child": 1,
                                                       "angles": {"x": -90.0, "y": 0.0, "z": 0.0}}}
            },
            "materials": {},
            "part_materials": {},
            "roots": [{"root": 2, "material": "default"}]
        }"#
            .into()
    }

    #[test]
    fn finds_a_y_axis_cylinder_at_target_radius() {
        let snap = evaluate_vcad(&cyl_along_y_vcad());
        assert!(snap.fatal.is_none(), "fatal: {:?}", snap.fatal);
        let found = find_non_z_cylinders(&snap, 5.0, 0.1);
        assert_eq!(found.len(), 1, "{:?}", found);
        assert!((found[0].radius - 5.0).abs() < 0.01);
    }

    #[test]
    fn ignores_wrong_radius() {
        let snap = evaluate_vcad(&cyl_along_y_vcad());
        let found = find_non_z_cylinders(&snap, 12.0, 0.1);
        assert!(found.is_empty());
    }

    #[test]
    fn ignores_z_aligned_cylinders() {
        // A plain Z-axis cylinder of radius 5 must NOT match.
        let vcad = r#"{
            "version": "0.1",
            "nodes": {"1":{"id":1,"name":"c","op":{"type":"Cylinder","radius":5.0,"height":10.0,"segments":32}}},
            "materials": {}, "part_materials": {},
            "roots": [{"root":1,"material":"default"}]
        }"#;
        let snap = evaluate_vcad(vcad);
        let found = find_non_z_cylinders(&snap, 5.0, 0.1);
        assert!(found.is_empty(), "{:?}", found);
    }
}
