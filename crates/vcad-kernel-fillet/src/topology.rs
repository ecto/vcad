//! Topology analysis helpers for fillet/chamfer operations.

use std::collections::HashMap;
use vcad_kernel_geom::SurfaceKind;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, Topology, VertexId};

/// Information about a face extracted from the B-rep.
#[derive(Debug, Clone)]
pub(crate) struct FaceInfo {
    pub face_id: FaceId,
    /// Vertices in loop order.
    pub vertex_ids: Vec<VertexId>,
    /// Vertex positions in loop order.
    pub positions: Vec<Point3>,
    /// Outward face normal (from vertex winding).
    pub normal: Vec3,
    /// When the face surface is a `CylinderSurface`, the cylinder's
    /// analytic parameters. Trim-vertex computation uses these to move
    /// cap-edge vertices axially into the face instead of relying on
    /// Newell's method, which is only approximate on curved loops.
    pub cylinder: Option<CylinderInfo>,
}

/// Analytic cylinder parameters captured alongside a cylindrical face.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CylinderInfo {
    pub center: Point3,
    pub axis: Vec3,
    #[allow(dead_code)]
    pub radius: f64,
}

/// Information about an edge.
#[derive(Debug, Clone)]
pub(crate) struct EdgeInfo {
    #[allow(dead_code)]
    pub edge_id: EdgeId,
    /// Start vertex (origin of the primary half-edge).
    pub v_start: VertexId,
    /// End vertex.
    pub v_end: VertexId,
    /// Face on the primary half-edge side.
    pub face_a: FaceId,
    /// Face on the twin half-edge side.
    pub face_b: FaceId,
}

/// Extended face information for curved surface classification.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct CurvedFaceInfo {
    pub face_id: FaceId,
    pub surface_index: usize,
    pub surface_kind: SurfaceKind,
    pub vertex_ids: Vec<VertexId>,
    pub positions: Vec<Point3>,
    /// Planar normal (only valid for planar faces).
    pub planar_normal: Option<Vec3>,
}

/// Extract face information from a B-rep solid.
pub(crate) fn extract_faces(brep: &BRepSolid) -> Vec<FaceInfo> {
    let topo = &brep.topology;
    let geom = &brep.geometry;
    let mut faces = Vec::new();

    for (face_id, face) in &topo.faces {
        let vertex_ids = topo.loop_vertices(face.outer_loop);
        let positions: Vec<Point3> = vertex_ids.iter().map(|&v| topo.vertices[v].point).collect();
        let normal = compute_face_normal(&positions);
        let surface = &geom.surfaces[face.surface_index];
        let cylinder = surface
            .as_any()
            .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            .map(|c| CylinderInfo {
                center: c.center,
                axis: *c.axis.as_ref(),
                radius: c.radius,
            });

        faces.push(FaceInfo {
            face_id,
            vertex_ids,
            positions,
            normal,
            cylinder,
        });
    }

    faces
}

/// Compute face normal from vertex positions using Newell's method.
pub(crate) fn compute_face_normal(positions: &[Point3]) -> Vec3 {
    let n = positions.len();
    if n < 3 {
        return Vec3::z();
    }
    let mut normal = Vec3::zeros();
    for i in 0..n {
        let curr = positions[i];
        let next = positions[(i + 1) % n];
        normal.x += (curr.y - next.y) * (curr.z + next.z);
        normal.y += (curr.z - next.z) * (curr.x + next.x);
        normal.z += (curr.x - next.x) * (curr.y + next.y);
    }
    let len = normal.norm();
    if len < 1e-15 {
        Vec3::z()
    } else {
        normal / len
    }
}

/// Extract edge information from a B-rep solid.
/// Only returns edges that have two adjacent faces (manifold edges).
pub(crate) fn extract_edges(brep: &BRepSolid) -> Vec<EdgeInfo> {
    let topo = &brep.topology;
    let mut edges = Vec::new();

    for (edge_id, edge) in &topo.edges {
        let he1 = edge.half_edge;
        let he2 = match topo.half_edges[he1].twin {
            Some(t) => t,
            None => continue,
        };

        let v_start = topo.half_edges[he1].origin;
        let v_end = topo.half_edges[he2].origin;

        let face_a = topo.half_edges[he1]
            .loop_id
            .and_then(|l| topo.loops[l].face);
        let face_b = topo.half_edges[he2]
            .loop_id
            .and_then(|l| topo.loops[l].face);

        if let (Some(fa), Some(fb)) = (face_a, face_b) {
            edges.push(EdgeInfo {
                edge_id,
                v_start,
                v_end,
                face_a: fa,
                face_b: fb,
            });
        }
    }

    // Sorted by endpoints: the edge arena's order reflects however the
    // input solid happened to pair its twins (a HashMap walk, in the
    // primitives), so leaving it alone made the fillet emit its blend
    // faces in a different order each run — and every boolean on the
    // result, whose split order follows face ids, drifted with it.
    edges.sort_by_key(|x| (x.v_start, x.v_end));
    edges
}

/// Compute the centroid of all faces' vertex positions.
pub(crate) fn compute_centroid(faces: &[FaceInfo]) -> Point3 {
    let mut sum = Vec3::zeros();
    let mut count = 0;
    for face in faces {
        for p in &face.positions {
            sum += p.to_vec();
            count += 1;
        }
    }
    if count == 0 {
        Point3::origin()
    } else {
        Point3::from(sum / count as f64)
    }
}

/// Pair twin half-edges by matching (origin, destination) vertex pairs.
pub(crate) fn pair_twin_half_edges(topo: &mut Topology) {
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    let he_ids: Vec<HalfEdgeId> = topo.half_edges.keys().collect();
    for he_id in &he_ids {
        let he = &topo.half_edges[*he_id];
        let origin = topo.vertices[he.origin].point;
        let next = match he.next {
            Some(n) => n,
            None => continue,
        };
        let dest = topo.vertices[topo.half_edges[next].origin].point;

        let origin_key = quantize(origin);
        let dest_key = quantize(dest);

        if let Some(&twin_id) = he_map.get(&(dest_key, origin_key)) {
            if topo.half_edges[*he_id].twin.is_none() && topo.half_edges[twin_id].twin.is_none() {
                topo.add_edge(*he_id, twin_id);
            }
        }

        he_map.insert((origin_key, dest_key), *he_id);
    }
}

/// Quantize a point to integer coordinates for hashing.
pub(crate) fn quantize(p: Point3) -> [i64; 3] {
    [
        (p.x * 1e9).round() as i64,
        (p.y * 1e9).round() as i64,
        (p.z * 1e9).round() as i64,
    ]
}
