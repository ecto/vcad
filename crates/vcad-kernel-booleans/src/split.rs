//! Face splitting along intersection curves.
//!
//! Given a face and intersection curves that cross it, split the face
//! into sub-faces. Each sub-face inherits the original face's surface
//! but has a new trim loop.
//!
//! For Phase 2, we focus on planar face splitting by lines/segments.
//! Curved face splitting extends naturally once the planar case works.

use vcad_kernel_math::{Point2, Point3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, HalfEdgeId, Orientation};

use crate::ssi::IntersectionCurve;

/// Result of splitting a face.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The face IDs of the newly created sub-faces.
    /// If no splitting occurred, contains just the original face ID.
    pub sub_faces: Vec<FaceId>,
}

/// For a sampled intersection curve, collect the polyline points strictly
/// between the entry and exit points (ordered entry → exit), so the cut
/// edge can follow the true curve instead of a single chord. A chord
/// disagrees with the curved-side split boundary by the arc's sagitta,
/// which is enough to flip classification probes near the cut (the torr
/// B1 family). Returns an empty list for non-sampled curves.
fn cut_polyline_between(
    curve: &IntersectionCurve,
    entry_point: &Point3,
    exit_point: &Point3,
) -> Vec<Point3> {
    let IntersectionCurve::Sampled(points) = curve else {
        return Vec::new();
    };
    if points.len() < 3 {
        return Vec::new();
    }
    // Project a point onto the polyline: (segment index + fraction) as a
    // scalar parameter in [0, N−1].
    let project = |p: &Point3| -> f64 {
        let mut best = 0.0f64;
        let mut best_d = f64::INFINITY;
        for i in 0..points.len() - 1 {
            let a = points[i];
            let b = points[i + 1];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            let q = a + t * ab;
            let d = (*p - q).norm_squared();
            if d < best_d {
                best_d = d;
                best = i as f64 + t;
            }
        }
        best
    };
    let s_entry = project(entry_point);
    let s_exit = project(exit_point);
    let (lo, hi, rev) = if s_entry <= s_exit {
        (s_entry, s_exit, false)
    } else {
        (s_exit, s_entry, true)
    };
    let mut via: Vec<Point3> = points
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i as f64) > lo + 1e-9 && (*i as f64) < hi - 1e-9)
        .map(|(_, p)| *p)
        .collect();
    if rev {
        via.reverse();
    }
    // On a CLOSED polyline the direct parameter walk lo→hi may be the long
    // way around (the cut interval straddles the parameter seam): walking
    // it drags far-side samples — points nowhere near this face — into the
    // cut edge and mints phantom geometry. Take the wrap-around complement
    // when it is shorter.
    let closed = points.len() > 3 && (points[0] - points[points.len() - 1]).norm() < 1e-9;
    if closed {
        let path_len = |pts: &[Point3], a: &Point3, b: &Point3| -> f64 {
            let mut total = 0.0;
            let mut prev = *a;
            for p in pts {
                total += (*p - prev).norm();
                prev = *p;
            }
            total + (*b - prev).norm()
        };
        let direct = path_len(&via, entry_point, exit_point);
        let mut wrap: Vec<Point3> = points
            .iter()
            .enumerate()
            .take(points.len() - 1) // skip the duplicated closing point
            .filter(|(i, _)| (*i as f64) < lo - 1e-9 || (*i as f64) > hi + 1e-9)
            .map(|(_, p)| *p)
            .collect();
        // Complement travels hi → end → start → lo; order it from the exit
        // side: rotate so it starts just after `hi`.
        let pivot = wrap.iter().position(|p| project(p) > hi).unwrap_or(0);
        wrap.rotate_left(pivot);
        if rev {
            wrap.reverse();
        }
        let wrapped = path_len(&wrap, entry_point, exit_point);
        if wrapped + 1e-9 < direct {
            return wrap;
        }
    }
    via
}

/// Whether `VCAD_SPLIT_DEBUG` is set, read once per process — `split_dbg!`
/// fires inside hot per-face loops, so the env lookup must not repeat.
pub(crate) fn split_debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VCAD_SPLIT_DEBUG").is_ok())
}

macro_rules! split_dbg {
    ($($arg:tt)*) => {
        if crate::split::split_debug_enabled() {
            eprintln!($($arg)*);
        }
    };
}

