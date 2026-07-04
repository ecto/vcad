//! Fast path for booleans whose operand boundaries do not cross.
//!
//! When surface-surface intersection produces no face splits, the two
//! solids' boundaries never cross: the operands are either fully disjoint
//! or one is nested inside the other. The general pipeline still tessellates
//! both operands and ray-classifies *every* face — O(|A|·|B|) work that
//! dominates document evaluation when thousands of disjoint solids are
//! unioned (e.g. chip-layer islands from the GDSII bridge). This module
//! resolves those cases with two point-in-solid queries instead.
//!
//! Contact without crossing (two boxes sharing a wall) must still use the
//! general pipeline so coincident faces are deduplicated; [`boundaries_touch`]
//! detects coincident-surface candidate pairs whose 2D regions actually
//! touch and vetoes the fast path.

use vcad_kernel_geom::Plane;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::tessellate_brep;
use vcad_kernel_topo::FaceId;

use crate::api::{BooleanOp, BooleanResult};
use crate::mesh::{empty_brep, point_in_mesh};
use crate::sew;

/// Geometric tolerance for contact detection, matched to the sew tolerance.
const TOL: f64 = 1e-6;

/// How the two non-crossing operands relate spatially.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Containment {
    /// Every point of A lies inside B.
    pub a_in_b: bool,
    /// Every point of B lies inside A.
    pub b_in_a: bool,
}

/// Detect whether any candidate face pair has coincident surfaces whose
/// bounded regions actually touch or overlap. Such contact (shared walls,
/// stacked boxes) needs the general pipeline's coincident-face handling.
///
/// Conservative: any coincident pair that cannot be cheaply proven disjoint
/// reports contact.
pub(crate) fn boundaries_touch(a: &BRepSolid, b: &BRepSolid, pairs: &[(FaceId, FaceId)]) -> bool {
    for &(fa, fb) in pairs {
        let surf_a = &a.geometry.surfaces[a.topology.faces[fa].surface_index];
        let surf_b = &b.geometry.surfaces[b.topology.faces[fb].surface_index];

        let (Some(plane_a), Some(plane_b)) = (
            surf_a.as_any().downcast_ref::<Plane>(),
            surf_b.as_any().downcast_ref::<Plane>(),
        ) else {
            // Non-planar candidate pair. SSI found no crossing, but two
            // coincident curved surfaces (coaxial equal-radius cylinders,
            // concentric spheres) can share area without an intersection
            // curve. Be conservative when the surfaces look coincident.
            if curved_surfaces_possibly_coincident(surf_a.as_ref(), surf_b.as_ref()) {
                return true;
            }
            continue;
        };

        let n_a = plane_a.normal_dir.into_inner();
        let n_b = plane_b.normal_dir.into_inner();
        if n_a.cross(n_b).norm() > 1e-9 {
            continue; // Not parallel → SSI already proved no crossing.
        }
        if (plane_b.origin - plane_a.origin).dot(n_a).abs() > TOL {
            continue; // Parallel but offset → disjoint planes.
        }

        // Coincident planes: do the bounded regions touch?
        if planar_regions_touch(a, fa, b, fb, &n_a) {
            return true;
        }
    }
    false
}

/// Conservative coincidence test for curved surface pairs: reports `true`
/// (possible contact) when both surfaces are curved and of the same kind.
/// Planar-vs-curved pairs cannot share area without an intersection curve.
fn curved_surfaces_possibly_coincident(
    surf_a: &dyn vcad_kernel_geom::Surface,
    surf_b: &dyn vcad_kernel_geom::Surface,
) -> bool {
    use vcad_kernel_geom::SurfaceKind;
    let ka = surf_a.surface_type();
    let kb = surf_b.surface_type();
    if ka == SurfaceKind::Plane || kb == SurfaceKind::Plane {
        return false;
    }
    ka == kb
}

