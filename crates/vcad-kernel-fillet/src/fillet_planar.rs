//! Planar fillet — cylindrical blend between two planar faces.

use std::collections::HashMap;
use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, SurfaceKind};
use vcad_kernel_math::{Dir3, Point3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::topology::{
    compute_centroid, extract_edges, extract_faces, pair_twin_half_edges, quantize, EdgeInfo,
    FaceInfo,
};
use crate::trim::{
    build_vertex_faces, compute_trim_vertices_with_setbacks, fillet_edge_setbacks, CornerBlend,
    TrimKey,
};

/// Dihedral angle threshold (radians) above which two adjacent faces count
/// as "nearly coplanar". Extruding an arc profile produces a strip of thin,
/// nearly-coplanar planar quads along the curved side wall — applying a
/// planar fillet to such edges trims each adjacent face by `radius /
/// tan(angle/2)` which diverges as the angle approaches 180°, producing
/// the exploding sawtooth artifact visible in the viewport. At ~170° the
/// blend would already be ~11× the fillet radius.
const COPLANAR_DIHEDRAL_THRESHOLD: f64 = 170.0 * std::f64::consts::PI / 180.0;

/// Fillet all edges of a B-rep solid with a constant radius.
///
/// Creates a new solid where each edge is replaced by a cylindrical blend
/// surface tangent to both adjacent faces.
///
/// # Requirements
///
/// - All faces must be planar
/// - The solid should be convex
/// - Radius must be positive and smaller than the shortest edge / 2
///
/// When the B-rep contains non-planar surfaces or any pair of adjacent
/// faces meeting at a near-coplanar dihedral angle (e.g. the tessellated
/// side wall of an extruded arc), this function returns the input
/// unchanged rather than producing broken geometry.
pub fn fillet_all_edges(brep: &BRepSolid, radius: f64) -> BRepSolid {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);

    if edges.is_empty() {
        return brep.clone();
    }

    if !is_fillet_safe(brep, &faces, &edges) {
        return brep.clone();
    }

    // Dihedral-correct tangent setbacks: r/tan(θ/2) per edge, capped at
    // 8·r. An infinite entry marks a knife edge — refuse the whole solid
    // rather than crack the shell around it.
    let setbacks = fillet_edge_setbacks(&faces, &edges, radius, 8.0 * radius);
    if setbacks.values().any(|s| !s.is_finite()) {
        return brep.clone();
    }

    let trims = compute_trim_vertices_with_setbacks(&faces, radius, &setbacks);

    // Feasibility: the trimmed faces must keep their original orientation
    // and stay non-degenerate. When the radius exceeds what the local
    // feature can host, insets cross over and faces invert — the rebuilt
    // shell is watertight but *wrong* (it can even gain volume). Refuse
    // cleanly instead.
    if !trimmed_faces_are_valid(&faces, &trims) {
        return brep.clone();
    }
    let face_map: HashMap<FaceId, &FaceInfo> = faces.iter().map(|f| (f.face_id, f)).collect();

    let mut vertex_edges: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
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

    // 1. Build modified original faces
    for face in &faces {
        let new_positions: Vec<Point3> = face
            .vertex_ids
            .iter()
            .filter_map(|&v_id| trims.get(&(v_id, face.face_id)).copied())
            .collect();

        if new_positions.len() < 3 {
            continue;
        }

        let verts: Vec<VertexId> = new_positions
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

        let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
        let loop_id = new_topo.add_loop(&hes);
        let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
        all_faces.push(face_id);
    }

    // 2. Build fillet faces (cylindrical blend for each edge)
    for edge_info in &edges {
        let fa = face_map[&edge_info.face_a];
        let fb = face_map[&edge_info.face_b];

        let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
        let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
        let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
        let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

        if let (Some(&pa_s), Some(&pa_e), Some(&pb_s), Some(&pb_e)) = (pa_s, pa_e, pb_s, pb_e) {
            let v_start_pos = brep.topology.vertices[edge_info.v_start].point;
            let v_end_pos = brep.topology.vertices[edge_info.v_end].point;
            let edge_dir = v_end_pos - v_start_pos;
            let edge_len = edge_dir.norm();
            if edge_len < 1e-12 {
                continue;
            }
            let edge_unit = edge_dir / edge_len;

            // Cylinder fillet center for the convex edge between two planar
            // faces. The axis is parallel to the edge, at distance `radius`
            // from each face along the *inward* normal — i.e. the side
            // where the solid lives. The original code had this with the
            // sign flipped (placing the center *outside* the solid), which
            // made every trim point sit at distance ≠ radius from the
            // resulting axis; the tessellator then drew a cylinder that
            // didn't connect to the trimmed face boundaries, producing
            // the disconnected "exploded box" rendering.
            //
            // For a dihedral angle θ (interior) between the two faces, the
            // edge → axis offset is `r / sin(θ/2)` along the inward
            // bisector. Using the identity sin²(θ/2) = (1 + n_a·n_b) / 2
            // for outward unit normals lets us write the offset purely in
            // terms of the dotted normals, with no trig calls.
            let normal_dot = fa.normal.dot(fb.normal);
            let sin2_half = (1.0 + normal_dot) * 0.5;
            if sin2_half < 1e-9 {
                // Faces nearly coplanar at this edge; fillet is
                // ill-defined. Skip — `is_fillet_safe` should already
                // have caught this for the whole solid.
                continue;
            }
            let center_offset = -radius * (fa.normal + fb.normal) / (2.0 * sin2_half);
            let center_start = v_start_pos + center_offset;

            let to_tangent_a = pa_s - center_start;
            let ref_dir = to_tangent_a - to_tangent_a.dot(edge_unit) * edge_unit;
            let ref_len = ref_dir.norm();
            if ref_len < 1e-12 {
                continue;
            }

            let cyl_surface = CylinderSurface {
                center: center_start,
                axis: Dir3::new_normalize(edge_unit),
                ref_dir: Dir3::new_normalize(ref_dir),
                radius,
            };
            let surf_idx = new_geom.add_surface(Box::new(cyl_surface));

            let solid_center = compute_centroid(&faces);
            let chamfer_center = Point3::from(
                (pa_s.to_vec() + pa_e.to_vec() + pb_e.to_vec() + pb_s.to_vec()) * 0.25,
            );
            let outward = chamfer_center - solid_center;

            let e1 = pa_e - pa_s;
            let e2 = pb_s - pa_s;
            let n = e1.cross(e2);

            let positions = if n.dot(outward) > 0.0 {
                vec![pa_s, pa_e, pb_e, pb_s]
            } else {
                vec![pa_s, pb_s, pb_e, pa_e]
            };

            let verts: Vec<VertexId> = positions
                .iter()
                .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
                .collect();

            let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
            let loop_id = new_topo.add_loop(&hes);
            let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);
        }
    }

    // 3. Build vertex faces — sphere octants where the corner has 3
    // orthogonal faces (cubes), planar fallback elsewhere.
    build_vertex_faces(
        &faces,
        &vertex_edges,
        &trims,
        brep,
        radius,
        CornerBlend::SphereWhenCube,
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

/// Check that every trimmed face polygon still faces the same way as the
/// original face and has meaningful area. Inverted or collapsed inset
/// polygons mean the requested radius exceeds the local feature size.
fn trimmed_faces_are_valid(faces: &[FaceInfo], trims: &HashMap<TrimKey, Point3>) -> bool {
    for face in faces {
        let pts: Vec<Point3> = face
            .vertex_ids
            .iter()
            .filter_map(|&v| trims.get(&(v, face.face_id)).copied())
            .collect();
        if pts.len() < 3 {
            return false;
        }
        if pts.iter().any(|p| !p.to_vec().norm().is_finite()) {
            return false;
        }
        // Newell normal of the inset polygon vs the original face normal.
        let mut n = vcad_kernel_math::Vec3::zeros();
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            n.x += (a.y - b.y) * (a.z + b.z);
            n.y += (a.z - b.z) * (a.x + b.x);
            n.z += (a.x - b.x) * (a.y + b.y);
        }
        if n.norm() < 1e-9 || n.dot(face.normal) <= 0.0 {
            return false;
        }
    }
    true
}

