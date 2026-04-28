//! Cylindrical-feature detection for `hole_count` / `hole_positions`.
//!
//! v0.0 scope: find Z-axis-parallel cylindrical surfaces of a target
//! diameter, cluster co-axial faces into single physical holes, return the
//! list as `(x, y, radius)` tuples in kernel space (mm). The seed corpus
//! is all Z-aligned through-holes, which keeps this tractable.
//!
//! Known limits (will revisit when corpus grows):
//! - Only Z-axis cylinders are detected; arbitrary-axis holes are ignored.
//! - We don't distinguish hole walls (inner) from outer cylindrical bodies
//!   (e.g. a flange's outer rim). The diameter filter in the check spec
//!   handles this naturally for our seed corpus — every spec calls out
//!   the diameter explicitly, and inner vs outer surfaces of the same
//!   diameter on the same axis are treated as the same hole. This will
//!   need an orientation-aware split when we add tasks where inner and
//!   outer cylinders share a diameter.

use crate::eval::EvalSnapshot;
use vcad_kernel_geom::{CylinderSurface, SurfaceKind};
use vcad_kernel_topo::Orientation;

/// One detected hole, in kernel-space mm.
#[derive(Debug, Clone, Copy)]
pub struct Hole {
    /// X coordinate of the cylinder axis at z = 0.
    pub x: f64,
    /// Y coordinate of the cylinder axis at z = 0.
    pub y: f64,
    /// Cylinder radius.
    pub radius: f64,
}

/// Tolerance used to call two cylindrical faces "the same hole" — they
/// must agree in XY position and radius within this many mm.
const MERGE_TOL_MM: f64 = 0.1;

/// Cosine of the smallest angle that still counts as "Z-aligned". cos(8°)
/// ≈ 0.9903 — generous enough to absorb numerical drift in boolean
/// outputs without admitting genuinely off-axis cylinders.
const Z_AXIS_COS_TOL: f64 = 0.99;

/// Find every Z-axis-parallel cylindrical *hole* (concave / inward-facing
/// cylindrical face) whose radius matches `target_diameter_mm / 2` within
/// `diameter_tol_mm / 2`. Cluster co-axial faces into one entry per
/// physical hole.
///
/// Distinguishes holes from protrusions via face orientation:
/// - Solid primitives create cylindrical lateral faces with
///   `Orientation::Forward` — the face normal points *outward* from the
///   axis, so this is the OUTSIDE of a solid cylinder body or a
///   protrusion sticking up.
/// - Boolean `Difference(body, cylinder)` keeps the cylinder's lateral
///   face but reverses its orientation — the face normal then points
///   *inward* toward the axis, which is the wall of a void/hole.
///
/// We only count `Orientation::Reversed` cylindrical faces, so a model
/// that mistakenly leaves a tower of cylinders sticking up out of a
/// plate (a real Claude failure mode on the flanged-cap task) doesn't
/// get credit for "drilling" holes that are actually protrusions.
pub fn find_z_holes(
    snap: &EvalSnapshot,
    target_diameter_mm: f64,
    diameter_tol_mm: f64,
) -> Vec<Hole> {
    let target_radius = target_diameter_mm / 2.0;
    let radius_tol = diameter_tol_mm / 2.0;
    let mut holes: Vec<Hole> = Vec::new();

    for solid in &snap.solids {
        let Some(brep) = solid.as_brep() else {
            continue;
        };
        for (_face_id, face) in &brep.topology.faces {
            // Only count concave (inward-facing) cylindrical faces.
            if face.orientation != Orientation::Reversed {
                continue;
            }
            let Some(surface) = brep.geometry.surfaces.get(face.surface_index) else {
                continue;
            };
            if surface.surface_type() != SurfaceKind::Cylinder {
                continue;
            }
            let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() else {
                continue;
            };
            // Z-aligned (sign-invariant — cylinders have no preferred axis direction).
            let axis = cyl.axis.as_ref();
            if axis.z.abs() < Z_AXIS_COS_TOL {
                continue;
            }
            if (cyl.radius - target_radius).abs() > radius_tol {
                continue;
            }
            let cand = Hole {
                x: cyl.center.x,
                y: cyl.center.y,
                radius: cyl.radius,
            };
            if !holes.iter().any(|h| same_hole(h, &cand)) {
                holes.push(cand);
            }
        }
    }
    holes
}

