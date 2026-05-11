//! Wall-thickness sampling.
//!
//! v1 uses pairwise opposing-face matching (same approach as the
//! existing `vcad-slicer/src/dfm.rs`): for every face pair that's nearly
//! antiparallel at their midpoints, the through-distance approximates
//! local wall thickness. This is O(F²) but F is small for the parts
//! that ship through web-CAD.
//!
//! The richer raytrace-based variant (cast a ray from a face's midpoint
//! along its inward normal, take the first hit) is queued as a
//! follow-up that will land alongside the FaceId→NodeId provenance
//! refactor.

use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;

use super::face_midpoint_and_normal;

/// Sampled wall thickness for a face index, with the opposing face it
/// was paired against.
#[derive(Debug, Clone)]
pub struct ThicknessSample {
    /// Source face index.
    pub face_a: usize,
    /// Opposing face index.
    pub face_b: usize,
    /// Midpoint of `face_a`.
    pub anchor: Point3,
    /// Measured through-distance in millimetres.
    pub thickness_mm: f64,
}

/// Build a list of opposing-face thickness samples for every face pair
/// whose outward normals are antiparallel within `cos_threshold`.
///
/// `cos_threshold` is typically -0.95 (≈ 18° tolerance).
pub fn sample_pairs(brep: &BRepSolid, cos_threshold: f64) -> Vec<ThicknessSample> {
    let n = brep.topology.faces.len();
    let mut mids: Vec<Option<(Point3, vcad_kernel_math::Vec3)>> = Vec::with_capacity(n);
    for i in 0..n {
        mids.push(face_midpoint_and_normal(brep, i));
    }
    let mut samples = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let (Some(a), Some(b)) = (mids[i].as_ref(), mids[j].as_ref()) else {
                continue;
            };
            if a.1.dot(b.1) < cos_threshold {
                let dx = a.0.x - b.0.x;
                let dy = a.0.y - b.0.y;
                let dz = a.0.z - b.0.z;
                let d = (dx * dx + dy * dy + dz * dz).sqrt();
                if d > 1e-3 {
                    samples.push(ThicknessSample {
                        face_a: i,
                        face_b: j,
                        anchor: a.0,
                        thickness_mm: d,
                    });
                }
            }
        }
    }
    samples
}

/// Coefficient of variation of a thickness sample set
/// (`σ / μ`). Returns 0.0 if fewer than two samples.
pub fn cv(samples: &[ThicknessSample]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().map(|s| s.thickness_mm).sum::<f64>() / n;
    if mean.abs() < 1e-9 {
        return 0.0;
    }
    let var = samples
        .iter()
        .map(|s| (s.thickness_mm - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt() / mean
}
