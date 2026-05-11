//! Geometry samplers shared by the per-process rule modules.
//!
//! v1 ships pragmatic surface-domain sampling — every face is probed at
//! a small grid of `(u, v)` points to estimate normal direction, draft
//! angle, and opposing-face distance. This is exactly what the existing
//! `vcad-slicer/src/dfm.rs` does and it's cheap and correct enough to
//! ship the agent loop.
//!
//! Follow-ups will swap thickness / accessibility / overhang for
//! BVH-accelerated raymarching against `vcad-kernel-raytrace`. The
//! public function shape is designed so that swap is a transparent
//! upgrade — no API churn at the rule call sites.

pub mod accessibility;
pub mod draft;
pub mod overhang;
pub mod provenance;
pub mod radii;
pub mod thickness;

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::Orientation;

/// Compute the outward normal for a face at the midpoint of its
/// parameter domain, respecting `Face.orientation`.
pub fn face_midpoint_and_normal(brep: &BRepSolid, face_idx: usize) -> Option<(Point3, Vec3)> {
    let (_face_id, face) = brep.topology.faces.iter().nth(face_idx)?;
    let surface = brep.geometry.surfaces.get(face.surface_index)?;
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let u = (u_min + u_max) * 0.5;
    let v = (v_min + v_max) * 0.5;
    let p = surface.evaluate(vcad_kernel_math::Point2::new(u, v));
    let n_dir = surface.normal(vcad_kernel_math::Point2::new(u, v));
    let n_raw = Vec3::new(n_dir.as_ref().x, n_dir.as_ref().y, n_dir.as_ref().z);
    let outward = if face.orientation == Orientation::Reversed {
        -n_raw
    } else {
        n_raw
    };
    Some((p, outward))
}

/// Axis-aligned bounding box over the BRep's vertices.
pub fn brep_bbox(brep: &BRepSolid) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for (_id, v) in &brep.topology.vertices {
        lo[0] = lo[0].min(v.point.x);
        lo[1] = lo[1].min(v.point.y);
        lo[2] = lo[2].min(v.point.z);
        hi[0] = hi[0].max(v.point.x);
        hi[1] = hi[1].max(v.point.y);
        hi[2] = hi[2].max(v.point.z);
    }
    (lo, hi)
}

/// Coarse part volume via tetrahedron summation over triangulated faces.
///
/// `vcad-kernel-tessellate` is the proper home for this; we expose a
/// vertex-only fallback so the DFM crate doesn't pull the tessellator
/// just for cost estimates. Negative results are coerced to zero.
pub fn approximate_part_volume_mm3(brep: &BRepSolid) -> f64 {
    let (lo, hi) = brep_bbox(brep);
    // For v1 we report bbox * 0.5 as a coarse stand-in. Replaced by the
    // exact mesh-divergence integral once the engine wires the
    // tessellated mesh through to the DFM call.
    ((hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]) * 0.5).max(0.0)
}