/// Pre-flight check that the planar fillet algorithm can produce correct
/// geometry for this B-rep. Rejects inputs that would explode the trim
/// calculation: non-planar surfaces and near-coplanar adjacent faces (the
/// pattern produced by tessellating an extruded arc profile).
fn is_fillet_safe(brep: &BRepSolid, faces: &[FaceInfo], edges: &[EdgeInfo]) -> bool {
    for surface in &brep.geometry.surfaces {
        if surface.surface_type() != SurfaceKind::Plane {
            return false;
        }
    }

    let face_map: HashMap<FaceId, &FaceInfo> = faces.iter().map(|f| (f.face_id, f)).collect();
    for edge in edges {
        let Some(fa) = face_map.get(&edge.face_a) else {
            continue;
        };
        let Some(fb) = face_map.get(&edge.face_b) else {
            continue;
        };
        let dot = fa.normal.dot(fb.normal).clamp(-1.0, 1.0);
        // Dihedral angle between outward face normals. `acos(dot)` is 0
        // for identical normals and π for opposite normals. Two adjacent
        // faces of a convex solid have an outward dihedral < π (the
        // smaller, the sharper the edge). Near-coplanar ⇒ dot near 1 ⇒
        // angle near 0. We reject when the interior-supplement (π − angle)
        // exceeds the threshold, i.e. when the surfaces are nearly in the
        // same plane.
        let angle = dot.acos();
        let interior = std::f64::consts::PI - angle;
        if interior > COPLANAR_DIHEDRAL_THRESHOLD {
            return false;
        }
    }
    true
}