/// Outer-loop vertices of a face.
fn face_outer_verts(brep: &BRepSolid, face: FaceId) -> Vec<Point3> {
    let topo = &brep.topology;
    topo.loop_half_edges(topo.faces[face].outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect()
}

/// Do two coplanar face regions touch or overlap? Projects both outer loops
/// into the shared plane and tests vertex containment (boundary-inclusive)
/// plus edge-edge intersection. Inner loops are ignored, which errs on the
/// side of reporting contact (safe: the general pipeline takes over).
fn planar_regions_touch(
    a: &BRepSolid,
    fa: FaceId,
    b: &BRepSolid,
    fb: FaceId,
    normal: &Vec3,
) -> bool {
    let verts_a = face_outer_verts(a, fa);
    let verts_b = face_outer_verts(b, fb);
    if verts_a.len() < 3 || verts_b.len() < 3 {
        // Degenerate loops (analytic disk caps) — be conservative.
        return true;
    }

    // Build a shared 2D frame on the plane.
    let u = pick_tangent(normal);
    let v = normal.cross(u);
    let origin = verts_a[0];
    let proj = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u), d.dot(v))
    };
    let poly_a: Vec<(f64, f64)> = verts_a.iter().map(&proj).collect();
    let poly_b: Vec<(f64, f64)> = verts_b.iter().map(&proj).collect();

    // Any vertex of one polygon inside (or on the boundary of) the other?
    if poly_b.iter().any(|p| point_in_poly_inclusive(*p, &poly_a))
        || poly_a.iter().any(|p| point_in_poly_inclusive(*p, &poly_b))
    {
        return true;
    }

    // Any edge crossing (including touching within tolerance)?
    let na = poly_a.len();
    let nb = poly_b.len();
    for i in 0..na {
        let a1 = poly_a[i];
        let a2 = poly_a[(i + 1) % na];
        for j in 0..nb {
            let b1 = poly_b[j];
            let b2 = poly_b[(j + 1) % nb];
            if segments_touch(a1, a2, b1, b2) {
                return true;
            }
        }
    }
    false
}

/// A unit vector orthogonal to `n`.
fn pick_tangent(n: &Vec3) -> Vec3 {
    let candidate = if n.x.abs() < 0.9 {
        Vec3::x()
    } else {
        Vec3::y()
    };
    let t = candidate - candidate.dot(n) * *n;
    t.normalize()
}

/// Boundary-inclusive point-in-polygon: true when `p` is inside the polygon
/// or within [`TOL`] of its boundary.
fn point_in_poly_inclusive(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    // On-boundary check.
    for i in 0..n {
        if point_segment_dist_sq(p, poly[i], poly[(i + 1) % n]) < TOL * TOL {
            return true;
        }
    }
    // Ray-cast parity.
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > p.1) != (yj > p.1)) && (p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Squared distance from point to segment in 2D.
fn point_segment_dist_sq(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq < 1e-30 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    let (ex, ey) = (p.0 - cx, p.1 - cy);
    ex * ex + ey * ey
}

/// Do two 2D segments intersect or come within [`TOL`] of each other?
fn segments_touch(a1: (f64, f64), a2: (f64, f64), b1: (f64, f64), b2: (f64, f64)) -> bool {
    let d1 = (a2.0 - a1.0, a2.1 - a1.1);
    let d2 = (b2.0 - b1.0, b2.1 - b1.1);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() > 1e-12 {
        let d = (b1.0 - a1.0, b1.1 - a1.1);
        let t = (d.0 * d2.1 - d.1 * d2.0) / denom;
        let u = (d.0 * d1.1 - d.1 * d1.0) / denom;
        if (-1e-9..=1.0 + 1e-9).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
            return true;
        }
    }
    // Parallel / near-parallel: endpoint-to-segment proximity.
    point_segment_dist_sq(a1, b1, b2) < TOL * TOL
        || point_segment_dist_sq(a2, b1, b2) < TOL * TOL
        || point_segment_dist_sq(b1, a1, a2) < TOL * TOL
        || point_segment_dist_sq(b2, a1, a2) < TOL * TOL
}

/// Resolve mutual containment of two solids whose boundaries provably do
/// not cross or touch. Returns `None` when a robust answer couldn't be
/// established (caller falls back to the general pipeline).
pub(crate) fn resolve_containment(
    a: &BRepSolid,
    b: &BRepSolid,
    segments: u32,
) -> Option<Containment> {
    let a_in_b = solid_sample_in_solid(a, b, segments)?;
    let b_in_a = solid_sample_in_solid(b, a, segments)?;
    Some(Containment { a_in_b, b_in_a })
}

