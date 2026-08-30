//! Up-front declaration of arrangements the B-rep splitters cannot handle.
//!
//! The boolean pipeline conforms two solids by intersecting their faces
//! pairwise and splitting each face along the resulting curves. That only
//! works when the surface–surface intersection has a form the splitters can
//! consume. For several curved pairs it does not: there is no analytic
//! intersection, so `ssi` falls back to a marching sampler that returns
//! loose point dust. The pipeline then records **no splits**, concludes the
//! boundaries never cross, and resolves the whole boolean by a containment
//! test — which is how a Ø20 cylinder bored through a Ø60 sphere came back
//! as an untouched sphere, and a cross-drilled bar came back with the
//! drill's surface merged into it.
//!
//! Rather than detect that after the fact (sampled validity oracles were
//! tried and cannot separate wrong solids from legitimate thin geometry —
//! see `validate`), the kernel declares the limit before running: if any
//! candidate face pair has no analytic intersection *and* the two faces
//! genuinely cross, the arrangement is unrepresentable and the caller uses
//! the mesh fallback instead.
//!
//! The crossing test is what keeps this narrow. Two curved faces whose
//! bounding boxes overlap usually do not intersect at all, and those cases
//! need no splits, so the B-rep pipeline handles them correctly and keeps
//! its analytic surfaces. Only a pair that actually interpenetrates — some
//! sample points of one face strictly inside the other solid, some strictly
//! outside — forces the fallback.

use vcad_kernel_geom::{BilinearSurface, CylinderSurface, Surface, SurfaceKind};
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::tessellate_brep;

use crate::bbox;
use crate::mesh::point_in_mesh;

/// Does this surface pair have a closed-form intersection the splitters can
/// consume?
///
/// Mirrors the analytic arms of [`crate::ssi::intersect_surfaces`], including
/// the guards *inside* them: cylinder × cylinder claims an analytic form only
/// for perpendicular, intersecting, equal-radius axes and itself defers to the
/// marching sampler otherwise, so a cross-drilled bar of unequal radii is not
/// analytic despite both surfaces being cylinders. Anything not listed here
/// reaches the sampler, whose loose point dust the splitters cannot use.
fn pair_is_analytic(a: &dyn Surface, b: &dyn Surface) -> bool {
    use SurfaceKind::{Cone, Cylinder, Plane, Sphere, Torus};
    match (a.surface_type(), b.surface_type()) {
        (Plane, Plane)
        | (Plane, Sphere)
        | (Sphere, Plane)
        | (Plane, Cylinder)
        | (Cylinder, Plane)
        | (Plane, Cone)
        | (Cone, Plane)
        | (Plane, Torus)
        | (Torus, Plane)
        | (Sphere, Sphere) => true,
        (Cylinder, Cylinder) => {
            let (Some(ca), Some(cb)) = (
                a.as_any().downcast_ref::<CylinderSurface>(),
                b.as_any().downcast_ref::<CylinderSurface>(),
            ) else {
                return false;
            };
            let dot = ca.axis.as_ref().dot(*cb.axis.as_ref());
            // Parallel/coaxial: the SSI reports Empty. That is right for
            // the nested-bore geometry this arises from, and such faces
            // never cross, so the crossing test below keeps it cheap.
            if (1.0 - dot.abs()).abs() < 1e-9 {
                return true;
            }
            // Only perpendicular, equal-radius, genuinely intersecting axes
            // have the closed form (two ellipses).
            if dot.abs() > 1e-6 || (ca.radius - cb.radius).abs() > 1e-6 {
                return false;
            }
            let d = cb.center - ca.center;
            let t = d.dot(*ca.axis.as_ref());
            let s = -d.dot(*cb.axis.as_ref());
            let pa = ca.center + t * (*ca.axis.as_ref());
            let pb = cb.center + s * (*cb.axis.as_ref());
            (pa - pb).norm() <= 1e-6
        }
        _ => false,
    }
}

/// How many points to sample across a face when testing whether it truly
/// crosses the other solid.
const FACE_SAMPLES: usize = 24;

/// Sample points spread over a face: its loop vertices plus edge midpoints,
/// which together straddle the face without needing a surface
/// parameterisation.
fn face_sample_points(solid: &BRepSolid, face: vcad_kernel_topo::FaceId) -> Vec<Point3> {
    let topo = &solid.topology;
    let Some(f) = topo.faces.get(face) else {
        return Vec::new();
    };
    let mut pts: Vec<Point3> = Vec::new();
    let loops = std::iter::once(f.outer_loop).chain(f.inner_loops.iter().copied());
    for loop_id in loops {
        let verts: Vec<Point3> = topo
            .loop_half_edges(loop_id)
            .map(|he| topo.vertices[topo.half_edges[he].origin].point)
            .collect();
        for (i, v) in verts.iter().enumerate() {
            pts.push(*v);
            let w = verts[(i + 1) % verts.len()];
            pts.push(Point3::from_vec((v.to_vec() + w.to_vec()) * 0.5));
            if pts.len() >= FACE_SAMPLES * 2 {
                break;
            }
        }
    }
    // Thin the list deterministically so cost stays bounded on dense loops.
    if pts.len() > FACE_SAMPLES {
        let step = pts.len() / FACE_SAMPLES;
        pts = pts.into_iter().step_by(step.max(1)).collect();
    }
    pts
}

