//! Trim vertex computation for fillet/chamfer operations.

use std::collections::HashMap;
use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, HalfEdgeId, Orientation, Topology, VertexId};

use crate::topology::{compute_centroid, quantize, CylinderInfo, EdgeInfo, FaceInfo};

/// Key for a trim vertex: (original_vertex, face_id).
pub(crate) type TrimKey = (VertexId, FaceId);

/// Compute trim vertices for all vertices on all faces.
///
/// For each vertex V on face F:
/// - The entering edge E_enter and leaving edge E_leave define two trim lines
///   (parallel to each edge, offset inward by `distance`)
/// - The trim vertex is at the intersection of these two trim lines
pub(crate) fn compute_trim_vertices(faces: &[FaceInfo], distance: f64) -> HashMap<TrimKey, Point3> {
    let mut trims = HashMap::new();

    for face in faces {
        let n = face.vertex_ids.len();

        // For cylindrical faces, Newell's method on the rich arc-
        // sampled outer loop only gives an average "outward radial"
        // that's imprecise for off-center vertices; the trim formula
        // then misbehaves and produces vertices outside the solid
        // (the z=-2/z=22 overhang in the pork-chop render). Compute
        // cylinder-face trim analytically: for a vertex on a cap-edge
        // (at the v_min or v_max extremum along the cylinder axis),
        // move it axially toward the face interior by `distance`.
        // For a vertex on a vertical seam, move it along the arc
        // tangent toward the interior. Both use the exact cylinder
        // parameters rather than the approximate face normal.
        if let Some(cyl) = face.cylinder {
            trim_cylinder_face(face, &cyl, distance, &mut trims);
            continue;
        }

        let normal = face.normal;

        for i in 0..n {
            let v_id = face.vertex_ids[i];
            let v_pos = face.positions[i];
            let prev_idx = (i + n - 1) % n;
            let next_idx = (i + 1) % n;

            let prev_pos = face.positions[prev_idx];
            let d_enter = v_pos - prev_pos;
            let d_enter_len = d_enter.norm();

            let next_pos = face.positions[next_idx];
            let d_leave = next_pos - v_pos;
            let d_leave_len = d_leave.norm();

            if d_enter_len < 1e-15 || d_leave_len < 1e-15 {
                trims.insert((v_id, face.face_id), v_pos);
                continue;
            }

            let d_enter = d_enter / d_enter_len;
            let d_leave = d_leave / d_leave_len;

            let perp_enter = normal.cross(d_enter);
            let pe_len = perp_enter.norm();
            let perp_leave = normal.cross(d_leave);
            let pl_len = perp_leave.norm();

            if pe_len < 1e-15 || pl_len < 1e-15 {
                trims.insert((v_id, face.face_id), v_pos);
                continue;
            }

            let perp_enter = perp_enter / pe_len;
            let perp_leave = perp_leave / pl_len;

            let delta = distance * (perp_enter - perp_leave);
            let cross_dirs = d_enter.cross(d_leave);
            let denom = cross_dirs.dot(normal);

            // The two trim lines intersect at v_pos + distance*perp_enter + t1*d_enter
            // where t1 = -(delta × d_leave) · normal / denom. `denom` is the
            // signed sine of the exterior angle at this vertex, so when two
            // edges are nearly collinear (as on a tessellated arc cap) denom
            // → 0 and t1 blows up, pushing the trim point hundreds of units
            // away from the actual vertex. That manifested as blend-face
            // vertices flung far outside the B-rep ("pork-chop diverges"
            // bug). Fall back to a tangent-bisector offset in both the
            // strict-degenerate case and when the computed `t1` would push
            // the trim further than a corner geometrically should support.
            let cross_delta = delta.cross(d_leave);
            let t1_safe_limit = d_enter_len.min(d_leave_len);
            let t1 = if denom.abs() < 1e-12 {
                f64::NAN
            } else {
                -cross_delta.dot(normal) / denom
            };
            if !t1.is_finite() || t1.abs() > t1_safe_limit {
                let p = v_pos + distance * 0.5 * (perp_enter + perp_leave);
                trims.insert((v_id, face.face_id), p);
                continue;
            }

            let p1 = v_pos + distance * perp_enter;
            let trim_point = Point3::from(p1.to_vec() + t1 * d_enter);
            trims.insert((v_id, face.face_id), trim_point);
        }
    }

    trims
}