/// Is (a sample point of) `inner` inside `outer`? Because the boundaries do
/// not cross, one sample decides the whole solid. Tries several boundary
/// vertices of `inner`, skipping ones that produce degenerate ray casts.
fn solid_sample_in_solid(inner: &BRepSolid, outer: &BRepSolid, segments: u32) -> Option<bool> {
    const MAX_ATTEMPTS: usize = 8;
    for (_, v) in inner.topology.vertices.iter().take(MAX_ATTEMPTS) {
        match point_in_brep(&v.point, outer) {
            RayParity::Inside => return Some(true),
            RayParity::Outside => return Some(false),
            RayParity::Degenerate => continue,
            RayParity::NeedsMesh => {
                // Curved faces along the ray — fall back to the mesh test.
                let mesh = tessellate_brep(outer, segments);
                return Some(point_in_mesh(&v.point, &mesh));
            }
        }
    }
    None
}

enum RayParity {
    Inside,
    Outside,
    /// The ray grazed an edge/vertex or the query point sits on a face.
    Degenerate,
    /// A curved face intersects the ray column; exact planar parity
    /// counting doesn't apply.
    NeedsMesh,
}

/// Exact +Z ray-parity test against a planar-faced B-rep. Faces are
/// prefiltered by AABB in the XY column above `p`.
fn point_in_brep(p: &Point3, solid: &BRepSolid) -> RayParity {
    let topo = &solid.topology;
    let mut crossings = 0u32;

    for (_fid, face) in topo.faces.iter() {
        // Cheap XY-column prefilter from loop vertices.
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut max_z = f64::NEG_INFINITY;
        let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for loop_id in loops {
            for he in topo.loop_half_edges(loop_id) {
                let q = topo.vertices[topo.half_edges[he].origin].point;
                min_x = min_x.min(q.x);
                max_x = max_x.max(q.x);
                min_y = min_y.min(q.y);
                max_y = max_y.max(q.y);
                max_z = max_z.max(q.z);
            }
        }
        if p.x < min_x - TOL
            || p.x > max_x + TOL
            || p.y < min_y - TOL
            || p.y > max_y + TOL
            || p.z > max_z + TOL
        {
            continue;
        }

        let surface = &solid.geometry.surfaces[face.surface_index];
        let Some(plane) = surface.as_any().downcast_ref::<Plane>() else {
            return RayParity::NeedsMesh;
        };
        // Degenerate loops (analytic disk caps) have vertex AABBs that
        // don't bound the face; punt to the mesh path.
        if topo.loop_len(face.outer_loop) < 3 {
            return RayParity::NeedsMesh;
        }

        let n = plane.normal_dir.into_inner();
        if n.z.abs() < 1e-12 {
            // Ray parallel to the face plane. If the query point lies in
            // the plane it may run along the face — degenerate.
            if (p - plane.origin).dot(n).abs() < TOL {
                return RayParity::Degenerate;
            }
            continue;
        }

        // Intersection of the vertical ray with the face plane.
        let t = (plane.origin - p).dot(n) / n.z;
        if t < -TOL {
            continue; // Below the query point.
        }

        // Containment of (p.x, p.y) in the face polygon, projected to XY
        // (valid because the plane is not vertical).
        let hit = (p.x, p.y);
        let outer_xy: Vec<(f64, f64)> = topo
            .loop_half_edges(face.outer_loop)
            .map(|he| {
                let q = topo.vertices[topo.half_edges[he].origin].point;
                (q.x, q.y)
            })
            .collect();
        match xy_containment(hit, &outer_xy) {
            XyContainment::Boundary => return RayParity::Degenerate,
            XyContainment::Outside => continue,
            XyContainment::Inside => {}
        }
        let mut in_hole = false;
        for &inner in &face.inner_loops {
            let hole_xy: Vec<(f64, f64)> = topo
                .loop_half_edges(inner)
                .map(|he| {
                    let q = topo.vertices[topo.half_edges[he].origin].point;
                    (q.x, q.y)
                })
                .collect();
            match xy_containment(hit, &hole_xy) {
                XyContainment::Boundary => return RayParity::Degenerate,
                XyContainment::Inside => {
                    in_hole = true;
                    break;
                }
                XyContainment::Outside => {}
            }
        }
        if in_hole {
            continue;
        }

        if t < TOL {
            // The query point lies on this face — on the boundary.
            return RayParity::Degenerate;
        }
        crossings += 1;
    }

    if crossings % 2 == 1 {
        RayParity::Inside
    } else {
        RayParity::Outside
    }
}

enum XyContainment {
    Inside,
    Outside,
    Boundary,
}