/// Does this face genuinely cross the other solid's boundary?
///
/// True when some sample sits strictly inside and some strictly outside.
/// Points within `skin` of the surface are ignored so that merely touching
/// or tangent faces — which need no splitting — do not count as crossing.
fn face_crosses(
    solid: &BRepSolid,
    face: vcad_kernel_topo::FaceId,
    other_mesh: &vcad_kernel_tessellate::TriangleMesh,
    skin: f64,
) -> bool {
    let mut any_in = false;
    let mut any_out = false;
    for p in face_sample_points(solid, face) {
        // Nudge along each axis: a sample exactly on the other surface is
        // ambiguous, and counting it would make tangency look like
        // crossing. Require agreement to count the sample at all.
        let probes = [
            Point3::new(p.x + skin, p.y, p.z),
            Point3::new(p.x - skin, p.y, p.z),
            Point3::new(p.x, p.y + skin, p.z),
            Point3::new(p.x, p.y - skin, p.z),
        ];
        let inside = probes
            .iter()
            .filter(|q| point_in_mesh(q, other_mesh))
            .count();
        if inside == probes.len() {
            any_in = true;
        } else if inside == 0 {
            any_out = true;
        }
        if any_in && any_out {
            return true;
        }
    }
    false
}

/// Is this pair of solids in an arrangement the B-rep splitters cannot
/// represent?
///
/// Checks only candidate face pairs the broadphase already flagged, and
/// only those whose surfaces have no analytic intersection. Returns as soon
/// as one such pair is found to genuinely cross.
pub(crate) fn arrangement_is_unrepresentable(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    segments: u32,
) -> bool {
    let pairs = bbox::find_candidate_face_pairs(solid_a, solid_b);
    // Collect the non-analytic candidate pairs first; most booleans have
    // none and pay nothing beyond the broadphase.
    let mut suspect: Vec<(vcad_kernel_topo::FaceId, vcad_kernel_topo::FaceId)> = Vec::new();
    for &(fa, fb) in &pairs {
        let (Some(face_a), Some(face_b)) = (
            solid_a.topology.faces.get(fa),
            solid_b.topology.faces.get(fb),
        ) else {
            continue;
        };
        let (Some(sa), Some(sb)) = (
            solid_a.geometry.surfaces.get(face_a.surface_index),
            solid_b.geometry.surfaces.get(face_b.surface_index),
        ) else {
            continue;
        };
        if !pair_is_analytic(sa.as_ref(), sb.as_ref()) {
            suspect.push((fa, fb));
        }
    }
    if suspect.is_empty() {
        return false;
    }

    let mesh_a = tessellate_brep(solid_a, segments);
    let mesh_b = tessellate_brep(solid_b, segments);
    let diag = {
        let bb = bbox::solid_aabb(solid_a);
        ((bb.max.x - bb.min.x).powi(2)
            + (bb.max.y - bb.min.y).powi(2)
            + (bb.max.z - bb.min.z).powi(2))
        .sqrt()
    };
    // Skin scales with the part so tessellation sag (which grows with
    // radius) never reads as a crossing.
    let skin = (diag * 1e-3).clamp(1e-4, 0.5);

    for (fa, fb) in suspect {
        if face_crosses(solid_a, fa, &mesh_b, skin) || face_crosses(solid_b, fb, &mesh_a, skin) {
            return true;
        }
    }
    false
}

/// Does this solid carry a non-planar bilinear face?
///
/// Helical and twisted sweeps emit bilinear patches that are *not* planes.
/// The SSI path approximates those as planes, which is the wrong
/// intersection on undercut / re-entrant geometry and is how a
/// `difference(tube, helical_channel)` used to return a plausible-looking
/// cracked solid. Planar bilinear (a straight sweep of a linear profile)
/// is fine: the plane approximation is exact.
pub(crate) fn has_nonplanar_bilinear(solid: &BRepSolid) -> bool {
    solid.geometry.surfaces.iter().any(|s| {
        s.as_any()
            .downcast_ref::<BilinearSurface>()
            .is_some_and(|b| !b.is_planar())
    })
}