/// Split a face along an intersection curve.
///
/// The curve must already be trimmed to the face's domain. This function:
/// 1. Projects the curve into UV space
/// 2. Finds where it enters/exits the face boundary
/// 3. Splits the boundary loop at entry/exit points
/// 4. Creates two new face loops, joined along the curve's polyline (or a
///    straight chord for analytic curves)
pub fn split_face_by_curve(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    entry_point: &Point3,
    exit_point: &Point3,
) -> SplitResult {
    // Get face info
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;
    let _surface = &brep.geometry.surfaces[surface_index];

    // Get outer loop vertices in order
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    let n = loop_verts.len();
    if n < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Find the two edges where the curve enters and exits the face. An
    // endpoint that lands on (or near) a CORNER is within tolerance of two
    // edges; picking the nearest edge independently for entry and exit can
    // then assign both to the same edge and reject a perfectly valid cut.
    // Instead collect every edge within slack of each endpoint and pick a
    // DISTINCT pair with minimal combined distance when one exists.
    let edge_candidates = |p: &Point3| -> Vec<(usize, f64)> {
        let (best_e, best_d) = find_closest_edge_with_dist(&loop_verts, p);
        let slack = (best_d * 1.5).max(1e-6);
        let mut out = Vec::new();
        for i in 0..loop_verts.len() {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % loop_verts.len()];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            let d = (*p - (a + t * ab)).norm();
            if d <= slack + 1e-9 {
                out.push((i, d));
            }
        }
        if out.is_empty() {
            out.push((best_e, best_d));
        }
        out
    };
    let entry_cands = edge_candidates(entry_point);
    let exit_cands = edge_candidates(exit_point);
    let mut entry_edge = entry_cands[0].0;
    let mut exit_edge = exit_cands[0].0;
    let mut entry_dist = entry_cands[0].1;
    let mut exit_dist = exit_cands[0].1;
    let mut best_sum = f64::INFINITY;
    let mut found_distinct = false;
    for &(e1, d1) in &entry_cands {
        for &(e2, d2) in &exit_cands {
            if e1 == e2 {
                continue;
            }
            if d1 + d2 < best_sum {
                best_sum = d1 + d2;
                entry_edge = e1;
                exit_edge = e2;
                entry_dist = d1;
                exit_dist = d2;
                found_distinct = true;
            }
        }
    }
    if !found_distinct {
        entry_edge = entry_cands[0].0;
        exit_edge = exit_cands[0].0;
        entry_dist = entry_cands[0].1;
        exit_dist = exit_cands[0].1;
    }

    // If entry or exit point is too far from any edge, the split line doesn't cross this face
    let max_dist_tolerance = 1.0; // Allow some tolerance for numerical precision
    if entry_dist > max_dist_tolerance || exit_dist > max_dist_tolerance {
        split_dbg!("sfbc: entry/exit off-boundary d=({entry_dist:.4},{exit_dist:.4})");
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // The cut edge between exit and entry follows the true intersection
    // curve when it is sampled (via points), not a single chord.
    let via = cut_polyline_between(curve, entry_point, exit_point);

    // How far the cut path departs from the straight entry→exit chord. A
    // curved cut (an ellipse where a bore meets an oblique face) bulges;
    // a cut that merely retraces an existing edge does not.
    //
    // This is measured from `via`, so it is only ever non-zero for a
    // `Sampled` curve — `cut_polyline_between` yields nothing for the
    // analytic variants. That is deliberate, and it is a real restriction on
    // the bite path below, so state it rather than leave it implicit:
    //
    // * `Line`/`TwoLines`/`Point` — a straight cut entering and leaving
    //   through one edge genuinely bounds no area. Zero bulge is the right
    //   answer and the old rejection is the right behaviour.
    // * `Circle` — a circular mouth CAN bite. It never arrives here in
    //   practice: `split_planar_face` routes circles to
    //   `split_planar_face_by_arc`, which carries its own `same_edge` case,
    //   and curved faces go to the cylindrical/conical/spherical splitters.
    //   Were one to arrive, admitting it on an analytic bulge alone would be
    //   worse than declining: the loops below close along `via`, so the
    //   bite's curved side would be built as a straight chord and would not
    //   weld against the mating wall. Making it work needs arc points, i.e.
    //   the `segments` this function is not given. The decline is logged.
    let chord_bulge = {
        let a = *entry_point;
        let ab = *exit_point - a;
        let len2 = ab.norm_squared();
        via.iter().fold(0.0f64, |acc, p| {
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            acc.max((*p - (a + t * ab)).norm())
        })
    };

    // Curve enters and leaves through the SAME boundary edge. For a straight
    // chord that is no cut at all. For a curved one it is: the arc bulges
    // into the interior and, together with the stretch of edge beneath it,
    // bounds a genuine sub-face — the elliptical mouth of a bore breaking
    // out through an oblique face is exactly this shape, and rejecting it
    // left the mouth unopened and the cut ~20% short with no error raised.
    let same_edge_bite = entry_edge == exit_edge
        && (*exit_point - *entry_point).norm() >= 1e-6
        && chord_bulge > 1e-9;

    if entry_edge == exit_edge && !same_edge_bite && via.is_empty() {
        // Surfaces the case the paragraph above reasons about, so a curve
        // type that starts reaching here shows up in the debug channel
        // instead of silently taking the old rejection.
        split_dbg!(
            "sfbc: same-edge cut declined — no interior samples to prove a bulge (curve {curve:?})"
        );
    }

    if !same_edge_bite && (entry_edge == exit_edge || (*exit_point - *entry_point).norm() < 1e-6) {
        // Curve enters and exits on the same edge, or grazes a single
        // vertex — no real cut.
        split_dbg!(
            "sfbc: same-edge or zero cut (edges {entry_edge},{exit_edge}) entry={:?} exit={:?} loop={:?}",
            entry_point,
            exit_point,
            loop_verts
                .iter()
                .map(|p| ((p.x * 100.0).round() / 100.0, (p.y * 100.0).round() / 100.0, (p.z * 100.0).round() / 100.0))
                .collect::<Vec<_>>()
        );
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // A cut whose midpoint lies ON the face boundary runs along an existing
    // edge (two operand planes can share their intersection line with this
    // face — e.g. a blade's tangent bottom plane and its side plane both
    // cross an end cap along the same chord). Splitting again along that
    // chord would emit a duplicate of the face plus a zero-area sliver.
    //
    // On a same-edge bite — and ONLY there — probe the midpoint of the cut
    // PATH rather than of the chord: the chord of a bite lies along the
    // boundary by construction, so the chord probe would reject every bite.
    // Everywhere else the chord stays the right proxy. Widening the probe to
    // all cuts admits genuine along-the-edge duplicates (it reopened the
    // rotated-blade union's unpaired half-edges, `zz_blade_union`).
    {
        let mid_dist = if via.is_empty() || !same_edge_bite {
            let mid = Point3::new(
                0.5 * (entry_point.x + exit_point.x),
                0.5 * (entry_point.y + exit_point.y),
                0.5 * (entry_point.z + exit_point.z),
            );
            find_closest_edge_with_dist(&loop_verts, &mid).1
        } else {
            // Probe the sample standing FURTHEST from the boundary, not the
            // one at the middle index. The middle is not the apex of this
            // metric: measured on the oblique-bore mouth this fix was built
            // for, distance-to-boundary runs 0 → 16.9 → 9.6 → 16.9 → 0 along
            // the arc, because near its centre the arc is closest to the
            // face's *far* edge. Worse, the samples adjacent to entry and
            // exit sit at distance 0 by construction, so a fixed index into
            // a short `via` can read ~0 and reject a legitimate bite as a
            // boundary-duplicate. The maximum is the only index-independent
            // answer, and it is the quantity the guard actually wants: does
            // this cut leave the boundary ANYWHERE?
            via.iter().fold(0.0f64, |acc, q| {
                acc.max(find_closest_edge_with_dist(&loop_verts, q).1)
            })
        };
        if mid_dist < 1e-7 {
            split_dbg!("sfbc: cut runs along boundary");
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    }

    // Insert new vertices at entry and exit points
    let _v_entry = brep.topology.add_vertex(*entry_point);
    let _v_exit = brep.topology.add_vertex(*exit_point);

    // Build two new vertex loops by walking the original loop
    // Loop 1: entry_point → (edges from entry to exit) → exit_point → (cut back)
    // Loop 2: exit_point → (edges from exit to entry) → entry_point → (cut back)

    let mut loop1_points: Vec<Point3> = Vec::new();
    let mut loop2_points: Vec<Point3> = Vec::new();

    if same_edge_bite {
        // Both endpoints sit on edge `entry_edge`, running A → B. Order them
        // along it so the bite and the remainder both keep the parent's
        // winding.
        let a = loop_verts[entry_edge];
        let b = loop_verts[(entry_edge + 1) % n];
        let ab = b - a;
        let len2 = ab.norm_squared().max(1e-18);
        let along = |p: &Point3| (*p - a).dot(ab) / len2;
        let entry_first = along(entry_point) <= along(exit_point);
        let (p_first, p_second) = if entry_first {
            (*entry_point, *exit_point)
        } else {
            (*exit_point, *entry_point)
        };
        // `via` runs entry→exit; re-orient it to run p_first→p_second.
        let via_fwd: Vec<Point3> = if entry_first {
            via.clone()
        } else {
            via.iter().rev().copied().collect()
        };

        // The bite: along the edge p_first → p_second, back along the curve.
        loop1_points.push(p_first);
        loop1_points.push(p_second);
        loop1_points.extend(via_fwd.iter().rev());

        // The remainder: the whole loop, with the stretch of edge under the
        // bite replaced by the curve.
        let mut idx = (entry_edge + 1) % n;
        loop {
            loop2_points.push(loop_verts[idx]);
            if idx == entry_edge {
                break;
            }
            idx = (idx + 1) % n;
        }
        loop2_points.push(p_first);
        loop2_points.extend(via_fwd.iter());
        loop2_points.push(p_second);
    } else {
        // Walk from entry_edge to exit_edge (one direction)
        loop1_points.push(*entry_point);
        let mut idx = (entry_edge + 1) % n;
        while idx != (exit_edge + 1) % n {
            loop1_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
        loop1_points.push(*exit_point);
        // Close along the cut curve: exit → entry (via reversed).
        loop1_points.extend(via.iter().rev());

        // Walk from exit_edge to entry_edge (other direction)
        loop2_points.push(*exit_point);
        idx = (exit_edge + 1) % n;
        while idx != (entry_edge + 1) % n {
            loop2_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
        loop2_points.push(*entry_point);
        // Close along the cut curve: entry → exit (via forward).
        loop2_points.extend(via.iter());
    }

    // Remove consecutive duplicate vertices (can happen when split points
    // coincide with existing vertices)
    let loop1_points = remove_consecutive_duplicates(&loop1_points, 1e-6);
    let loop2_points = remove_consecutive_duplicates(&loop2_points, 1e-6);
    {
        let zr = |pts: &[Point3]| {
            (
                pts.iter().map(|p| p.z).fold(f64::MAX, f64::min),
                pts.iter().map(|p| p.z).fold(f64::MIN, f64::max),
            )
        };
        let (fz0, fz1) = zr(&loop_verts);
        let (a0, a1) = zr(&loop1_points);
        let (b0, b1) = zr(&loop2_points);
        if a0 < fz0 - 1.0 || a1 > fz1 + 1.0 || b0 < fz0 - 1.0 || b1 > fz1 + 1.0 {
            split_dbg!(
                "sfbc RANGE: face z[{fz0:.2},{fz1:.2}] loops z[{a0:.2},{a1:.2}]/[{b0:.2},{b1:.2}] entry {entry_point:?} exit {exit_point:?} via {} pts",
                via.len()
            );
        }
    }

    // Need at least 3 vertices for a valid face
    if loop1_points.len() < 3 || loop2_points.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Check for degenerate faces (zero or near-zero area)
    // This can happen when the split line lies along an existing edge
    fn polygon_area_3d(points: &[Point3]) -> f64 {
        if points.len() < 3 {
            return 0.0;
        }
        // Sum cross products of triangles from first vertex
        let mut total = vcad_kernel_math::Vec3::zeros();
        let p0 = points[0];
        for i in 1..points.len() - 1 {
            let e1 = points[i] - p0;
            let e2 = points[i + 1] - p0;
            total += e1.cross(e2);
        }
        0.5 * total.norm()
    }

    let area1 = polygon_area_3d(&loop1_points);
    let area2 = polygon_area_3d(&loop2_points);
    // Minimum area threshold - faces smaller than this are considered degenerate
    // The value 0.001 catches thin strips while allowing legitimate small faces
    let min_area = 0.001;

    if area1 < min_area || area2 < min_area {
        split_dbg!("sfbc: degenerate areas {area1:.5} {area2:.5}");
        // One of the faces is degenerate (near-zero area)
        // This happens when the split line lies along an existing edge
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create topology for the two new faces
    // A cut that passes THROUGH an inner loop cannot be resolved by
    // redistributing the hole to one side — the hole would need to be
    // split and merged into the sub-faces' outer boundaries. Refuse the
    // split instead (phantom cuts across holed caps are the common case:
    // the intersection line of a distant operand plane runs across the
    // whole face, hole included).
    {
        let parent_inner = brep.topology.faces[face_id].inner_loops.clone();
        for inner in &parent_inner {
            let verts: Vec<Point3> = brep
                .topology
                .loop_half_edges(*inner)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if verts.is_empty() {
                continue;
            }
            let n = verts.len() as f64;
            let c = Point3::new(
                verts.iter().map(|v| v.x).sum::<f64>() / n,
                verts.iter().map(|v| v.y).sum::<f64>() / n,
                verts.iter().map(|v| v.z).sum::<f64>() / n,
            );
            let r = verts.iter().map(|v| (*v - c).norm()).fold(0.0f64, f64::max);
            // Distance from the hole center to the cut path (chord + via).
            let mut path: Vec<Point3> = vec![*entry_point];
            path.extend(via.iter().copied());
            path.push(*exit_point);
            let mut d = f64::INFINITY;
            for w in path.windows(2) {
                let ab = w[1] - w[0];
                let len2 = ab.norm_squared();
                let t = if len2 < 1e-18 {
                    0.0
                } else {
                    ((c - w[0]).dot(ab) / len2).clamp(0.0, 1.0)
                };
                d = d.min((c - (w[0] + t * ab)).norm());
            }
            if d < r + 1e-6 {
                return SplitResult {
                    sub_faces: vec![face_id],
                };
            }
        }
    }

    let face1 = create_face_from_points(brep, &loop1_points, surface_index, orientation);
    let face2 = create_face_from_points(brep, &loop2_points, surface_index, orientation);

    // Distribute the parent's inner loops (holes) to whichever sub-face
    // contains them — dropping them silently REFILLS the holes (a phantom
    // line split across an annular cap would otherwise fill in the bore).
    let parent_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
    if !parent_inner.is_empty() {
        // 2D frame on the parent plane for containment tests.
        let origin = loop1_points[0];
        let e1 = (loop1_points[1] - origin).normalize();
        let mut e2n = vcad_kernel_math::Vec3::zeros();
        for p in loop1_points.iter().chain(loop2_points.iter()) {
            let d = *p - origin;
            let perp = d - e1 * d.dot(e1);
            if perp.norm() > 1e-9 {
                e2n = perp.normalize();
                break;
            }
        }
        let project = |p: &Point3| -> (f64, f64) {
            let d = *p - origin;
            (d.dot(e1), d.dot(e2n))
        };
        let poly1: Vec<(f64, f64)> = loop1_points.iter().map(&project).collect();
        let poly2: Vec<(f64, f64)> = loop2_points.iter().map(&project).collect();
        for inner in parent_inner {
            let verts: Vec<Point3> = brep
                .topology
                .loop_half_edges(inner)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if verts.is_empty() {
                continue;
            }
            let n = verts.len() as f64;
            let c = Point3::new(
                verts.iter().map(|v| v.x).sum::<f64>() / n,
                verts.iter().map(|v| v.y).sum::<f64>() / n,
                verts.iter().map(|v| v.z).sum::<f64>() / n,
            );
            let (cx, cy) = project(&c);
            let target = if point_in_polygon_2d(cx, cy, &poly1) {
                face1
            } else if point_in_polygon_2d(cx, cy, &poly2) {
                face2
            } else {
                face1 // conservative: keep the hole somewhere
            };
            brep.topology.faces[target].inner_loops.push(inner);
            brep.topology.loops[inner].face = Some(target);
        }
    }

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        // Set shell on new faces
        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face from topology (it's been replaced by sub-faces)
    brep.topology.faces.remove(face_id);

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Find which edge of a polygon a point lies closest to.
/// Returns the index of the starting vertex of that edge.
#[cfg(test)]
fn find_closest_edge(polygon: &[Point3], point: &Point3) -> usize {
    find_closest_edge_with_dist(polygon, point).0
}

/// Find which edge of a polygon a point lies closest to.
/// Returns (edge_index, distance) where edge_index is the starting vertex of that edge.
fn find_closest_edge_with_dist(polygon: &[Point3], point: &Point3) -> (usize, f64) {
    let n = polygon.len();
    let mut best = 0;
    let mut best_dist = f64::INFINITY;

    for i in 0..n {
        let j = (i + 1) % n;
        let dist = point_to_segment_dist(point, &polygon[i], &polygon[j]);
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }

    (best, best_dist)
}

/// Snap a coordinate to 0 if it's very close to 0.
/// This prevents floating point errors like -1e-15 from causing "ear" artifacts.
/// Using 1e-6 as threshold to catch numerical errors from trig operations.
fn snap_coord(v: f64) -> f64 {
    if v.abs() < 1e-6 {
        0.0
    } else {
        v
    }
}

/// Snap a point's coordinates to 0 if they're very close to 0.
fn snap_point(p: Point3) -> Point3 {
    Point3::new(snap_coord(p.x), snap_coord(p.y), snap_coord(p.z))
}

/// Find an existing vertex at the given point, or create a new one.
pub(crate) fn find_or_create_vertex(
    brep: &mut BRepSolid,
    point: &Point3,
    tolerance: f64,
) -> vcad_kernel_topo::VertexId {
    // Snap small values to exactly 0 to avoid floating point artifacts
    let snapped = snap_point(*point);

    // Search for existing vertex within tolerance
    for (vid, vertex) in &brep.topology.vertices {
        let dist = (vertex.point - snapped).norm();
        if dist < tolerance {
            return vid;
        }
    }
    // No existing vertex found, create new one
    brep.topology.add_vertex(snapped)
}

/// Remove consecutive duplicate points from a loop.
/// Also removes duplicates between the last and first point (to handle closed loops).
fn remove_consecutive_duplicates(points: &[Point3], tolerance: f64) -> Vec<Point3> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(points.len());
    result.push(points[0]);

    for p in points.iter().skip(1) {
        let last = result.last().unwrap();
        if (*p - *last).norm() > tolerance {
            result.push(*p);
        }
    }

    // Check if last point duplicates the first (closed loop)
    if result.len() > 1 {
        let first = result[0];
        let last = *result.last().unwrap();
        if (last - first).norm() <= tolerance {
            result.pop();
        }
    }

    result
}

/// Distance from a point to a line segment.
fn point_to_segment_dist(p: &Point3, a: &Point3, b: &Point3) -> f64 {
    let ab = b - a;
    let ap = p - a;
    let len2 = ab.norm_squared();
    if len2 < 1e-20 {
        return ap.norm();
    }
    let t = ap.dot(ab) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj = a + t * ab;
    (p - proj).norm()
}

/// Find where an infinite line crosses the edges of a 3D polygon.
///
/// The polygon vertices must be coplanar. Returns the crossing points
/// in order along the line direction.
///
/// Uses exact orient2d predicates for robust line-segment intersection detection.
fn find_line_polygon_crossings(polygon: &[Point3], line: &vcad_kernel_geom::Line3d) -> Vec<Point3> {
    use vcad_kernel_math::predicates::{orient2d, Sign};

    let n = polygon.len();
    if n < 3 {
        return Vec::new();
    }

    // Compute the polygon's plane normal from the first 3 vertices
    let e1 = polygon[1] - polygon[0];
    let e2 = polygon[2] - polygon[0];
    let plane_normal = e1.cross(e2);
    let plane_normal_len = plane_normal.norm();
    if plane_normal_len < 1e-12 {
        return Vec::new(); // Degenerate polygon
    }
    let plane_normal = plane_normal / plane_normal_len;

    // Build a 2D coordinate system on the plane
    let x_axis = e1.normalize();
    let y_axis = plane_normal.cross(x_axis);

    // Project polygon vertices and line to 2D
    let project_to_2d = |p: &Point3| -> Point2 {
        let d = *p - polygon[0];
        Point2::new(d.dot(x_axis), d.dot(y_axis))
    };

    let poly_2d: Vec<Point2> = polygon.iter().map(&project_to_2d).collect();

    // Project line origin and direction
    let line_origin_2d = project_to_2d(&line.origin);
    let dx = line.direction.dot(x_axis);
    let dy = line.direction.dot(y_axis);
    let dir_2d_len = (dx * dx + dy * dy).sqrt();

    if dir_2d_len < 1e-12 {
        // Line is perpendicular to the polygon plane - no crossing
        return Vec::new();
    }

    // Create two points on the line for orient2d tests
    let line_pt1 = line_origin_2d;
    let line_pt2 = Point2::new(line_origin_2d.x + dx, line_origin_2d.y + dy);

    let mut crossings = Vec::new();

    for i in 0..n {
        let j = (i + 1) % n;
        let a = &poly_2d[i];
        let b = &poly_2d[j];

        // Skip degenerate segments
        let seg_len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if seg_len < 1e-12 {
            continue;
        }

        // Use orient2d to determine if segment endpoints are on opposite sides of the line
        let sign_a = orient2d(&line_pt1, &line_pt2, a);
        let sign_b = orient2d(&line_pt1, &line_pt2, b);

        // If both endpoints are on the same side (and neither is on the line), no crossing
        if sign_a == sign_b && sign_a != Sign::Zero {
            continue;
        }

        // Handle special cases
        if sign_a == Sign::Zero && sign_b == Sign::Zero {
            // Segment lies on the line - no single crossing point
            continue;
        }

        // Compute intersection parameter using Cramer's rule
        // This is only for finding the intersection point location, not for detection
        let sx = b.x - a.x;
        let sy = b.y - a.y;
        let det = sx * dy - dx * sy;

        if det.abs() < 1e-15 {
            // Lines are parallel (orient2d should have caught this, but be safe)
            continue;
        }

        let rhs_x = a.x - line_origin_2d.x;
        let rhs_y = a.y - line_origin_2d.y;

        let t = (sx * rhs_y - sy * rhs_x) / det;
        let s = (dx * rhs_y - dy * rhs_x) / det;

        // s is the parameter along the segment [0, 1]
        // Use a small tolerance for numerical stability
        if !(-1e-9..=1.0 + 1e-9).contains(&s) {
            continue;
        }

        // Compute the 3D intersection point and snap to clean values
        let intersection = snap_point(line.origin + t * line.direction);

        // Avoid duplicate crossings at vertices
        let is_duplicate = crossings
            .iter()
            .any(|c: &Point3| (*c - intersection).norm() < 0.01);
        if !is_duplicate {
            crossings.push(intersection);
        }
    }

    // Sort crossings by their parameter along the line
    let line_dir = line.direction.normalize();
    crossings.sort_by(|a, b| {
        let ta = (*a - line.origin).dot(line_dir);
        let tb = (*b - line.origin).dot(line_dir);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });

    crossings
}

/// Create a new face in the BRep from a set of 3D points.
///
/// Reuses existing vertices within tolerance, creating new ones only when needed.
fn create_face_from_points(
    brep: &mut BRepSolid,
    points: &[Point3],
    surface_index: usize,
    orientation: Orientation,
) -> FaceId {
    // Create or reuse vertices - reuse existing vertices within tolerance
    let tolerance = 1e-6;
    let verts: Vec<_> = points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Create half-edges
    let hes: Vec<_> = verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    // Create loop
    let loop_id = brep.topology.add_loop(&hes);

    // Create face
    brep.topology.add_face(loop_id, surface_index, orientation)
}

/// Split all intersected faces of a solid.
///
/// For each face that has intersection curves crossing it,
/// split the face into sub-faces.
///
/// Returns a mapping from original face IDs to their split results.
pub fn split_intersected_faces(
    brep: &mut BRepSolid,
    face_intersections: &[(FaceId, IntersectionCurve, Point3, Point3)],
) -> Vec<SplitResult> {
    let mut results = Vec::new();

    for (face_id, curve, entry, exit) in face_intersections {
        let result = split_face_by_curve(brep, *face_id, curve, entry, exit);
        results.push(result);
    }

    results
}

// =============================================================================
// Planar Face Splitting by Circle
// =============================================================================

/// Check if a face's underlying surface is a plane.
pub fn is_planar_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Plane
}

/// Whether `circle` lies on a cylindrical wall of `other` — i.e. it was
/// minted by SSI against a cylinder, whose (frozen) band machinery emits
/// canonical-grid ring points. Only such circles need canonical sampling in
/// planar splits; a circle mated to a cone or sphere wall must keep the
/// legacy circle-frame sampling that matches those tessellations.
pub fn circle_on_cylinder_wall(other: &BRepSolid, circle: &vcad_kernel_geom::Circle3d) -> bool {
    let n = circle
        .x_dir
        .into_inner()
        .cross(circle.y_dir.into_inner())
        .normalize();
    other.geometry.surfaces.iter().any(|s| {
        if let Some(cyl) = s
            .as_any()
            .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
        {
            let axis = cyl.axis.into_inner();
            if axis.cross(n).norm() > 1e-6 {
                return false;
            }
            let d = circle.center - cyl.center;
            let radial = d - axis * d.dot(axis);
            return radial.norm() < 1e-6 && (cyl.radius - circle.radius).abs() < 1e-6;
        }
        // Cone walls carry canonical (frozen) rims too: a constant-v circle
        // on a cone must ride the same canonical grid the frozen band and
        // its splits use, or the planar hole ring can never conform.
        if let Some(cone) = s.as_any().downcast_ref::<vcad_kernel_geom::ConeSurface>() {
            let axis = cone.axis.into_inner();
            if axis.cross(n).norm() > 1e-6 {
                return false;
            }
            let d = circle.center - cone.apex;
            let radial = d - axis * d.dot(axis);
            let r_at = d.dot(axis) * cone.half_angle.tan();
            return radial.norm() < 1e-6 && (r_at - circle.radius).abs() < 1e-6 && r_at > 1e-9;
        }
        false
    })
}

/// Split a planar face along a circle intersection curve.
///
/// When a cylinder axis is perpendicular to a plane and they intersect,
/// the result is a circle on the plane. This function splits the planar face into:
/// - An inner face (the disk bounded by the circle)
/// - An outer face (the original polygon with a circular hole)
///
/// The outer face has two loops:
/// - Outer loop: the original polygon boundary
/// - Inner loop: the circle (oriented opposite to outer loop)
///
/// For tessellation, the inner circle is approximated with `segments` vertices.
pub fn split_planar_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    canonical: bool,
) -> SplitResult {
    // A circle approximated by fewer than 3 vertices can't form a valid face
    // loop; constructing one would feed an empty slice to `add_loop` and panic.
    // The kernel resolves the `0 = auto` sentinel upstream, so this only trips
    // for callers that drive the booleans crate directly with a bad count —
    // leave the face unsplit rather than crash the whole evaluation.
    if segments < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;

    // Get outer loop vertices to check if circle is inside the face
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    if loop_verts.len() < 3 {
        // Degenerate outer loop (1-2 vertices) — circular cap face (e.g. cylinder cap).
        // Check analytically if the intersection circle is inside this cap's boundary.
        // If so, create an inner disk face and add the circle as a hole on the cap.
        // Keep the degenerate outer loop as-is (tessellation handles it).
        if let Some(plane) = brep.geometry.surfaces[surface_index]
            .as_any()
            .downcast_ref::<vcad_kernel_geom::Plane>()
        {
            if let Some(&first_pt) = loop_verts.first() {
                let center = plane.origin;
                let to_v = first_pt - center;
                let normal = plane.normal_dir.into_inner();
                let on_plane = to_v - to_v.dot(normal) * normal;
                let cap_radius = on_plane.norm();

                // Check: is the intersection circle coplanar and fully inside?
                // Tangency counts as inside — a boss ring exactly inscribed to
                // the cap rim (offset + r == cap_radius) must still punch its
                // hole through the cap; leaving the membrane whole corrupts
                // the volume integral by the full contact-disk flux.
                let circle_to_plane = (circle.center - center).dot(normal).abs();
                let circle_center_offset = {
                    let d = circle.center - center;
                    (d - d.dot(normal) * normal).norm()
                };
                // A circle that IS the cap's own rim (concentric, same
                // radius — e.g. a press-fit ring whose wall lies exactly on
                // the wall that bounds this cap) has nothing to split:
                // admitting it would punch a hole covering the entire cap
                // and leave a duplicate full-size disk. This is the
                // degenerate-loop analog of `circle_is_own_boundary`.
                if circle_to_plane < 1e-6
                    && circle_center_offset < 1e-6
                    && (circle.radius - cap_radius).abs() < 1e-6
                {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
                let circle_inside = circle_to_plane < 1e-6
                    && circle_center_offset + circle.radius <= cap_radius + 1e-6;
                // Partial overlap: part of the circle is within the rim, part
                // sticks out (a boss overhanging the edge of a disc).
                let circle_overlaps = circle_to_plane < 1e-6
                    && !circle_inside
                    && circle_center_offset - circle.radius < cap_radius - 1e-6
                    && circle_center_offset < cap_radius + circle.radius - 1e-6;

                if circle_overlaps && cap_radius > 1e-12 {
                    // The degenerate single-vertex outer loop can't express a
                    // partial split. Sample the rim into an explicit polygon
                    // outer loop and re-enter this function: the polygonal
                    // path detects the partial overlap and routes to the arc
                    // splitter. Sample densely — the polygon becomes the
                    // cap's real boundary from here on, and a coarse rim
                    // under-integrates the cap area.
                    let n_rim = segments.max(128) as usize;
                    let cap_x = on_plane.normalize();
                    let cap_y = normal.cross(cap_x);
                    let rim_ids: Vec<_> = (0..n_rim)
                        .map(|i| {
                            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n_rim as f64);
                            let p =
                                center + cap_radius * (theta.cos() * cap_x + theta.sin() * cap_y);
                            find_or_create_vertex(brep, &p, 1e-6)
                        })
                        .collect();
                    let rim_hes: Vec<_> = rim_ids
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let new_outer = brep.topology.add_loop(&rim_hes);
                    brep.topology.faces[face_id].outer_loop = new_outer;
                    return split_planar_face_by_circle(brep, face_id, circle, segments, canonical);
                }

                if circle_inside && cap_radius > 1e-12 {
                    let tolerance = 1e-6;

                    // Generate circle vertices for the inner disk face on
                    // the CANONICAL grid — the frozen cylinder wall bounded
                    // by this same circle emits identical points, so the
                    // hole rim and the wall ring conform exactly.
                    // ...but only when the mating wall IS a cylinder
                    // (`canonical`); cone/sphere walls tessellate their rings
                    // in the circle's own frame at `segments`, and a
                    // canonical-grid rim would not conform with them.
                    let circle_normal = circle.x_dir.into_inner().cross(circle.y_dir.into_inner());
                    let raw_circle_verts: Vec<Point3> = if canonical {
                        canonical_circle_points(
                            circle.center,
                            circle.radius,
                            circle_normal,
                            segments,
                        )
                    } else {
                        (0..segments)
                            .map(|i| {
                                let theta =
                                    2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                                let (sin_t, cos_t) = theta.sin_cos();
                                circle.center
                                    + circle.radius
                                        * (cos_t * circle.x_dir.into_inner()
                                            + sin_t * circle.y_dir.into_inner())
                            })
                            .collect()
                    };

                    // The SSI circle's (x_dir, y_dir) frame is arbitrary, so
                    // the generated ring can wind either way. The tessellator
                    // expects loops CCW around the STORED surface normal
                    // (`Orientation::Reversed` flips triangles afterwards) —
                    // normalize the disk loop to that convention. The disk is
                    // usually classified Inside and dropped, but when it
                    // survives (a bore mouth kept as the visible floor under
                    // a press-fit ring) a backwards loop tessellates facing
                    // into the solid and the volume integral goes negative.
                    let cap_x_dir = on_plane.normalize();
                    let cap_y_dir = normal.cross(cap_x_dir);
                    let circle_signed_area = {
                        let project = |p: &Point3| -> (f64, f64) {
                            let d = *p - center;
                            (d.dot(cap_x_dir), d.dot(cap_y_dir))
                        };
                        let pts_2d: Vec<_> = raw_circle_verts.iter().map(project).collect();
                        let mut a = 0.0;
                        for i in 0..pts_2d.len() {
                            let j = (i + 1) % pts_2d.len();
                            a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
                        }
                        a / 2.0
                    };
                    let circle_verts: Vec<Point3> = if circle_signed_area < 0.0 {
                        raw_circle_verts.iter().rev().cloned().collect()
                    } else {
                        raw_circle_verts
                    };

                    // Create inner disk face (will be classified as "inside" and removed)
                    let inner_verts: Vec<_> = circle_verts
                        .iter()
                        .map(|p| find_or_create_vertex(brep, p, tolerance))
                        .collect();
                    let inner_hes: Vec<_> = inner_verts
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let inner_loop = brep.topology.add_loop(&inner_hes);
                    let inner_face = brep
                        .topology
                        .add_face(inner_loop, surface_index, orientation);

                    // Route the cap's pre-existing inner loops (holes from prior
                    // booleans) to the sub-face that now contains them, mirroring
                    // the polygonal-outer-loop path below. A hole inside the
                    // splitting circle belongs to the new disk face; leaving it on
                    // the cap both strands a nested hole on the ring and lets the
                    // disk seal the hole with a phantom membrane (e.g. a bearing
                    // bore closed at its mouth), which corrupts point-in-solid
                    // parity for every later boolean against this solid.
                    let existing_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
                    for lp in existing_inner {
                        let lp_verts: Vec<Point3> = brep
                            .topology
                            .loop_half_edges(lp)
                            .map(|he| {
                                brep.topology.vertices[brep.topology.half_edges[he].origin].point
                            })
                            .collect();
                        if lp_verts.is_empty() {
                            continue;
                        }
                        if loop_vs_circle(&lp_verts, circle, tolerance) == LoopVsCircle::Inside {
                            brep.topology.faces[face_id]
                                .inner_loops
                                .retain(|&l| l != lp);
                            brep.topology.faces[inner_face].inner_loops.push(lp);
                        }
                    }

                    // Add the circle as an inner loop (hole) on the original cap face.
                    // Inner loops wind opposite to the outer convention: the
                    // disk loop is CCW around the surface normal, so the hole
                    // is its reverse (CW).
                    let hole_verts: Vec<Point3> = circle_verts.iter().rev().cloned().collect();

                    let hole_vert_ids: Vec<_> = hole_verts
                        .iter()
                        .map(|p| find_or_create_vertex(brep, p, tolerance))
                        .collect();
                    let hole_hes: Vec<_> = hole_vert_ids
                        .iter()
                        .map(|&v| brep.topology.add_half_edge(v))
                        .collect();
                    let hole_loop = brep.topology.add_loop(&hole_hes);
                    brep.topology.faces[face_id].inner_loops.push(hole_loop);

                    // Add twin edges between inner disk and hole
                    for i in 0..segments as usize {
                        let inner_he = inner_hes[i];
                        let outer_he = hole_hes[(segments as usize - 1 - i) % segments as usize];
                        brep.topology.add_edge(inner_he, outer_he);
                    }

                    // Add faces to shell
                    if let Some(shell_id) = brep.topology.faces[face_id].shell {
                        brep.topology.shells[shell_id].faces.push(inner_face);
                        brep.topology.faces[inner_face].shell = Some(shell_id);
                    }

                    brep.geometry.add_curve_3d(Box::new(circle.clone()));

                    return SplitResult {
                        sub_faces: vec![inner_face, face_id],
                    };
                }
            }
        }

        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // A circle that lies inside one of this face's HOLES is not inside the
    // face at all: the face's material is the outer loop minus its inner
    // loops. Splitting on it manufactures a disk sub-face covering the hole
    // (a membrane the tessellator then draws over the bore) plus a
    // redundant nested hole on the ring — measured as 691 non-manifold
    // edges on a stacked-ring union, where B's smaller wall circle lands
    // inside the ring that B's larger wall had already opened.
    let inner_loops = brep.topology.faces[face_id].inner_loops.clone();
    for lp in inner_loops {
        let hole_verts: Vec<Point3> = brep
            .topology
            .loop_half_edges(lp)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();
        if hole_verts.len() < 3 {
            continue;
        }
        // Coincident with the hole rim: that boundary already exists.
        if loop_vs_circle(&hole_verts, circle, 1e-6) == LoopVsCircle::Coincident
            || circle_fully_inside_polygon(&hole_verts, circle)
        {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    }

    // Check if the FULL circle is inside the polygon
    // We need to verify not just that the circle overlaps, but that it's fully contained.
    // If only partially inside, use arc-based splitting instead.
    if !circle_fully_inside_polygon(&loop_verts, circle) {
        // Check if circle partially intersects (crosses exactly 2 edges)
        if circle_partially_inside_polygon(&loop_verts, circle) {
            return split_planar_face_by_arc(brep, face_id, circle, segments);
        }
        // The circle doesn't cross this face, but it may be TANGENT to one
        // of its edges from outside (the neighbor of a cell the circle is
        // inscribed in). The inscribed split puts a vertex at every tangent
        // point, so this face's straight edge must gain the same vertex or
        // the shared boundary T-junctions and the shell zippers open.
        insert_circle_tangent_vertices(brep, face_id, circle);
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Generate circle vertices in the SSI circle's own (x_dir, y_dir) frame;
    // the winding relative to the face is normalized below.
    // Canonical grid sampling when the mating wall is a cylinder — must
    // match the frozen cylinder wall rings bounded by the same circle (see
    // canonical_circle_points). Cone/sphere-mated circles keep the legacy
    // circle-frame sampling their wall tessellations conform to.
    let raw_circle_verts: Vec<Point3> = if canonical {
        canonical_circle_points(
            circle.center,
            circle.radius,
            circle.x_dir.into_inner().cross(circle.y_dir.into_inner()),
            segments,
        )
    } else {
        (0..segments)
            .map(|i| {
                let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                let (sin_t, cos_t) = theta.sin_cos();
                circle.center
                    + circle.radius
                        * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
            })
            .collect()
    };

    // Compute the face's 2D coordinate system using the plane surface normal
    // instead of deriving from vertices, which can produce inconsistent normals
    // for rotated/non-axis-aligned faces depending on vertex ordering.
    let face_normal = if let Some(plane) = brep.geometry.surfaces[surface_index]
        .as_any()
        .downcast_ref::<vcad_kernel_geom::Plane>()
    {
        let n = plane.normal_dir.into_inner();
        // Account for face orientation: reversed faces flip the normal
        if orientation == Orientation::Reversed {
            -n
        } else {
            n
        }
    } else {
        // Fallback: derive from vertices (shouldn't happen for planar faces)
        let e1 = loop_verts[1] - loop_verts[0];
        let e2 = loop_verts[2] - loop_verts[0];
        e1.cross(e2)
    };
    let e1 = loop_verts[1] - loop_verts[0];
    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(e1).normalize();
    let origin = loop_verts[0];

    // Project to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u_axis), d.dot(v_axis))
    };

    // Compute signed area to determine winding direction
    // Positive = CCW, Negative = CW in our 2D projection
    let signed_area = |pts: &[Point3]| -> f64 {
        let pts_2d: Vec<_> = pts.iter().map(&project).collect();
        let mut area = 0.0;
        for i in 0..pts_2d.len() {
            let j = (i + 1) % pts_2d.len();
            area += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
        }
        area / 2.0
    };

    let outer_area = signed_area(&loop_verts);
    let circle_area = signed_area(&raw_circle_verts);

    // The disk sub-face's outer loop must wind the SAME way as the parent's
    // outer loop (both are outer loops of faces with identical surface and
    // orientation — the tessellator triangulates by loop order and flips for
    // `Orientation::Reversed`, so a backwards disk loop would tessellate
    // facing into the solid and corrupt the volume integral whenever the
    // disk survives classification). The hole loop is its reverse.
    let circle_verts: Vec<Point3> = if (outer_area > 0.0) == (circle_area > 0.0) {
        raw_circle_verts
    } else {
        raw_circle_verts.iter().rev().cloned().collect()
    };

    // Create inner face (disk) - uses circle vertices as its outer loop
    let tolerance = 1e-6;
    let inner_verts: Vec<_> = circle_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    let inner_hes: Vec<_> = inner_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let inner_loop = brep.topology.add_loop(&inner_hes);
    let inner_face = brep
        .topology
        .add_face(inner_loop, surface_index, orientation);

    // Create outer face (polygon with hole): the outer loop stays the same;
    // the circle joins as an inner loop with opposite winding to the outer.
    let inner_loop_verts: Vec<Point3> = circle_verts.iter().rev().cloned().collect();

    let outer_inner_verts: Vec<_> = inner_loop_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Create new outer loop (copy of original)
    let outer_verts: Vec<_> = loop_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    let outer_hes: Vec<_> = outer_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let new_outer_loop = brep.topology.add_loop(&outer_hes);
    let outer_face = brep
        .topology
        .add_face(new_outer_loop, surface_index, orientation);

    // Add the inner loop (hole) to the outer face
    let hole_hes: Vec<_> = outer_inner_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();

    let hole_loop = brep.topology.add_loop(&hole_hes);
    brep.topology.faces[outer_face].inner_loops.push(hole_loop);

    // Copy existing inner loops from the original face, routing each to the correct
    // sub-face: if a loop is inside the new circle it belongs to the inner face (disk),
    // otherwise it belongs to the outer face (polygon with hole).
    let existing_inner_loops = brep.topology.faces[face_id].inner_loops.clone();
    for existing_loop in existing_inner_loops {
        // Re-create the inner loop with new half-edges for the target face
        let loop_verts_existing: Vec<Point3> = brep
            .topology
            .loop_half_edges(existing_loop)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();

        // Determine which face this inner loop belongs to by checking if its
        // representative vertex is inside the new circle
        let target_face = if !loop_verts_existing.is_empty() {
            match loop_vs_circle(&loop_verts_existing, circle, tolerance) {
                // Inside the circle → belongs to the disk sub-face.
                LoopVsCircle::Inside => inner_face,
                // The loop IS the splitting circle: the split has already
                // created that boundary as `hole_loop` on the outer face
                // and as the disk's own outer loop. Re-adding it would
                // give the outer face two copies of one hole.
                LoopVsCircle::Coincident => continue,
                // Outside, or crossing the circle (which no valid hole
                // does — keep it on the ring, where the parent had it).
                _ => outer_face,
            }
        } else {
            outer_face // Empty loop, default to outer
        };

        let new_verts: Vec<_> = loop_verts_existing
            .iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect();

        let new_hes: Vec<_> = new_verts
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();

        let new_loop = brep.topology.add_loop(&new_hes);
        brep.topology.faces[target_face].inner_loops.push(new_loop);
    }

    // Add twin edges between inner face circle and outer face hole
    // (they share the same physical edges but with opposite orientation)
    for i in 0..segments as usize {
        let inner_he = inner_hes[i];
        // The outer hole is reversed, so we need to match edges correctly
        // inner_hes[i] corresponds to outer_inner_hes[segments - 1 - i]
        let outer_he = hole_hes[(segments as usize - 1 - i) % segments as usize];
        brep.topology.add_edge(inner_he, outer_he);
    }

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(inner_face);
        brep.topology.shells[shell_id].faces.push(outer_face);

        brep.topology.faces[inner_face].shell = Some(shell_id);
        brep.topology.faces[outer_face].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add the 3D circle curve to geometry
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![inner_face, outer_face],
    }
}

/// Insert vertices into a planar face's outer loop where `circle` is
/// tangent to one of its edges.
///
/// When a circle is inscribed in one grid cell (see
/// `split_planar_face_inscribed_circle`), each tangent point becomes a
/// loop vertex on that cell's sub-faces. The neighboring cell shares the
/// tangent edge but never sees a split, so without the matching vertex the
/// two sides tessellate different point schedules along the shared edge —
/// a T-junction. This rebuilds the neighbor's loop with the tangent points
/// inserted; the face itself is not split.
fn insert_circle_tangent_vertices(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) {
    let tol = 1e-6;
    let outer_loop = brep.topology.faces[face_id].outer_loop;
    let loop_verts: Vec<Point3> = brep
        .topology
        .loop_half_edges(outer_loop)
        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();
    let n = loop_verts.len();
    if n < 3 {
        return;
    }
    let normal = circle.normal.into_inner();

    // For each edge, the closest interior point to the circle center;
    // tangency means that point sits on the circle (and in its plane).
    let mut inserts: Vec<(usize, f64, Point3)> = Vec::new();
    for i in 0..n {
        let a = loop_verts[i];
        let b = loop_verts[(i + 1) % n];
        let ab = b - a;
        let len2 = ab.norm_squared();
        if len2 < 1e-18 {
            continue;
        }
        let t = ((circle.center - a).dot(ab) / len2).clamp(0.0, 1.0);
        // Endpoints are already vertices; only interior tangencies matter.
        if !(1e-6..=(1.0 - 1e-6)).contains(&t) {
            continue;
        }
        let foot = a + t * ab;
        if (foot - circle.center).dot(normal).abs() > tol {
            continue;
        }
        let d = foot - circle.center;
        if ((d.norm()) - circle.radius).abs() > tol {
            continue;
        }
        // Snap onto the exact circle so it merges with the tangent vertex
        // the inscribed split creates on the other side of the edge.
        let point = circle.center + circle.radius * (d / d.norm());
        if (point - a).norm() < tol || (point - b).norm() < tol {
            continue;
        }
        // Only ADOPT a tangent vertex the inscribed split already created;
        // never mint one. A circle can be tangent to this edge without
        // anything having been split across it — a cylinder grazing a cube
        // face by ~1e-6 is tangent to within `tol` while the neighbor keeps
        // its original two-vertex edge. Inserting there would put a vertex
        // on one side of a shared edge and nothing on the other, which is
        // the very T-junction this function exists to prevent.
        let snapped = snap_point(point);
        if !brep
            .topology
            .vertices
            .values()
            .any(|v| (v.point - snapped).norm() < tol)
        {
            continue;
        }
        inserts.push((i, t, point));
    }
    if inserts.is_empty() {
        return;
    }
    inserts.sort_by(|x, y| {
        (x.0 as f64 + x.1)
            .partial_cmp(&(y.0 as f64 + y.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Rebuild the outer loop with the tangent vertices inserted. The old
    // loop is left orphaned (no face references it), matching the
    // degenerate-cap rebuild above.
    let mut points: Vec<Point3> = Vec::with_capacity(n + inserts.len());
    for (i, &v) in loop_verts.iter().enumerate() {
        points.push(v);
        for &(ei, _, p) in &inserts {
            if ei == i {
                points.push(p);
            }
        }
    }
    let vert_ids: Vec<_> = points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tol))
        .collect();
    let hes: Vec<_> = vert_ids
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let new_loop = brep.topology.add_loop(&hes);
    brep.topology.faces[face_id].outer_loop = new_loop;
}

/// Check if a circle is FULLY inside a polygon (in 3D, assumes coplanar).
///
/// Returns true only if the entire circle is contained within the polygon.
/// Used by split_planar_face_by_circle to decide whether to create a full disk.
fn circle_fully_inside_polygon(polygon: &[Point3], circle: &vcad_kernel_geom::Circle3d) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for the projection plane instead of
    // deriving from the first 3 polygon vertices, which can produce an
    // inconsistent normal direction for rotated/non-axis-aligned faces.
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    // Build a 2D coordinate system from the circle's normal
    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(u_axis);
    let v_axis_len = v_axis.norm();
    if v_axis_len < 1e-12 {
        return false;
    }
    let v_axis = v_axis / v_axis_len;

    let project = |p: &Point3| -> (f64, f64) {
        let d = p - v0;
        (d.dot(u_axis), d.dot(v_axis))
    };

    // Project circle center
    let (cx, cy) = project(&circle.center);

    // Project polygon vertices
    let poly_2d: Vec<(f64, f64)> = polygon.iter().map(project).collect();

    // Check if circle center is inside the polygon
    if !point_in_polygon_2d(cx, cy, &poly_2d) {
        return false;
    }

    // Check that the circle doesn't cross any polygon edge
    // i.e., distance from center to each edge must be > radius.
    // A circle tangent to a polygon edge (distance ≈ radius) also disqualifies
    // it from the "strictly inside" path: splitting a face whose inner hole
    // touches its outer boundary produces degenerate tessellation, and the
    // inscribed cylinder case is handled correctly by leaving the face whole
    // and relying on coincidence-based classification.
    let n = poly_2d.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = poly_2d[i];
        let (x2, y2) = poly_2d[j];

        let dist = point_to_segment_dist_2d(cx, cy, x1, y1, x2, y2);
        if dist < circle.radius + 1e-6 {
            return false;
        }
    }

    true
}

/// Where an existing hole loop sits relative to a splitting circle.
///
/// Routing holes by their CENTROID is wrong for the commonest hole shape
/// there is: a circle. Every concentric loop — whatever its radius —
/// has its centroid at the shared center, so a hole *larger* than the
/// splitting circle tested "inside" and was handed to the disk sub-face.
/// The result is a face whose hole is bigger than its own outer loop
/// (measured: an r24 disk carrying an r27.5 hole), which the tessellator
/// draws as the full disk — a membrane over the hole, doubled surface,
/// and non-manifold edges in the exported mesh. Nested annular caps are
/// exactly what a union-of-differences produces, which is why the defect
/// only showed up on stacked rings.
///
/// Classify by the loop's VERTICES instead, which is what containment
/// actually means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopVsCircle {
    /// Every vertex strictly inside the circle.
    Inside,
    /// Every vertex strictly outside.
    Outside,
    /// The loop *is* the circle (within tolerance).
    Coincident,
    /// The loop crosses the circle — it belongs to neither sub-face
    /// cleanly.
    Straddles,
}

fn loop_vs_circle(
    loop_verts: &[Point3],
    circle: &vcad_kernel_geom::Circle3d,
    tol: f64,
) -> LoopVsCircle {
    let mut inside = false;
    let mut outside = false;
    let mut on = false;
    for p in loop_verts {
        // Distance measured in the circle's own plane: a loop on a
        // parallel plane (the far cap of a through-hole) must still
        // classify by its radius, not by its 3D distance to the center.
        let d = *p - circle.center;
        let n = circle.normal.into_inner();
        let radial = (d - d.dot(n) * n).norm();
        if radial < circle.radius - tol {
            inside = true;
        } else if radial > circle.radius + tol {
            outside = true;
        } else {
            on = true;
        }
    }
    match (inside, outside) {
        (true, false) => LoopVsCircle::Inside,
        (false, true) => LoopVsCircle::Outside,
        (false, false) if on => LoopVsCircle::Coincident,
        _ => LoopVsCircle::Straddles,
    }
}

/// Point-in-polygon test using ray casting (2D version).
fn point_in_polygon_2d(px: f64, py: f64, polygon: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = polygon.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];

        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from point to line segment (2D).
pub(crate) fn point_to_segment_dist_2d(
    px: f64,
    py: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-15 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = ((px - x1) * dx + (py - y1) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
}

// =============================================================================
// Arc-Based Planar Face Splitting
// =============================================================================

/// A circle-polygon intersection point with metadata.
#[derive(Debug, Clone)]
struct CirclePolygonIntersection {
    /// The 3D intersection point.
    point: Point3,
    /// The 2D intersection point (projected onto the polygon plane).
    point_2d: (f64, f64),
    /// The edge index (starting vertex) where the intersection occurs.
    edge_index: usize,
    /// The parameter t along the edge (0 = start vertex, 1 = end vertex).
    t_along_edge: f64,
    /// The angle on the circle (0 to 2π).
    angle: f64,
}

/// Find where a circle intersects a polygon's edges.
///
/// Returns intersection points sorted by angle on the circle.
/// Each intersection includes the edge index, parameter along edge, and angle on circle.
fn find_circle_polygon_intersections(
    _polygon_3d: &[Point3],
    polygon_2d: &[(f64, f64)],
    circle_center_2d: (f64, f64),
    radius: f64,
    origin_3d: Point3,
    u_axis: vcad_kernel_math::Vec3,
    v_axis: vcad_kernel_math::Vec3,
) -> Vec<CirclePolygonIntersection> {
    let n = polygon_2d.len();
    let mut intersections = Vec::new();
    let (cx, cy) = circle_center_2d;
    let tol = 1e-9;

    for i in 0..n {
        let j = (i + 1) % n;
        let (x1, y1) = polygon_2d[i];
        let (x2, y2) = polygon_2d[j];

        // Solve for line-circle intersection in 2D.
        // Line: P(t) = (x1, y1) + t * (x2 - x1, y2 - y1)
        // Circle: (x - cx)² + (y - cy)² = r²
        //
        // Substituting:
        // (x1 + t*dx - cx)² + (y1 + t*dy - cy)² = r²
        // Let ax = x1 - cx, ay = y1 - cy, dx = x2 - x1, dy = y2 - y1
        // (ax + t*dx)² + (ay + t*dy)² = r²
        // ax² + 2*ax*t*dx + t²*dx² + ay² + 2*ay*t*dy + t²*dy² = r²
        // t²*(dx² + dy²) + 2*t*(ax*dx + ay*dy) + (ax² + ay² - r²) = 0

        let dx = x2 - x1;
        let dy = y2 - y1;
        let ax = x1 - cx;
        let ay = y1 - cy;

        let a = dx * dx + dy * dy;
        let b = 2.0 * (ax * dx + ay * dy);
        let c = ax * ax + ay * ay - radius * radius;

        if a.abs() < tol {
            // Degenerate edge (zero length)
            continue;
        }

        let discriminant = b * b - 4.0 * a * c;
        if discriminant < -tol {
            // No intersection
            continue;
        }

        let discriminant = discriminant.max(0.0).sqrt();

        for sign in [-1.0, 1.0] {
            let t = (-b + sign * discriminant) / (2.0 * a);

            // Check if intersection is within the segment [0, 1]
            if t < -tol || t > 1.0 + tol {
                continue;
            }

            // Clamp t to [0, 1] for robustness
            let t = t.clamp(0.0, 1.0);

            // Compute 2D intersection point
            let px = x1 + t * dx;
            let py = y1 + t * dy;

            // Compute angle on circle
            let angle = (py - cy).atan2(px - cx);
            let angle = if angle < 0.0 {
                angle + 2.0 * std::f64::consts::PI
            } else {
                angle
            };

            // Compute 3D point
            let point_3d = origin_3d + px * u_axis + py * v_axis;

            // Avoid duplicate intersections (at corners)
            let is_duplicate = intersections
                .iter()
                .any(|other: &CirclePolygonIntersection| {
                    let dist_2d =
                        ((px - other.point_2d.0).powi(2) + (py - other.point_2d.1).powi(2)).sqrt();
                    dist_2d < 0.01
                });

            if !is_duplicate {
                intersections.push(CirclePolygonIntersection {
                    point: point_3d,
                    point_2d: (px, py),
                    edge_index: i,
                    t_along_edge: t,
                    angle,
                });
            }
        }
    }

    // Sort by angle for consistent ordering
    intersections.sort_by(|a, b| {
        a.angle
            .partial_cmp(&b.angle)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    intersections
}

/// Split a planar face along an arc where a circle partially intersects it.
///
/// When a circle only partially overlaps a polygon face, this function:
/// 1. Finds where the circle crosses polygon edges (2 intersection points)
/// 2. Determines which arc is inside the polygon
/// 3. Creates two faces:
///    - Face with arc boundary (inside the circle)
///    - Face with chord boundary (outside the circle)
///
/// Returns the original face unchanged if:
/// - The circle doesn't intersect the polygon at exactly 2 points
/// - The intersections are too close together
pub fn split_planar_face_by_arc(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let outer_loop = face.outer_loop;

    // Get outer loop vertices
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(outer_loop).collect();
    let loop_verts: Vec<Point3> = loop_hes
        .iter()
        .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();

    if loop_verts.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Build the 2D frame from the CIRCLE's normal, not from the first three
    // loop vertices. Those three are routinely colinear: every split inserts
    // vertices along the edge it cuts, so a face that has already been split
    // once often begins with three points on one straight edge, and
    // `e1 × e2` is then zero. The bail that followed was silent — it left
    // the entry mouth of a bore uncut where the tool met an already-split
    // wall, and the only symptom was an unwelded rim in the exported mesh.
    //
    // The circle lies in this face's plane by construction, and the
    // containment predicates that route here (`circle_fully_inside_polygon`,
    // `circle_partially_inside_polygon`) already use exactly this frame, so
    // taking it from the circle also keeps their verdicts consistent with
    // the split that acts on them.
    let v0 = loop_verts[0];
    let normal = circle.normal.into_inner();
    // First loop vertex genuinely distinct from v0, projected into the
    // plane so u stays orthogonal to the normal under round-off.
    let u_axis = match loop_verts.iter().skip(1).find_map(|p| {
        let d = *p - v0;
        let d = d - d.dot(normal) * normal;
        (d.norm() > 1e-12).then(|| d.normalize())
    }) {
        Some(u) => u,
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };
    let v_axis = normal.cross(u_axis);
    let v_axis_len = v_axis.norm();
    if v_axis_len < 1e-12 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }
    let v_axis = v_axis / v_axis_len;
    let origin = v0;

    // Project polygon vertices to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u_axis), d.dot(v_axis))
    };

    let poly_2d: Vec<(f64, f64)> = loop_verts.iter().map(&project).collect();

    // Project circle center to 2D
    let center_2d = project(&circle.center);

    // Find circle-polygon intersections
    let intersections = find_circle_polygon_intersections(
        &loop_verts,
        &poly_2d,
        center_2d,
        circle.radius,
        origin,
        u_axis,
        v_axis,
    );

    // Fewer than 2 crossings: the circle doesn't cross the boundary at all
    // (fully inside/outside, handled by the caller) or only grazes it.
    if intersections.len() < 2 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // More than 2 crossings (e.g. a fillet circle clipping a face corner
    // crosses two adjacent edges twice each): trace the polygon∩disk and
    // polygon∖disk regions generically so every boundary follows the true
    // arc instead of leaving the face chorded (or unsplit) while the curved
    // partner follows the circle.
    if intersections.len() > 2 {
        return split_planar_face_by_multi_arc(
            brep,
            face_id,
            circle,
            segments,
            &loop_verts,
            &poly_2d,
            center_2d,
            origin,
            u_axis,
            v_axis,
            &intersections,
        );
    }

    let int1 = &intersections[0];
    let int2 = &intersections[1];

    // Check if intersections are too close together (would create degenerate faces)
    let dist = ((int1.point_2d.0 - int2.point_2d.0).powi(2)
        + (int1.point_2d.1 - int2.point_2d.1).powi(2))
    .sqrt();
    if dist < 0.01 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Determine which arc (from int1 to int2) is inside the polygon.
    // The arc from angle1 to angle2 (CCW) might be inside or outside.
    // Check by sampling the arc midpoint.

    let angle1 = int1.angle;
    let angle2 = int2.angle;

    // Arc 1: from angle1 to angle2 (CCW, shorter if angle2 > angle1)
    // Arc 2: from angle2 to angle1 (CCW, wrapping around)
    let arc1_mid_angle = if angle2 >= angle1 {
        (angle1 + angle2) / 2.0
    } else {
        // Wraps around
        let mid = (angle1 + angle2 + 2.0 * std::f64::consts::PI) / 2.0;
        if mid >= 2.0 * std::f64::consts::PI {
            mid - 2.0 * std::f64::consts::PI
        } else {
            mid
        }
    };

    let arc1_mid_x = center_2d.0 + circle.radius * arc1_mid_angle.cos();
    let arc1_mid_y = center_2d.1 + circle.radius * arc1_mid_angle.sin();
    let arc1_inside = point_in_polygon_2d(arc1_mid_x, arc1_mid_y, &poly_2d);

    // Tangent-inside case: when the circle is fully inside the polygon but
    // tangent to two polygon edges, both arcs (the two halves of the circle
    // separated by the chord through the tangent points) lie inside the
    // polygon. Picking either arc arbitrarily produces a face whose boundary
    // bulges through the disk in the wrong direction (issue #165). Split the
    // polygon along the chord first, then recurse: each sub-polygon contains
    // exactly one of the two arcs, so the inside/outside check resolves
    // unambiguously.
    let arc2_mid_angle = arc1_mid_angle + std::f64::consts::PI;
    let arc2_mid_x = center_2d.0 + circle.radius * arc2_mid_angle.cos();
    let arc2_mid_y = center_2d.1 + circle.radius * arc2_mid_angle.sin();
    let arc2_inside = point_in_polygon_2d(arc2_mid_x, arc2_mid_y, &poly_2d);
    if arc1_inside && arc2_inside {
        return split_planar_face_tangent_inside(brep, face_id, circle, segments, int1, int2);
    }
    if !arc1_inside && !arc2_inside {
        // Neither arc's midpoint lies inside the polygon: the circle runs
        // along the face boundary (a re-application of a circle this face
        // was already cut by — its arc IS an edge now, and the midpoint
        // probe lands on/outside it). Blindly taking the complementary arc
        // here swept a near-full circle through the face and minted a
        // phantom sub-face covering the far side of the disk.
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Determine which arc is inside and which edge indices to walk
    let (inside_start, inside_end, inside_start_angle, inside_end_angle) = if arc1_inside {
        (int1, int2, angle1, angle2)
    } else {
        (int2, int1, angle2, angle1)
    };

    // Compute arc span (always positive, CCW direction)
    let arc_span = if inside_end_angle >= inside_start_angle {
        inside_end_angle - inside_start_angle
    } else {
        2.0 * std::f64::consts::PI - inside_start_angle + inside_end_angle
    };

    let _ = arc_span;
    // Arc travels CCW in the (u, v) plane frame = CCW about u×v. Interior
    // points MUST come from the canonical absolute grid so the cylindrical
    // wall bordering this same circle emits identical vertices.
    let plane_normal = u_axis.cross(v_axis);
    let arc_points_3d = canonical_arc_points(
        circle.center,
        circle.radius,
        plane_normal,
        inside_start.point,
        inside_end.point,
        segments,
    );

    // A cut whose arc midpoint lies ON the face boundary runs along an
    // existing arc edge: the same circle reaches this face once per wall
    // piece of the other operand, and re-splitting along the previous cut
    // emits a duplicate face plus a phantom sliver (the arc analog of the
    // duplicate-chord guard in split_face_by_curve). The probe must be the
    // TRUE angular midpoint of the arc — short arcs carry no interior
    // polyline vertices, and an endpoint always sits on the boundary.
    {
        let n_hat = plane_normal.normalize();
        let d0 = inside_start.point - circle.center;
        let d1 = inside_end.point - circle.center;
        let ang = {
            let cross = d0.cross(d1).dot(n_hat);
            let a = cross.atan2(d0.dot(d1));
            a.rem_euclid(2.0 * std::f64::consts::PI)
        };
        let half = 0.5 * ang;
        let (sin_h, cos_h) = half.sin_cos();
        // Rodrigues rotation of d0 by `half` about n̂.
        let rot = d0 * cos_h + n_hat.cross(d0) * sin_h + n_hat * n_hat.dot(d0) * (1.0 - cos_h);
        let mid = circle.center + rot.normalize() * circle.radius;
        let mut min_d = f64::INFINITY;
        let n = loop_verts.len();
        for i in 0..n {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % n];
            let ab = b - a;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((mid - a).dot(ab) / len2).clamp(0.0, 1.0)
            };
            min_d = min_d.min((mid - (a + t * ab)).norm());
        }
        split_dbg!("arc guard: mid {mid:?} min_d {min_d:.2e} nv {n}");
        let fz0 = loop_verts.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let fz1 = loop_verts.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        let az0 = arc_points_3d.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let az1 = arc_points_3d.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        if az0 < fz0 - 1.0 || az1 > fz1 + 1.0 {
            split_dbg!(
                "arc RANGE: face z[{fz0:.2},{fz1:.2}] arc z[{az0:.2},{az1:.2}] start {:?} end {:?}",
                inside_start.point,
                inside_end.point
            );
        }
        // Tolerance covers the ≤1 µm chord sag between the true circle and
        // a previously inserted arc polyline (arc_segments' SAG = 1e-3),
        // with slack; a fresh cut hugging the boundary this closely would
        // only mint a sub-sag sliver anyway. Keep it BELOW the smallest
        // distinct-feature separation near tangencies (tangent-cyl stadium
        // cutters carry legitimately distinct arcs a few µm apart).
        if min_d < 2e-3 && std::env::var("VCAD_NO_ARCGUARD").is_err() {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    }

    // Build Face 1: the inside-circle portion
    // Walk polygon from inside_end edge to inside_start edge, then add arc back
    let n = loop_verts.len();
    let same_edge = inside_start.edge_index == inside_end.edge_index;
    let mut face1_points: Vec<Point3> = Vec::new();

    // Start at inside_end intersection
    face1_points.push(inside_end.point);

    // Walk polygon from inside_end edge to inside_start edge. When both
    // intersections lie on the same polygon edge, the inside-circle piece
    // is bounded by only the arc and the chord segment on that edge — no
    // polygon vertices are traversed.
    if !same_edge {
        let mut idx = (inside_end.edge_index + 1) % n;
        while idx != (inside_start.edge_index + 1) % n {
            face1_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    }

    // Add inside_start intersection
    face1_points.push(inside_start.point);

    // Add arc points (from inside_start to inside_end, forward direction)
    // arc_points_3d goes from inside_start to inside_end, so iterate forward
    // This completes the loop: ... → inside_start → arc → (closes to inside_end)
    for pt in arc_points_3d
        .iter()
        .skip(1)
        .take(arc_points_3d.len().saturating_sub(2))
    {
        face1_points.push(*pt);
    }

    // Build Face 2: the outside-circle portion (polygon outside the arc)
    // Walk polygon from inside_start edge to inside_end edge, then add arc back
    let mut face2_points: Vec<Point3> = Vec::new();

    // Start at inside_start intersection
    face2_points.push(inside_start.point);

    // Walk polygon from inside_start edge to inside_end edge. In the same-edge
    // case, the outside-circle piece encloses the *entire* polygon apart from
    // the chord segment, so we walk all n polygon vertices starting after
    // inside_start's edge.
    if same_edge {
        let mut idx = (inside_start.edge_index + 1) % n;
        for _ in 0..n {
            face2_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    } else {
        let mut idx = (inside_start.edge_index + 1) % n;
        while idx != (inside_end.edge_index + 1) % n {
            face2_points.push(loop_verts[idx]);
            idx = (idx + 1) % n;
        }
    }

    // Add inside_end intersection
    face2_points.push(inside_end.point);

    // Add arc points from inside_end back to inside_start (forward arc direction)
    // arc_points_3d goes from inside_start → inside_end, so we take interior and reverse
    // to go from inside_end → inside_start
    for pt in arc_points_3d.iter().skip(1).rev().skip(1) {
        face2_points.push(*pt);
    }

    // When the intersection circle passes through polygon vertices, the polygon walk
    // includes the vertex AND the function then pushes the intersection point, creating
    // a zero-length duplicate edge. Remove consecutive duplicates (including wrap-around)
    // before validating and building faces.
    let tolerance = 1e-6;
    let face1_points = remove_consecutive_duplicates(&face1_points, tolerance);
    let face2_points = remove_consecutive_duplicates(&face2_points, tolerance);

    // Validate faces have at least 3 vertices
    if face1_points.len() < 3 || face2_points.len() < 3 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create the two new faces

    // Face 1 (arc-bounded, inside circle)
    let face1_verts: Vec<_> = face1_points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let face1_hes: Vec<_> = face1_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let face1_loop = brep.topology.add_loop(&face1_hes);
    let face1 = brep
        .topology
        .add_face(face1_loop, surface_index, orientation);

    // Face 2 (chord-bounded, outside circle)
    let face2_verts: Vec<_> = face2_points
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let face2_hes: Vec<_> = face2_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let face2_loop = brep.topology.add_loop(&face2_hes);
    let face2 = brep
        .topology
        .add_face(face2_loop, surface_index, orientation);

    // Add twin edges for the chord (shared edge between face1 and face2)
    // In face1, the chord goes from inside_end to inside_start (first edge after arc)
    // In face2, the chord goes from inside_start to inside_end (last edge)
    // These need to be matched correctly based on which edges share the intersection vertices
    let chord_he1 = face1_hes[0]; // First edge of face1 starts at inside_end
    let chord_he2 = face2_hes[face2_hes.len() - 1]; // Last edge of face2 ends at inside_end
    brep.topology.add_edge(chord_he1, chord_he2);

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Re-home the original face's inner loops (holes from prior booleans)
    // onto whichever sub-face contains them — dropping them here would seal
    // each hole with a phantom membrane that corrupts point-in-solid parity
    // and the volume integral. A degenerate single-vertex loop is a full
    // circle; its seam vertex stands in for the whole hole, which is safe
    // because the arc split never runs between a hole and its own boundary.
    let existing_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
    if !existing_inner.is_empty() {
        let face1_2d: Vec<(f64, f64)> = face1_points
            .iter()
            .map(|p| {
                let d = *p - origin;
                (d.dot(u_axis), d.dot(v_axis))
            })
            .collect();
        for lp in existing_inner {
            let lp_verts: Vec<Point3> = brep
                .topology
                .loop_half_edges(lp)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if lp_verts.is_empty() {
                continue;
            }
            let test_pt = if lp_verts.len() == 1 {
                lp_verts[0]
            } else {
                let n = lp_verts.len() as f64;
                Point3::new(
                    lp_verts.iter().map(|v| v.x).sum::<f64>() / n,
                    lp_verts.iter().map(|v| v.y).sum::<f64>() / n,
                    lp_verts.iter().map(|v| v.z).sum::<f64>() / n,
                )
            };
            let d = test_pt - origin;
            let target = if point_in_polygon_2d(d.dot(u_axis), d.dot(v_axis), &face1_2d) {
                face1
            } else {
                face2
            };
            brep.topology.faces[target].inner_loops.push(lp);
        }
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the arc
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Split a planar face along a circle that crosses its boundary at more
/// than two points.
///
/// A circle can cross a polygon boundary 4+ times — the canonical case is a
/// fillet circle clipping a face corner, crossing each of two adjacent edges
/// twice. The two-crossing splitter can't express that, and leaving the face
/// unsplit (or chorded) while the curved partner face follows the true arc
/// zippers the sewn shell open along the seam.
///
/// This traces the regions of a Weiler–Atherton-style clip generically:
/// crossings are visited in polygon-boundary order for the straight
/// sections and in circle-angle order for the arcs, alternating
/// entering/exiting. Each polygon∩disk region and each polygon∖disk region
/// becomes its own sub-face; every arc section samples the shared circle via
/// `canonical_arc_points`, so the neighboring curved face emits identical
/// vertices and the seam conforms.
///
/// Falls back to returning the face unchanged (before any mutation) when the
/// crossing pattern doesn't alternate cleanly (tangential grazing) or any
/// traced region degenerates.
#[allow(clippy::too_many_arguments)]
fn split_planar_face_by_multi_arc(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    loop_verts: &[Point3],
    poly_2d: &[(f64, f64)],
    center_2d: (f64, f64),
    origin: Point3,
    u_axis: vcad_kernel_math::Vec3,
    v_axis: vcad_kernel_math::Vec3,
    intersections: &[CirclePolygonIntersection],
) -> SplitResult {
    let unchanged = SplitResult {
        sub_faces: vec![face_id],
    };
    let n = loop_verts.len();
    let m = intersections.len();
    // Three contacts is the floor for either path: a triangular cell with an
    // inscribed circle touches exactly three times. The crossing tracer needs
    // more than that (and an even count) — but it is gated below, after the
    // contacts are classified, so an odd all-tangency count still reaches the
    // inscribed path instead of being rejected here for the tracer's reasons.
    if m < 3 {
        return unchanged;
    }

    // Boundary walk with crossings inserted: (2D point, vertex index or
    // crossing index) in loop order. Crossings on one edge sort by t.
    enum Node {
        Vert(usize),
        Cross(usize),
    }
    let mut seq: Vec<((f64, f64), Node)> = Vec::with_capacity(n + m);
    for (vi, &p2) in poly_2d.iter().enumerate() {
        seq.push((p2, Node::Vert(vi)));
        let mut on_edge: Vec<usize> = (0..m)
            .filter(|&ci| intersections[ci].edge_index == vi)
            .collect();
        on_edge.sort_by(|&a, &b| {
            intersections[a]
                .t_along_edge
                .partial_cmp(&intersections[b].t_along_edge)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for ci in on_edge {
            seq.push((intersections[ci].point_2d, Node::Cross(ci)));
        }
    }
    let seq_len = seq.len();
    let mut seq_pos = vec![usize::MAX; m];
    for (k, (_, node)) in seq.iter().enumerate() {
        if let Node::Cross(ci) = node {
            seq_pos[*ci] = k;
        }
    }

    // Classify each crossing: entering the disk iff the boundary immediately
    // after it (midpoint to the next walk node) lies inside the circle.
    let (cx, cy) = center_2d;
    let inside_disk = |p: (f64, f64)| -> bool {
        ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt() < circle.radius
    };
    // Probe the boundary just before and just after each contact point.
    // A genuine crossing flips inside/outside; a tangential touch keeps the
    // boundary outside (or inside) the disk on both sides.
    let mut entering = vec![false; m];
    let mut before_in = vec![false; m];
    for ci in 0..m {
        let k = seq_pos[ci];
        let next = seq[(k + 1) % seq_len].0;
        let prev = seq[(k + seq_len - 1) % seq_len].0;
        let here = seq[k].0;
        entering[ci] = inside_disk(((here.0 + next.0) / 2.0, (here.1 + next.1) / 2.0));
        before_in[ci] = inside_disk(((here.0 + prev.0) / 2.0, (here.1 + prev.1) / 2.0));
    }
    let all_crossings = (0..m).all(|ci| entering[ci] != before_in[ci]);
    let all_touch_outside = (0..m).all(|ci| !entering[ci] && !before_in[ci]);
    if !all_crossings && all_touch_outside {
        // Every contact is a tangency with the polygon staying outside the
        // disk: the circle is inscribed in the face (e.g. a corner-sphere
        // circle inside the cell that neighboring cylinder-line splits
        // carved out). Split into the disk plus one sliver per tangent gap.
        return split_planar_face_inscribed_circle(
            brep,
            face_id,
            circle,
            segments,
            loop_verts,
            poly_2d,
            center_2d,
            origin,
            u_axis,
            v_axis,
            intersections,
        );
    }
    if !all_crossings {
        // Mixed crossings and tangencies — no clean alternation to trace.
        return unchanged;
    }
    // Past here every contact is a genuine crossing, so the tracer's own
    // preconditions apply: it walks enter/exit pairs, which needs at least
    // two of each. An odd count means a graze slipped through the probes.
    if m < 4 || !m.is_multiple_of(2) {
        return unchanged;
    }
    // Alternation must hold both around the circle (intersections are
    // angle-sorted) and along the boundary walk; anything else is a graze.
    for ci in 0..m {
        if entering[ci] == entering[(ci + 1) % m] {
            return unchanged;
        }
    }
    let boundary_order: Vec<usize> = seq
        .iter()
        .filter_map(|(_, node)| match node {
            Node::Cross(ci) => Some(*ci),
            Node::Vert(_) => None,
        })
        .collect();
    let mut next_on_boundary = vec![usize::MAX; m];
    for (k, &ci) in boundary_order.iter().enumerate() {
        next_on_boundary[ci] = boundary_order[(k + 1) % m];
    }
    for k in 0..m {
        if entering[boundary_order[k]] == entering[boundary_order[(k + 1) % m]] {
            return unchanged;
        }
    }

    // Polygon winding in the (u, v) frame decides arc travel direction:
    // inside-disk region boundaries traverse the circle the same way the
    // polygon winds; outside regions traverse it the opposite way.
    let poly_area: f64 = {
        let mut a = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            a += poly_2d[i].0 * poly_2d[j].1 - poly_2d[j].0 * poly_2d[i].1;
        }
        a / 2.0
    };
    let poly_ccw = poly_area > 0.0;
    let plane_normal = u_axis.cross(v_axis);

    // Arc points from one crossing to another, travelling CCW (in the 2D
    // face frame) when `ccw`; sampled on the circle's own raw-`segments`
    // grid so the curved partner face emits identical vertices.
    let circ_normal = circle.x_dir.into_inner().cross(circle.y_dir.into_inner());
    let aligned = plane_normal.dot(circ_normal) > 0.0;
    let arc_between = |from: usize, to: usize, ccw: bool| -> Vec<Point3> {
        circle_frame_arc_points(
            circle,
            intersections[from].point,
            intersections[to].point,
            ccw == aligned,
            segments,
        )
    };
    // Neighbor crossing around the circle in the given travel direction
    // (intersections are sorted by angle, so angle rank == index).
    let next_on_circle = |ci: usize, ccw: bool| -> usize {
        if ccw {
            (ci + 1) % m
        } else {
            (ci + m - 1) % m
        }
    };

    // Trace one region loop. `start` is a crossing where the polygon walk
    // enters the region (entering crossing for disk-side regions, exiting
    // for the outside). Returns None on an inconsistent pattern.
    let trace = |start: usize, arcs_ccw: bool, visited: &mut [bool]| -> Option<Vec<Point3>> {
        let mut points: Vec<Point3> = Vec::new();
        let mut cur = start;
        loop {
            if visited[cur] {
                return None;
            }
            visited[cur] = true;
            // Straight section: walk the polygon boundary to the next crossing.
            points.push(intersections[cur].point);
            let mut k = (seq_pos[cur] + 1) % seq_len;
            loop {
                match seq[k].1 {
                    Node::Vert(vi) => points.push(loop_verts[vi]),
                    Node::Cross(ci) => {
                        debug_assert_eq!(ci, next_on_boundary[cur]);
                        // Arc section: follow the circle to the adjacent
                        // crossing in the region's travel direction, where
                        // the polygon walk re-enters the region.
                        let nxt = next_on_circle(ci, arcs_ccw);
                        let arc = arc_between(ci, nxt, arcs_ccw);
                        points.extend(&arc[..arc.len() - 1]);
                        cur = nxt;
                        break;
                    }
                }
                k = (k + 1) % seq_len;
            }
            if cur == start {
                return Some(points);
            }
        }
    };

    // Trace every region before mutating anything, so an inconsistent
    // pattern falls back to the unsplit face instead of a half-split shell.
    let tolerance = 1e-6;
    let min_area = 0.001;
    let mut regions: Vec<Vec<Point3>> = Vec::new();
    for pass in [true, false] {
        // pass=true: disk-side regions (start at entering crossings);
        // pass=false: outside regions (start at exiting crossings).
        let arcs_ccw = if pass { poly_ccw } else { !poly_ccw };
        let mut visited = vec![false; m];
        for ci in 0..m {
            if entering[ci] != pass || visited[ci] {
                continue;
            }
            let Some(points) = trace(ci, arcs_ccw, &mut visited) else {
                return unchanged;
            };
            let points = remove_consecutive_duplicates(&points, tolerance);
            if points.len() < 3 {
                return unchanged;
            }
            let area: f64 = {
                let project = |p: &Point3| {
                    let d = *p - origin;
                    (d.dot(u_axis), d.dot(v_axis))
                };
                let pts_2d: Vec<_> = points.iter().map(project).collect();
                let mut a = 0.0;
                for i in 0..pts_2d.len() {
                    let j = (i + 1) % pts_2d.len();
                    a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
                }
                (a / 2.0).abs()
            };
            if area < min_area {
                return unchanged;
            }
            regions.push(points);
        }
    }
    if regions.len() < 2 {
        return unchanged;
    }

    finish_planar_regions(brep, face_id, circle, regions, origin, u_axis, v_axis)
}

/// Replace `face_id` with one sub-face per traced region.
///
/// Shared tail of the multi-crossing and inscribed-circle planar splitters:
/// builds a face per region loop, moves them onto the parent's shell,
/// re-homes the parent's inner loops (holes from prior booleans) onto
/// whichever region contains them, removes the parent, and records the
/// circle curve.
fn finish_planar_regions(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    regions: Vec<Vec<Point3>>,
    origin: Point3,
    u_axis: vcad_kernel_math::Vec3,
    v_axis: vcad_kernel_math::Vec3,
) -> SplitResult {
    let tolerance = 1e-6;
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;

    let mut sub_faces = Vec::with_capacity(regions.len());
    for points in &regions {
        let verts: Vec<_> = points
            .iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect();
        let hes: Vec<_> = verts
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let loop_id = brep.topology.add_loop(&hes);
        sub_faces.push(brep.topology.add_face(loop_id, surface_index, orientation));
    }

    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        for &f in &sub_faces {
            brep.topology.shells[shell_id].faces.push(f);
            brep.topology.faces[f].shell = Some(shell_id);
        }
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    let existing_inner: Vec<_> = brep.topology.faces[face_id].inner_loops.clone();
    for lp in existing_inner {
        let lp_verts: Vec<Point3> = brep
            .topology
            .loop_half_edges(lp)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();
        if lp_verts.is_empty() {
            continue;
        }
        let test_pt = if lp_verts.len() == 1 {
            lp_verts[0]
        } else {
            let nv = lp_verts.len() as f64;
            Point3::new(
                lp_verts.iter().map(|v| v.x).sum::<f64>() / nv,
                lp_verts.iter().map(|v| v.y).sum::<f64>() / nv,
                lp_verts.iter().map(|v| v.z).sum::<f64>() / nv,
            )
        };
        let d = test_pt - origin;
        let test_2d = (d.dot(u_axis), d.dot(v_axis));
        for (ri, points) in regions.iter().enumerate() {
            let region_2d: Vec<(f64, f64)> = points
                .iter()
                .map(|p| {
                    let d = *p - origin;
                    (d.dot(u_axis), d.dot(v_axis))
                })
                .collect();
            if point_in_polygon_2d(test_2d.0, test_2d.1, &region_2d) {
                brep.topology.faces[sub_faces[ri]].inner_loops.push(lp);
                break;
            }
        }
    }

    brep.topology.faces.remove(face_id);
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult { sub_faces }
}

/// Split a planar face along a circle inscribed in it: the circle touches
/// the boundary at 3+ tangent points but never crosses it.
///
/// Canonical case: a fillet's corner-sphere circle inside the square cell
/// that the neighboring edge-cylinders' line splits carved out of a planar
/// face — the circle is tangent to all four cell edges. The face splits into
/// the disk (bounded by the full circle through the tangent points) plus one
/// sliver per tangent gap (polygon walk between consecutive tangent points,
/// closed by the arc back). Arcs sample the shared circle via
/// `canonical_arc_points`, so the curved partner face conforms.
///
/// Falls back to the unsplit face (before any mutation) when the circle
/// center is outside the polygon, the tangent points' boundary order
/// disagrees with their circle order, or any region degenerates.
#[allow(clippy::too_many_arguments)]
fn split_planar_face_inscribed_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    loop_verts: &[Point3],
    poly_2d: &[(f64, f64)],
    center_2d: (f64, f64),
    origin: Point3,
    u_axis: vcad_kernel_math::Vec3,
    v_axis: vcad_kernel_math::Vec3,
    intersections: &[CirclePolygonIntersection],
) -> SplitResult {
    let unchanged = SplitResult {
        sub_faces: vec![face_id],
    };
    let n = loop_verts.len();
    let m = intersections.len();
    if m < 3 || !point_in_polygon_2d(center_2d.0, center_2d.1, poly_2d) {
        return unchanged;
    }

    // Tangent points in boundary-walk order.
    let mut border: Vec<usize> = (0..m).collect();
    border.sort_by(|&a, &b| {
        let ka = intersections[a].edge_index as f64 + intersections[a].t_along_edge;
        let kb = intersections[b].edge_index as f64 + intersections[b].t_along_edge;
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let poly_area: f64 = {
        let mut a = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            a += poly_2d[i].0 * poly_2d[j].1 - poly_2d[j].0 * poly_2d[i].1;
        }
        a / 2.0
    };
    let poly_ccw = poly_area > 0.0;
    let plane_normal = u_axis.cross(v_axis);

    // Boundary order must agree with circle-angle order (walking the
    // boundary in the polygon's winding direction visits the tangent points
    // in the same rotational order around the circle) — anything else means
    // the contact pattern isn't a simple inscription.
    // `intersections` is angle-sorted, so index == CCW angle rank.
    for k in 0..m {
        let a = border[k];
        let b = border[(k + 1) % m];
        let expect = if poly_ccw {
            (a + 1) % m
        } else {
            (a + m - 1) % m
        };
        if b != expect {
            return unchanged;
        }
    }

    let circ_normal = circle.x_dir.into_inner().cross(circle.y_dir.into_inner());
    let aligned = plane_normal.dot(circ_normal) > 0.0;
    let arc_between = |from: usize, to: usize, ccw: bool| -> Vec<Point3> {
        circle_frame_arc_points(
            circle,
            intersections[from].point,
            intersections[to].point,
            ccw == aligned,
            segments,
        )
    };

    let tolerance = 1e-6;
    let min_area = 0.001;
    let region_area = |points: &[Point3]| -> f64 {
        let pts_2d: Vec<_> = points
            .iter()
            .map(|p| {
                let d = *p - origin;
                (d.dot(u_axis), d.dot(v_axis))
            })
            .collect();
        let mut a = 0.0;
        for i in 0..pts_2d.len() {
            let j = (i + 1) % pts_2d.len();
            a += pts_2d[i].0 * pts_2d[j].1 - pts_2d[j].0 * pts_2d[i].1;
        }
        (a / 2.0).abs()
    };

    // Disk region: the full circle through the tangent points, traversed in
    // the polygon's winding direction so the loop matches the parent's
    // orientation convention.
    let mut regions: Vec<Vec<Point3>> = Vec::with_capacity(m + 1);
    let mut disk: Vec<Point3> = Vec::new();
    for k in 0..m {
        let (from, to) = if poly_ccw {
            (k, (k + 1) % m)
        } else {
            (m - 1 - k, (m - k) % m)
        };
        let arc = arc_between(from, to, poly_ccw);
        disk.extend(&arc[..arc.len() - 1]);
    }
    let disk = remove_consecutive_duplicates(&disk, tolerance);
    if disk.len() < 3 || region_area(&disk) < min_area {
        return unchanged;
    }
    regions.push(disk);

    // Sliver regions: for consecutive tangent points A→B along the boundary,
    // walk the polygon from A to B, then close along the arc B→A traversed
    // against the polygon winding (the short way past the corner).
    for k in 0..m {
        let a = border[k];
        let b = border[(k + 1) % m];
        let mut points: Vec<Point3> = vec![intersections[a].point];
        // Polygon vertices strictly between A and B along the walk.
        let mut idx = (intersections[a].edge_index + 1) % n;
        let stop = (intersections[b].edge_index + 1) % n;
        // A and B on the same edge with B ahead of A means no vertex between.
        let same_edge_forward = intersections[a].edge_index == intersections[b].edge_index
            && intersections[b].t_along_edge >= intersections[a].t_along_edge;
        if !same_edge_forward {
            let mut steps = 0;
            while idx != stop {
                points.push(loop_verts[idx]);
                idx = (idx + 1) % n;
                steps += 1;
                if steps > n {
                    return unchanged;
                }
            }
        }
        let arc = arc_between(b, a, !poly_ccw);
        points.extend(&arc[..arc.len() - 1]);
        let points = remove_consecutive_duplicates(&points, tolerance);
        if points.len() < 3 || region_area(&points) < min_area {
            return unchanged;
        }
        regions.push(points);
    }

    finish_planar_regions(brep, face_id, circle, regions, origin, u_axis, v_axis)
}

/// Handle the tangent-inside case where a circle lies fully inside a polygon
/// but is tangent to two polygon edges.
///
/// Strategy: split the polygon along the chord between the two tangent points
/// using `split_face_by_curve` (line-based), producing two simple sub-polygons.
/// Each sub-polygon contains exactly one of the two circle arcs, so calling
/// `split_planar_face_by_arc` on each resolves the disk's relevant half-circle
/// unambiguously.
///
/// This is essential when a Difference cutter is itself a Union of primitives
/// whose lateral surfaces share a tangent boundary (e.g. a rect with a tangent
/// cylinder), where the cylinder's full SSI circle on the target's planar face
/// would otherwise straddle two regions of different classification.
fn split_planar_face_tangent_inside(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    int1: &CirclePolygonIntersection,
    int2: &CirclePolygonIntersection,
) -> SplitResult {
    // Build a Line3d through the two tangent points to act as the chord.
    let chord_dir = int2.point - int1.point;
    let chord_len = chord_dir.norm();
    if chord_len < 1e-9 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }
    let chord_line = vcad_kernel_geom::Line3d {
        origin: int1.point,
        direction: chord_dir / chord_len,
    };
    let chord_curve = IntersectionCurve::Line(chord_line);

    // Split the polygon along the chord; sub-faces split on the original face's
    // outer loop at int1 and int2.
    let chord_result = split_face_by_curve(brep, face_id, &chord_curve, &int1.point, &int2.point);
    if chord_result.sub_faces.len() < 2 {
        return chord_result;
    }

    // Now split each sub-face by the arc. Each sub-polygon should contain
    // exactly one of the two arc midpoints, so the inside/outside check
    // resolves correctly.
    let mut all_sub_faces = Vec::new();
    for sub_id in chord_result.sub_faces {
        if !brep.topology.faces.contains_key(sub_id) {
            continue;
        }
        let sub_result = split_planar_face_by_arc(brep, sub_id, circle, segments);
        // If the recursive split made progress (>= 2 sub-faces), use those;
        // otherwise the sub-face was left intact (e.g. its arc midpoint missed
        // the polygon by a tolerance margin) — keep the unsplit sub-face.
        if sub_result.sub_faces.len() >= 2 {
            all_sub_faces.extend(sub_result.sub_faces);
        } else {
            all_sub_faces.push(sub_id);
        }
    }

    SplitResult {
        sub_faces: all_sub_faces,
    }
}

/// Check if a circle partially intersects a polygon (crosses exactly 2 edges).
///
/// Returns true if the circle crosses the polygon boundary at exactly 2 points,
/// meaning it's only partially inside and needs arc-based splitting.
fn circle_partially_inside_polygon(
    polygon: &[Point3],
    circle: &vcad_kernel_geom::Circle3d,
) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    // Use the circle's known normal for consistent projection
    let v0 = polygon[0];
    let normal = circle.normal.into_inner();

    let e1 = polygon[1] - v0;
    let u_axis = e1.normalize();
    let v_axis = normal.cross(e1).normalize();

    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - v0;
        (d.dot(u_axis), d.dot(v_axis))
    };

    let poly_2d: Vec<(f64, f64)> = polygon.iter().map(&project).collect();
    let center_2d = project(&circle.center);

    let intersections = find_circle_polygon_intersections(
        polygon,
        &poly_2d,
        center_2d,
        circle.radius,
        v0,
        u_axis,
        v_axis,
    );

    // Partial intersection means the circle crosses the boundary: 2 crossings
    // is the simple secant case; more (e.g. 4 when a fillet circle clips a
    // corner across two adjacent edges) routes to the multi-arc splitter.
    intersections.len() >= 2
}

/// Split a planar face along an intersection curve.
///
/// This dispatches to the appropriate split method based on the curve type:
/// - Circle: creates inner disk + outer face with hole
/// - Line: entry/exit split (existing implementation)
pub fn split_planar_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    entry: &Point3,
    exit: &Point3,
    segments: u32,
    canonical: bool,
) -> SplitResult {
    match curve {
        IntersectionCurve::Circle(circle) => {
            split_planar_face_by_circle(brep, face_id, circle, segments, canonical)
        }
        IntersectionCurve::Line(line) => {
            // Get face boundary vertices
            let face = &brep.topology.faces[face_id];
            let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
            let loop_verts: Vec<Point3> = loop_hes
                .iter()
                .map(|&he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();

            // Find where the line intersects the polygon edges
            let crossings = find_line_polygon_crossings(&loop_verts, line);

            if crossings.len() >= 2 {
                // Use the first two crossings as entry/exit
                let actual_entry = crossings[0];
                let actual_exit = crossings[1];
                // A grazing line (both crossings at one vertex) is a
                // zero-length cut: "splitting" would emit a copy of the
                // face plus a degenerate sliver.
                if (actual_exit - actual_entry).norm() < 1e-6 {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
                split_face_by_curve(brep, face_id, curve, &actual_entry, &actual_exit)
            } else {
                // Line doesn't cross the polygon boundary at two points
                SplitResult {
                    sub_faces: vec![face_id],
                }
            }
        }
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            split_planar_face(
                brep,
                face_id,
                &IntersectionCurve::Line(line1.clone()),
                entry,
                exit,
                segments,
                canonical,
            )
        }
        _ => {
            // Use existing line-based split
            split_face_by_curve(brep, face_id, curve, entry, exit)
        }
    }
}

// =============================================================================
// Cylindrical Face Splitting
// =============================================================================

/// Split a spherical face by a circle intersection curve.
///
/// When a plane or another sphere intersects a sphere, the result is a circle
/// on the sphere's surface. This function:
/// 1. Adds the circle as an inner loop (hole) on the sphere face
/// 2. Creates a planar disk face bounded by the circle (for classification)
///
/// The sphere face retains its degenerate pole loop as the outer boundary.
/// The circle becomes an inner loop, similar to how cylinder cap splits work.
pub fn split_spherical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify this is a sphere surface
    let sph = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::SphereSurface>()
    {
        Some(s) => s.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Verify the circle lies on the sphere: distance from center to circle center
    // plus Pythagorean check should satisfy r_circle^2 + d^2 ≈ R^2
    let d = (circle.center - sph.center).norm();
    let expected_r = (sph.radius * sph.radius - d * d).max(0.0).sqrt();
    if (expected_r - circle.radius).abs() > 1e-4 {
        // Circle doesn't lie on this sphere — skip
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let tolerance = 1e-6;

    // A face whose loop is a real spherical polygon (a fillet corner
    // patch, or a sub-face left by an earlier circle split) must be
    // CLIPPED by the circle — the whole-sphere path below throws the
    // existing boundary away and hands back two faces bounded by this
    // circle alone. Splitting such a face three times (the three planes
    // of a box cutting a corner blend) then yielded four identical,
    // fully-overlapping patches: volume counted several times over and a
    // shell full of holes. An untrimmed sphere keeps the 4-vertex seam
    // cycle, which is not a boundary, so it still takes the path below.
    if brep
        .topology
        .loop_len(brep.topology.faces[face_id].outer_loop)
        != 4
    {
        if let Some(result) =
            clip_spherical_face_by_circle(brep, face_id, circle, segments, tolerance)
        {
            return result;
        }
        // The clip declined (circle misses the patch, crosses its
        // boundary somewhere other than exactly twice, or can't seat its
        // connector). Fall through to the whole-sphere split — that is
        // what this face got before the clip existed, so declining never
        // costs behavior that used to work.
    }

    // Generate the N shared circle vertices.
    let n = segments as usize;
    // NOTE: sphere ring pairing relies on the circle's own frame — do NOT
    // switch this to the canonical grid (see fillet-defects notes).
    let circle_verts: Vec<Point3> = (0..segments)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (sin_t, cos_t) = theta.sin_cos();
            circle.center
                + circle.radius
                    * (cos_t * circle.x_dir.into_inner() + sin_t * circle.y_dir.into_inner())
        })
        .collect();
    let circle_vert_ids: Vec<_> = circle_verts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    // Cap-A walks the circle in forward order: v[0], v[1], ..., v[N-1].
    // Half-edge i has origin v[i] and points toward v[(i+1) mod N].
    let he_a: Vec<_> = (0..n)
        .map(|i| brep.topology.add_half_edge(circle_vert_ids[i]))
        .collect();
    let loop_a = brep.topology.add_loop(&he_a);

    // Cap-B walks the circle in reverse: v[0], v[N-1], v[N-2], ..., v[1].
    // Half-edge j has origin v[(N - j) mod N] (so j=0 → v[0], j=1 → v[N-1], ...).
    // Cap-B's edge j spans {v[(N-j) mod N], v[(N-1-j) mod N]} — same physical
    // edge as cap-A's edge i = N-1-j, traversed in reverse.
    let he_b: Vec<_> = (0..n)
        .map(|j| brep.topology.add_half_edge(circle_vert_ids[(n - j) % n]))
        .collect();
    let loop_b = brep.topology.add_loop(&he_b);

    // Repurpose the original face as cap-A: clear holes, replace outer loop.
    // Same surface, orientation, shell. The original 4-edge seam loop is
    // now orphaned (no face references it) — benign because all kernel
    // traversal goes from faces.
    {
        let f = &mut brep.topology.faces[face_id];
        f.outer_loop = loop_a;
        f.inner_loops.clear();
    }

    // Cap-B is a new face with the same surface and orientation.
    let face_b = brep.topology.add_face(loop_b, surface_index, orientation);

    // Twin-pair the two caps' half-edges along the shared circle.
    for i in 0..n {
        brep.topology.add_edge(he_a[i], he_b[n - 1 - i]);
    }

    // Add cap-B to the same shell as the original face.
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face_b);
        brep.topology.faces[face_b].shell = Some(shell_id);
    }

    // Add the SSI circle as a 3D curve in the geometry store.
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![face_id, face_b],
    }
}

/// Clip an already-trimmed spherical face by a circle lying on its sphere.
///
/// The face's loop is walked as a spherical polygon; where it crosses the
/// circle's plane the crossing point is solved on the great arc between
/// the straddling vertices, and the two sides are closed off with a
/// shared, identically-sampled arc of the circle so they weld.
///
/// Returns `None` — leaving the caller to fall back to the whole-sphere
/// split — when the circle misses the patch, when it crosses the
/// boundary anywhere other than exactly twice (a multiply-crossing clip
/// needs region tracking this doesn't do), or when the connecting arc
/// can't be seated inside the patch.
fn clip_spherical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    tolerance: f64,
) -> Option<SplitResult> {
    use vcad_kernel_math::Vec3;

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let sph = brep.geometry.surfaces[surface_index]
        .as_any()
        .downcast_ref::<vcad_kernel_geom::SphereSurface>()?
        .clone();
    if !face.inner_loops.is_empty() {
        return None;
    }
    let center = sph.center;
    let radius = sph.radius.abs();
    if radius < 1e-9 {
        return None;
    }

    let x_dir = circle.x_dir.into_inner();
    let y_dir = circle.y_dir.into_inner();
    let normal = x_dir.cross(y_dir).normalize();
    let plane_d = (circle.center - center).dot(normal);
    let side = |p: &Point3| (*p - center).dot(normal) - plane_d;

    let verts: Vec<Point3> = brep
        .topology
        .loop_vertices(brep.topology.faces[face_id].outer_loop)
        .iter()
        .map(|v| brep.topology.vertices[*v].point)
        .collect();
    let m = verts.len();
    if m < 3 {
        return None;
    }
    let eps = (radius * 1e-9).max(1e-9);
    let sides: Vec<f64> = verts.iter().map(side).collect();
    if sides.iter().all(|s| *s > -eps) || sides.iter().all(|s| *s < eps) {
        // The circle doesn't cross this patch's boundary. Either it lies
        // somewhere else on the sphere entirely — in which case there is
        // nothing to split and falling through to the whole-sphere path
        // would REPLACE this patch with two copies bounded by a circle
        // it never touches — or it sits wholly inside the patch, which
        // needs an inner-loop split this routine doesn't build yet.
        // Probe the circle: if no point of it is in the face, report a
        // clean no-op instead of declining.
        let mut any_inside = false;
        for k in 0..16 {
            let ang = 2.0 * std::f64::consts::PI * k as f64 / 16.0;
            let p = circle.center + circle.radius * (ang.cos() * x_dir + ang.sin() * y_dir);
            if crate::trim::point_in_face(brep, face_id, &p) {
                any_inside = true;
                break;
            }
        }
        if !any_inside {
            return Some(SplitResult {
                sub_faces: vec![face_id],
            });
        }
        return None;
    }

    // Crossing point on the great arc between two straddling vertices:
    // bisect the arc on the plane's signed distance.
    let arc_crossing = |a: &Point3, b: &Point3| -> Point3 {
        let (ua, ub) = ((*a - center) / radius, (*b - center) / radius);
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        let point_at = |t: f64| -> Point3 {
            let dir = ua + t * (ub - ua);
            let n = dir.norm();
            if n < 1e-12 {
                center + radius * ua
            } else {
                center + radius * (dir / n)
            }
        };
        let s_lo = side(&point_at(0.0));
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if side(&point_at(mid)) * s_lo > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        point_at(0.5 * (lo + hi))
    };

    let mut crossings: Vec<(usize, Point3)> = Vec::new();
    for i in 0..m {
        let j = (i + 1) % m;
        let (si, sj) = (sides[i], sides[j]);
        if (si > eps && sj < -eps) || (si < -eps && sj > eps) {
            crossings.push((i, arc_crossing(&verts[i], &verts[j])));
        }
    }
    if crossings.len() > 2 {
        // A circle can cross a spherical polygon's boundary 4+ times (a
        // cutter-plane circle traversing a fillet corner patch already
        // clipped by the other cutter planes). Trace the regions generically
        // instead of declining — the caller's whole-sphere fallback would
        // REPLACE the patch with two full caps and resurface trimmed
        // geometry.
        return clip_spherical_face_multi(
            brep, face_id, circle, segments, tolerance, &verts, &crossings,
        );
    }
    if crossings.len() != 2 {
        return None;
    }
    let (e1, x_a) = crossings[0];
    let (e2, x_b) = crossings[1];

    // The two boundary chains between the crossings.
    let walk = |from: usize, to: usize| -> Vec<Point3> {
        let mut out = Vec::new();
        let mut i = (from + 1) % m;
        loop {
            if i == (to + 1) % m {
                break;
            }
            out.push(verts[i]);
            i = (i + 1) % m;
        }
        out
    };
    let chain1 = walk(e1, e2);
    let chain2 = walk(e2, e1);

    // Connector: the arc of the circle between the two crossings that
    // runs through the patch. Both candidate arcs are tested by their
    // midpoint; the one inside the original face wins.
    let angle_of = |p: &Point3| -> f64 {
        let d = *p - circle.center;
        d.dot(y_dir).atan2(d.dot(x_dir))
    };
    let (a_ang, b_ang) = (angle_of(&x_a), angle_of(&x_b));
    let on_circle = |ang: f64| -> Point3 {
        circle.center + circle.radius * (ang.cos() * x_dir + ang.sin() * y_dir)
    };
    let mut sweep = b_ang - a_ang;
    while sweep <= -std::f64::consts::PI {
        sweep += 2.0 * std::f64::consts::PI;
    }
    while sweep > std::f64::consts::PI {
        sweep -= 2.0 * std::f64::consts::PI;
    }
    let alt = if sweep > 0.0 {
        sweep - 2.0 * std::f64::consts::PI
    } else {
        sweep + 2.0 * std::f64::consts::PI
    };
    let inside = |s: f64| crate::trim::point_in_face(brep, face_id, &on_circle(a_ang + 0.5 * s));
    let sweep = if inside(sweep) {
        sweep
    } else if inside(alt) {
        alt
    } else {
        // `point_in_face` can reject BOTH candidate midpoints (it is
        // unreliable on a pristine analytic patch whose loop is only a
        // few vertices). Declining here sends the face to the
        // whole-sphere fallback, which throws its trim away — far worse
        // than a heuristic. Pick the arc whose midpoint lies nearer the
        // patch's own interior, proxied by the loop centroid projected
        // onto the sphere.
        let mut c = Vec3::zeros();
        for v in &verts {
            c += *v - center;
        }
        let c_norm = c.norm();
        if c_norm < 1e-9 {
            return None;
        }
        let interior = center + radius * (c / c_norm);
        let d_main = (on_circle(a_ang + 0.5 * sweep) - interior).norm();
        let d_alt = (on_circle(a_ang + 0.5 * alt) - interior).norm();
        if d_main <= d_alt {
            sweep
        } else {
            alt
        }
    };

    // Sample the connector on the circle's CANONICAL grid — the same
    // θ = 2πk/segments schedule every other splitter uses for this
    // circle — rather than by evenly dividing this particular sweep.
    // Any face bounded by the same circle then lands on identical
    // points instead of a phase-shifted set that zippers open.
    let two_pi = 2.0 * std::f64::consts::PI;
    let step = two_pi / segments.max(3) as f64;
    let ang_eps = step * 1e-6;
    let mut conn_angles: Vec<f64> = Vec::new();
    if sweep > 0.0 {
        let mut k = (a_ang / step).floor() + 1.0;
        while k * step < a_ang + sweep - ang_eps {
            if k * step > a_ang + ang_eps {
                conn_angles.push(k * step);
            }
            k += 1.0;
        }
    } else {
        let mut k = (a_ang / step).ceil() - 1.0;
        while k * step > a_ang + sweep + ang_eps {
            if k * step < a_ang - ang_eps {
                conn_angles.push(k * step);
            }
            k -= 1.0;
        }
    }
    let connector: Vec<Point3> = conn_angles.iter().map(|a| on_circle(*a)).collect();

    // Materialize vertices. The connector is shared by both sub-faces
    // (same ids, opposite traversal) so the seam pairs into edges.
    let va = find_or_create_vertex(brep, &x_a, tolerance);
    let vb = find_or_create_vertex(brep, &x_b, tolerance);
    let conn_ids: Vec<_> = connector
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();
    let chain_ids = |brep: &mut BRepSolid, pts: &[Point3]| -> Vec<vcad_kernel_topo::VertexId> {
        pts.iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect()
    };
    let chain1_ids = chain_ids(brep, &chain1);
    let chain2_ids = chain_ids(brep, &chain2);

    // Loop A: x_a → chain1 → x_b → connector reversed → back to x_a.
    let mut loop_a_verts = vec![va];
    loop_a_verts.extend(chain1_ids);
    loop_a_verts.push(vb);
    loop_a_verts.extend(conn_ids.iter().rev().copied());
    // Loop B: x_b → chain2 → x_a → connector forward → back to x_b.
    let mut loop_b_verts = vec![vb];
    loop_b_verts.extend(chain2_ids);
    loop_b_verts.push(va);
    loop_b_verts.extend(conn_ids.iter().copied());

    loop_a_verts.dedup();
    loop_b_verts.dedup();
    if loop_a_verts.len() < 3 || loop_b_verts.len() < 3 {
        return None;
    }

    let make_loop = |brep: &mut BRepSolid, vs: &[vcad_kernel_topo::VertexId]| {
        let hes: Vec<HalfEdgeId> = vs.iter().map(|v| brep.topology.add_half_edge(*v)).collect();
        let loop_id = brep.topology.add_loop(&hes);
        (loop_id, hes)
    };
    let (loop_a, he_a) = make_loop(brep, &loop_a_verts);
    let (loop_b, he_b) = make_loop(brep, &loop_b_verts);

    {
        let f = &mut brep.topology.faces[face_id];
        f.outer_loop = loop_a;
        f.inner_loops.clear();
    }
    let face_b = brep.topology.add_face(loop_b, surface_index, orientation);
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face_b);
        brep.topology.faces[face_b].shell = Some(shell_id);
    }

    // Pair the shared seam: A walks x_b → connector → x_a, B walks the
    // same vertices in reverse.
    let a_seam_start = loop_a_verts.len() - conn_ids.len() - 1;
    let b_seam_start = loop_b_verts.len() - conn_ids.len() - 1;
    let seam_len = conn_ids.len() + 1;
    for k in 0..seam_len {
        let ha = he_a[(a_seam_start + k) % he_a.len()];
        let hb = he_b[(b_seam_start + seam_len - 1 - k) % he_b.len()];
        brep.topology.add_edge(ha, hb);
    }

    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    let _ = Vec3::zeros();
    Some(SplitResult {
        sub_faces: vec![face_id, face_b],
    })
}

