//! Correct per-edge fillet for an independent set of plane-plane edges.
//!
//! The shared-`trims` pipeline in [`crate::fillet_curved`] insets *every*
//! face of the solid by `radius` — which is only correct when *every* edge
//! is being filleted. For a proper subset it shrinks the whole body and
//! emits blends for the selected edges alone, producing a malformed,
//! non-watertight solid (a single-edge cube fillet came out at ~522 mm³
//! instead of ~995 mm³).
//!
//! This module implements the geometrically correct construction for the
//! case that matters to per-edge (differentiable) fillet radii: a set of
//! **plane-plane** edges no two of which share a vertex (an *independent
//! set*). For that case each selected edge is a self-contained rounding:
//!
//! * the two faces adjacent to the edge (its *side* faces) are inset only
//!   along that edge,
//! * a single quarter-cylinder blend replaces the edge,
//! * the faces that merely *touch* an endpoint of the edge (its *cap*
//!   faces) have that one corner rounded by the blend's terminating arc —
//!   the cylinder ends flush against them, so no spherical corner patch is
//!   required.
//!
//! Every face, cap arc and cylinder ring is generated from the same
//! tangent-point / arc-sampling formulas, so coincident points quantize
//! to a single welded vertex and the result is watertight.

use std::collections::{HashMap, HashSet};
use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, SurfaceKind};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::topology::{extract_edges, extract_faces, pair_twin_half_edges, quantize};
use crate::FilletResult;

/// Number of segments each 90°-ish blend arc is sampled with. Chosen
/// large enough that the tessellator treats the samples as dense anchors
/// (so it reuses them verbatim and the cylinder welds to the adjacent cap
/// / side faces) and that the tessellated volume tracks the analytic
/// closed form to well under 1e-6 relative.
const ARC_SEGMENTS: usize = 128;

/// Precomputed geometry for one selected plane-plane edge.
struct BlendGeom {
    v_start: VertexId,
    v_end: VertexId,
    face_a: FaceId,
    face_b: FaceId,
    n_a: Vec3,
    n_b: Vec3,
    /// Cylinder axis point in the plane through `v_start` (⊥ to the edge).
    center_start: Point3,
    /// Cylinder axis point in the plane through `v_end`.
    center_end: Point3,
    /// Unit edge direction, `v_start` → `v_end`.
    axis: Vec3,
    radius: f64,
}

impl BlendGeom {
    /// Axis point in the plane through vertex `v` (must be an endpoint).
    fn center_at(&self, v: VertexId) -> Point3 {
        if v == self.v_start {
            self.center_start
        } else {
            self.center_end
        }
    }
    /// Tangent point on `face_a` in the plane through vertex `v`.
    fn tan_a_at(&self, v: VertexId) -> Point3 {
        self.center_at(v) + self.n_a * self.radius
    }
    /// Tangent point on `face_b` in the plane through vertex `v`.
    fn tan_b_at(&self, v: VertexId) -> Point3 {
        self.center_at(v) + self.n_b * self.radius
    }
}

/// True when every `edge_id` names a plane-plane manifold edge and no two
/// of the selected edges share a vertex. This is the domain the correct
/// subset builder covers; callers fall back to the legacy pipeline
/// otherwise.
pub(crate) fn is_independent_plane_plane_set(brep: &BRepSolid, edge_ids: &[EdgeId]) -> bool {
    if edge_ids.is_empty() {
        return false;
    }
    let selected: HashSet<EdgeId> = edge_ids.iter().copied().collect();
    if selected.len() != edge_ids.len() {
        return false;
    }
    let edges = extract_edges(brep);
    let mut seen_vertices: HashSet<VertexId> = HashSet::new();
    let mut matched = 0usize;
    for e in &edges {
        if !selected.contains(&e.edge_id) {
            continue;
        }
        matched += 1;
        let sa = brep.geometry.surfaces[brep.topology.faces[e.face_a].surface_index].surface_type();
        let sb = brep.geometry.surfaces[brep.topology.faces[e.face_b].surface_index].surface_type();
        if sa != SurfaceKind::Plane || sb != SurfaceKind::Plane {
            return false;
        }
        if !seen_vertices.insert(e.v_start) || !seen_vertices.insert(e.v_end) {
            return false; // two selected edges share this vertex
        }
    }
    matched == edge_ids.len()
}

