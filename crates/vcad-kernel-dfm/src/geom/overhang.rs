//! Overhang detection for additive processes.
//!
//! For each face, measure the angle between its outward normal and
//! `+Z` (build direction). Faces pointing down past `max_overhang_deg`
//! contribute to support-area estimates. Cheap surface-midpoint
//! sampling for v1; raymarched per-fragment area follows once the
//! raytrace crate is wired in.

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// One overhang flag: face index plus measured angle from `+Z` in degrees.
#[derive(Debug, Clone, Copy)]
pub struct OverhangSample {
    /// Source face index.
    pub face: usize,
    /// Angle from `+Z` in degrees.
    pub angle_from_up_deg: f64,
}

/// Return every face whose outward normal points "downward" past the
/// threshold. `max_overhang_deg` is measured from `+Z`, so 135° flags
/// a face leaning 45° below horizontal.
pub fn sample(brep: &BRepSolid, max_overhang_deg: f64) -> Vec<OverhangSample> {
    let z_up = Vec3::new(0.0, 0.0, 1.0);
    let n = brep.topology.faces.len();
    let mut out = Vec::new();
    for i in 0..n {
        let Some((_, normal)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        let dot = normal.dot(z_up).clamp(-1.0, 1.0);
        let angle = dot.acos().to_degrees();
        if angle > max_overhang_deg {
            out.push(OverhangSample {
                face: i,
                angle_from_up_deg: angle,
            });
        }
    }
    out
}