/// Clip a spherical face whose boundary a circle crosses more than twice.
///
/// Same region tracing as `split_planar_face_by_multi_arc`, on the sphere:
/// straight sections walk the sampled boundary polyline between crossings;
/// at each crossing the trace continues along the one angle-adjacent circle
/// arc whose midpoint lies inside the original face (exactly one of the two
/// adjacent arcs does at a transversal crossing). Arc interiors ride the
/// same θ = 2πk/segments circle-frame grid as the two-crossing clip's
/// connector, so every neighbor bounded by this circle conforms.
///
/// Returns `Some` with the face unchanged — NOT `None` — when the pattern
/// can't be traced (odd count, tangential grazing): the caller's fallback
/// for `None` is the whole-sphere split, which would throw the patch's
/// trim away and resurface deleted geometry.
fn clip_spherical_face_multi(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
    tolerance: f64,
    verts: &[Point3],
    crossings: &[(usize, Point3)],
) -> Option<SplitResult> {
    let unchanged = Some(SplitResult {
        sub_faces: vec![face_id],
    });
    let m = crossings.len();
    let n = verts.len();
    if !m.is_multiple_of(2) {
        return unchanged;
    }

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;

    let x_dir = circle.x_dir.into_inner();
    let y_dir = circle.y_dir.into_inner();
    let angle_of = |p: &Point3| -> f64 {
        let d = *p - circle.center;
        let a = d.dot(y_dir).atan2(d.dot(x_dir));
        if a < 0.0 {
            a + 2.0 * std::f64::consts::PI
        } else {
            a
        }
    };
    let on_circle = |ang: f64| -> Point3 {
        circle.center + circle.radius * (ang.cos() * x_dir + ang.sin() * y_dir)
    };

    // Crossings by circle angle. `crossings` itself is already in
    // boundary-walk order (edges scanned in loop order, one sign change
    // per segment).
    let angles: Vec<f64> = crossings.iter().map(|(_, p)| angle_of(p)).collect();
    let mut by_angle: Vec<usize> = (0..m).collect();
    by_angle.sort_by(|&a, &b| {
        angles[a]
            .partial_cmp(&angles[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut angle_rank = vec![0usize; m];
    for (rank, &ci) in by_angle.iter().enumerate() {
        angle_rank[ci] = rank;
    }

    // For each angle-adjacent arc (rank k → rank k+1, increasing angle),
    // does its midpoint lie inside the face? All probes run BEFORE any
    // mutation.
    let two_pi = 2.0 * std::f64::consts::PI;
    let arc_inside: Vec<bool> = (0..m)
        .map(|k| {
            let a = angles[by_angle[k]];
            let b = angles[by_angle[(k + 1) % m]];
            let span = (b - a).rem_euclid(two_pi);
            let mid = on_circle(a + 0.5 * span);
            crate::trim::point_in_face(brep, face_id, &mid)
        })
        .collect();
    // At a transversal crossing exactly one adjacent arc is inside.
    for &rank in angle_rank.iter().take(m) {
        let after = arc_inside[rank];
        let before = arc_inside[(rank + m - 1) % m];
        if after == before {
            return unchanged;
        }
    }

    // Next crossing along the boundary walk.
    let next_on_boundary = |ci: usize| (ci + 1) % m;

    // Trace region loops.
    let mut chain_used = vec![false; m];
    let mut regions: Vec<Vec<Point3>> = Vec::new();
    for start in 0..m {
        if chain_used[start] {
            continue;
        }
        let mut points: Vec<Point3> = Vec::new();
        let mut cur = start;
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > m + 1 {
                return unchanged;
            }
            if chain_used[cur] {
                return unchanged;
            }
            chain_used[cur] = true;
            // Boundary chain: crossing `cur` → next crossing on the walk.
            let nxt = next_on_boundary(cur);
            points.push(crossings[cur].1);
            let (e_cur, e_nxt) = (crossings[cur].0, crossings[nxt].0);
            let mut i = (e_cur + 1) % n;
            loop {
                if i == (e_nxt + 1) % n {
                    break;
                }
                points.push(verts[i]);
                i = (i + 1) % n;
            }
            // Arc: continue along the one inside adjacent arc at `nxt`.
            let rank = angle_rank[nxt];
            let (to, ccw) = if arc_inside[rank] {
                (by_angle[(rank + 1) % m], true)
            } else {
                (by_angle[(rank + m - 1) % m], false)
            };
            let arc =
                circle_frame_arc_points(circle, crossings[nxt].1, crossings[to].1, ccw, segments);
            points.extend(&arc[..arc.len() - 1]);
            cur = to;
            if cur == start {
                break;
            }
        }
        let points = remove_consecutive_duplicates(&points, tolerance);
        if points.len() < 3 {
            return unchanged;
        }
        regions.push(points);
    }
    if regions.len() < 2 {
        return unchanged;
    }

    // Build the sub-faces: the first region reuses the original face.
    let shell = brep.topology.faces[face_id].shell;
    let mut sub_faces = Vec::with_capacity(regions.len());
    for (ri, points) in regions.iter().enumerate() {
        let vert_ids: Vec<_> = points
            .iter()
            .map(|p| find_or_create_vertex(brep, p, tolerance))
            .collect();
        let hes: Vec<_> = vert_ids
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let loop_id = brep.topology.add_loop(&hes);
        if ri == 0 {
            let f = &mut brep.topology.faces[face_id];
            f.outer_loop = loop_id;
            f.inner_loops.clear();
            sub_faces.push(face_id);
        } else {
            let new_face = brep.topology.add_face(loop_id, surface_index, orientation);
            if let Some(shell_id) = shell {
                brep.topology.shells[shell_id].faces.push(new_face);
                brep.topology.faces[new_face].shell = Some(shell_id);
            }
            sub_faces.push(new_face);
        }
    }

    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    Some(SplitResult { sub_faces })
}

/// Check if a face's underlying surface is a cylinder.
pub fn is_cylindrical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder
}

/// Check if a face's underlying surface is a sphere.
pub fn is_spherical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Sphere
}

/// Check if a face's underlying surface is a torus.
pub fn is_toroidal_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Torus
}