/// Analytical trim for a cylindrical face. For every vertex on the
/// cylinder's bottom arc (axial minimum), shift it axially +distance
/// toward the face interior; for every vertex on the top arc (axial
/// maximum), shift -distance. Vertices at the face's u-extrema (the
/// vertical seam edges at arc junctions) are additionally shifted
/// along the arc tangent toward the face interior by the same
/// distance, so the rectangular corner of the cylinder face (at the
/// intersection of a cap edge and a vertical seam) gets a diagonal
/// offset matching what the fillet pipeline expects.
fn trim_cylinder_face(
    face: &FaceInfo,
    cyl: &CylinderInfo,
    distance: f64,
    trims: &mut HashMap<TrimKey, Point3>,
) {
    let n = face.vertex_ids.len();
    if n == 0 {
        return;
    }
    let axis = cyl.axis.normalize();

    // Signed v (axial) coordinate of each vertex, and a u angle
    // around the cylinder from an arbitrary reference.
    let ref_dir = {
        let base = Vec3::new(1.0, 0.0, 0.0);
        let proj = axis.dot(&base);
        let r = base - axis * proj;
        if r.norm() < 1e-6 {
            let alt = Vec3::new(0.0, 1.0, 0.0);
            let proj2 = axis.dot(&alt);
            (alt - axis * proj2).normalize()
        } else {
            r.normalize()
        }
    };
    let bidir = axis.cross(&ref_dir);

    let mut vs: Vec<f64> = Vec::with_capacity(n);
    let mut us: Vec<f64> = Vec::with_capacity(n);
    for p in &face.positions {
        let d = *p - cyl.center;
        vs.push(d.dot(&axis));
        let ru = d.dot(&ref_dir);
        let bu = d.dot(&bidir);
        us.push(bu.atan2(ru));
    }
    let v_min = vs.iter().cloned().fold(f64::INFINITY, f64::min);
    let v_max = vs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // Wrap-aware u span: sort and find the largest gap.
    let (u_lo, u_hi) = {
        let mut sorted: Vec<f64> = us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        sorted.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        if sorted.len() < 2 {
            (sorted.first().copied().unwrap_or(0.0),
             sorted.first().copied().unwrap_or(0.0))
        } else {
            // Largest gap determines which side of the seam the face
            // does NOT cover; u_lo/u_hi bracket the covered side.
            let mut best_gap = 0.0_f64;
            let mut best_idx = 0_usize;
            for i in 0..sorted.len() - 1 {
                let g = sorted[i + 1] - sorted[i];
                if g > best_gap {
                    best_gap = g;
                    best_idx = i;
                }
            }
            let wrap_gap = (sorted[0] + 2.0 * std::f64::consts::PI) - sorted[sorted.len() - 1];
            if wrap_gap > best_gap {
                (sorted[0], sorted[sorted.len() - 1])
            } else {
                (sorted[best_idx + 1], sorted[best_idx])
            }
        }
    };

    let h = v_max - v_min;
    let eps_v = (h * 1e-4).max(1e-6);
    let eps_u = 1e-3;

    for i in 0..n {
        let v_id = face.vertex_ids[i];
        let v_pos = face.positions[i];
        let v = vs[i];
        let u = us[i];

        let mut shift = Vec3::zeros();

        // Axial shift based on which cap edge this vertex lies on.
        if (v - v_min).abs() < eps_v {
            shift += axis * distance;
        } else if (v_max - v).abs() < eps_v {
            shift -= axis * distance;
        }

        // Tangential shift based on which seam edge this vertex lies on.
        // At u_lo the face body is in the +u direction; at u_hi the
        // face body is in the −u direction. Wrap-unaware comparison is
        // fine because we normalized u via atan2 into [-π, π].
        let at_lo = (u - u_lo).abs() < eps_u
            || (u - u_lo - 2.0 * std::f64::consts::PI).abs() < eps_u
            || (u - u_lo + 2.0 * std::f64::consts::PI).abs() < eps_u;
        let at_hi = (u - u_hi).abs() < eps_u
            || (u - u_hi - 2.0 * std::f64::consts::PI).abs() < eps_u
            || (u - u_hi + 2.0 * std::f64::consts::PI).abs() < eps_u;
        if at_lo || at_hi {
            // Tangent to the cylinder at this point, in the face's
            // circumferential direction.
            let radial = v_pos - cyl.center - axis * v;
            let tangent = axis.cross(&radial);
            let tangent_len = tangent.norm();
            if tangent_len > 1e-12 {
                let t = tangent / tangent_len;
                shift += if at_lo { t * distance } else { -t * distance };
            }
        }

        trims.insert((v_id, face.face_id), v_pos + shift);
    }
}