fn same_hole(a: &Hole, b: &Hole) -> bool {
    (a.x - b.x).abs() <= MERGE_TOL_MM
        && (a.y - b.y).abs() <= MERGE_TOL_MM
        && (a.radius - b.radius).abs() <= MERGE_TOL_MM
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluate_vcad;

    fn plate_with_holes_vcad() -> String {
        // 50×30×10 plate at origin, four 3mm-diameter holes 5mm in from
        // each corner of the top face, all axes parallel to Z. Matches
        // the geometry asserted by tasks/a1-plate-01.json.
        r#"{
  "version": "0.1",
  "nodes": {
    "1": {"id": 1, "name": "plate", "op": {"type": "Cube", "size": {"x": 50.0, "y": 30.0, "z": 10.0}}},
    "2": {"id": 2, "name": "plate_c", "op": {"type": "Translate", "child": 1, "offset": {"x": -25.0, "y": -15.0, "z": 0.0}}},
    "3": {"id": 3, "name": "drill", "op": {"type": "Cylinder", "radius": 1.5, "height": 20.0, "segments": 32}},
    "4": {"id": 4, "name": "drill_lower", "op": {"type": "Translate", "child": 3, "offset": {"x": 0.0, "y": 0.0, "z": -5.0}}},
    "5": {"id": 5, "name": "h_pp", "op": {"type": "Translate", "child": 4, "offset": {"x": 20.0, "y": 10.0, "z": 0.0}}},
    "6": {"id": 6, "name": "h_pn", "op": {"type": "Translate", "child": 4, "offset": {"x": 20.0, "y": -10.0, "z": 0.0}}},
    "7": {"id": 7, "name": "h_np", "op": {"type": "Translate", "child": 4, "offset": {"x": -20.0, "y": 10.0, "z": 0.0}}},
    "8": {"id": 8, "name": "h_nn", "op": {"type": "Translate", "child": 4, "offset": {"x": -20.0, "y": -10.0, "z": 0.0}}},
    "9":  {"id": 9,  "name": "u1", "op": {"type": "Union", "left": 5, "right": 6}},
    "10": {"id": 10, "name": "u2", "op": {"type": "Union", "left": 7, "right": 8}},
    "11": {"id": 11, "name": "u",  "op": {"type": "Union", "left": 9, "right": 10}},
    "12": {"id": 12, "name": "out", "op": {"type": "Difference", "left": 2, "right": 11}}
  },
  "materials": {},
  "part_materials": {},
  "roots": [{"root": 12, "material": "default"}]
}"#
            .into()
    }

    #[test]
    fn finds_four_holes_in_a_drilled_plate() {
        let snap = evaluate_vcad(&plate_with_holes_vcad());
        assert!(snap.fatal.is_none(), "fatal: {:?}", snap.fatal);
        let holes = find_z_holes(&snap, 3.0, 0.1);
        assert_eq!(holes.len(), 4, "got {:?}", holes);
        // The four expected XY positions (in any order).
        let mut got: Vec<(i32, i32)> = holes
            .iter()
            .map(|h| (h.x.round() as i32, h.y.round() as i32))
            .collect();
        got.sort();
        assert_eq!(got, vec![(-20, -10), (-20, 10), (20, -10), (20, 10)]);
    }

    /// A cylinder unioned on top of a plate is a protrusion, not a hole.
    /// The orientation filter must reject it. Reproduces a real Claude
    /// failure mode on the flanged-cap task — bolt cylinders sticking up
    /// instead of holes drilled down.
    #[test]
    fn ignores_protrusions_with_matching_diameter() {
        let vcad = r#"{
            "version": "0.1",
            "nodes": {
                "1": {"id": 1, "name": "plate",
                      "op": {"type": "Cube", "size": {"x": 60, "y": 60, "z": 4}}},
                "2": {"id": 2, "name": "plate_t",
                      "op": {"type": "Translate", "child": 1,
                             "offset": {"x": -30, "y": -30, "z": 0}}},
                "3": {"id": 3, "name": "stub",
                      "op": {"type": "Cylinder", "radius": 1.5, "height": 8, "segments": 32}},
                "4": {"id": 4, "name": "stub_t",
                      "op": {"type": "Translate", "child": 3,
                             "offset": {"x": 0, "y": 0, "z": 4}}},
                "5": {"id": 5, "name": "out",
                      "op": {"type": "Union", "left": 2, "right": 4}}
            },
            "materials": {}, "part_materials": {},
            "roots": [{"root": 5, "material": "default"}]
        }"#;
        let snap = evaluate_vcad(vcad);
        assert!(snap.fatal.is_none(), "fatal: {:?}", snap.fatal);
        let holes = find_z_holes(&snap, 3.0, 0.1);
        assert!(
            holes.is_empty(),
            "protrusion was misdetected as hole: {:?}",
            holes
        );
    }

    /// A cylinder DIFFERENCED out of a plate is a real hole. Same
    /// geometry as the protrusion test, just the boolean flipped.
    #[test]
    fn finds_a_real_drilled_hole() {
        let vcad = r#"{
            "version": "0.1",
            "nodes": {
                "1": {"id": 1, "name": "plate",
                      "op": {"type": "Cube", "size": {"x": 60, "y": 60, "z": 8}}},
                "2": {"id": 2, "name": "plate_t",
                      "op": {"type": "Translate", "child": 1,
                             "offset": {"x": -30, "y": -30, "z": 0}}},
                "3": {"id": 3, "name": "drill",
                      "op": {"type": "Cylinder", "radius": 1.5, "height": 12, "segments": 32}},
                "4": {"id": 4, "name": "drill_t",
                      "op": {"type": "Translate", "child": 3,
                             "offset": {"x": 0, "y": 0, "z": -2}}},
                "5": {"id": 5, "name": "out",
                      "op": {"type": "Difference", "left": 2, "right": 4}}
            },
            "materials": {}, "part_materials": {},
            "roots": [{"root": 5, "material": "default"}]
        }"#;
        let snap = evaluate_vcad(vcad);
        assert!(snap.fatal.is_none(), "fatal: {:?}", snap.fatal);
        let holes = find_z_holes(&snap, 3.0, 0.1);
        assert_eq!(holes.len(), 1, "expected 1 drilled hole, got {:?}", holes);
        assert!(holes[0].x.abs() < 0.1 && holes[0].y.abs() < 0.1);
    }

    #[test]
    fn ignores_holes_of_the_wrong_diameter() {
        let snap = evaluate_vcad(&plate_with_holes_vcad());
        let holes = find_z_holes(&snap, 10.0, 0.1);
        assert!(holes.is_empty(), "{:?}", holes);
    }
}