/// Fillet an independent set of plane-plane edges. Precondition:
/// [`is_independent_plane_plane_set`] returned `true` for `edge_ids`.
pub(crate) fn fillet_independent_plane_edges(
    brep: &BRepSolid,
    edge_ids: &[EdgeId],
    radius: f64,
) -> (BRepSolid, Vec<FilletResult>) {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);
    let topo = &brep.topology;
    let selected: HashSet<EdgeId> = edge_ids.iter().copied().collect();

    let face_normal: HashMap<FaceId, Vec3> = faces.iter().map(|f| (f.face_id, f.normal)).collect();

    // Precompute blend geometry for every selected edge.
    let mut blends: HashMap<EdgeId, BlendGeom> = HashMap::new();
    let mut results: Vec<FilletResult> = Vec::new();
    for e in &edges {
        if !selected.contains(&e.edge_id) {
            continue;
        }
        let n_a = face_normal[&e.face_a];
        let n_b = face_normal[&e.face_b];
        let sin2_half = (1.0 + n_a.dot(n_b)) * 0.5;
        if sin2_half < 1e-9 {
            results.push(FilletResult::DegenerateGeometry { edge_id: e.edge_id });
            continue;
        }
        let v_start_pos = topo.vertices[e.v_start].point;
        let v_end_pos = topo.vertices[e.v_end].point;
        let edge_vec = v_end_pos - v_start_pos;
        let edge_len = edge_vec.norm();
        if edge_len < 1e-12 {
            results.push(FilletResult::DegenerateGeometry { edge_id: e.edge_id });
            continue;
        }
        let axis = edge_vec / edge_len;
        let offset = -radius * (n_a + n_b) / (2.0 * sin2_half);
        blends.insert(
            e.edge_id,
            BlendGeom {
                v_start: e.v_start,
                v_end: e.v_end,
                face_a: e.face_a,
                face_b: e.face_b,
                n_a,
                n_b,
                center_start: v_start_pos + offset,
                center_end: v_end_pos + offset,
                axis,
                radius,
            },
        );
        results.push(FilletResult::Success);
    }

    // For each vertex, which selected edge touches it (independent set ⇒
    // at most one).
    let mut edge_at_vertex: HashMap<VertexId, EdgeId> = HashMap::new();
    for (&eid, b) in &blends {
        edge_at_vertex.insert(b.v_start, eid);
        edge_at_vertex.insert(b.v_end, eid);
    }

    let mut new_topo = Topology::new();
    let mut new_geom = GeometryStore::new();
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();
    let mut all_faces: Vec<FaceId> = Vec::new();

    // 1. Rebuild every original face with the correct local modification.
    for face in &faces {
        let n = face.vertex_ids.len();
        let mut loop_positions: Vec<Point3> = Vec::with_capacity(n);

        for i in 0..n {
            let v = face.vertex_ids[i];
            let pos = face.positions[i];
            let Some(&eid) = edge_at_vertex.get(&v) else {
                loop_positions.push(pos);
                continue;
            };
            let b = &blends[&eid];

            if face.face_id == b.face_a {
                loop_positions.push(b.tan_a_at(v)); // side face inset
            } else if face.face_id == b.face_b {
                loop_positions.push(b.tan_b_at(v)); // side face inset
            } else {
                // Cap face: round this corner with the blend arc.
                let prev = face.positions[(i + n - 1) % n];
                let next = face.positions[(i + 1) % n];
                let ta = b.tan_a_at(v);
                let tb = b.tan_b_at(v);
                let (prev_tan, next_tan) = orient_cap_tangents(pos, prev, next, ta, tb);
                let arc = sample_arc(b.center_at(v), prev_tan, next_tan, b.axis, ARC_SEGMENTS);
                loop_positions.extend(arc);
            }
        }

        if loop_positions.len() < 3 {
            continue;
        }

        let surf_idx =
            new_geom.add_surface(Box::new(Plane::from_normal(loop_positions[0], face.normal)));
        let orientation = topo.faces[face.face_id].orientation;
        add_face(
            &loop_positions,
            surf_idx,
            orientation,
            &mut vertex_cache,
            &mut new_topo,
            &mut all_faces,
        );
    }

    // 2. Emit the quarter-cylinder blend for every selected edge.
    for b in blends.values() {
        let ta_s = b.tan_a_at(b.v_start);
        let tb_s = b.tan_b_at(b.v_start);
        let ta_e = b.tan_a_at(b.v_end);
        let tb_e = b.tan_b_at(b.v_end);

        let ring_start = sample_arc(b.center_start, ta_s, tb_s, b.axis, ARC_SEGMENTS);
        let ring_end = sample_arc(b.center_end, ta_e, tb_e, b.axis, ARC_SEGMENTS);

        // Closed loop: start ring (ta_s → tb_s) then end ring reversed
        // (tb_e → ta_e). Winding chosen so the CCW face normal points
        // radially outward (away from the axis, i.e. out of the solid).
        let mut loop_positions = ring_start;
        loop_positions.extend(ring_end.into_iter().rev());

        let ref_dir = (ta_s - b.center_start).normalize();
        let cyl = CylinderSurface {
            center: b.center_start,
            axis: Dir3::new_normalize(b.axis),
            ref_dir: Dir3::new_normalize(ref_dir),
            radius: b.radius,
        };

        // Determine orientation so the tessellated normal (radially
        // outward for a `Forward` cylinder) faces out of the solid. The
        // solid lies on the axis side, so outward == radial-out ==
        // `Forward`; but if the loop happens to wind the other way we
        // flip via `Reversed`.
        let orientation = cylinder_orientation(&loop_positions, &b.axis, &b.center_start);
        let surf_idx = new_geom.add_surface(Box::new(cyl));
        add_face(
            &loop_positions,
            surf_idx,
            orientation,
            &mut vertex_cache,
            &mut new_topo,
            &mut all_faces,
        );
    }

    pair_twin_half_edges(&mut new_topo);
    let shell = new_topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = new_topo.add_solid(shell);

    (
        BRepSolid {
            topology: new_topo,
            geometry: new_geom,
            solid_id,
        },
        results,
    )
}