/// Check if a face's underlying surface is a cone.
pub fn is_conical_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    surface.surface_type() == vcad_kernel_geom::SurfaceKind::Cone
}

/// Split a conical face along an intersection curve.
pub fn split_conical_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    entry: &Point3,
    exit: &Point3,
    segments: u32,
) -> SplitResult {
    match curve {
        IntersectionCurve::Circle(circle) => {
            split_conical_face_by_circle(brep, face_id, circle, segments)
        }
        IntersectionCurve::Line(line) => {
            split_conical_face_by_ruling(brep, face_id, line, entry, exit, segments)
        }
        IntersectionCurve::Sampled(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::Empty | IntersectionCurve::Point(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::TwoLines(line1, line2) => {
            // The pipeline expands TwoLines into individual Line splits, so
            // this arm is normally not reached — but a direct caller gets
            // both rulings applied: split by the first, then run the second
            // over every resulting sub-face.
            let first = split_conical_face_by_ruling(brep, face_id, line1, entry, exit, segments);
            let mut out = Vec::new();
            for fid in first.sub_faces {
                if brep.topology.faces.contains_key(fid) {
                    out.extend(
                        split_conical_face_by_ruling(brep, fid, line2, entry, exit, segments)
                            .sub_faces,
                    );
                }
            }
            SplitResult { sub_faces: out }
        }
        IntersectionCurve::TwoSampled(_, _) => SplitResult {
            sub_faces: vec![face_id],
        },
    }
}

/// Split a conical face along a ruling (a straight line through the apex,
/// lying on the cone surface — the intersection of the cone with a plane
/// through its apex, e.g. a box side plane containing the cone axis).
///
/// In the cone's UV space `[0, 2π] × [v_min, v_max]` a ruling is a vertical
/// line at constant `u = u_split`, exactly like an axis-parallel line on a
/// cylinder. Mirrors `split_cylindrical_face_by_line`: the rim arcs of the
/// two sub-faces are emitted as DENSE canonical chains
/// (`canonical_arc_points` on each rim circle) so they conform vertex-for-
/// vertex with the neighboring cap faces' arcs.
///
/// Handles full lateral frustum faces (degenerate seam loop) and sector
/// faces from previous ruling splits (dense two-rim loops). Pointed cones
/// (a rim collapsed to the apex) are declined and returned unsplit.
fn split_conical_face_by_ruling(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
    entry: &Point3,
    exit: &Point3,
    segments: u32,
) -> SplitResult {
    use std::f64::consts::PI;
    macro_rules! unsplit {
        ($fid:expr) => {{
            split_dbg!("by_ruling declined at split.rs:{}", line!());
            return SplitResult {
                sub_faces: vec![$fid],
            };
        }};
    }
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let cone = match brep.geometry.surfaces[surface_index]
        .as_any()
        .downcast_ref::<vcad_kernel_geom::ConeSurface>()
    {
        Some(c) => c.clone(),
        None => unsplit!(face_id),
    };

    let axis = *cone.axis.as_ref();
    let ref_dir = *cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);
    let ca = cone.half_angle.cos();
    let sa = cone.half_angle.sin();

    // The line must be a ruling: through the apex, at the cone's half-angle
    // to the axis (on the positive-v side).
    let mut dir = line.direction.normalize();
    if dir.dot(axis) < 0.0 {
        dir = -dir;
    }
    if (dir.dot(axis) - ca).abs() > 1e-6 {
        unsplit!(face_id);
    }
    let apex_off = cone.apex - line.origin;
    if (apex_off - apex_off.dot(dir) * dir).norm() > 1e-6 {
        unsplit!(face_id);
    }
    let dir_perp = dir - dir.dot(axis) * axis;
    let u_split = {
        let u = dir_perp.dot(y_dir).atan2(dir_perp.dot(ref_dir));
        if u < 0.0 {
            u + 2.0 * PI
        } else {
            u
        }
    };

    let cone_u = |p: &Point3| -> Option<f64> {
        let d = *p - cone.apex;
        let d_perp = d - d.dot(axis) * axis;
        if d_perp.norm() < 1e-9 {
            return None; // apex vertex, u undefined
        }
        let u = d_perp.dot(y_dir).atan2(d_perp.dot(ref_dir));
        Some(if u < 0.0 { u + 2.0 * PI } else { u })
    };
    let cone_v = |p: &Point3| -> f64 { (*p - cone.apex).dot(axis) / ca };

    // Collect the loop's unique vertices with (v, u).
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        unsplit!(face_id);
    }
    let mut all_verts: Vec<(vcad_kernel_topo::VertexId, f64, f64)> = Vec::new();
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let v = cone_v(&point);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
        if !all_verts.iter().any(|(id, _, _)| *id == vid) {
            let Some(u) = cone_u(&point) else {
                // Pointed cone (rim collapsed to the apex) — decline.
                unsplit!(face_id);
            };
            all_verts.push((vid, v, u));
        }
    }
    if v_max - v_min < 1e-9 || v_min < 1e-9 {
        unsplit!(face_id);
    }

    // The recorded segment [entry, exit] is the stretch of the ruling that
    // lies on BOTH intersecting faces (clipped at record time). A sub-band
    // the cut never reaches must stay unsplit: splitting it would put rim
    // corner vertices on its rims that the neighboring cap faces (which the
    // cut also never reaches) don't carry — an open T-junction.
    {
        let v_entry = cone_v(entry);
        let v_exit = cone_v(exit);
        let (seg_lo, seg_hi) = (v_entry.min(v_exit), v_entry.max(v_exit));
        if seg_hi < v_min + 1e-6 || seg_lo > v_max - 1e-6 {
            unsplit!(face_id);
        }
    }

    let bottom_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_min).abs() < 1e-6)
        .cloned()
        .collect();
    let top_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_max).abs() < 1e-6)
        .cloned()
        .collect();
    // Every loop vertex must sit on one of the two rims (constant-v rims are
    // the only shape this splitter produces or understands).
    if bottom_verts.len() + top_verts.len() != all_verts.len() {
        unsplit!(face_id);
    }

    // Determine the face's u-extent and its four corner vertices.
    let (u_start, u_end, v_start_bot, v_end_bot, v_start_top, v_end_top, is_full_face) =
        if bottom_verts.len() == 1 && top_verts.len() == 1 {
            // Full lateral face: degenerate seam loop, u spans the whole turn.
            let seam_u = bottom_verts[0].2;
            (
                seam_u,
                seam_u + 2.0 * PI,
                bottom_verts[0].0,
                bottom_verts[0].0,
                top_verts[0].0,
                top_verts[0].0,
                true,
            )
        } else if bottom_verts.len() >= 2 && top_verts.len() >= 2 {
            // Sector face (possibly with dense rim chains): walk the loop to
            // find its corners — a corner is a rim vertex adjacent (in loop
            // order) to the other rim's chain, i.e. the rim chain's first
            // and last vertices.
            let on_bottom = |vid| bottom_verts.iter().any(|(id, _, _)| *id == vid);
            let n = loop_hes.len();
            let vid_at = |i: usize| brep.topology.half_edges[loop_hes[i % n]].origin;
            // Find a loop position where the walk transitions top→bottom;
            // that position starts the bottom chain.
            let mut start = None;
            for i in 0..n {
                if !on_bottom(vid_at(i)) && on_bottom(vid_at(i + 1)) {
                    start = Some((i + 1) % n);
                    break;
                }
            }
            let Some(start) = start else {
                unsplit!(face_id);
            };
            let mut bot_chain: Vec<vcad_kernel_topo::VertexId> = Vec::new();
            let mut top_chain: Vec<vcad_kernel_topo::VertexId> = Vec::new();
            let mut i = start;
            while on_bottom(vid_at(i)) {
                bot_chain.push(vid_at(i));
                i += 1;
                if bot_chain.len() > n {
                    unsplit!(face_id);
                }
            }
            while !on_bottom(vid_at(i)) {
                top_chain.push(vid_at(i));
                i += 1;
                if top_chain.len() > n {
                    unsplit!(face_id);
                }
            }
            if bot_chain.len() + top_chain.len() != n {
                // More than two rim runs — not a plain sector.
                unsplit!(face_id);
            }
            // Loop order: bottom chain ascending u, then top chain
            // descending u (the convention both make_cone and this splitter
            // emit). Corners: bottom first/last, top last/first.
            let u_of = |vid| {
                all_verts
                    .iter()
                    .find(|(id, _, _)| *id == vid)
                    .map(|(_, _, u)| *u)
                    .unwrap_or(0.0)
            };
            let sb = *bot_chain.first().unwrap();
            let eb = *bot_chain.last().unwrap();
            let st = *top_chain.last().unwrap();
            let et = *top_chain.first().unwrap();
            let u0 = u_of(sb);
            let u1 = u_of(eb);
            // A dense FROZEN full band (freeze_circle_loops rewrote its two
            // analytic rim circles into canonical polylines) walks the full
            // turn: its rim chain has no gap wider than a couple of grid
            // steps. Treat it like the degenerate full face, with the seam
            // at the chain's start/end vertex.
            let is_frozen_full = {
                let mut us: Vec<f64> = bot_chain.iter().map(|&v| u_of(v)).collect();
                us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mut max_gap = 2.0 * PI - (us[us.len() - 1] - us[0]);
                for w in us.windows(2) {
                    max_gap = max_gap.max(w[1] - w[0]);
                }
                max_gap < 0.5
            };
            if is_frozen_full {
                if angle_dist(u0, u1) > 1e-6 || angle_dist(u_of(st), u_of(et)) > 1e-6 {
                    unsplit!(face_id);
                }
                (u0, u0 + 2.0 * PI, sb, eb, et, st, true)
            } else {
                // Corner u values must agree between rims (rulings are
                // vertical).
                if angle_dist(u1, u_of(et)) > 1e-6 || angle_dist(u0, u_of(st)) > 1e-6 {
                    unsplit!(face_id);
                }
                let wraps_around = u1 < u0 - 0.01;
                let end_u = if wraps_around { u1 + 2.0 * PI } else { u1 };
                (u0, end_u, sb, eb, st, et, false)
            }
        } else {
            unsplit!(face_id);
        };

    // Is the split ruling within the face's u-range?
    let in_range = if is_full_face {
        let seam_u = u_start;
        angle_dist(u_split, seam_u) > 0.01
    } else {
        angle_in_range(u_split, u_start, u_end)
    };
    if !in_range {
        unsplit!(face_id);
    }

    // 3D points at the split ruling's two rim crossings.
    let (sin_u, cos_u) = u_split.sin_cos();
    let ruling_dir = ca * axis + sa * (cos_u * ref_dir + sin_u * y_dir);
    let point_bottom = cone.apex + v_min * ruling_dir;
    let point_top = cone.apex + v_max * ruling_dir;

    let tolerance = 1e-6;
    let v_split_bottom = find_or_create_vertex(brep, &point_bottom, tolerance);
    let v_split_top = find_or_create_vertex(brep, &point_top, tolerance);

    // Rim circle data: center on the axis, radius v·sin(α).
    let r_bot = v_min * sa;
    let r_top = v_max * sa;
    let bot_center = cone.apex + v_min * ca * axis;
    let top_center = cone.apex + v_max * ca * axis;

    let chain_vids =
        |brep: &mut BRepSolid, center: Point3, radius: f64, from: Point3, to: Point3| {
            // Increasing u = CCW about the cone axis.
            canonical_arc_points(center, radius, axis, from, to, segments)
                .into_iter()
                .map(|p| find_or_create_vertex(brep, &p, tolerance))
                .collect::<Vec<_>>()
        };
    let build_face = |brep: &mut BRepSolid,
                      v_bot_a: vcad_kernel_topo::VertexId,
                      v_bot_b: vcad_kernel_topo::VertexId,
                      v_top_a: vcad_kernel_topo::VertexId,
                      v_top_b: vcad_kernel_topo::VertexId|
     -> (FaceId, HalfEdgeId, HalfEdgeId) {
        let p_bot_a = brep.topology.vertices[v_bot_a].point;
        let p_bot_b = brep.topology.vertices[v_bot_b].point;
        let p_top_a = brep.topology.vertices[v_top_a].point;
        let p_top_b = brep.topology.vertices[v_top_b].point;
        // Bottom chain ascending u (a→b), up ruling, top chain descending u
        // (b→a), down ruling — same loop shape as make_cone's lateral face.
        let mut bot = chain_vids(brep, bot_center, r_bot, p_bot_a, p_bot_b);
        let mut top = chain_vids(brep, top_center, r_top, p_top_a, p_top_b);
        *bot.first_mut().unwrap() = v_bot_a;
        *bot.last_mut().unwrap() = v_bot_b;
        *top.first_mut().unwrap() = v_top_a;
        *top.last_mut().unwrap() = v_top_b;
        top.reverse(); // descending u: b → a

        let mut origins: Vec<vcad_kernel_topo::VertexId> = Vec::new();
        origins.extend(&bot[..bot.len() - 1]);
        origins.extend(&top[..top.len() - 1]);
        origins.insert(bot.len() - 1, bot[bot.len() - 1]);
        origins.push(top[top.len() - 1]);

        let hes: Vec<_> = origins
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let lp = brep.topology.add_loop(&hes);
        let face = brep.topology.add_face(lp, surface_index, orientation);
        let up_he = hes[bot.len() - 1];
        let down_he = hes[hes.len() - 1];
        (face, up_he, down_he)
    };

    let (face1, he1_up, _he1_down) =
        build_face(brep, v_start_bot, v_split_bottom, v_start_top, v_split_top);
    let (face2, _he2_up, he2_down) =
        build_face(brep, v_split_bottom, v_end_bot, v_split_top, v_end_top);

    // Twin the shared split ruling: face1 goes up at u_split, face2 comes
    // back down at u_split.
    brep.topology.add_edge(he1_up, he2_down);
    if is_full_face {
        brep.topology.add_edge(_he1_down, _he2_up);
    }

    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);
        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    brep.topology.faces.remove(face_id);
    brep.geometry.add_curve_3d(Box::new(line.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Minimal cyclic distance between two angles in radians.
fn angle_dist(a: f64, b: f64) -> f64 {
    use std::f64::consts::PI;
    let d = (a - b).rem_euclid(2.0 * PI);
    d.min(2.0 * PI - d)
}

/// Split a dense (frozen-polyline) full conical band at a constant-v
/// circle, producing two dense bands.
///
/// `freeze_circle_loops` rewrites primitive frusta rims into canonical
/// polylines before the pipeline runs, so a cone lateral face arrives here
/// as a dense two-ring loop (bottom ring +u, seam up, top ring −u, seam
/// down — make_cone's shape, densified). The legacy splitter's degenerate
/// seam-loop reconstruction would thaw those rims back into analytic
/// circles that can never conform; instead, keep both existing rings
/// verbatim and realize the new mid ring on the same canonical grid.
fn split_dense_conical_band_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    cone: &vcad_kernel_geom::ConeSurface,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    use std::f64::consts::PI;
    let unsplit = |face_id| SplitResult {
        sub_faces: vec![face_id],
    };
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let shell = face.shell;

    let axis = *cone.axis.as_ref();
    let ref_dir = *cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);
    let ca = cone.half_angle.cos();
    let sa = cone.half_angle.sin();
    let cone_v = |p: &Point3| -> f64 { (*p - cone.apex).dot(axis) / ca };
    let cone_u = |p: &Point3| -> Option<f64> {
        let d = *p - cone.apex;
        let d_perp = d - d.dot(axis) * axis;
        if d_perp.norm() < 1e-9 {
            return None;
        }
        let u = d_perp.dot(y_dir).atan2(d_perp.dot(ref_dir));
        Some(if u < 0.0 { u + 2.0 * PI } else { u })
    };

    // Loop origins in order, with rim classification.
    let loop_vids: Vec<vcad_kernel_topo::VertexId> = brep
        .topology
        .loop_half_edges(face.outer_loop)
        .map(|he| brep.topology.half_edges[he].origin)
        .collect();
    let vs: Vec<f64> = loop_vids
        .iter()
        .map(|&v| cone_v(&brep.topology.vertices[v].point))
        .collect();
    let (v_min, v_max) = vs
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    if v_max - v_min < 1e-9 {
        return unsplit(face_id);
    }
    const V_TOL: f64 = 1e-6;
    if vs
        .iter()
        .any(|&v| (v - v_min).abs() >= V_TOL && (v - v_max).abs() >= V_TOL)
    {
        return unsplit(face_id); // wavy boundary — not a plain band
    }
    let on_bottom: Vec<bool> = vs.iter().map(|&v| (v - v_min).abs() < V_TOL).collect();
    let n = loop_vids.len();
    let transitions = (0..n)
        .filter(|&i| on_bottom[i] != on_bottom[(i + 1) % n])
        .count();
    if transitions != 2 {
        return unsplit(face_id);
    }
    let start = match (0..n).find(|&i| on_bottom[i] && !on_bottom[(i + n - 1) % n]) {
        Some(s) => s,
        None => return unsplit(face_id),
    };
    let idx = |k: usize| (start + k) % n;
    let n_bot = (0..n).take_while(|&k| on_bottom[idx(k)]).count();
    let bot_run: Vec<_> = (0..n_bot).map(|k| loop_vids[idx(k)]).collect();
    let top_run: Vec<_> = (n_bot..n).map(|k| loop_vids[idx(k)]).collect();
    if bot_run.len() < 3 || top_run.len() < 3 {
        return unsplit(face_id);
    }

    // Both rims must be full rings (frozen band); a sector's rim would
    // leave a wide angular gap.
    let full_ring = |run: &[vcad_kernel_topo::VertexId]| -> bool {
        let mut us: Vec<f64> = run
            .iter()
            .filter_map(|&v| cone_u(&brep.topology.vertices[v].point))
            .collect();
        if us.len() < 3 {
            return false;
        }
        us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut max_gap = 2.0 * PI - (us[us.len() - 1] - us[0]);
        for w in us.windows(2) {
            max_gap = max_gap.max(w[1] - w[0]);
        }
        max_gap < 0.5
    };
    if !full_ring(&bot_run) || !full_ring(&top_run) {
        return unsplit(face_id);
    }
    // The runs carry the seam vertex at both ends (ring closing edge +
    // seam edge origin); drop the duplicate.
    let dedup_run = |run: &[vcad_kernel_topo::VertexId]| -> Vec<vcad_kernel_topo::VertexId> {
        let mut r = run.to_vec();
        if r.len() >= 2 && r[0] == r[r.len() - 1] {
            r.pop();
        }
        r
    };
    let bot_ring = dedup_run(&bot_run);
    let top_ring = dedup_run(&top_run);

    // The split circle's v must be strictly inside the band.
    let v_split = (circle.center - cone.apex).dot(axis) / ca;
    if v_split <= v_min + 1e-9 || v_split >= v_max - 1e-9 {
        return unsplit(face_id);
    }

    // Travel directions: the mid ring must travel like the run it
    // replaces. canonical_arc_points travels +u (CCW about the axis); the
    // bottom run travels whichever way the original loop wound. (Computed
    // before any topology mutation — cone_u borrows the topology.)
    let run_ascending = |ring: &[vcad_kernel_topo::VertexId]| -> bool {
        for w in ring.windows(2) {
            let (Some(a), Some(b)) = (
                cone_u(&brep.topology.vertices[w[0]].point),
                cone_u(&brep.topology.vertices[w[1]].point),
            ) else {
                continue;
            };
            let mut du = (b - a).rem_euclid(2.0 * PI);
            if du > PI {
                du -= 2.0 * PI;
            }
            if du.abs() > 1e-9 {
                return du > 0.0;
            }
        }
        true
    };
    let bot_asc = run_ascending(&bot_ring);

    // Realize the mid ring on the canonical grid, seamed at the same u as
    // the band's own seam.
    let seam_u = match cone_u(&brep.topology.vertices[bot_ring[0]].point) {
        Some(u) => u,
        None => return unsplit(face_id),
    };
    let (sin_s, cos_s) = seam_u.sin_cos();
    let ruling_dir = ca * axis + sa * (cos_s * ref_dir + sin_s * y_dir);
    let mid_seam_pt = cone.apex + v_split * ruling_dir;
    let mid_center = cone.apex + v_split * ca * axis;
    let mid_radius = v_split * sa;
    let tolerance = 1e-6;
    let mut mid_ring_pts = canonical_arc_points(
        mid_center,
        mid_radius,
        axis,
        mid_seam_pt,
        mid_seam_pt,
        segments,
    );
    mid_ring_pts.pop(); // duplicate seam endpoint
    let mid_ring_asc: Vec<vcad_kernel_topo::VertexId> = mid_ring_pts
        .iter()
        .map(|p| find_or_create_vertex(brep, p, tolerance))
        .collect();

    let mid_like_bot: Vec<vcad_kernel_topo::VertexId> = if bot_asc {
        mid_ring_asc.clone()
    } else {
        let mut r = mid_ring_asc.clone();
        r[1..].reverse();
        r
    };
    let mid_like_top: Vec<vcad_kernel_topo::VertexId> = {
        // Opposite travel to the bottom-style ring, same seam start.
        let mut r = mid_like_bot.clone();
        r[1..].reverse();
        r
    };

    // Assemble a band face from two rings (each in its travel order,
    // starting at its seam vertex): ring1 edges (closing back to seam),
    // seam-up, ring2 edges, seam-down.
    let mut build_band = |ring_lo: &[vcad_kernel_topo::VertexId],
                          ring_hi: &[vcad_kernel_topo::VertexId]|
     -> (
        FaceId,
        Vec<HalfEdgeId>,
        HalfEdgeId,
        Vec<HalfEdgeId>,
        HalfEdgeId,
    ) {
        let mut origins: Vec<vcad_kernel_topo::VertexId> = Vec::new();
        origins.extend(ring_lo);
        origins.push(ring_lo[0]); // seam-up
        origins.extend(ring_hi);
        origins.push(ring_hi[0]); // seam-down
        let hes: Vec<HalfEdgeId> = origins
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let lp = brep.topology.add_loop(&hes);
        let f = brep.topology.add_face(lp, surface_index, orientation);
        let k = ring_lo.len();
        let m = ring_hi.len();
        (
            f,
            hes[..k].to_vec(),
            hes[k],
            hes[k + 1..k + 1 + m].to_vec(),
            hes[k + 1 + m],
        )
    };

    let (lower_face, _lo_bot, lo_up, lo_top, lo_down) = build_band(&bot_ring, &mid_like_top);
    let (upper_face, up_bot, up_up, _up_top, up_down) = build_band(&mid_like_bot, &top_ring);

    // Seam edges twin within each band (make_cone's convention).
    brep.topology.add_edge(lo_up, lo_down);
    brep.topology.add_edge(up_up, up_down);
    // Mid-ring edges pair between the bands: lo_top[i] runs
    // mid_like_top[i] → mid_like_top[i+1]; the upper band traverses the
    // same ring in the opposite direction. Edge k of one direction pairs
    // with edge (m-1-k) of the other.
    let m = lo_top.len();
    debug_assert_eq!(m, up_bot.len());
    for i in 0..m {
        brep.topology.add_edge(lo_top[i], up_bot[m - 1 - i]);
    }

    if let Some(shell_id) = shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);
        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }
    brep.topology.faces.remove(face_id);
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![lower_face, upper_face],
    }
}

