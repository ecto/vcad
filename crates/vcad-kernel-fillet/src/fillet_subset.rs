//! Correct per-edge fillet for a set of plane-plane edges: independent
//! edges and **chains of adjacent edges joined by miter corners**.
//!
//! The shared-`trims` pipeline in [`crate::fillet_curved`] insets *every*
//! face of the solid by `radius` — which is only correct when *every* edge
//! is being filleted. For a proper subset it shrinks the whole body and
//! emits blends for the selected edges alone, producing a malformed,
//! non-watertight solid (a single-edge cube fillet came out at ~522 mm³
//! instead of ~995 mm³).
//!
//! This module implements the geometrically correct construction for
//! **plane-plane** edges where each vertex is touched by at most **two**
//! selected edges:
//!
//! * An edge endpoint touched by no other selected edge is a *cap* end:
//!   the two faces adjacent to the edge (its *side* faces) are inset only
//!   along that edge, a quarter-cylinder blend replaces the edge, and the
//!   face that merely touches the endpoint (the *cap* face) has that one
//!   corner rounded by the blend's terminating arc.
//!
//! * An endpoint shared by **two** selected edges is a *miter corner*.
//!   When the two blends have equal radius and the corner is mirror
//!   symmetric — the edges share exactly one face `S`, and their other
//!   faces `A`, `C` make equal dihedral angles with `S` (every prism-like
//!   corner: boxes, L-brackets, top rims) — the two blend cylinders
//!   intersect **exactly** in a planar curve on the bisector plane of the
//!   two edge directions. The corner therefore needs no spherical patch:
//!   each cylinder is trimmed at the miter plane, both trimmed ends share
//!   the *same* sampled curve (welding is exact by construction), the
//!   shared face's corner collapses to the curve's end on `S`, and faces
//!   `A`/`C` corner at the curve's other end (which lies on `A ∩ C`, the
//!   surviving third edge). Closed rings of edges (a box's top rim) are
//!   chains whose every vertex is a miter.
//!
//! Every face, cap arc, miter curve and cylinder ring is generated from
//! the same tangent-point / arc-sampling formulas, so coincident points
//! quantize to a single welded vertex and the result is watertight.

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

/// Tolerance for the miter-symmetry test: the two non-shared faces must
/// make equal dihedral angles with the shared face (compared via normal
/// dot products) for the bisector-plane trim to be an exact intersection
/// curve of both cylinders.
const MITER_SYM_TOL: f64 = 1e-7;

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
    /// Unit edge direction pointing away from endpoint `v`.
    fn dir_away_from(&self, v: VertexId) -> Vec3 {
        if v == self.v_start {
            self.axis
        } else {
            -self.axis
        }
    }
}

