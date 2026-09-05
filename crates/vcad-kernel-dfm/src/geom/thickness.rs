//! Wall-thickness sampling.
//!
//! When the `raytrace` feature is enabled (default), this module casts
//! a ray from each face's midpoint along its inward normal and reports
//! the first opposing-face hit as the local wall thickness. That gives
//! per-face thickness on parts where face-pair matching breaks (curved
//! walls, non-planar opposing surfaces, complex booleans).
//!
//! With `--no-default-features` the module falls back to pairwise
//! antiparallel-midpoint matching — the v1 surface-domain sampler that
//! the slicer's existing dfm.rs uses. Slower asymptotically (O(F²))
//! but only depends on vcad-kernel-geom + topo.

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

/// Build a list of thickness samples for every face.
///
/// `cos_threshold` is the antiparallel-tolerance for the fallback path
/// (typically -0.95 ≈ 18°); ignored when raycasting.
pub fn sample_pairs(brep: &BRepSolid, cos_threshold: f64) -> Vec<ThicknessSample> {
    #[cfg(feature = "raytrace")]
    {
        let _ = cos_threshold;
        sample_via_raycast(brep)
    }
    #[cfg(not(feature = "raytrace"))]
    {
        sample_via_pairs(brep, cos_threshold)
    }
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

/// O(F²) pairwise fallback: every face pair whose outward normals are
/// antiparallel within `cos_threshold` produces one sample.
#[allow(dead_code)]
fn sample_via_pairs(brep: &BRepSolid, cos_threshold: f64) -> Vec<ThicknessSample> {
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

/// O(F · log F) BVH raycast: for each face, push the midpoint a hair
/// inward and trace along the inward normal. The first hit's `t` is
/// the local wall thickness.
#[cfg(feature = "raytrace")]
fn sample_via_raycast(brep: &BRepSolid) -> Vec<ThicknessSample> {
    use vcad_kernel_math::Vec3;
    use vcad_kernel_raytrace::{Bvh, Ray};
    use vcad_kernel_raytrace::BrepBvh;

    let bvh = Bvh::build_brep(brep);
    let n = brep.topology.faces.len();
    // Build the face_id ↔ index lookup once so we can map RayHit.face_id
    // back to a stable usize index for ThicknessSample.
    let face_id_to_idx: std::collections::HashMap<_, _> = brep
        .topology
        .faces
        .iter()
        .enumerate()
        .map(|(idx, (id, _))| (id, idx))
        .collect();

    let mut samples = Vec::with_capacity(n);
    for (i, _) in brep.topology.faces.iter().enumerate() {
        let Some((midpoint, outward)) = face_midpoint_and_normal(brep, i) else {
            continue;
        };
        // Step a tiny bit inward to avoid self-hit, then cast inward.
        let inward = -outward;
        let eps = 1e-4;
        let origin = midpoint + Vec3::new(inward.x * eps, inward.y * eps, inward.z * eps);
        let ray = Ray::new(origin, inward);
        let hits = bvh.trace(&ray);
        // First hit that isn't this same face. (The eps step usually
        // skips it but keep the guard for robustness.)
        let opposing = hits.into_iter().find(|h| {
            face_id_to_idx
                .get(&bvh.face_id(h).unwrap_or_default())
                .copied()
                .map(|idx| idx != i)
                .unwrap_or(true)
                && h.t > 1e-3
        });
        let Some(hit) = opposing else { continue };
        let face_b = face_id_to_idx
            .get(&bvh.face_id(&hit).unwrap_or_default())
            .copied()
            .unwrap_or(usize::MAX);
        samples.push(ThicknessSample {
            face_a: i,
            face_b,
            anchor: midpoint,
            // Add the eps back so the recorded thickness is from the
            // true face surface, not the offset origin.
            thickness_mm: hit.t + eps,
        });
    }
    samples
}