/// Split a conical face along a circle intersection curve.
fn split_conical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
    segments: u32,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    let cone = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::ConeSurface>()
    {
        Some(c) => c.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    let ca = cone.half_angle.cos();
    let apex_to_circle = (circle.center - cone.apex).dot(cone.axis.as_ref());

    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    // This splitter reconstructs its sub-faces as degenerate seam loops
    // spanning the FULL circumference — only correct when the input face is
    // itself a full lateral band with analytic rims (primitive frusta and
    // their circle-split sub-bands, ≤ 6 half-edges). Dense loops (frozen
    // bands and ruling-split sectors) go through the dense v-split path.
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }
    if loop_hes.len() > 6 {
        return split_dense_conical_band_by_circle(brep, face_id, &cone, circle, segments);
    }

    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    for &he_id in &loop_hes {
        let v_id = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[v_id].point;
        let d = point - cone.apex;
        let v = d.dot(cone.axis.as_ref()) / ca;
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    let v_split = apex_to_circle / ca;
    if v_split <= v_min + 1e-9 || v_split >= v_max - 1e-9 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    let sa = cone.half_angle.sin();
    let seam_point =
        cone.apex + v_split * (ca * cone.axis.into_inner() + sa * cone.ref_dir.into_inner());
    let v_split_seam = brep.topology.add_vertex(seam_point);

    let mut v_bottom = None;
    let mut v_top = None;
    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let d = point - cone.apex;
        let v = d.dot(cone.axis.as_ref()) / ca;
        if (v - v_min).abs() < 1e-6 {
            v_bottom = Some(vid);
        }
        if (v - v_max).abs() < 1e-6 {
            v_top = Some(vid);
        }
    }

    let (v_bottom, v_top) = match (v_bottom, v_top) {
        (Some(b), Some(t)) => (b, t),
        _ => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    // Lower face: v_min to v_split
    let he_lower_bot = brep.topology.add_half_edge(v_bottom);
    let he_lower_seam_up = brep.topology.add_half_edge(v_bottom);
    let he_lower_split = brep.topology.add_half_edge(v_split_seam);
    let he_lower_seam_down = brep.topology.add_half_edge(v_split_seam);
    let lower_loop = brep.topology.add_loop(&[
        he_lower_bot,
        he_lower_seam_up,
        he_lower_split,
        he_lower_seam_down,
    ]);
    let lower_face = brep
        .topology
        .add_face(lower_loop, surface_index, orientation);

    // Upper face: v_split to v_max
    let he_upper_split = brep.topology.add_half_edge(v_split_seam);
    let he_upper_seam_up = brep.topology.add_half_edge(v_split_seam);
    let he_upper_top = brep.topology.add_half_edge(v_top);
    let he_upper_seam_down = brep.topology.add_half_edge(v_top);
    let upper_loop = brep.topology.add_loop(&[
        he_upper_split,
        he_upper_seam_up,
        he_upper_top,
        he_upper_seam_down,
    ]);
    let upper_face = brep
        .topology
        .add_face(upper_loop, surface_index, orientation);

    brep.topology.add_edge(he_lower_seam_up, he_lower_seam_down);
    brep.topology.add_edge(he_upper_seam_up, he_upper_seam_down);
    brep.topology.add_edge(he_lower_split, he_upper_split);

    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);
        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    brep.topology.faces.remove(face_id);
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![lower_face, upper_face],
    }
}
/// Debug-logging wrapper for [`split_cylindrical_face_by_circle`]
/// (enable with `VCAD_SPLIT_DEBUG=1`).
pub fn split_cylindrical_face_by_circle_logged(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) -> SplitResult {
    split_dbg!(
        "legacy circle split on face {face_id:?} nv={} at z {:.4}",
        brep.topology
            .loop_len(brep.topology.faces[face_id].outer_loop),
        circle.center.z
    );
    split_cylindrical_face_by_circle(brep, face_id, circle)
}

