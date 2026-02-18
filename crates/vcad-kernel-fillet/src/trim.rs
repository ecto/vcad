//! Trim vertex computation for fillet/chamfer operations.

use std::collections::HashMap;
use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, HalfEdgeId, Orientation, Topology, VertexId};

use crate::topology::{compute_centroid, quantize, EdgeInfo, FaceInfo};

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

            let perp_enter = normal.cross(&d_enter);
            let pe_len = perp_enter.norm();
            let perp_leave = normal.cross(&d_leave);
            let pl_len = perp_leave.norm();

            if pe_len < 1e-15 || pl_len < 1e-15 {
                trims.insert((v_id, face.face_id), v_pos);
                continue;
            }

            let perp_enter = perp_enter / pe_len;
            let perp_leave = perp_leave / pl_len;

            let delta = distance * (perp_enter - perp_leave);
            let cross_dirs = d_enter.cross(&d_leave);
            let denom = cross_dirs.dot(&normal);

            if denom.abs() < 1e-15 {
                let p = v_pos + distance * 0.5 * (perp_enter + perp_leave);
                trims.insert((v_id, face.face_id), p);
                continue;
            }

            let cross_delta = delta.cross(&d_leave);
            let t1 = -cross_delta.dot(&normal) / denom;

            let p1 = v_pos + distance * perp_enter;
            let trim_point = Point3::from(p1.coords + t1 * d_enter);
            trims.insert((v_id, face.face_id), trim_point);
        }
    }

    trims
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
        let u_dir = axis.cross(&arbitrary).normalize();
        let v_dir = axis.cross(&u_dir);

        let center = vertex_face_points
            .iter()
            .fold(Vec3::zeros(), |acc, p| acc + p.coords)
            / vertex_face_points.len() as f64;
        let center = Point3::from(center);

        let mut indexed: Vec<(usize, f64)> = vertex_face_points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let d = *p - center;
                (i, d.dot(&v_dir).atan2(d.dot(&u_dir)))
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
            let n = e1.cross(&e2);
            let outward = center - solid_center;

            let final_positions = if n.dot(&outward) > 0.0 {
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