/// Classify a 2D point against a polygon: near-boundary (within [`TOL`]),
/// strictly inside, or outside.
fn xy_containment(p: (f64, f64), poly: &[(f64, f64)]) -> XyContainment {
    let n = poly.len();
    if n < 3 {
        return XyContainment::Outside;
    }
    for i in 0..n {
        if point_segment_dist_sq(p, poly[i], poly[(i + 1) % n]) < TOL * TOL {
            return XyContainment::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > p.1) != (yj > p.1)) && (p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        XyContainment::Inside
    } else {
        XyContainment::Outside
    }
}

/// Build the boolean result for two non-crossing, non-touching solids from
/// their containment relation.
pub(crate) fn no_crossing_result(
    a: BRepSolid,
    b: BRepSolid,
    op: BooleanOp,
    rel: Containment,
) -> BooleanResult {
    let all_faces = |s: &BRepSolid| -> Vec<FaceId> { s.topology.faces.keys().collect() };
    match op {
        BooleanOp::Union => {
            if rel.a_in_b {
                BooleanResult::BRep(Box::new(b))
            } else if rel.b_in_a {
                BooleanResult::BRep(Box::new(a))
            } else {
                // Disjoint (or B nested in a cavity of A): keep both shells.
                let fa = all_faces(&a);
                let fb = all_faces(&b);
                BooleanResult::BRep(Box::new(sew::sew_faces(&a, &fa, &b, &fb, false, TOL)))
            }
        }
        BooleanOp::Difference => {
            if rel.a_in_b {
                BooleanResult::BRep(Box::new(empty_brep()))
            } else if rel.b_in_a {
                // B carves a cavity inside A: keep A plus B's faces reversed.
                let fa = all_faces(&a);
                let fb = all_faces(&b);
                BooleanResult::BRep(Box::new(sew::sew_faces(&a, &fa, &b, &fb, true, TOL)))
            } else {
                BooleanResult::BRep(Box::new(a))
            }
        }
        BooleanOp::Intersection => {
            if rel.a_in_b {
                BooleanResult::BRep(Box::new(a))
            } else if rel.b_in_a {
                BooleanResult::BRep(Box::new(b))
            } else {
                BooleanResult::BRep(Box::new(empty_brep()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::boolean_op;
    use vcad_kernel_primitives::make_cube;

    fn translated(mut s: BRepSolid, d: Vec3) -> BRepSolid {
        let t = vcad_kernel_math::Transform::translation(d.x, d.y, d.z);
        for (_, v) in s.topology.vertices.iter_mut() {
            v.point = t.apply_point(&v.point);
        }
        s.geometry.surfaces = s
            .geometry
            .surfaces
            .drain(..)
            .map(|surf| surf.transform(&t))
            .collect();
        s
    }

    fn volume(s: &BRepSolid) -> f64 {
        let mesh = tessellate_brep(s, 32);
        let verts = &mesh.vertices;
        let mut vol = 0.0;
        for tri in mesh.indices.chunks(3) {
            let i = [
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            ];
            let v: Vec<[f64; 3]> = i
                .iter()
                .map(|&k| [verts[k] as f64, verts[k + 1] as f64, verts[k + 2] as f64])
                .collect();
            vol += v[0][0] * (v[1][1] * v[2][2] - v[2][1] * v[1][2])
                - v[1][0] * (v[0][1] * v[2][2] - v[2][1] * v[0][2])
                + v[2][0] * (v[0][1] * v[1][2] - v[1][1] * v[0][2]);
        }
        (vol / 6.0).abs()
    }

    #[test]
    fn disjoint_union_keeps_both_shells() {
        // AABBs overlap (diagonal offset in x only), geometry disjoint.
        let a = make_cube(10.0, 10.0, 10.0);
        let b = translated(make_cube(10.0, 10.0, 10.0), Vec3::new(20.0, 0.5, 0.5));
        let result = boolean_op(&a, &b, BooleanOp::Union, 16)
            .into_brep()
            .unwrap();
        assert!((volume(&result) - 2000.0).abs() < 1.0);
        assert_eq!(result.topology.faces.len(), 12);
    }

    #[test]
    fn nested_union_returns_outer() {
        let a = make_cube(10.0, 10.0, 10.0);
        let b = translated(make_cube(2.0, 2.0, 2.0), Vec3::new(4.0, 4.0, 4.0));
        let result = boolean_op(&a, &b, BooleanOp::Union, 16)
            .into_brep()
            .unwrap();
        assert!((volume(&result) - 1000.0).abs() < 1.0);
    }

    #[test]
    fn nested_difference_carves_cavity() {
        let a = make_cube(10.0, 10.0, 10.0);
        let b = translated(make_cube(2.0, 2.0, 2.0), Vec3::new(4.0, 4.0, 4.0));
        let result = boolean_op(&a, &b, BooleanOp::Difference, 16)
            .into_brep()
            .unwrap();
        assert!((volume(&result) - (1000.0 - 8.0)).abs() < 1.0);
    }

    #[test]
    fn disjoint_intersection_is_empty() {
        let a = make_cube(10.0, 10.0, 10.0);
        let b = translated(make_cube(10.0, 10.0, 10.0), Vec3::new(20.0, 0.5, 0.5));
        let result = boolean_op(&a, &b, BooleanOp::Intersection, 16)
            .into_brep()
            .unwrap();
        assert_eq!(result.topology.faces.len(), 0);
    }

    #[test]
    fn touching_boxes_still_use_general_pipeline() {
        // Boxes sharing a full wall: contact must be detected so the
        // general pipeline can deduplicate the coincident faces.
        let a = make_cube(10.0, 10.0, 10.0);
        let b = translated(make_cube(10.0, 10.0, 10.0), Vec3::new(10.0, 0.0, 0.0));
        let pairs = crate::bbox::find_candidate_face_pairs(&a, &b);
        assert!(boundaries_touch(&a, &b, &pairs));

        let result = boolean_op(&a, &b, BooleanOp::Union, 16)
            .into_brep()
            .unwrap();
        assert!((volume(&result) - 2000.0).abs() < 1.0);
    }

    #[test]
    fn point_in_brep_basic() {
        let cube = make_cube(10.0, 10.0, 10.0);
        assert!(matches!(
            point_in_brep(&Point3::new(5.0, 5.0, 5.0), &cube),
            RayParity::Inside
        ));
        assert!(matches!(
            point_in_brep(&Point3::new(15.0, 5.0, 5.0), &cube),
            RayParity::Outside
        ));
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use crate::api::boolean_op;
    use vcad_kernel_sketch_probe::*;

    /// Two interdigitated comb prisms, geometrically disjoint but with
    /// heavily overlapping AABBs and coplanar caps — the GDS island shape.
    #[test]
    fn interdigitated_combs_take_fast_path() {
        let comb = |ox: f64, flip: bool| -> BRepSolid {
            // Comb: base bar plus 3 teeth.
            let mut pts: Vec<(f64, f64)> = Vec::new();
            if !flip {
                pts.push((0.0, 0.0));
                pts.push((1.0, 0.0));
                for i in 0..3 {
                    let y0 = 1.0 + i as f64 * 2.0;
                    pts.push((1.0, y0));
                    pts.push((5.0, y0));
                    pts.push((5.0, y0 + 1.0));
                    pts.push((1.0, y0 + 1.0));
                }
                pts.push((1.0, 8.0));
                pts.push((0.0, 8.0));
            } else {
                pts.push((6.0, 0.0));
                pts.push((7.0, 0.0));
                pts.push((7.0, 8.0));
                pts.push((6.0, 8.0));
                for i in (0..3).rev() {
                    // Teeth sit inside A's notches (y gaps (2,3), (4,5),
                    // (6,7)) with 0.25 clearance on every side.
                    let y0 = 2.25 + i as f64 * 2.0;
                    pts.push((6.0, y0 + 0.5));
                    pts.push((2.0, y0 + 0.5));
                    pts.push((2.0, y0));
                    pts.push((6.0, y0));
                }
            }
            let pts: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| (x + ox, y)).collect();
            prism(&pts, 2.0)
        };
        let a = comb(0.0, false);
        let b = comb(0.0, true);
        eprintln!("vol a = {}, vol b = {}", volume(&a), volume(&b));
        let result = boolean_op(&a, &b, BooleanOp::Union, 16)
            .into_brep()
            .unwrap();
        eprintln!(
            "vol result = {}, faces = {}",
            volume(&result),
            result.topology.faces.len()
        );
        let expected = volume(&a) + volume(&b);
        assert!((volume(&result) - expected).abs() < 1e-3 * expected);
        // Fast path keeps faces unfragmented: exact sum of input faces.
        assert_eq!(
            result.topology.faces.len(),
            a.topology.faces.len() + b.topology.faces.len(),
            "expected no-crossing fast path (no phantom splits)"
        );
    }

    fn volume(s: &BRepSolid) -> f64 {
        let mesh = tessellate_brep(s, 16);
        let verts = &mesh.vertices;
        let mut vol = 0.0;
        for tri in mesh.indices.chunks(3) {
            let i = [
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            ];
            let v: Vec<[f64; 3]> = i
                .iter()
                .map(|&k| [verts[k] as f64, verts[k + 1] as f64, verts[k + 2] as f64])
                .collect();
            vol += v[0][0] * (v[1][1] * v[2][2] - v[2][1] * v[1][2])
                - v[1][0] * (v[0][1] * v[2][2] - v[2][1] * v[0][2])
                + v[2][0] * (v[0][1] * v[1][2] - v[1][1] * v[0][2]);
        }
        (vol / 6.0).abs()
    }
}

/// Test-only prism builder from a 2D polygon (CCW), extruded along +Z.
#[cfg(test)]
mod vcad_kernel_sketch_probe {
    use vcad_kernel_geom::{GeometryStore, Plane};
    use vcad_kernel_topo::{Orientation, ShellType, Topology};

    pub use vcad_kernel_primitives::BRepSolid;

    pub fn prism(pts: &[(f64, f64)], h: f64) -> BRepSolid {
        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();
        let n = pts.len();
        let bot: Vec<_> = pts
            .iter()
            .map(|&(x, y)| topo.add_vertex(vcad_kernel_math::Point3::new(x, y, 0.0)))
            .collect();
        let top: Vec<_> = pts
            .iter()
            .map(|&(x, y)| topo.add_vertex(vcad_kernel_math::Point3::new(x, y, h)))
            .collect();
        let mut faces = Vec::new();
        let mut he_pairs: std::collections::HashMap<(u64, u64), vcad_kernel_topo::HalfEdgeId> =
            Default::default();
        let key = |v: vcad_kernel_topo::VertexId| -> u64 {
            // slotmap keys are unique; hash via Debug format
            let s = format!("{v:?}");
            let mut acc = 0u64;
            for b in s.bytes() {
                acc = acc.wrapping_mul(131).wrapping_add(b as u64);
            }
            acc
        };
        let mut mk_face = |topo: &mut Topology,
                           geom: &mut GeometryStore,
                           ring: &[vcad_kernel_topo::VertexId],
                           normal_hint: Option<vcad_kernel_math::Vec3>|
         -> vcad_kernel_topo::FaceId {
            let p0 = topo.vertices[ring[0]].point;
            let p1 = topo.vertices[ring[1]].point;
            let p2 = topo.vertices[ring[2]].point;
            let plane = match normal_hint {
                Some(nrm) => Plane::from_normal(p0, nrm),
                None => Plane::new(p0, p1 - p0, p2 - p1),
            };
            let sidx = geom.add_surface(Box::new(plane));
            let hes: Vec<_> = ring.iter().map(|&v| topo.add_half_edge(v)).collect();
            for (i, &he) in hes.iter().enumerate() {
                let a = ring[i];
                let b = ring[(i + 1) % ring.len()];
                if let Some(&other) = he_pairs.get(&(key(b), key(a))) {
                    topo.add_edge(he, other);
                } else {
                    he_pairs.insert((key(a), key(b)), he);
                }
            }
            let l = topo.add_loop(&hes);
            let f = topo.add_face(l, sidx, Orientation::Forward);
            faces.push(f);
            f
        };
        for i in 0..n {
            let j = (i + 1) % n;
            mk_face(
                &mut topo,
                &mut geom,
                &[bot[i], bot[j], top[j], top[i]],
                None,
            );
        }
        let bot_rev: Vec<_> = bot.iter().rev().copied().collect();
        mk_face(
            &mut topo,
            &mut geom,
            &bot_rev,
            Some(-vcad_kernel_math::Vec3::z()),
        );
        mk_face(
            &mut topo,
            &mut geom,
            &top,
            Some(vcad_kernel_math::Vec3::z()),
        );
        let shell = topo.add_shell(faces, ShellType::Outer);
        let solid_id = topo.add_solid(shell);
        BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        }
    }
}