/// Split a full-tube cylindrical face by a perpendicular circle at the
/// circle's height.
///
/// This function splits the cylindrical face into two strips:
/// - Lower strip: `[0, 2π] × [v_min, h]`
/// - Upper strip: `[0, 2π] × [h, v_max]`
///
/// The split is performed by:
/// 1. Computing the intersection height `v_split` from the circle center
/// 2. Creating new 3D vertices at the intersection points (on the seam)
/// 3. Creating two new face loops that share the intersection edge
/// 4. Removing the original face and adding the two new sub-faces
pub fn split_cylindrical_face_by_circle(
    brep: &mut BRepSolid,
    face_id: FaceId,
    circle: &vcad_kernel_geom::Circle3d,
) -> SplitResult {
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let _outer_loop = face.outer_loop;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify surface is a cylinder
    let cyl = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        Some(c) => c.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Compute the split height: v_split = projection of circle center onto cylinder axis
    // v = (circle.center - cyl.center) · axis
    let v_split = (circle.center - cyl.center).dot(cyl.axis.as_ref());

    // Get the current face's v bounds from its boundary vertices
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // For a cylinder lateral face, we typically have:
    // - 2 vertices (top and bottom seam points)
    // - 4 half-edges: bottom circle, seam up, top circle, seam down
    // The v coordinates of these vertices give us v_min and v_max
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;

    for &he_id in &loop_hes {
        let v_id = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[v_id].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    // Check if split height is within the face's v range
    if v_split <= v_min + 1e-9 || v_split >= v_max - 1e-9 {
        // Split line doesn't cross the face interior
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Create new vertex at the seam point at height v_split
    // At u=0, the point is: center + radius * ref_dir + v_split * axis
    let seam_point_at_split =
        cyl.center + cyl.radius * cyl.ref_dir.as_ref() + v_split * cyl.axis.as_ref();
    let v_split_seam = brep.topology.add_vertex(seam_point_at_split);

    // Get the existing top and bottom seam vertices
    // For a standard cylinder lateral face:
    // - loop_hes[0] origin = bottom seam point (at v_min)
    // - loop_hes[2] origin = top seam point (at v_max)
    // But this depends on how the cylinder was constructed.
    // Let's identify vertices by their v coordinate.
    let mut v_bottom = None;
    let mut v_top = None;

    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        if (v - v_min).abs() < 1e-9 {
            v_bottom = Some(vid);
        }
        if (v - v_max).abs() < 1e-9 {
            v_top = Some(vid);
        }
    }

    let (v_bottom, v_top) = match (v_bottom, v_top) {
        (Some(b), Some(t)) => (b, t),
        _ => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    // Now create the two new sub-faces.
    // Each face has a similar structure to the original:
    // - A circular edge at one end
    // - A seam edge connecting to the split
    // - The split circle edge
    // - A seam edge back

    // For simplicity, we'll create new faces with the same topology structure
    // but different boundary vertices.

    // Lower face: v_min to v_split
    // Boundary: bottom_circle (v_bottom → v_bottom) → seam_up (v_bottom → v_split_seam)
    //        → split_circle (v_split_seam → v_split_seam) → seam_down (v_split_seam → v_bottom)
    let he_lower_bot = brep.topology.add_half_edge(v_bottom);
    let he_lower_seam_up = brep.topology.add_half_edge(v_bottom);
    let he_lower_split = brep.topology.add_half_edge(v_split_seam);
    let he_lower_seam_down = brep.topology.add_half_edge(v_split_seam);

    let lower_loop = brep.topology.add_loop(&[
        he_lower_bot,
        he_lower_seam_up,
        he_lower_split,
        he_lower_seam_down,
    ]);
    let lower_face = brep
        .topology
        .add_face(lower_loop, surface_index, orientation);

    // Upper face: v_split to v_max
    // Boundary: split_circle (v_split_seam → v_split_seam) → seam_up (v_split_seam → v_top)
    //        → top_circle (v_top → v_top) → seam_down (v_top → v_split_seam)
    let he_upper_split = brep.topology.add_half_edge(v_split_seam);
    let he_upper_seam_up = brep.topology.add_half_edge(v_split_seam);
    let he_upper_top = brep.topology.add_half_edge(v_top);
    let he_upper_seam_down = brep.topology.add_half_edge(v_top);

    let upper_loop = brep.topology.add_loop(&[
        he_upper_split,
        he_upper_seam_up,
        he_upper_top,
        he_upper_seam_down,
    ]);
    let upper_face = brep
        .topology
        .add_face(upper_loop, surface_index, orientation);

    // Add twin edges
    // Lower seam edges
    brep.topology.add_edge(he_lower_seam_up, he_lower_seam_down);
    // Upper seam edges
    brep.topology.add_edge(he_upper_seam_up, he_upper_seam_down);
    // The split circle edges from upper and lower faces are twins
    brep.topology.add_edge(he_lower_split, he_upper_split);

    // Link bottom circle: lower face shares with bottom cap
    // Link top circle: upper face shares with top cap
    // These would need to be re-linked if we had access to the original edges
    // For now, we'll skip re-linking circular edges as they're handled elsewhere

    // Add the new faces to the shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(lower_face);
        brep.topology.shells[shell_id].faces.push(upper_face);

        brep.topology.faces[lower_face].shell = Some(shell_id);
        brep.topology.faces[upper_face].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curves for the split circle
    brep.geometry.add_curve_3d(Box::new(circle.clone()));

    SplitResult {
        sub_faces: vec![lower_face, upper_face],
    }
}

/// Number of points a boolean-result circular rim of `radius` is
/// discretized into, given the caller's requested `segments`.
///
/// This is the *canonical rim grid*: sag-adaptive, and therefore usually
/// FINER than the requested `circle_segments` — the count is
/// `max(requested, sag_count)`. For r = 2.5 at the default sag it is 112,
/// not 32. Callers that need the exact discrete geometry of a boolean
/// result (closed-form volume of a through hole, the node count of a frozen
/// rim, a discrete dV/dr) must ask this function rather than assume the rim
/// is the inscribed `circle_segments`-gon.
///
/// Both the planar-cap and cylinder-wall splitters must use this same count
/// so their shared arcs discretize identically.
///
/// CAVEAT, and a live lead: before #758 this deliberately returned the
/// caller's count, because the display tessellator re-samples ANALYTIC
/// circle boundaries that survive a boolean untouched at exactly `n`
/// (`TessellationParams::from_segments(n)`) — so densifying here makes a
/// frozen rim disagree with an analytic one it borders. #758 made it
/// sag-adaptive anyway, to match `ssi::ellipse_samples`. That trade may be
/// what the quarantined `through_hole_reconstructs_full_circles` is
/// measuring (STEP reconstructs 6 circles where it should find 8);
/// unproven, but it is the first place to look.
pub fn arc_segments(radius: f64, segments: u32) -> u32 {
    // Must match ssi::ellipse_samples' sag: SSI polylines and canonical
    // rings share vertices only when both discretize at the same density.
    const SAG: f64 = 1e-3;
    let n = if radius > SAG {
        let arg = (1.0 - SAG / radius).clamp(-1.0, 1.0);
        (std::f64::consts::PI / arg.acos()).ceil() as u32
    } else {
        3
    };
    n.max(segments).clamp(3, 512)
}

/// Discretization of a circular arc on the circle's own raw-`segments`
/// grid: θ_k = 2πk/segments in the circle's (x_dir, y_dir) frame.
///
/// This is the schedule `clip_spherical_face_by_circle` uses for its
/// connector and the full-disk path uses for an inscribed hole — the
/// convention for SPHERE-sourced circles. A planar split bordering such a
/// circle must ride this grid; the sag-adaptive canonical grid
/// (`canonical_arc_points`, the cylinder-wall convention) lands on
/// different points and the seam zippers open. Returns `[start,
/// interior…, end]`; `ccw` is travel counterclockwise about
/// `x_dir × y_dir`.
fn circle_frame_arc_points(
    circle: &vcad_kernel_geom::Circle3d,
    start: Point3,
    end: Point3,
    ccw: bool,
    segments: u32,
) -> Vec<Point3> {
    use std::f64::consts::PI;
    let x_dir = circle.x_dir.into_inner();
    let y_dir = circle.y_dir.into_inner();
    let angle_of = |p: Point3| -> f64 {
        let d = p - circle.center;
        let a = d.dot(y_dir).atan2(d.dot(x_dir));
        if a < 0.0 {
            a + 2.0 * PI
        } else {
            a
        }
    };
    let a_start = angle_of(start);
    let a_end = angle_of(end);
    let span = if ccw {
        (a_end - a_start).rem_euclid(2.0 * PI)
    } else {
        (a_start - a_end).rem_euclid(2.0 * PI)
    };
    // Coincident endpoints (or two points that project to the same angle)
    // give a zero span, which would otherwise return the bare two-point
    // `[start, end]` — a degenerate arc the caller can only discard. Say so
    // here rather than leaving it to `remove_consecutive_duplicates` and the
    // `len < 3` bail downstream.
    if span <= 1e-12 {
        return vec![start, end];
    }
    let n = segments.max(3);
    let step = 2.0 * PI / n as f64;
    let eps = 1e-9;
    let mut pts = vec![start];
    let mut idx = if ccw {
        (a_start / step).floor() + 1.0
    } else {
        (a_start / step).ceil() - 1.0
    };
    loop {
        let ang = idx * step;
        let traveled = if ccw {
            (ang - a_start).rem_euclid(2.0 * PI)
        } else {
            (a_start - ang).rem_euclid(2.0 * PI)
        };
        if traveled <= eps || traveled >= span - eps {
            break;
        }
        let a = ang.rem_euclid(2.0 * PI);
        let (sin_a, cos_a) = a.sin_cos();
        pts.push(snap_point(
            circle.center + circle.radius * (cos_a * x_dir + sin_a * y_dir),
        ));
        if ccw {
            idx += 1.0;
        } else {
            idx -= 1.0;
        }
        if pts.len() > n as usize + 2 {
            break; // safety
        }
    }
    pts.push(end);
    pts
}

/// Canonical, frame-independent discretization of a circular arc.
///
/// Every face that borders (a sub-arc of) the same 3D circle must emit the
/// same interior points, or the sewn shell can never conform: the planar cap
/// samples in its plane frame, the cylinder wall in its `ref_dir` frame, and
/// relative-fraction sampling gives every arc its own phase. This helper
/// derives a canonical in-plane frame purely from the circle geometry
/// (sign-normalized axis + most-orthogonal global axis) and samples interior
/// points on the absolute angular grid `θ_k = 2πk/n` in that frame, so two
/// faces sharing an arc reproduce bit-identical interior points regardless
/// of their own parameterizations. Returns `[start, interior…, end]`; travel
/// is counterclockwise about `normal` from `start` to `end`.
/// Canonical full-circle polyline: every grid point of the sag-dense
/// canonical frame, CCW about `normal`. Any two faces sampling the same
/// circle (a frozen wall ring, a cap hole boundary) get identical points.
pub(crate) fn canonical_circle_points(
    center: Point3,
    radius: f64,
    normal: vcad_kernel_math::Vec3,
    segments: u32,
) -> Vec<Point3> {
    // Reuse the arc sampler with start == end: it returns
    // [start, interior grid..., start]; drop the duplicated closing point.
    let x_axis = {
        // Same canonical-frame derivation as canonical_arc_points.
        let mut n_hat = normal.normalize();
        for c in [n_hat.x, n_hat.y, n_hat.z] {
            if c.abs() > 1e-9 {
                if c < 0.0 {
                    n_hat = -n_hat;
                }
                break;
            }
        }
        let cand = [
            vcad_kernel_math::Vec3::new(1.0, 0.0, 0.0),
            vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0),
            vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
        ];
        let e = cand
            .into_iter()
            .min_by(|a, b| a.dot(n_hat).abs().partial_cmp(&b.dot(n_hat).abs()).unwrap())
            .unwrap();
        (e - n_hat * e.dot(n_hat)).normalize()
    };
    let seam = center + radius * x_axis;
    let mut ring = canonical_arc_points(center, radius, normal, seam, seam, segments);
    ring.pop();
    ring
}

pub(crate) fn canonical_arc_points(
    center: Point3,
    radius: f64,
    normal: vcad_kernel_math::Vec3,
    start: Point3,
    end: Point3,
    segments: u32,
) -> Vec<Point3> {
    use std::f64::consts::PI;
    let mut n_hat = normal.normalize();
    // Sign-canonicalize the axis so the grid is invariant under normal flip.
    let mut flipped = false;
    for c in [n_hat.x, n_hat.y, n_hat.z] {
        if c.abs() > 1e-9 {
            if c < 0.0 {
                n_hat = -n_hat;
                flipped = true;
            }
            break;
        }
    }
    // Canonical in-plane frame: global axis least parallel to n̂.
    let cand = [
        vcad_kernel_math::Vec3::new(1.0, 0.0, 0.0),
        vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0),
        vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
    ];
    let e = cand
        .into_iter()
        .min_by(|a, b| a.dot(n_hat).abs().partial_cmp(&b.dot(n_hat).abs()).unwrap())
        .unwrap();
    let x_axis = (e - n_hat * e.dot(n_hat)).normalize();
    let y_axis = n_hat.cross(x_axis);

    let angle_of = |p: Point3| -> f64 {
        let d = p - center;
        let a = d.dot(y_axis).atan2(d.dot(x_axis));
        if a < 0.0 {
            a + 2.0 * PI
        } else {
            a
        }
    };
    let a_start = angle_of(start);
    let a_end = angle_of(end);
    // CCW about the ORIGINAL normal; in the canonical frame that is CCW
    // unless the axis was flipped.
    let ccw = !flipped;
    let span = if ccw {
        (a_end - a_start).rem_euclid(2.0 * PI)
    } else {
        (a_start - a_end).rem_euclid(2.0 * PI)
    };
    let span = if span < 1e-12 { 2.0 * PI } else { span };

    let n = arc_segments(radius, segments);
    let step = 2.0 * PI / n as f64;
    let mut pts = vec![start];
    // Interior grid angles strictly inside the traversal (ε away from the
    // endpoints so a grid point coincident with an endpoint isn't doubled).
    let eps = 1e-9;
    // Walk grid indices in traversal order: find the first grid angle
    // strictly after a_start (in travel direction), then step until a_end.
    let first_idx = if ccw {
        (a_start / step).floor() + 1.0
    } else {
        (a_start / step).ceil() - 1.0
    };
    // `traveled` must accumulate monotonically — computing it per-index with
    // rem_euclid(2π) wraps back below `span` after one full revolution, so a
    // full-circle arc whose seam is off the grid re-emits the first grid
    // points as duplicates (and only the safety cap stopped it).
    let mut idx = first_idx;
    let mut traveled = {
        let ang = idx * step;
        if ccw {
            (ang - a_start).rem_euclid(2.0 * PI)
        } else {
            (a_start - ang).rem_euclid(2.0 * PI)
        }
    };
    while traveled < span - eps {
        if traveled > eps {
            let a = (idx * step).rem_euclid(2.0 * PI);
            let (sin_a, cos_a) = a.sin_cos();
            pts.push(snap_point(
                center + radius * (cos_a * x_axis + sin_a * y_axis),
            ));
        }
        if ccw {
            idx += 1.0;
        } else {
            idx -= 1.0;
        }
        traveled += step;
    }
    pts.push(end);
    pts
}

