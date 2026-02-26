//! Chamfer operation — replaces edges with planar bevel faces.

use std::collections::HashMap;
use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::topology::{
    compute_centroid, extract_edges, extract_faces, pair_twin_half_edges, quantize,
};
use crate::trim::{build_vertex_faces, compute_trim_vertices};

/// Chamfer all edges of a B-rep solid by the given distance.
///
/// Creates a new solid where each edge is replaced by a planar bevel face,
/// each original face is trimmed back, and each vertex becomes a polygon face.
///
/// # Requirements
///
/// - All faces must be planar (analytic surfaces)
/// - The solid should be convex (concave solids may produce incorrect results)
/// - Distance must be positive and small enough that offset vertices don't overlap
///
/// # Panics
///
/// Panics if the solid has no edges or if offset computation fails.
pub fn chamfer_all_edges(brep: &BRepSolid, distance: f64) -> BRepSolid {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);

    if edges.is_empty() {
        return brep.clone();
    }

    let trims = compute_trim_vertices(&faces, distance);

    let mut vertex_edges: HashMap<VertexId, Vec<&crate::topology::EdgeInfo>> = HashMap::new();
    for edge in &edges {
        vertex_edges.entry(edge.v_start).or_default().push(edge);
        vertex_edges.entry(edge.v_end).or_default().push(edge);
    }

    let mut new_topo = Topology::new();
    let mut new_geom = GeometryStore::new();
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();

    let get_or_create_vertex =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    let mut all_faces = Vec::new();

    // 1. Build modified original faces (same vertex count, using trim vertices)
    for face in &faces {
        let new_positions: Vec<Point3> = face
            .vertex_ids
            .iter()
            .filter_map(|&v_id| trims.get(&(v_id, face.face_id)).copied())
            .collect();

        if new_positions.len() < 3 {
            continue;
        }

        let new_verts: Vec<VertexId> = new_positions
            .iter()
            .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
            .collect();

        let p0 = new_positions[0];
        let x_dir = new_positions[1] - p0;
        let y_dir = new_positions[new_positions.len() - 1] - p0;
        let surf_idx = if x_dir.norm() > 1e-12 && y_dir.norm() > 1e-12 {
            new_geom.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)))
        } else {
            new_geom.add_surface(Box::new(Plane::from_normal(p0, face.normal)))
        };

        let hes: Vec<HalfEdgeId> = new_verts
            .iter()
            .map(|&v| new_topo.add_half_edge(v))
            .collect();
        let loop_id = new_topo.add_loop(&hes);
        let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
        all_faces.push(face_id);
    }

    // 2. Build chamfer faces (one per edge)
    for edge_info in &edges {
        let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
        let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
        let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
        let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

        if let (Some(&pa_s), Some(&pa_e), Some(&pb_s), Some(&pb_e)) = (pa_s, pa_e, pb_s, pb_e) {
            let chamfer_center = Point3::from(
                (pa_s.to_vec() + pa_e.to_vec() + pb_e.to_vec() + pb_s.to_vec()) * 0.25,
            );
            let solid_center = compute_centroid(&faces);
            let outward_dir = chamfer_center - solid_center;

            let e1 = pa_e - pa_s;
            let e2 = pb_s - pa_s;
            let n = e1.cross(&e2);

            let positions = if n.dot(&outward_dir) > 0.0 {
                vec![pa_s, pa_e, pb_e, pb_s]
            } else {
                vec![pa_s, pb_s, pb_e, pa_e]
            };

            let verts: Vec<VertexId> = positions
                .iter()
                .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
                .collect();

            let x_dir = positions[1] - positions[0];
            let y_dir = positions[3] - positions[0];
            let surf_idx = new_geom.add_surface(Box::new(Plane::new(positions[0], x_dir, y_dir)));

            let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
            let loop_id = new_topo.add_loop(&hes);
            let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);
        }
    }

    // 3. Build vertex faces
    build_vertex_faces(
        &faces,
        &vertex_edges,
        &trims,
        brep,
        &mut vertex_cache,
        &mut new_topo,
        &mut new_geom,
        &mut all_faces,
    );

    // 4. Pair twin half-edges
    pair_twin_half_edges(&mut new_topo);

    // 5. Build shell and solid
    let shell = new_topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = new_topo.add_solid(shell);

    BRepSolid {
        topology: new_topo,
        geometry: new_geom,
        solid_id,
    }
}
