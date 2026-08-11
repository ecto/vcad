//! Freezing of analytic circle loops into canonical polylines.
//!
//! Primitive B-reps carry *analytic* full-circle edges (a single half-edge
//! whose origin equals its destination, e.g. a cylinder cap boundary). The
//! boolean pipeline, in contrast, emits *frozen* polyline boundaries
//! (`split::canonical_arc_points`). A result that mixes the two is
//! watertight only by resolution coincidence — the analytic side is
//! re-sampled at display time with its own count and phase — and its
//! topology can never conform, which is exactly what poisons any further
//! boolean taken on the result (the "boolean-result operand" failure family
//! in the torr catalogue).
//!
//! `freeze_circle_loops` rewrites every full-circle edge of both operands
//! into the canonical polyline before the pipeline runs, so everything
//! downstream — SSI splitting, classification, sewing, tessellation — sees
//! one concrete, conforming boundary representation.

use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::HalfEdgeId;

use crate::split::canonical_arc_points;

/// Replace analytic full-circle edges with canonical polyline chains.
///
/// A full-circle edge is a half-edge whose origin and destination are the
/// same vertex and whose own face or twin face is a cylinder. Both the
/// half-edge and its twin (when present) are rewritten against the same
/// vertex chain, so the rebuilt edges pair exactly.
pub(crate) fn freeze_circle_loops(brep: &mut BRepSolid, segments: u32) {
    let he_ids: Vec<HalfEdgeId> = brep.topology.half_edges.keys().collect();
    let mut done: std::collections::HashSet<HalfEdgeId> = std::collections::HashSet::new();

    for he_id in he_ids {
        if done.contains(&he_id) || !brep.topology.half_edges.contains_key(he_id) {
            continue;
        }
        let he = &brep.topology.half_edges[he_id];
        let loop_id = match he.loop_id {
            Some(l) => l,
            None => continue,
        };
        let next = match he.next {
            Some(n) => n,
            None => continue,
        };
        let origin = he.origin;
        let dest = brep.topology.half_edges[next].origin;
        if origin != dest {
            continue; // not a closed-curve edge
        }
        // origin == dest alone is ambiguous: a PINCH DUPLICATE in a dense
        // band loop (two consecutive half-edges sharing an origin, kept
        // deliberately by realize_bands) looks identical from endpoints.
        // A genuine analytic full-circle edge only occurs in the tiny
        // legacy loops (1-he cap seam loops, ≤4-he wall rectangles);
        // splicing a ring into a dense loop would thread a phantom
        // full-circle polyline through the middle of the face.
        if brep.topology.loop_len(loop_id) > 4 {
            continue;
        }
        let twin = he.twin;

        // Find the cylinder or cone that carries this circle (own face or
        // twin's). Both carry rings perpendicular to their axis; all the
        // machinery below needs is the axis direction and a point on it.
        let axis_of = |hid: HalfEdgeId| -> Option<(Vec3, vcad_kernel_math::Point3)> {
            let lp = brep.topology.half_edges[hid].loop_id?;
            let face_id = brep.topology.loops[lp].face?;
            let face = &brep.topology.faces[face_id];
            let surface = &brep.geometry.surfaces[face.surface_index];
            if let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() {
                return Some((*cyl.axis.as_ref(), cyl.center));
            }
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                return Some((*cone.axis.as_ref(), cone.apex));
            }
            None
        };
        let (axis, axis_pt) = match axis_of(he_id).or_else(|| twin.and_then(axis_of)) {
            Some(c) => c,
            None => continue, // sphere poles etc. — skip
        };

        let v_pt = brep.topology.vertices[origin].point;
        let d = v_pt - axis_pt;
        let center = axis_pt + axis * d.dot(axis);
        let radius = (v_pt - center).norm();
        if radius < 1e-9 {
            continue;
        }

        // Which travel direction does `he_id` take? Prefer the planar-face
        // side to decide: producers wind loops CCW about the STORED surface
        // normal (Orientation only flips the effective normal). Fall back
        // to the cylinder-wall convention: the lower circle travels +u
        // (CCW about the axis), the upper one −u.
        let ccw_about = |hid: HalfEdgeId| -> Option<Vec3> {
            let lp = brep.topology.half_edges[hid].loop_id?;
            let face_id = brep.topology.loops[lp].face?;
            let face = &brep.topology.faces[face_id];
            let surface = &brep.geometry.surfaces[face.surface_index];
            if surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::Plane>()
                .is_some()
            {
                Some(
                    *surface
                        .normal(vcad_kernel_math::Point2::new(0.0, 0.0))
                        .as_ref(),
                )
            } else {
                None
            }
        };
        // A ring on a cylinder is ALWAYS perpendicular to the axis; the
        // bordering planar face's stored normal may only choose the SIGN.
        // Using the plane normal directly tilts the ring wherever the twin
        // is an oblique face (a blade plane grazing the wall), sweeping
        // phantom vertices far outside the solid.
        let sign_of = |n: Vec3| -> Option<f64> {
            let d = n.dot(axis);
            if d.abs() > 0.1 {
                Some(d.signum())
            } else {
                None
            }
        };
        let he_normal: Vec3 = if let Some(s) = ccw_about(he_id).and_then(sign_of) {
            axis * s
        } else if let Some(s) = twin.and_then(ccw_about).and_then(sign_of) {
            axis * -s
        } else {
            // Wall-only circle (e.g. an inner bore after a difference):
            // lower ring +u, upper ring −u, matching `make_cylinder`.
            let lp = brep.topology.half_edges[he_id].loop_id;
            let is_lower = lp
                .map(|l| {
                    let mut vmin = f64::MAX;
                    for h in brep.topology.loop_half_edges(l) {
                        let p = brep.topology.vertices[brep.topology.half_edges[h].origin].point;
                        vmin = vmin.min((p - axis_pt).dot(axis));
                    }
                    (d.dot(axis) - vmin).abs() < 1e-6
                })
                .unwrap_or(true);
            if is_lower {
                axis
            } else {
                -axis
            }
        };

        // Canonical ring, CCW about `he_normal`; interior points only.
        let ring = canonical_arc_points(center, radius, he_normal, v_pt, v_pt, segments);
        let interior = &ring[1..ring.len() - 1];
        if interior.is_empty() {
            continue;
        }
        let interior_vids: Vec<_> = interior
            .iter()
            .map(|p| brep.topology.add_vertex(*p))
            .collect();

        // Rechain `he_id` through the interior vertices (forward order) and
        // the twin through them in reverse, then pair the sub-edges.
        let chain = |topo: &mut vcad_kernel_topo::Topology,
                     start_he: HalfEdgeId,
                     vids: &[vcad_kernel_topo::VertexId]|
         -> Vec<HalfEdgeId> {
            let lp = topo.half_edges[start_he].loop_id;
            let next = topo.half_edges[start_he].next.unwrap();
            let mut hes = vec![start_he];
            let mut prev = start_he;
            for &v in vids {
                let h = topo.add_half_edge(v);
                topo.half_edges[h].loop_id = lp;
                topo.half_edges[h].prev = Some(prev);
                topo.half_edges[h].next = Some(next);
                topo.half_edges[prev].next = Some(h);
                topo.half_edges[next].prev = Some(h);
                prev = h;
                hes.push(h);
            }
            hes
        };
        let hes_a = chain(&mut brep.topology, he_id, &interior_vids);
        done.insert(he_id);

        if let Some(twin_id) = twin {
            // Twin travels the ring in the opposite direction.
            let rev: Vec<_> = interior_vids.iter().rev().copied().collect();
            let hes_b = chain(&mut brep.topology, twin_id, &rev);
            done.insert(twin_id);
            // Unlink the old whole-circle edge and re-pair segmentwise:
            // hes_a[i] runs v_i → v_{i+1}; its twin is hes_b[n-1-i].
            brep.topology.half_edges[he_id].twin = None;
            brep.topology.half_edges[twin_id].twin = None;
            let n = hes_a.len();
            for i in 0..n {
                let a = hes_a[i];
                let b = hes_b[n - 1 - i];
                brep.topology.add_edge(a, b);
            }
        }
    }
}