fn compute_cylinder_u(point: &Point3, cyl: &vcad_kernel_geom::CylinderSurface) -> f64 {
    let d = *point - cyl.center;
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);
    let u = d.dot(y_dir).atan2(d.dot(ref_dir));
    if u < 0.0 {
        u + 2.0 * std::f64::consts::PI
    } else {
        u
    }
}

/// Check if angle `u` is within the range from `u_start` to `u_end` (CCW direction).
/// Handles wrap-around at 2π. For wrap-around cases, u_end may be > 2π.
fn angle_in_range(u: f64, u_start: f64, u_end: f64) -> bool {
    let tol = 0.01;
    let two_pi = 2.0 * std::f64::consts::PI;

    // If u_end > 2π, the face wraps around. Check if u is in [u_start, 2π) or [0, u_end - 2π)
    if u_end > two_pi {
        let end_wrapped = u_end - two_pi;
        // u is in range if it's in [u_start, 2π) or [0, end_wrapped)
        (u > u_start + tol && u < two_pi - tol) || (u > tol && u < end_wrapped - tol)
    } else if u_end >= u_start {
        // Simple case: range doesn't wrap around
        u > u_start + tol && u < u_end - tol
    } else {
        // Range wraps around 2π (e.g., from 5.5 to 0.5)
        u > u_start + tol || u < u_end - tol
    }
}

/// Split a cylindrical face along a line intersection curve.
///
/// When a plane parallel to the cylinder axis intersects the cylinder,
/// the result is a vertical line on the cylinder surface. In the cylinder's
/// UV space `[0, 2π] × [v_min, v_max]`, this line becomes a vertical line
/// at constant u = u_split.
///
/// This function splits the cylindrical face into two parts:
/// - One part: `[u_min, u_split] × [v_min, v_max]`
/// - Other part: `[u_split, u_max] × [v_min, v_max]`
///
/// Works for both:
/// - Full lateral faces (single seam vertex, u spans 0 to 2π)
/// - Partial lateral faces (4 corner vertices from previous splits)
pub fn split_cylindrical_face_by_line(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
    segments: u32,
) -> SplitResult {
    split_dbg!(
        "legacy line split on face {face_id:?} nv={}",
        brep.topology
            .loop_len(brep.topology.faces[face_id].outer_loop)
    );
    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;
    let surface = &brep.geometry.surfaces[surface_index];

    // Verify surface is a cylinder
    let cyl = match surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        Some(c) => c.clone(),
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            };
        }
    };

    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = cyl.axis.as_ref().cross(ref_dir);

    // Find the U parameter of the split line
    let d = line.origin - cyl.center;
    let u_split = d.dot(y_dir).atan2(d.dot(ref_dir));
    let u_split = if u_split < 0.0 {
        u_split + 2.0 * std::f64::consts::PI
    } else {
        u_split
    };

    // Get the current face's vertex bounds
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.is_empty() {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Collect all unique vertices with their (v, u) coordinates
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    let mut all_verts: Vec<(vcad_kernel_topo::VertexId, f64, f64)> = Vec::new(); // (vid, v, u)

    for &he_id in &loop_hes {
        let vid = brep.topology.half_edges[he_id].origin;
        let point = brep.topology.vertices[vid].point;
        let v = (point - cyl.center).dot(cyl.axis.as_ref());
        v_min = v_min.min(v);
        v_max = v_max.max(v);

        // Only add if not duplicate
        if !all_verts.iter().any(|(id, _, _)| *id == vid) {
            let u = compute_cylinder_u(&point, &cyl);
            all_verts.push((vid, v, u));
        }
    }

    // Separate into top and bottom vertices
    let bottom_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_min).abs() < 1e-6)
        .cloned()
        .collect();
    let top_verts: Vec<_> = all_verts
        .iter()
        .filter(|(_, v, _)| (*v - v_max).abs() < 1e-6)
        .cloned()
        .collect();

    // Determine face type and get corner vertices
    let (u_start, u_end, v_start_bot, v_end_bot, v_start_top, v_end_top, is_full_face) =
        if bottom_verts.len() == 1 && top_verts.len() == 1 {
            // Full cylindrical face with single seam vertex at each end
            // U spans from 0 (seam) around to 2π (back to seam)
            let seam_u = bottom_verts[0].2;
            (
                seam_u,
                seam_u + 2.0 * std::f64::consts::PI,
                bottom_verts[0].0,
                bottom_verts[0].0,
                top_verts[0].0,
                top_verts[0].0,
                true,
            )
        } else if bottom_verts.len() == 2 && top_verts.len() == 2 {
            // Partial cylindrical face with 4 corner vertices
            // Use the loop order to determine the U direction (CCW in UV space)
            //
            // For a face with loop: u0 -> u1 -> u1 -> u0, going CCW:
            // - If u1 > u0, the face spans [u0, u1]
            // - If u1 < u0, the face spans [u0, 2π] ∪ [0, u1] (wrap-around)

            // Find the first two distinct U values in the loop
            let mut first_u: Option<f64> = None;
            let mut second_u: Option<f64> = None;
            for &he_id in &loop_hes {
                let vid = brep.topology.half_edges[he_id].origin;
                let point = brep.topology.vertices[vid].point;
                let u = compute_cylinder_u(&point, &cyl);

                match first_u {
                    None => first_u = Some(u),
                    Some(fu) if (u - fu).abs() > 0.01 => {
                        second_u = Some(u);
                        break;
                    }
                    _ => {}
                }
            }

            let (u0, u1) = match (first_u, second_u) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            };

            // Determine if the face wraps around based on the direction of travel
            // If we go from u0 to u1 CCW and u1 < u0, we're wrapping around 2π
            let wraps_around = u1 < u0 - 0.01;

            // Find start/end vertices based on the U values
            let (b1, b2) = (bottom_verts[0], bottom_verts[1]);
            let (t1, t2) = (top_verts[0], top_verts[1]);

            let (start_bot, end_bot) = if (b1.2 - u0).abs() < 0.01 {
                (b1, b2)
            } else {
                (b2, b1)
            };

            let (start_top, end_top) = if (t1.2 - u0).abs() < 0.01 {
                (t1, t2)
            } else {
                (t2, t1)
            };

            // For wrap-around faces, adjust end_u to be > 2π for proper range checking
            let end_u = if wraps_around {
                end_bot.2 + 2.0 * std::f64::consts::PI
            } else {
                end_bot.2
            };

            (
                start_bot.2,
                end_u,
                start_bot.0,
                end_bot.0,
                start_top.0,
                end_top.0,
                false,
            )
        } else {
            // Unexpected face structure
            return SplitResult {
                sub_faces: vec![face_id],
            };
        };

    // Check if split line is within the face's U range
    let in_range = if is_full_face {
        // For full face, any u_split is valid (except exactly at the seam)
        let seam_u = u_start;
        (u_split - seam_u).abs() > 0.01
            && (u_split - seam_u - 2.0 * std::f64::consts::PI).abs() > 0.01
    } else {
        angle_in_range(u_split, u_start, u_end)
    };

    if !in_range {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Compute 3D points at the split line's top and bottom
    let sin_u = u_split.sin();
    let cos_u = u_split.cos();
    let radial = cyl.radius * (cos_u * ref_dir + sin_u * y_dir);
    let point_bottom = cyl.center + radial + v_min * cyl.axis.as_ref();
    let point_top = cyl.center + radial + v_max * cyl.axis.as_ref();

    // Create or reuse vertices at the split points
    let tolerance = 1e-6;
    let v_split_bottom = find_or_create_vertex(brep, &point_bottom, tolerance);
    let v_split_top = find_or_create_vertex(brep, &point_top, tolerance);

    // Create two new faces by splitting at the u_split line:
    // Face 1: from start to split (smaller U arc)
    // Face 2: from split to end (larger U arc, or to seam for full face)
    //
    // The top/bottom arcs are emitted as DENSE canonical chains (not single
    // chords): the neighboring cap faces discretize the same circles via
    // `canonical_arc_points`, so both sides produce identical vertices and
    // the sewn shell conforms. The resulting two-chain loops are exactly the
    // shape `tessellate_ruled_two_chain` renders verbatim.
    let axis = *cyl.axis.as_ref();
    let bot_center = cyl.center + v_min * axis;
    let top_center = cyl.center + v_max * axis;
    let chain_vids = |brep: &mut BRepSolid, center: Point3, from: Point3, to: Point3| {
        // Increasing u = CCW about the cylinder axis.
        canonical_arc_points(center, cyl.radius, axis, from, to, segments)
            .into_iter()
            .map(|p| find_or_create_vertex(brep, &p, tolerance))
            .collect::<Vec<_>>()
    };
    let build_face = |brep: &mut BRepSolid,
                      v_bot_a: vcad_kernel_topo::VertexId,
                      v_bot_b: vcad_kernel_topo::VertexId,
                      v_top_a: vcad_kernel_topo::VertexId,
                      v_top_b: vcad_kernel_topo::VertexId|
     -> (FaceId, HalfEdgeId, HalfEdgeId) {
        let p_bot_a = brep.topology.vertices[v_bot_a].point;
        let p_bot_b = brep.topology.vertices[v_bot_b].point;
        let p_top_a = brep.topology.vertices[v_top_a].point;
        let p_top_b = brep.topology.vertices[v_top_b].point;
        // Bottom chain ascending u (a→b), then up, then top chain
        // descending u (b→a), then down.
        let mut bot = chain_vids(brep, bot_center, p_bot_a, p_bot_b);
        let mut top = chain_vids(brep, top_center, p_top_a, p_top_b);
        // Reuse the canonical endpoint vertex ids.
        *bot.first_mut().unwrap() = v_bot_a;
        *bot.last_mut().unwrap() = v_bot_b;
        *top.first_mut().unwrap() = v_top_a;
        *top.last_mut().unwrap() = v_top_b;
        top.reverse(); // descending u: b → a

        // Loop origins: bottom chain a..b (b starts the "up" edge), then top
        // chain b..a (a starts the "down" edge closing to bottom a).
        let mut origins: Vec<vcad_kernel_topo::VertexId> = Vec::new();
        origins.extend(&bot[..bot.len() - 1]);
        origins.extend(&top[..top.len() - 1]);
        // `up` half-edge is the one whose origin is bot-end (last of bot
        // slice above is bot[len-2]; the up edge origin is bot[len-1]).
        origins.insert(bot.len() - 1, bot[bot.len() - 1]);
        // and the closing `down` half-edge originates at top's last (== a).
        origins.push(top[top.len() - 1]);

        let hes: Vec<_> = origins
            .iter()
            .map(|&v| brep.topology.add_half_edge(v))
            .collect();
        let lp = brep.topology.add_loop(&hes);
        let face = brep.topology.add_face(lp, surface_index, orientation);
        // Return (face, up_he, down_he) — the vertical edges at u=b and u=a.
        let up_he = hes[bot.len() - 1];
        let down_he = hes[hes.len() - 1];
        (face, up_he, down_he)
    };

    let (face1, he1_up, he1_down) =
        build_face(brep, v_start_bot, v_split_bottom, v_start_top, v_split_top);
    let (face2, he2_up, he2_down) =
        build_face(brep, v_split_bottom, v_end_bot, v_split_top, v_end_top);

    // Twin the shared vertical split line: face1 goes up at u_split, face2
    // comes back down at u_split.
    brep.topology.add_edge(he1_up, he2_down);
    // For a full face the two sub-faces also share the seam line.
    if is_full_face {
        brep.topology.add_edge(he1_down, he2_up);
    }

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the split line
    brep.geometry.add_curve_3d(Box::new(line.clone()));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Split a cylindrical face along an intersection curve.
///
/// This dispatches to the appropriate split method based on the curve type:
/// - Circle: horizontal split (perpendicular plane intersection)
/// - Line: vertical split (parallel plane intersection)
/// - Sampled: general oblique split via the band machinery (`cyl_band`)
///
/// Faces with wavy (non-constant-v) boundaries — produced by earlier
/// oblique splits — must NEVER reach the legacy rectangular splitters,
/// which infer a full-height rectangle from the v-extremes of the loop and
/// would emit overlapping phantom geometry. When the band machinery
/// declines such a face (curve misses it, or lands on its boundary), the
/// face is returned unsplit instead.
pub fn split_cylindrical_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    segments: u32,
) -> SplitResult {
    let wavy = crate::cyl_band::face_is_wavy_band(brep, face_id);
    // A dense (frozen-polyline) loop must not reach the legacy circle
    // splitter: it re-emits analytic seam loops sampled on the raw
    // `segments` grid, silently thawing a frozen boundary back into one
    // that can never conform with its sag-dense neighbors. The band
    // machinery preserves the loop's own columns. (Line splits stay
    // legacy-first: the legacy line splitter emits canonical dense chains
    // itself and handles full-face seam bookkeeping the band path lacks.)
    let dense = brep
        .topology
        .loop_len(brep.topology.faces[face_id].outer_loop)
        > 6;
    match curve {
        IntersectionCurve::Circle(circle) => {
            if wavy || dense {
                // `require_wavy` only when the face really is wavy: a dense
                // frozen band is rectangular (constant-v rings) and must
                // still be split by the band machinery. When the band path
                // declines (e.g. the circle grazes a band edge), fall
                // through to the legacy splitter rather than giving up.
                if let Some(r) = crate::cyl_band::split_wavy_band_by_circle(
                    brep, face_id, circle, wavy, segments,
                ) {
                    return r;
                }
                if wavy {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            }
            // The legacy splitter reconstructs its sub-faces as degenerate
            // seam loops spanning the FULL circumference — correct only when
            // the input face is itself a full tube. A u-sector rectangle
            // (from earlier axis-parallel line splits) routed through it
            // would balloon into two overlapping full tubes. Gate it to
            // loops whose vertices all sit on one seam angle.
            let is_seam_loop = {
                let face = &brep.topology.faces[face_id];
                if let Some(cyl) = brep.geometry.surfaces[face.surface_index]
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
                {
                    let mut u0: Option<f64> = None;
                    brep.topology.loop_half_edges(face.outer_loop).all(|he| {
                        let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
                        let u = compute_cylinder_u(&p, cyl);
                        match u0 {
                            None => {
                                u0 = Some(u);
                                true
                            }
                            Some(base) => {
                                let mut d = (u - base).rem_euclid(2.0 * std::f64::consts::PI);
                                if d > std::f64::consts::PI {
                                    d = 2.0 * std::f64::consts::PI - d;
                                }
                                d < 1e-6
                            }
                        }
                    })
                } else {
                    false
                }
            };
            let result = if is_seam_loop {
                split_cylindrical_face_by_circle_logged(brep, face_id, circle)
            } else {
                SplitResult {
                    sub_faces: vec![face_id],
                }
            };
            if result.sub_faces.len() >= 2 {
                return result;
            }
            // The legacy splitter only parses degenerate seam loops and
            // 4-corner rectangles; arc-extruded walls carry dense sampled
            // loops it refuses. Retry through the band machinery, which
            // parses any two-chain loop (rectangular included).
            crate::cyl_band::split_wavy_band_by_circle(brep, face_id, circle, false, segments)
                .unwrap_or(result)
        }
        IntersectionCurve::Line(line) => {
            if wavy || dense {
                if let Some(r) = crate::cyl_band::split_wavy_band_by_line(brep, face_id, line, wavy)
                {
                    return r;
                }
                if wavy {
                    return SplitResult {
                        sub_faces: vec![face_id],
                    };
                }
            }
            let result = split_cylindrical_face_by_line(brep, face_id, line, segments);
            if result.sub_faces.len() >= 2 {
                return result;
            }
            crate::cyl_band::split_wavy_band_by_line(brep, face_id, line, false).unwrap_or(result)
        }
        IntersectionCurve::Sampled(points) => {
            // Oblique intersection (e.g. a tilted plane crossing the
            // cylinder in an ellipse): split in (u, v) parameter space via
            // the band machinery.
            match crate::cyl_band::split_cylindrical_face_by_sampled(brep, face_id, points) {
                Some(result) => result,
                None => {
                    // The curve is not a closed single-valued profile (e.g.
                    // a bounded cylinder-cylinder quartic arc) or the face
                    // is outside the band family. Log so the caller sees
                    // the operation failed instead of silently producing a
                    // geometrically wrong result.
                    let loop_len = brep
                        .topology
                        .loop_half_edges(brep.topology.faces[face_id].outer_loop)
                        .count();
                    let holes = brep.topology.faces[face_id].inner_loops.len();
                    eprintln!(
                        "[vcad-kernel-booleans] split_cylindrical_face: Sampled \
                         intersection ({} points) could not be applied to {face_id:?} \
                         ({loop_len}-vertex outer loop, {holes} hole(s)); returning \
                         face unsplit. Downstream boolean result may be incorrect \
                         for this face.",
                        points.len()
                    );
                    SplitResult {
                        sub_faces: vec![face_id],
                    }
                }
            }
        }
        IntersectionCurve::Empty | IntersectionCurve::Point(_) => SplitResult {
            sub_faces: vec![face_id],
        },
        IntersectionCurve::TwoLines(line1, _line2) => {
            // TwoLines should be expanded before calling this function.
            // If we get here, just process the first line.
            split_cylindrical_face(
                brep,
                face_id,
                &IntersectionCurve::Line(line1.clone()),
                segments,
            )
        }
        IntersectionCurve::TwoSampled(_, _) => {
            // TwoSampled is the analytic Steinmetz pair from
            // `ssi::cylinder_cylinder`. Cylinder × cylinder unions are
            // routed by `boolean_op` to a specialized mesh emitter
            // (`cylinder_cylinder_mesh_boolean`) that handles the
            // figure-8 boundary directly, so by the time we reach the
            // BRep splitter pipeline this curve type should already be
            // diverted. Defensive no-op for any caller that didn't
            // pre-filter.
            SplitResult {
                sub_faces: vec![face_id],
            }
        }
    }
}

// =============================================================================
// Circular Face (Disk) Splitting by Line
// =============================================================================

/// Check if a face is a circular disk (a planar face bounded by a single circle).
///
/// A circular disk has:
/// - A planar underlying surface
/// - A single vertex in its outer loop (the seam point on the circle)
/// - No inner loops
pub fn is_circular_disk_face(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    // Must be a plane
    if surface.surface_type() != vcad_kernel_geom::SurfaceKind::Plane {
        return false;
    }

    // Check if it has a single vertex (analytic circular boundary) or a
    // dense frozen ring (canonical polyline from crate::freeze) — every
    // vertex equidistant from the plane origin.
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 && !is_frozen_ring(brep, face_id) {
        return false;
    }

    // No inner loops
    face.inner_loops.is_empty()
}

/// True when the face's outer loop is a dense ring of vertices equidistant
/// from the plane origin — the canonical polyline `crate::freeze` writes in
/// place of an analytic full-circle boundary. 8+ vertices distinguishes a
/// frozen ring from box faces and low-count polygons that happen to be
/// cyclic.
fn is_frozen_ring(brep: &BRepSolid, face_id: FaceId) -> bool {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let plane = match surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
        Some(p) => p,
        None => return false,
    };
    let center = plane.origin;
    let normal = *plane.normal_dir.as_ref();
    let mut radius: Option<f64> = None;
    let mut pts: Vec<Point3> = Vec::new();
    for he in brep.topology.loop_half_edges(face.outer_loop) {
        let p = brep.topology.vertices[brep.topology.half_edges[he].origin].point;
        let r = (p - center).norm();
        match radius {
            None => radius = Some(r),
            Some(r0) => {
                if (r - r0).abs() > 1e-6_f64.max(r0 * 1e-9) {
                    return false;
                }
            }
        }
        pts.push(p);
    }
    if pts.len() < 8 {
        return false;
    }
    // All vertices equidistant is necessary but NOT sufficient: a half-disk
    // produced by a diameter cut has every vertex ON the circle too (the
    // chord endpoints are ring vertices), and treating it as a full disk
    // makes the next same-line split rebuild BOTH halves from the full
    // circle — duplicate faces and phantom slivers. A genuine frozen ring
    // walks the circle in uniform sag-scale steps; any large angular jump
    // between consecutive loop vertices is a chord, not a ring edge.
    let x_axis = {
        let d = pts[0] - center;
        let on = d - d.dot(normal) * normal;
        if on.norm() < 1e-9 {
            return false;
        }
        on.normalize()
    };
    let y_axis = normal.cross(x_axis);
    let ang = |p: &Point3| -> f64 {
        let d = *p - center;
        d.dot(y_axis).atan2(d.dot(x_axis))
    };
    for w in 0..pts.len() {
        let a = ang(&pts[w]);
        let b = ang(&pts[(w + 1) % pts.len()]);
        let mut d = (b - a).rem_euclid(2.0 * std::f64::consts::PI);
        if d > std::f64::consts::PI {
            d = 2.0 * std::f64::consts::PI - d;
        }
        if d > 0.2 {
            return false;
        }
    }
    true
}