/// Build a plane-plane cylindrical blend (used by fillet_edges_detailed).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_plane_plane_blend(
    edge_info: &EdgeInfo,
    fa: &FaceInfo,
    fb: &FaceInfo,
    trims: &HashMap<TrimKey, Point3>,
    faces: &[FaceInfo],
    radius: f64,
    brep: &BRepSolid,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) -> bool {
    let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
    let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
    let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
    let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

    let (pa_s, pa_e, pb_s, pb_e) = match (pa_s, pa_e, pb_s, pb_e) {
        (Some(&a), Some(&b), Some(&c), Some(&d)) => (a, b, c, d),
        _ => return false,
    };

    let v_start_pos = brep.topology.vertices[edge_info.v_start].point;
    let v_end_pos = brep.topology.vertices[edge_info.v_end].point;
    let edge_dir = v_end_pos - v_start_pos;
    let edge_len = edge_dir.norm();
    if edge_len < 1e-12 {
        return false;
    }
    let edge_unit = edge_dir / edge_len;

    // Same convex-edge cylinder placement as in fillet_all_edges — see the
    // comment there for the geometry. Inward bisector offset of
    // `r / sin(θ/2)`, written via the (1 + n_a·n_b)/2 identity.
    let normal_dot = fa.normal.dot(fb.normal);
    let sin2_half = (1.0 + normal_dot) * 0.5;
    if sin2_half < 1e-9 {
        return false;
    }
    let center_offset = -radius * (fa.normal + fb.normal) / (2.0 * sin2_half);
    let center_start = v_start_pos + center_offset;

    let to_tangent_a = pa_s - center_start;
    let ref_dir = to_tangent_a - to_tangent_a.dot(edge_unit) * edge_unit;
    let ref_len = ref_dir.norm();
    if ref_len < 1e-12 {
        return false;
    }

    let cyl_surface = CylinderSurface {
        center: center_start,
        axis: Dir3::new_normalize(edge_unit),
        ref_dir: Dir3::new_normalize(ref_dir),
        radius,
    };
    let surf_idx = new_geom.add_surface(Box::new(cyl_surface));

    let solid_center = compute_centroid(faces);
    let chamfer_center =
        Point3::from((pa_s.to_vec() + pa_e.to_vec() + pb_e.to_vec() + pb_s.to_vec()) * 0.25);
    let outward = chamfer_center - solid_center;

    let e1 = pa_e - pa_s;
    let e2 = pb_s - pa_s;
    let n = e1.cross(e2);

    let positions = if n.dot(outward) > 0.0 {
        vec![pa_s, pa_e, pb_e, pb_s]
    } else {
        vec![pa_s, pb_s, pb_e, pa_e]
    };

    let get_or_create =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    let verts: Vec<VertexId> = positions
        .iter()
        .map(|p| get_or_create(vertex_cache, new_topo, *p))
        .collect();

    let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
    let loop_id = new_topo.add_loop(&hes);
    let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
    all_faces.push(face_id);
    true
}
