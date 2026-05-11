//! Tool-axis accessibility check.
//!
//! v1 is a coarse heuristic: a face is "accessible" from `+Z` if its
//! outward normal has a positive component along `+Z`. This catches
//! "obviously inaccessible" features (faces pointing sideways or down
//! on a 3-axis mill) without the cost of BVH raymarching.
//!
//! Follow-up: cast a ray from the face midpoint along the tool axis
//! and confirm the closest hit is the face itself.

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// Face index + dot product with the tool axis.
#[derive(Debug, Clone, Copy)]
pub struct AccessibilitySample {
    /// Source face index.
    pub face: usize,
    /// `normal · tool_axis`. Positive = accessible from that direction.
    pub dot_with_axis: f64,
}

/// For each face, report its dot product with the tool axis. Callers
/// filter for `dot_with_axis <= threshold` to flag inaccessibility.
pub fn sample(brep: &BRepSolid, tool_axis: Vec3) -> Vec<AccessibilitySample> {
    let axis = tool_axis.normalize();
    let n = brep.topology.faces.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let Some((_, normal)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        out.push(AccessibilitySample {
            face: i,
            dot_with_axis: normal.dot(axis),
        });
    }
    out
}