/// Get the circle parameters of a circular disk face.
///
/// Returns (center, radius, normal) if the face is a valid circular disk.
pub fn get_disk_circle_params(
    brep: &BRepSolid,
    face_id: FaceId,
) -> Option<(Point3, f64, vcad_kernel_math::Vec3)> {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;

    // Get the seam vertex - this is on the circle at angle 0. A dense
    // frozen ring qualifies too: all its vertices are equidistant from the
    // plane origin, so the first one fixes the radius.
    let loop_hes: Vec<_> = brep.topology.loop_half_edges(face.outer_loop).collect();
    if loop_hes.len() != 1 && !is_frozen_ring(brep, face_id) {
        return None;
    }

    let seam_vertex_id = brep.topology.half_edges[loop_hes[0]].origin;
    let seam_point = brep.topology.vertices[seam_vertex_id].point;

    // Circle center is the plane origin
    let center = plane.origin;
    let radius = (seam_point - center).norm();
    let normal = *plane.normal_dir.as_ref();

    Some((center, radius, normal))
}

/// Split a circular disk face along a line intersection curve.
///
/// When a plane intersects another plane that contains a circular disk,
/// the result is a line that may cross the disk. This function splits
/// the disk into two parts along the line:
///
/// - If the line passes through the center: two half-disks (semicircles)
/// - If the line is a chord: a smaller chord segment + larger segment
///
/// The line must actually cross the disk boundary at two points for
/// splitting to occur.
///
/// Each resulting face has:
/// - A straight edge along the split line
/// - An arc edge along the original circle
pub fn split_circular_face_by_line(
    brep: &mut BRepSolid,
    face_id: FaceId,
    line: &vcad_kernel_geom::Line3d,
    segments: u32,
) -> SplitResult {
    // Get disk parameters
    let (center, radius, normal) = match get_disk_circle_params(brep, face_id) {
        Some(params) => params,
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    let face = &brep.topology.faces[face_id];
    let surface_index = face.surface_index;
    let orientation = face.orientation;

    // Project the line onto the disk's plane to find intersection points with the circle
    // The line-circle intersection in 2D:
    // Circle: |p - center| = radius
    // Line: p = origin + t * direction

    // Find direction perpendicular to line in the plane
    let line_dir = line.direction.normalize();

    // Check if line is parallel to the plane normal (no intersection)
    if line_dir.dot(normal).abs() > 0.999 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Project line onto the plane
    // Find the closest point on the line to the circle center
    let to_center = center - line.origin;
    let t_closest = to_center.dot(line_dir);
    let closest_point = line.origin + t_closest * line_dir;

    // Distance from line to center
    let dist_to_center = (closest_point - center).norm();

    // If line doesn't intersect the circle, no split needed
    if dist_to_center > radius - 1e-9 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Compute intersection points with circle
    // Half-chord length: sqrt(r² - d²)
    let half_chord = (radius * radius - dist_to_center * dist_to_center).sqrt();

    // Intersection points
    let p1 = closest_point - half_chord * line_dir;
    let p2 = closest_point + half_chord * line_dir;

    // Verify both points are on the plane (within tolerance)
    let surface = &brep.geometry.surfaces[surface_index];
    let plane = match surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
        Some(p) => p,
        None => {
            return SplitResult {
                sub_faces: vec![face_id],
            }
        }
    };

    if plane.signed_distance(&p1).abs() > 0.1 || plane.signed_distance(&p2).abs() > 0.1 {
        return SplitResult {
            sub_faces: vec![face_id],
        };
    }

    // Compute angles of intersection points relative to center
    // Use the plane's local coordinate system
    let x_axis = plane.x_dir.normalize();
    let y_axis = plane.y_dir.normalize();

    let to_p1 = p1 - center;
    let to_p2 = p2 - center;

    let angle1 = to_p1.dot(y_axis).atan2(to_p1.dot(x_axis));
    let angle2 = to_p2.dot(y_axis).atan2(to_p2.dot(x_axis));

    // Normalize angles to [0, 2π)
    let angle1 = if angle1 < 0.0 {
        angle1 + 2.0 * std::f64::consts::PI
    } else {
        angle1
    };
    let angle2 = if angle2 < 0.0 {
        angle2 + 2.0 * std::f64::consts::PI
    } else {
        angle2
    };

    // Order angles so we know which arc is which
    let (_start_angle, _end_angle, start_pt, end_pt) = if angle1 < angle2 {
        (angle1, angle2, p1, p2)
    } else {
        (angle2, angle1, p2, p1)
    };

    // Create vertices for the intersection points
    let tolerance = 1e-6;
    let v_start = find_or_create_vertex(brep, &start_pt, tolerance);
    let v_end = find_or_create_vertex(brep, &end_pt, tolerance);

    // Generate arc vertices for both faces
    // Face 1: arc from start_angle to end_angle (shorter arc if < π, longer otherwise)
    // Face 2: arc from end_angle to start_angle (wrapping around 2π)

    // Both arcs travel counterclockwise about the plane frame's normal.
    // Sampling MUST be the canonical absolute grid — the cylinder-wall
    // splitter borders the same circles and has to emit identical points.
    let circle_normal = x_axis.cross(y_axis);
    let arc1_points = canonical_arc_points(
        center,
        radius,
        circle_normal,
        snap_point(start_pt),
        snap_point(end_pt),
        segments,
    );
    let arc2_points = canonical_arc_points(
        center,
        radius,
        circle_normal,
        snap_point(end_pt),
        snap_point(start_pt),
        segments,
    );

    // Create Face 1: arc from start to end + chord from end to start
    // Loop: start → arc points → end → chord → back to start
    let mut face1_verts: Vec<vcad_kernel_topo::VertexId> = Vec::new();
    face1_verts.push(v_start);
    for pt in arc1_points.iter().skip(1).take(arc1_points.len() - 2) {
        face1_verts.push(find_or_create_vertex(brep, pt, tolerance));
    }
    face1_verts.push(v_end);

    // Create half-edges and loop for face 1
    let hes1: Vec<_> = face1_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let loop1 = brep.topology.add_loop(&hes1);
    let face1 = brep.topology.add_face(loop1, surface_index, orientation);

    // Create Face 2: arc from end to start + chord from start to end
    let mut face2_verts: Vec<vcad_kernel_topo::VertexId> = Vec::new();
    face2_verts.push(v_end);
    for pt in arc2_points.iter().skip(1).take(arc2_points.len() - 2) {
        face2_verts.push(find_or_create_vertex(brep, pt, tolerance));
    }
    face2_verts.push(v_start);

    // Create half-edges and loop for face 2
    let hes2: Vec<_> = face2_verts
        .iter()
        .map(|&v| brep.topology.add_half_edge(v))
        .collect();
    let loop2 = brep.topology.add_loop(&hes2);
    let face2 = brep.topology.add_face(loop2, surface_index, orientation);

    // Add twin edges for the chord (shared edge between face1 and face2)
    // In face1, the chord goes from v_end to v_start (last edge)
    // In face2, the chord goes from v_start to v_end (last edge)
    // These are twins
    let chord_he1 = hes1[hes1.len() - 1]; // v_end → v_start in face1
    let chord_he2 = hes2[hes2.len() - 1]; // v_start → v_end in face2
    brep.topology.add_edge(chord_he1, chord_he2);

    // Add faces to shell
    if let Some(shell_id) = brep.topology.faces[face_id].shell {
        brep.topology.shells[shell_id].faces.push(face1);
        brep.topology.shells[shell_id].faces.push(face2);

        brep.topology.faces[face1].shell = Some(shell_id);
        brep.topology.faces[face2].shell = Some(shell_id);

        // Remove original face from shell
        brep.topology.shells[shell_id]
            .faces
            .retain(|&f| f != face_id);
    }

    // Remove the original face
    brep.topology.faces.remove(face_id);

    // Add 3D curve for the split line (chord)
    brep.geometry
        .add_curve_3d(Box::new(vcad_kernel_geom::Line3d::from_points(
            start_pt, end_pt,
        )));

    SplitResult {
        sub_faces: vec![face1, face2],
    }
}

/// Split a circular disk face along an intersection curve.
///
/// Dispatches to the appropriate method based on curve type:
/// - Line: splits disk into two arc-bounded segments
/// - Circle: not applicable (circle on circle is degenerate)
/// - Other: no split
pub fn split_circular_disk_face(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve: &IntersectionCurve,
    segments: u32,
) -> SplitResult {
    match curve {
        IntersectionCurve::Line(line) => split_circular_face_by_line(brep, face_id, line, segments),
        IntersectionCurve::TwoLines(line1, line2) => {
            // Split by the first line, then by the second
            let result1 = split_circular_face_by_line(brep, face_id, line1, segments);
            if result1.sub_faces.len() < 2 {
                return result1;
            }
            // Now split each resulting face by the second line
            let mut all_faces = Vec::new();
            for &fid in &result1.sub_faces {
                // Check if this face is still a circular disk (it won't be after the first split)
                // For non-disk faces after first split, we'd need polygon splitting
                // For now, just add them as-is
                if is_circular_disk_face(brep, fid) {
                    let result2 = split_circular_face_by_line(brep, fid, line2, segments);
                    all_faces.extend(result2.sub_faces);
                } else {
                    // The face is now a chord-segment, not a full disk
                    // Try to split it as a planar face by the line
                    let result2 = split_planar_face(
                        brep,
                        fid,
                        &IntersectionCurve::Line(line2.clone()),
                        &Point3::origin(),
                        &Point3::origin(),
                        segments,
                        false,
                    );
                    all_faces.extend(result2.sub_faces);
                }
            }
            SplitResult {
                sub_faces: all_faces,
            }
        }
        _ => {
            // No split for other curve types on circular faces
            SplitResult {
                sub_faces: vec![face_id],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_find_closest_edge() {
        let square = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        // Point on the bottom edge
        let edge = find_closest_edge(&square, &Point3::new(5.0, 0.0, 0.0));
        assert_eq!(edge, 0);

        // Point on the right edge
        let edge = find_closest_edge(&square, &Point3::new(10.0, 5.0, 0.0));
        assert_eq!(edge, 1);

        // Point on the top edge
        let edge = find_closest_edge(&square, &Point3::new(5.0, 10.0, 0.0));
        assert_eq!(edge, 2);

        // Point on the left edge
        let edge = find_closest_edge(&square, &Point3::new(0.0, 5.0, 0.0));
        assert_eq!(edge, 3);
    }

    #[test]
    fn test_point_to_segment_dist() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(10.0, 0.0, 0.0);

        // Point on the segment
        assert!(point_to_segment_dist(&Point3::new(5.0, 0.0, 0.0), &a, &b) < 1e-10);

        // Point above the segment midpoint
        let dist = point_to_segment_dist(&Point3::new(5.0, 3.0, 0.0), &a, &b);
        assert!((dist - 3.0).abs() < 1e-10);

        // Point beyond endpoint
        let dist = point_to_segment_dist(&Point3::new(15.0, 0.0, 0.0), &a, &b);
        assert!((dist - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_split_face_cube() {
        let mut brep = make_cube(10.0, 10.0, 10.0);

        // Find the bottom face (z=0)
        let bottom_face = brep
            .topology
            .faces
            .iter()
            .find(|(fid, _)| {
                let verts: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(brep.topology.faces[*fid].outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                verts.iter().all(|v| v.z.abs() < 1e-10)
            })
            .map(|(fid, _)| fid);

        if let Some(face_id) = bottom_face {
            let initial_face_count = brep.topology.faces.len();

            // Split the bottom face with a line from (5,0,0) to (5,10,0)
            let entry = Point3::new(5.0, 0.0, 0.0);
            let exit = Point3::new(5.0, 10.0, 0.0);
            let curve = IntersectionCurve::Line(vcad_kernel_geom::Line3d {
                origin: entry,
                direction: exit - entry,
            });

            let result = split_face_by_curve(&mut brep, face_id, &curve, &entry, &exit);

            // Should produce 2 sub-faces
            assert_eq!(result.sub_faces.len(), 2);

            // Total faces should increase by 1 (original removed, 2 new added: +2 - 1 = +1)
            assert_eq!(brep.topology.faces.len(), initial_face_count + 1);
        }
    }

    /// Test splitting a cube's z=0 face by a circle centered at its corner.
    /// This is the exact scenario for cube-cylinder difference at origin.
    #[test]
    fn test_split_z0_face_by_corner_circle() {
        use vcad_kernel_geom::Circle3d;

        let mut brep = make_cube(20.0, 20.0, 20.0);
        println!("\n=== Test: Split z=0 face by corner circle ===");

        // Find the z=0 face (bottom)
        let z0_face = brep
            .topology
            .faces
            .iter()
            .find(|(fid, _)| {
                let face = &brep.topology.faces[*fid];
                let verts: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                // z=0 face has all vertices with z ≈ 0
                verts.iter().all(|v| v.z.abs() < 0.01)
            })
            .map(|(fid, _)| fid);

        let z0_face_id = z0_face.expect("Should find z=0 face");
        println!("Found z=0 face: {:?}", z0_face_id);

        // Print face vertices
        let face = &brep.topology.faces[z0_face_id];
        let verts: Vec<Point3> = brep
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
            .collect();
        println!("Face vertices ({}):", verts.len());
        for (i, v) in verts.iter().enumerate() {
            println!("  v{}: ({:.1}, {:.1}, {:.1})", i, v.x, v.y, v.z);
        }

        // Create circle centered at corner (0,0,0) with radius 10
        let circle = Circle3d::with_normal(
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            vcad_kernel_math::Vec3::new(0.0, 0.0, 1.0),
        );
        println!(
            "Circle: center=({:.1},{:.1},{:.1}), r={:.1}",
            circle.center.x, circle.center.y, circle.center.z, circle.radius
        );

        // Check if circle is partially inside
        let is_partial = circle_partially_inside_polygon(&verts, &circle);
        println!("Circle partially inside polygon: {}", is_partial);

        // Try to split
        let initial_faces = brep.topology.faces.len();
        let result = split_planar_face_by_circle(&mut brep, z0_face_id, &circle, 32, true);
        println!("Split result: {} sub-faces", result.sub_faces.len());
        println!(
            "Total faces after: {} (was {})",
            brep.topology.faces.len(),
            initial_faces
        );

        // Print result face info
        for &fid in &result.sub_faces {
            if brep.topology.faces.contains_key(fid) {
                let f = &brep.topology.faces[fid];
                let vs: Vec<Point3> = brep
                    .topology
                    .loop_half_edges(f.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();

                let min_x = vs.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
                let max_x = vs.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
                let min_y = vs.iter().map(|v| v.y).fold(f64::INFINITY, f64::min);
                let max_y = vs.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);

                println!(
                    "  {:?}: {} verts, x=[{:.1},{:.1}], y=[{:.1},{:.1}]",
                    fid,
                    vs.len(),
                    min_x,
                    max_x,
                    min_y,
                    max_y
                );
            }
        }

        // The split should produce 2 faces
        assert!(
            result.sub_faces.len() >= 2,
            "Expected at least 2 sub-faces from arc split, got {}",
            result.sub_faces.len()
        );
    }
}

/// Sub-resolution fallback for thin faces: when a sampled cut curve crosses
/// a face narrower than its own sample spacing, the trim finds no interval,
/// but the two exact crossing vertices already sit ON the face's boundary
/// (inserted by the adjacent faces' splits against the same surface pair).
/// Detect exactly two such vertices and split along the chord between them.
pub(crate) fn split_thin_face_at_curve_vertices(
    brep: &mut BRepSolid,
    face_id: FaceId,
    curve_pts: &[Point3],
) -> Option<SplitResult> {
    if curve_pts.len() < 4 {
        return None;
    }
    let face = &brep.topology.faces[face_id];
    if !face.inner_loops.is_empty() {
        return None;
    }
    let loop_verts: Vec<Point3> = brep
        .topology
        .loop_half_edges(face.outer_loop)
        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
        .collect();
    let n = loop_verts.len();
    if n < 4 {
        return None;
    }
    // Distance from a point to the sampled polyline. The crossing vertices
    // lie ON the true curve, which sags up to ~5 µm off the polyline chords.
    let dist_to_curve = |p: &Point3| -> f64 {
        let mut best = f64::INFINITY;
        for w in curve_pts.windows(2) {
            let ab = w[1] - w[0];
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - w[0]).dot(ab) / len2).clamp(0.0, 1.0)
            };
            best = best.min((*p - (w[0] + t * ab)).norm());
        }
        best
    };
    // Candidate endpoints: TOPOLOGY vertices (they were inserted by the
    // neighboring faces' splits and are not yet part of THIS face's loop)
    // lying on both the cut curve and this face's boundary polyline, but
    // not coincident with the loop's own corners.
    const ON_CURVE_TOL: f64 = 6e-3;
    let dist_to_boundary = |p: &Point3| -> f64 {
        let mut best = f64::INFINITY;
        for k in 0..n {
            let e0 = loop_verts[k];
            let e1 = loop_verts[(k + 1) % n];
            let ab = e1 - e0;
            let len2 = ab.norm_squared();
            let t = if len2 < 1e-18 {
                0.0
            } else {
                ((*p - e0).dot(ab) / len2).clamp(0.0, 1.0)
            };
            best = best.min((*p - (e0 + t * ab)).norm());
        }
        best
    };
    let mut hits: Vec<Point3> = Vec::new();
    for (_vid, v) in &brep.topology.vertices {
        let p = v.point;
        if dist_to_curve(&p) < ON_CURVE_TOL
            && dist_to_boundary(&p) < ON_CURVE_TOL
            && !loop_verts.iter().any(|q| (*q - p).norm() < 1e-6)
            && !hits.iter().any(|q| (*q - p).norm() < 1e-6)
        {
            hits.push(p);
        }
    }
    split_dbg!("thin-face fallback: {face_id:?} nv={n} hits {}", hits.len());
    if hits.len() != 2 {
        return None;
    }
    let (a, b) = (hits[0], hits[1]);
    let chord = (b - a).norm();
    // Sag-adaptive curves sample non-uniformly; gauge the chord against the
    // LARGEST inter-sample step so a short first segment (high-curvature
    // start) can't reject a chord that is a normal interior step.
    let sample_step = curve_pts
        .windows(2)
        .map(|w| (w[1] - w[0]).norm())
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    if chord < 1e-6 || chord > 2.0 * sample_step {
        return None;
    }
    // Chord midpoint must be strictly inside the face (not along an edge).
    let mid = Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z));
    let mut min_d = f64::INFINITY;
    for k in 0..n {
        let e0 = loop_verts[k];
        let e1 = loop_verts[(k + 1) % n];
        let ab = e1 - e0;
        let len2 = ab.norm_squared();
        let t = if len2 < 1e-18 {
            0.0
        } else {
            ((mid - e0).dot(ab) / len2).clamp(0.0, 1.0)
        };
        min_d = min_d.min((mid - (e0 + t * ab)).norm());
    }
    if min_d < 1e-7 || !crate::trim::point_in_face(brep, face_id, &mid) {
        return None;
    }
    let result = split_face_by_curve(brep, face_id, &IntersectionCurve::Empty, &a, &b);
    if result.sub_faces.len() >= 2 {
        Some(result)
    } else {
        None
    }
}
