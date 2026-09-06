//! Overhang detection for additive processes.
//!
//! Cheap path (always available): face normal vs `+Z`. Faces past the
//! threshold are flagged as needing support.
//!
//! With the `raytrace` feature on (default), each flagged face also
//! gets an estimated support-column length: cast a ray from the face
//! midpoint downward and report the distance to the next surface (or
//! to the build plate). The support volume estimate the FDM cost
//! function consumes adds these columns up.

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// One overhang flag.
#[derive(Debug, Clone, Copy)]
pub struct OverhangSample {
    /// Source face index.
    pub face: usize,
    /// Angle from `+Z` in degrees.
    pub angle_from_up_deg: f64,
    /// Estimated support column length in mm (0.0 when raytrace
    /// isn't enabled or the face floats above the build plate).
    pub support_column_mm: f64,
    /// Anchor point for annotations / cost integration.
    pub anchor: Point3,
}

/// Sample overhangs.
pub fn sample(brep: &BRepSolid, max_overhang_deg: f64) -> Vec<OverhangSample> {
    let z_up = Vec3::new(0.0, 0.0, 1.0);
    let n = brep.topology.faces.len();
    let mut out = Vec::new();

    #[cfg(feature = "raytrace")]
    use vcad_kernel_raytrace::BrepBvh;
    let bvh = vcad_kernel_raytrace::Bvh::build_brep(brep);
    #[cfg(feature = "raytrace")]
    let face_id_to_idx: std::collections::HashMap<_, _> = brep
        .topology
        .faces
        .iter()
        .enumerate()
        .map(|(idx, (id, _))| (id, idx))
        .collect();

    for i in 0..n {
        let Some((midpoint, normal)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        let dot = normal.dot(z_up).clamp(-1.0, 1.0);
        let angle = dot.acos().to_degrees();
        if angle <= max_overhang_deg {
            continue;
        }

        #[allow(unused_assignments, unused_mut)]
        let mut support = 0.0;
        #[cfg(feature = "raytrace")]
        {
            // Cast downward from the midpoint; first hit's t is the
            // support column length. If nothing's underneath, the
            // column extends to the build plate (z = 0).
            let down = Vec3::new(0.0, 0.0, -1.0);
            let eps = 1e-3;
            let origin = midpoint + Vec3::new(0.0, 0.0, -eps);
            let ray = vcad_kernel_raytrace::Ray::new(origin, down);
            let next = bvh.trace(&ray).into_iter().find(|h| {
                h.t > eps
                    && face_id_to_idx
                        .get(&bvh.face_id(h).unwrap_or_default())
                        .copied()
                        .map(|idx| idx != i)
                        .unwrap_or(true)
            });
            support = match next {
                Some(hit) => hit.t,
                None => midpoint.z.max(0.0),
            };
        }

        out.push(OverhangSample {
            face: i,
            angle_from_up_deg: angle,
            support_column_mm: support,
            anchor: midpoint,
        });
    }
    out
}

/// Total support volume estimate (mm³). Each face's support column is
/// multiplied by a placeholder per-face area; the FDM cost function
/// consumes this for the support-material surcharge.
///
/// v1 uses a coarse 1 mm² per face area; the proper integral per
/// face area lives behind a follow-up that walks the face's loops.
pub fn total_support_volume_mm3(samples: &[OverhangSample]) -> f64 {
    samples
        .iter()
        .map(|s| s.support_column_mm.max(0.0))
        .sum::<f64>()
}