/// True when every `edge_id` names a plane-plane manifold edge, no vertex
/// is touched by more than two of them, and every shared vertex is a
/// symmetric trihedral miter corner (see the module docs). This is the
/// domain the correct subset builder covers; callers fall back to the
/// legacy pipeline otherwise.
pub(crate) fn is_plane_plane_chain_set(brep: &BRepSolid, edge_ids: &[EdgeId], radius: f64) -> bool {
    if edge_ids.is_empty() {
        return false;
    }
    let selected: HashSet<EdgeId> = edge_ids.iter().copied().collect();
    if selected.len() != edge_ids.len() {
        return false;
    }
    let edges = extract_edges(brep);
    let faces = extract_faces(brep);
    let face_normal: HashMap<FaceId, Vec3> = faces.iter().map(|f| (f.face_id, f.normal)).collect();

    // Faces incident to each vertex (for the trihedral test).
    let mut faces_at_vertex: HashMap<VertexId, HashSet<FaceId>> = HashMap::new();
    for f in &faces {
        for &v in &f.vertex_ids {
            faces_at_vertex.entry(v).or_default().insert(f.face_id);
        }
    }

    let mut matched = 0usize;
    let mut at_vertex: HashMap<VertexId, Vec<&crate::topology::EdgeInfo>> = HashMap::new();
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
        at_vertex.entry(e.v_start).or_default().push(e);
        at_vertex.entry(e.v_end).or_default().push(e);
    }
    if matched != edge_ids.len() {
        return false;
    }

    for (&v, touching) in &at_vertex {
        match touching.len() {
            1 => {}
            2 => {
                let (e1, e2) = (touching[0], touching[1]);
                // Exactly one shared face.
                let f1: HashSet<FaceId> = [e1.face_a, e1.face_b].into_iter().collect();
                let f2: HashSet<FaceId> = [e2.face_a, e2.face_b].into_iter().collect();
                let shared: Vec<FaceId> = f1.intersection(&f2).copied().collect();
                if shared.len() != 1 {
                    return false;
                }
                let s = shared[0];
                let a = if e1.face_a == s { e1.face_b } else { e1.face_a };
                let c = if e2.face_a == s { e2.face_b } else { e2.face_a };
                if a == c {
                    return false;
                }
                // Trihedral corner: exactly the three faces S, A, C meet at v.
                match faces_at_vertex.get(&v) {
                    Some(fs)
                        if fs.len() == 3
                            && fs.contains(&s)
                            && fs.contains(&a)
                            && fs.contains(&c) => {}
                    _ => return false,
                }
                // Mirror symmetry: equal dihedral angles against S.
                let (ns, na, nc) = (face_normal[&s], face_normal[&a], face_normal[&c]);
                if (na.dot(ns) - nc.dot(ns)).abs() > MITER_SYM_TOL {
                    return false;
                }
                // Edge directions away from v must not be parallel (the
                // miter plane's normal is their difference).
                let d1 = edge_dir_away(brep, e1, v);
                let d2 = edge_dir_away(brep, e2, v);
                if (d1 - d2).norm() < 1e-9 {
                    return false;
                }
                // Both edges must be long enough that the miter (which
                // consumes up to ~radius of axial length) cannot cross the
                // opposite end's trim.
                for e in [e1, e2] {
                    let len = (brep.topology.vertices[e.v_end].point
                        - brep.topology.vertices[e.v_start].point)
                        .norm();
                    if len <= 2.0 * radius {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// Unit direction of edge `e` pointing away from its endpoint `v`.
fn edge_dir_away(brep: &BRepSolid, e: &crate::topology::EdgeInfo, v: VertexId) -> Vec3 {
    let a = brep.topology.vertices[e.v_start].point;
    let b = brep.topology.vertices[e.v_end].point;
    let d = (b - a).normalize();
    if v == e.v_start {
        d
    } else {
        -d
    }
}

/// A miter corner: the shared face and the trimmed-intersection curve both
/// blend cylinders terminate on. `ring[0]` lies on `A ∩ C` (the surviving
/// third edge of the corner); `ring[last]` lies on the shared face `S`.
struct MiterCorner {
    shared_face: FaceId,
    ring: Vec<Point3>,
}

/// Sample the miter curve at vertex `v` from blend `b` (either blend works
/// by symmetry): the blend's arc from its tangency on its non-shared face
/// to its tangency on `shared`, with each sample slid along the axis onto
/// the miter plane `{x : (x − v)·m = 0}`.
fn sample_miter_ring(
    b: &BlendGeom,
    v: VertexId,
    v_pos: Point3,
    shared: FaceId,
    m: Vec3,
) -> Vec<Point3> {
    let center = b.center_at(v);
    let r = b.radius;
    let (from, to) = if b.face_a == shared {
        (b.tan_b_at(v), b.tan_a_at(v))
    } else {
        (b.tan_a_at(v), b.tan_b_at(v))
    };
    let u = (from - center) / r;
    let mut w = b.axis.cross(u);
    let wn = w.norm();
    if wn < 1e-12 {
        return vec![from, to];
    }
    w = w / wn;
    let d = to - center;
    let ang = d.dot(w).atan2(d.dot(u));
    let am = b.axis.dot(m);
    let mut pts = Vec::with_capacity(ARC_SEGMENTS + 1);
    for i in 0..=ARC_SEGMENTS {
        let t = ang * (i as f64 / ARC_SEGMENTS as f64);
        let radial = u * t.cos() + w * t.sin();
        let p0 = center + radial * r;
        // Slide along the axis onto the miter plane.
        let s = (v_pos - p0).dot(m) / am;
        pts.push(p0 + b.axis * s);
    }
    pts
}

/// Fillet a chain set of plane-plane edges. Precondition:
/// [`is_plane_plane_chain_set`] returned `true` for `edge_ids`.
pub(crate) fn fillet_plane_chain_edges(
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

    // Selected edges touching each vertex (1 = cap end, 2 = miter corner).
    let mut edges_at_vertex: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
    for (&eid, b) in &blends {
        edges_at_vertex.entry(b.v_start).or_default().push(eid);
        edges_at_vertex.entry(b.v_end).or_default().push(eid);
    }
    for list in edges_at_vertex.values_mut() {
        list.sort(); // determinism: the miter ring samples from the lower id
    }

    // Precompute every miter corner's shared face and trim curve. Both
    // blends at the corner reference the SAME point list, so their welding
    // along the miter is exact by construction.
    let mut miters: HashMap<VertexId, MiterCorner> = HashMap::new();
    for (&v, eids) in &edges_at_vertex {
        if eids.len() != 2 {
            continue;
        }
        let (b1, b2) = (&blends[&eids[0]], &blends[&eids[1]]);
        let f1: HashSet<FaceId> = [b1.face_a, b1.face_b].into_iter().collect();
        let shared = if f1.contains(&b2.face_a) {
            b2.face_a
        } else {
            b2.face_b
        };
        let v_pos = topo.vertices[v].point;
        let m = (b1.dir_away_from(v) - b2.dir_away_from(v)).normalize();
        let ring = sample_miter_ring(b1, v, v_pos, shared, m);
        miters.insert(
            v,
            MiterCorner {
                shared_face: shared,
                ring,
            },
        );
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
            let Some(eids) = edges_at_vertex.get(&v) else {
                loop_positions.push(pos);
                continue;
            };

            if let Some(corner) = miters.get(&v) {
                // Miter corner: the shared face's corner collapses to the
                // curve's end on S; the two non-shared side faces (and no
                // others — the corner is trihedral) corner at the curve's
                // end on A ∩ C.
                if face.face_id == corner.shared_face {
                    loop_positions.push(*corner.ring.last().unwrap());
                } else {
                    loop_positions.push(corner.ring[0]);
                }
                continue;
            }

            // Cap end: exactly one selected edge at this vertex.
            let b = &blends[&eids[0]];
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

    // 2. Emit the blend cylinder for every selected edge. Each end's ring
    //    runs tangent-A → tangent-B: a cap end uses the quarter arc in the
    //    endpoint plane, a mitered end uses the corner's trim curve
    //    (oriented so it starts on this blend's face_a side).
    for b in blends.values() {
        let ring_at = |v: VertexId, center: Point3, ta: Point3, tb: Point3| -> Vec<Point3> {
            match miters.get(&v) {
                Some(corner) => {
                    // ring[0] is on A ∩ C (this blend's non-shared side),
                    // ring[last] on the shared face.
                    if b.face_a == corner.shared_face {
                        corner.ring.iter().rev().copied().collect()
                    } else {
                        corner.ring.clone()
                    }
                }
                None => sample_arc(center, ta, tb, b.axis, ARC_SEGMENTS),
            }
        };
        let ring_start = ring_at(
            b.v_start,
            b.center_start,
            b.tan_a_at(b.v_start),
            b.tan_b_at(b.v_start),
        );
        let ring_end = ring_at(
            b.v_end,
            b.center_end,
            b.tan_a_at(b.v_end),
            b.tan_b_at(b.v_end),
        );

        // Closed loop: start ring (ta_s → tb_s) then end ring reversed
        // (tb_e → ta_e). Winding chosen so the CCW face normal points
        // radially outward (away from the axis, i.e. out of the solid).
        let mut loop_positions = ring_start;
        loop_positions.extend(ring_end.into_iter().rev());

        let ref_dir = (b.tan_a_at(b.v_start) - b.center_start).normalize();
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
pub(crate) fn orient_cap_tangents(
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
pub(crate) fn sample_arc(
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
pub(crate) fn add_face(
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
