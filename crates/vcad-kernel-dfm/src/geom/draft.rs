//! Draft-angle measurement against a pull direction.
//!
//! For injection / casting we need every non-perpendicular face to have
//! enough draft (the angle between the face normal and the parting
//! plane) to release from the mold. v1 measures at the face midpoint;
//! a follow-up will sample multiple `(u, v)` points and report the
//! worst.

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// Per-face draft measurement.
#[derive(Debug, Clone, Copy)]
pub struct DraftSample {
    /// Source face index.
    pub face: usize,
    /// Signed angle in degrees. Positive = drafted in the pull direction,
    /// negative = undercut, ±90° = vertical wall along pull axis.
    pub draft_deg: f64,
}

/// Sample draft angle for every face against a unit pull direction.
pub fn sample(brep: &BRepSolid, pull_dir: Vec3) -> Vec<DraftSample> {
    let pull = pull_dir.normalize();
    let n = brep.topology.faces.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let Some((_, normal)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        // Angle between normal and pull plane (= 90° - angle(normal, pull)).
        let cos_n_pull = normal.dot(pull).clamp(-1.0, 1.0);
        let angle_to_pull_deg = cos_n_pull.acos().to_degrees();
        let draft_deg = 90.0 - angle_to_pull_deg;
        out.push(DraftSample { face: i, draft_deg });
    }
    out
}