/// Build vertex faces for all vertices where >=3 edges meet.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_vertex_faces(
    faces: &[FaceInfo],
    vertex_edges: &HashMap<VertexId, Vec<&EdgeInfo>>,
    trims: &HashMap<TrimKey, Point3>,
    brep: &BRepSolid,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) {
    let get_or_create_vertex =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    for (&v_id, v_edges) in vertex_edges {
        if v_edges.len() < 3 {
            continue;
        }

        let v_pos = brep.topology.vertices[v_id].point;

        let mut vertex_face_points: Vec<Point3> = Vec::new();
        for face in faces {
            if face.vertex_ids.contains(&v_id) {
                if let Some(&p) = trims.get(&(v_id, face.face_id)) {
                    vertex_face_points.push(p);
                }
            }
        }

        if vertex_face_points.len() < 3 {
            continue;
        }

        let solid_center = compute_centroid(faces);
        let axis = (v_pos - solid_center).normalize();

        let arbitrary = if axis.x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };
        let u_dir = axis.cross(arbitrary).normalize();
        let v_dir = axis.cross(u_dir);

        let center = vertex_face_points
            .iter()
            .fold(Vec3::zeros(), |acc, p| acc + p.to_vec())
            / vertex_face_points.len() as f64;
        let center = Point3::from(center);

        let mut indexed: Vec<(usize, f64)> = vertex_face_points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let d = *p - center;
                (i, d.dot(v_dir).atan2(d.dot(u_dir)))
            })
            .collect();
        indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let sorted_positions: Vec<Point3> = indexed
            .iter()
            .map(|(i, _)| vertex_face_points[*i])
            .collect();

        if sorted_positions.len() >= 3 {
            let e1 = sorted_positions[1] - sorted_positions[0];
            let e2 = sorted_positions[2] - sorted_positions[0];
            let n = e1.cross(e2);
            let outward = center - solid_center;

            let final_positions = if n.dot(outward) > 0.0 {
                sorted_positions
            } else {
                let mut rev = sorted_positions;
                rev.reverse();
                rev
            };

            let verts: Vec<VertexId> = final_positions
                .iter()
                .map(|p| get_or_create_vertex(vertex_cache, new_topo, *p))
                .collect();

            let x_dir = final_positions[1] - final_positions[0];
            let y_dir = final_positions[final_positions.len() - 1] - final_positions[0];
            let surf_idx =
                new_geom.add_surface(Box::new(Plane::new(final_positions[0], x_dir, y_dir)));

            let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
            let loop_id = new_topo.add_loop(&hes);
            let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);
        }
    }
}
