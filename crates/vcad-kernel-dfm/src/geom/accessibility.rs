//! Tool-axis accessibility check.
//!
//! With the `raytrace` feature on (default), this module casts a ray
//! from each face's midpoint along the tool axis and confirms the
//! closest hit is the face itself. If the ray hits something else
//! first the face is occluded → flag as inaccessible.
//!
//! Without the feature it falls back to the heuristic "outward normal
//! · tool axis > 0" — same as the v1 cheap pass.

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// Face index + dot product with the tool axis.
#[derive(Debug, Clone, Copy)]
pub struct AccessibilitySample {
    /// Source face index.
    pub face: usize,
    /// `normal · tool_axis`. Positive = accessible from that direction.
    /// (Kept for backwards compat — callers can read it directly.)
    pub dot_with_axis: f64,
}

/// For each face, report its dot product with the tool axis. Callers
/// filter for `dot_with_axis <= threshold` to flag inaccessibility.
pub fn sample(brep: &BRepSolid, tool_axis: Vec3) -> Vec<AccessibilitySample> {
    let axis = tool_axis.normalize();
    #[cfg(feature = "raytrace")]
    {
        sample_via_raycast(brep, axis)
    }
    #[cfg(not(feature = "raytrace"))]
    {
        sample_via_normal(brep, axis)
    }
}

#[allow(dead_code)]
fn sample_via_normal(brep: &BRepSolid, axis: Vec3) -> Vec<AccessibilitySample> {
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

#[cfg(feature = "raytrace")]
fn sample_via_raycast(brep: &BRepSolid, axis: Vec3) -> Vec<AccessibilitySample> {
    use vcad_kernel_raytrace::{Bvh, Ray};

    let bvh = Bvh::build(brep);
    let face_id_to_idx: std::collections::HashMap<_, _> = brep
        .topology
        .faces
        .iter()
        .enumerate()
        .map(|(idx, (id, _))| (id, idx))
        .collect();

    let n = brep.topology.faces.len();
    let mut out = Vec::with_capacity(n);
    for (i, _) in brep.topology.faces.iter().enumerate() {
        let Some((midpoint, normal)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        // Front-facing fast path: if the face's normal already points
        // away from the tool, no point raycasting.
        let dot = normal.dot(axis);
        if dot <= 0.0 {
            out.push(AccessibilitySample {
                face: i,
                dot_with_axis: dot,
            });
            continue;
        }
        // Step a hair away from the face along the tool axis, then
        // raycast back toward the face — closest hit should be this
        // face. If something else hits first, the face is occluded.
        let eps = 1e-3;
        let above = midpoint + Vec3::new(axis.x * 100.0, axis.y * 100.0, axis.z * 100.0);
        let direction = -axis;
        let origin = above + Vec3::new(direction.x * eps, direction.y * eps, direction.z * eps);
        let ray = Ray::new(origin, direction);
        let first = bvh.trace(&ray).into_iter().find(|h| h.t > eps);
        let occluded = match first {
            Some(hit) => face_id_to_idx
                .get(&hit.face_id)
                .copied()
                .map(|idx| idx != i)
                .unwrap_or(false),
            None => false,
        };
        out.push(AccessibilitySample {
            face: i,
            // Encode occlusion as a negative dot so the existing
            // `dot_with_axis < threshold` filter in cnc.rs still works.
            dot_with_axis: if occluded { -dot } else { dot },
        });
    }
    out
}