/// Decide which of the two tangent points continues the loop toward the
/// previous vertex and which toward the next, so the inserted arc keeps
/// the face's boundary orientation.
fn orient_cap_tangents(
    pos: Point3,
    prev: Point3,
    next: Point3,
    ta: Point3,
    tb: Point3,
) -> (Point3, Point3) {
    let dir_prev = (prev - pos).normalize();
    let to_ta = (ta - pos).normalize();
    if to_ta.dot(dir_prev) > 0.0 {
        (ta, tb)
    } else {
        let _ = next;
        (tb, ta)
    }
}

/// Sample the short circular arc from `from` to `to` about `center`,
/// returning `segments + 1` points (inclusive of both ends). `axis` only
/// fixes the rotation plane; the traversal always takes the ≤π arc.
fn sample_arc(
    center: Point3,
    from: Point3,
    to: Point3,
    axis: Vec3,
    segments: usize,
) -> Vec<Point3> {
    let r_vec = from - center;
    let r = r_vec.norm();
    if r < 1e-12 {
        return vec![from, to];
    }
    let u = r_vec / r;
    let mut v = axis.cross(u);
    if v.norm() < 1e-12 {
        return vec![from, to];
    }
    v = v.normalize();
    let d = to - center;
    let ang = d.dot(v).atan2(d.dot(u));
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = ang * (i as f64 / segments as f64);
        pts.push(center + (u * t.cos() + v * t.sin()) * r);
    }
    pts
}

/// Pick the face orientation whose tessellated (radially outward) normal
/// points out of the solid for a convex blend cylinder.
fn cylinder_orientation(loop_positions: &[Point3], axis: &Vec3, center: &Point3) -> Orientation {
    // Newell normal of the polygon loop.
    let n = loop_positions.len();
    let mut normal = Vec3::zeros();
    for i in 0..n {
        let a = loop_positions[i];
        let b = loop_positions[(i + 1) % n];
        normal.x += (a.y - b.y) * (a.z + b.z);
        normal.y += (a.z - b.z) * (a.x + b.x);
        normal.z += (a.x - b.x) * (a.y + b.y);
    }
    // Radially-outward reference at the loop centroid.
    let mut c = Vec3::zeros();
    for p in loop_positions {
        c += p.to_vec();
    }
    let centroid = Point3::from(c / n as f64);
    let d = centroid - *center;
    let along = d.dot(axis);
    let radial = d - *axis * along;
    if normal.dot(radial) >= 0.0 {
        Orientation::Forward
    } else {
        Orientation::Reversed
    }
}

/// Add a planar/cylindrical face to the working topology from an ordered
/// list of boundary positions, welding coincident vertices via the cache.
fn add_face(
    positions: &[Point3],
    surf_idx: usize,
    orientation: Orientation,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    all_faces: &mut Vec<FaceId>,
) {
    let verts: Vec<VertexId> = positions
        .iter()
        .map(|p| {
            let key = quantize(*p);
            *vertex_cache
                .entry(key)
                .or_insert_with(|| new_topo.add_vertex(*p))
        })
        .collect();
    let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
    let loop_id = new_topo.add_loop(&hes);
    let face_id = new_topo.add_face(loop_id, surf_idx, orientation);
    all_faces.push(face_id);
}
