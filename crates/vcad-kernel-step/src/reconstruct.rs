//! Analytic edge reconstruction for STEP export.
//!
//! The topology reaching the writer carries no curve references: a circular
//! intersection edge (cylinder ∩ plane) arrives as a chain of chord edges
//! between tessellation vertices, and a pristine primitive's circular
//! boundary arrives as a single closed edge with one vertex. Emitting those
//! as LINEs pairs exact analytic surfaces with polyline boundaries that
//! deviate from the surfaces by the chord sagitta, so conforming importers
//! (with the spec-conventional 1e-6 uncertainty) fail to sew edge to surface
//! and drop the face.
//!
//! This module reconstructs the analytic edges before writing:
//! - chains of consecutive edges between the same face pair whose interior
//!   vertices have total degree 2 are merged into one LINE (collinear) or one
//!   CIRCLE arc (concyclic, verified against every chain vertex);
//! - closed chains (full circles) are split into two semicircular arcs, since
//!   many importers reject a closed edge with a single vertex;
//! - a single closed edge bounding a plane and a cylinder/cone (the pristine
//!   primitive case) is rebuilt as a circle from the adjacent surfaces and
//!   split at a synthesized antipodal vertex.
//!
//! Everything that fails verification falls back to the existing per-edge
//! LINE path — the reconstruction is strictly opportunistic and fail-safe.

use std::collections::HashMap;

use vcad_kernel_geom::{ConeSurface, CylinderSurface, Plane, SurfaceKind};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, HalfEdgeId, Orientation, VertexId};

/// Endpoint of a reconstructed segment: an existing topology vertex, or an
/// index into [`ChainPlan::synth_points`] for a vertex synthesized to split a
/// closed circle into two arcs.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SegEnd {
    /// An existing topology vertex.
    Topo(VertexId),
    /// Index into the owning chain's `synth_points`.
    Synth(usize),
}

/// One STEP edge replacing a run of topology edges.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Segment {
    /// Start endpoint (in chain-forward direction).
    pub start: SegEnd,
    /// End endpoint.
    pub end: SegEnd,
}

/// The analytic curve a chain was verified to lie on.
#[derive(Debug, Clone)]
pub(crate) enum ChainGeom {
    /// All chain vertices are collinear; the chain becomes one LINE edge.
    Line,
    /// All chain vertices lie on this circle. Chain-forward traversal is
    /// counterclockwise about `normal`.
    Arc {
        /// Circle center.
        center: Point3,
        /// Circle radius.
        radius: f64,
        /// Circle plane normal; chain-forward is CCW about it.
        normal: Dir3,
    },
}

/// A verified run of topology edges to be replaced by analytic segments.
#[derive(Debug)]
pub(crate) struct ChainPlan {
    /// Member edges (unordered; used for run-length accounting).
    pub edges: Vec<EdgeId>,
    /// Ordered vertices along the chain. Open: `edges.len() + 1` entries.
    /// Closed: `edges.len()` entries with an implicit wrap to the first.
    pub vertices: Vec<VertexId>,
    /// Whether the chain closes on itself (a full circle).
    pub closed: bool,
    /// Verified analytic geometry.
    pub geom: ChainGeom,
    /// Replacement segments in chain-forward order.
    pub segments: Vec<Segment>,
    /// Points for synthesized split vertices referenced by `SegEnd::Synth`.
    pub synth_points: Vec<Point3>,
}

/// The full merge plan for one solid.
#[derive(Debug, Default)]
pub(crate) struct EdgeMergePlan {
    /// Verified chains.
    pub chains: Vec<ChainPlan>,
    /// Maps every member edge to its chain index.
    pub edge_chain: HashMap<EdgeId, usize>,
}

/// Absolute + relative tolerance for on-curve verification.
pub(crate) fn fit_tol(scale: f64) -> f64 {
    1e-6 * (1.0 + scale.abs())
}

/// Build the edge merge plan for a solid.
pub(crate) fn plan_edge_merges(solid: &BRepSolid) -> EdgeMergePlan {
    let topo = &solid.topology;

    // Per-edge endpoints and adjacent-face pair (sorted so (a,b) == (b,a)).
    struct EdgeInfo {
        v0: VertexId,
        v1: VertexId,
        faces: Option<(vcad_kernel_topo::FaceId, vcad_kernel_topo::FaceId)>,
    }
    let mut info: HashMap<EdgeId, EdgeInfo> = HashMap::new();
    for (eid, edge) in &topo.edges {
        let he = edge.half_edge;
        let v0 = topo.half_edges[he].origin;
        // half_edge_dest panics on a half-edge without `next`; guard.
        if topo.half_edges[he].next.is_none() {
            continue;
        }
        let v1 = topo.half_edge_dest(he);
        let faces = match topo.edge_faces(eid) {
            (Some(a), Some(b)) => {
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                Some((a, b))
            }
            _ => None,
        };
        info.insert(eid, EdgeInfo { v0, v1, faces });
    }

    // Global vertex degree: edges plus orphan half-edges (edge == None). A
    // vertex is only merged through when nothing else in the model needs a
    // break there.
    let mut vertex_edges: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
    let mut orphan_touch: HashMap<VertexId, usize> = HashMap::new();
    for (eid, ei) in &info {
        if ei.v0 != ei.v1 {
            vertex_edges.entry(ei.v0).or_default().push(*eid);
            vertex_edges.entry(ei.v1).or_default().push(*eid);
        }
    }
    for (he_id, he) in &topo.half_edges {
        if he.edge.is_none() {
            *orphan_touch.entry(he.origin).or_default() += 1;
            if topo.half_edges[he_id].next.is_some() {
                *orphan_touch.entry(topo.half_edge_dest(he_id)).or_default() += 1;
            }
        }
    }

    // A vertex we may merge through: exactly two incident edges, no orphan
    // half-edges, and both edges bound the same (fully paired) face pair.
    let merge_through = |v: VertexId| -> Option<(EdgeId, EdgeId)> {
        if orphan_touch.contains_key(&v) {
            return None;
        }
        let es = vertex_edges.get(&v)?;
        if es.len() != 2 {
            return None;
        }
        let (a, b) = (es[0], es[1]);
        let fa = info.get(&a)?.faces?;
        let fb = info.get(&b)?.faces?;
        (fa == fb).then_some((a, b))
    };

    // Walk maximal chains through merge-through vertices.
    let mut visited: HashMap<EdgeId, bool> = HashMap::new();
    let mut raw_chains: Vec<(Vec<EdgeId>, Vec<VertexId>, bool)> = Vec::new();
    for (eid, ei) in &info {
        if visited.contains_key(eid) || ei.v0 == ei.v1 || ei.faces.is_none() {
            continue;
        }
        // Only start from an edge that can merge with a neighbor at all.
        if merge_through(ei.v0).is_none() && merge_through(ei.v1).is_none() {
            continue;
        }
        let mut chain_edges = vec![*eid];
        let mut chain_verts = vec![ei.v0, ei.v1];
        visited.insert(*eid, true);

        // Extend forward from the tail, then backward from the head.
        loop {
            let tail = *chain_verts.last().unwrap();
            if tail == chain_verts[0] {
                break; // closed
            }
            let Some((a, b)) = merge_through(tail) else {
                break;
            };
            let cur = *chain_edges.last().unwrap();
            let next = if a == cur { b } else { a };
            if visited.contains_key(&next) {
                break;
            }
            let ni = &info[&next];
            let nv = if ni.v0 == tail { ni.v1 } else { ni.v0 };
            visited.insert(next, true);
            chain_edges.push(next);
            chain_verts.push(nv);
        }
        let closed = chain_verts.len() > 2 && chain_verts[0] == *chain_verts.last().unwrap();
        if !closed {
            loop {
                let head = chain_verts[0];
                let Some((a, b)) = merge_through(head) else {
                    break;
                };
                let cur = chain_edges[0];
                let next = if a == cur { b } else { a };
                if visited.contains_key(&next) {
                    break;
                }
                let ni = &info[&next];
                let nv = if ni.v0 == head { ni.v1 } else { ni.v0 };
                visited.insert(next, true);
                chain_edges.insert(0, next);
                chain_verts.insert(0, nv);
            }
        }
        let closed = chain_verts.len() > 2 && chain_verts[0] == *chain_verts.last().unwrap();
        if closed {
            chain_verts.pop(); // implicit wrap
        }
        if chain_edges.len() >= 2 {
            raw_chains.push((chain_edges, chain_verts, closed));
        }
    }

    let mut plan = EdgeMergePlan::default();

    for (edges, vertices, closed) in raw_chains {
        let points: Vec<Point3> = vertices.iter().map(|v| topo.vertices[*v].point).collect();
        let geom = classify_chain(&points, closed);
        let Some(geom) = geom else { continue };
        let segments = match (&geom, closed) {
            (_, false) => vec![Segment {
                start: SegEnd::Topo(vertices[0]),
                end: SegEnd::Topo(*vertices.last().unwrap()),
            }],
            (ChainGeom::Arc { .. }, true) => {
                let mid = vertices.len() / 2;
                vec![
                    Segment {
                        start: SegEnd::Topo(vertices[0]),
                        end: SegEnd::Topo(vertices[mid]),
                    },
                    Segment {
                        start: SegEnd::Topo(vertices[mid]),
                        end: SegEnd::Topo(vertices[0]),
                    },
                ]
            }
            (ChainGeom::Line, true) => continue, // impossible; be safe
        };
        let idx = plan.chains.len();
        for e in &edges {
            plan.edge_chain.insert(*e, idx);
        }
        plan.chains.push(ChainPlan {
            edges,
            vertices,
            closed,
            geom,
            segments,
            synth_points: Vec::new(),
        });
    }

    // Pristine-primitive case: a single closed edge (start == end) bounding a
    // plane and a cylinder/cone. Rebuild the circle from the surfaces.
    for (eid, ei) in &info {
        if ei.v0 != ei.v1 || plan.edge_chain.contains_key(eid) {
            continue;
        }
        let Some(chain) = single_closed_circle(solid, *eid, ei.v0) else {
            continue;
        };
        let idx = plan.chains.len();
        plan.edge_chain.insert(*eid, idx);
        plan.chains.push(chain);
    }

    validate_loop_consecutiveness(solid, &mut plan);
    plan
}

/// Verify a candidate chain's points against a line or circle.
///
/// Also used for orphan half-edge runs (boolean sewing leaves the chord
/// chains of curved intersections as orphan half-edges with no twins). The
/// classification depends only on the point sequence, never on which side of
/// the boundary is being walked, so the two loops sharing a boundary always
/// reconstruct identically — a requirement for importers to sew them.
pub(crate) fn classify_chain(points: &[Point3], closed: bool) -> Option<ChainGeom> {
    let n = points.len();
    if n < 3 {
        return None; // only a 2-edge closed chain lands here; nothing to fit
    }

    if !closed {
        // Collinear?
        let p0 = points[0];
        let pk = points[n - 1];
        let d = pk - p0;
        let len = d.norm();
        if len > 1e-9 {
            let dir = d / len;
            let tol = fit_tol(len);
            let collinear = points.iter().all(|p| {
                let w = *p - p0;
                (w - dir * w.dot(dir)).norm() < tol
            });
            if collinear {
                return Some(ChainGeom::Line);
            }
        }
    }

    // Any 3 points are concyclic and 4-6 exactly-concyclic points can be a
    // genuine regular polygon; demand enough samples that "these all lie on
    // one circle to 1e-6" is real evidence of a tessellated circle.
    let min_pts = if closed { 6 } else { 5 };
    if n < min_pts {
        return None;
    }
    circle_fit(points, closed)
}

/// Canonical split indices for a closed circular run: the lexicographically
/// smallest point and the point farthest from it. Both loops sharing the
/// boundary walk the same vertices (in opposite orders), so this yields the
/// same two split vertices on both sides regardless of traversal direction.
pub(crate) fn canonical_split(points: &[Point3]) -> (usize, usize) {
    let lex = |p: &Point3, q: &Point3| {
        p.x.partial_cmp(&q.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(p.y.partial_cmp(&q.y).unwrap_or(std::cmp::Ordering::Equal))
            .then(p.z.partial_cmp(&q.z).unwrap_or(std::cmp::Ordering::Equal))
    };
    let mut a = 0;
    for i in 1..points.len() {
        if lex(&points[i], &points[a]) == std::cmp::Ordering::Less {
            a = i;
        }
    }
    let mut b = if a == 0 { 1 } else { 0 };
    let mut best = (points[b] - points[a]).norm_squared();
    for (i, p) in points.iter().enumerate() {
        if i == a {
            continue;
        }
        let d = (*p - points[a]).norm_squared();
        if d > best + 1e-18
            || ((d - best).abs() <= 1e-18 && lex(p, &points[b]) == std::cmp::Ordering::Less)
        {
            best = d;
            b = i;
        }
    }
    (a, b)
}

/// Fit a circle through the points and verify every point lies on it, with
/// monotone CCW traversal. Returns the chain-forward-oriented circle.
fn circle_fit(points: &[Point3], closed: bool) -> Option<ChainGeom> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    // Three well-spread sample points.
    let (i0, i1, i2) = (0, n / 3, (2 * n) / 3);
    if i0 == i1 || i1 == i2 {
        return None;
    }
    let (a, b, c) = (points[i0], points[i1], points[i2]);
    let u = b - a;
    let v = c - a;
    let w = u.cross(v);
    let w2 = w.norm_squared();
    if w2 < 1e-18 {
        return None;
    }
    // Circumcenter of the triangle (a, b, c).
    let center = a + (v.cross(w) * u.norm_squared() + w.cross(u) * v.norm_squared()) / (2.0 * w2);
    let radius = (a - center).norm();
    if radius < 1e-9 {
        return None;
    }
    let mut normal = w / w2.sqrt();
    let tol = fit_tol(radius);

    // Every point on the circle: coplanar and equidistant.
    for p in points {
        let r = *p - center;
        if (r.norm() - radius).abs() > tol || r.dot(normal).abs() > tol {
            return None;
        }
    }

    // Orient the normal so chain order is CCW, then require monotone angles
    // (guards against zigzag point sets that happen to be concyclic).
    let mut signed = Vec3::zeros();
    let last = if closed { n } else { n - 1 };
    for i in 0..last {
        let p = points[i] - center;
        let q = points[(i + 1) % n] - center;
        signed += p.cross(q);
    }
    if signed.dot(normal) < 0.0 {
        normal = -normal;
    }
    let x_dir = (points[0] - center) / radius;
    let y_dir = normal.cross(x_dir);
    let mut prev = 0.0_f64;
    let mut total = 0.0_f64;
    for i in 1..=last {
        let p = points[i % n] - center;
        let mut ang = p.dot(y_dir).atan2(p.dot(x_dir));
        if i == n {
            ang = 2.0 * std::f64::consts::PI; // wrap of a closed chain
        } else if ang <= 0.0 {
            ang += 2.0 * std::f64::consts::PI;
        }
        if ang <= prev + 1e-12 {
            return None;
        }
        total = ang;
        prev = ang;
    }
    if closed && (total - 2.0 * std::f64::consts::PI).abs() > 1e-6 {
        return None;
    }

    Some(ChainGeom::Arc {
        center,
        radius,
        normal: Dir3::new_normalize(normal),
    })
}

/// Rebuild a single closed edge (one vertex, start == end) bounding a plane
/// and a cylinder or cone as a full circle split at a synthesized antipode.
fn single_closed_circle(solid: &BRepSolid, eid: EdgeId, v: VertexId) -> Option<ChainPlan> {
    let topo = &solid.topology;
    let geom = &solid.geometry;

    let he1 = topo.edges[eid].half_edge;
    let he2 = topo.half_edges[he1].twin?;
    let face_of = |he: HalfEdgeId| topo.half_edges[he].loop_id.and_then(|l| topo.loops[l].face);
    let f1 = face_of(he1)?;
    let f2 = face_of(he2)?;

    let surf = |f: vcad_kernel_topo::FaceId| &geom.surfaces[topo.faces[f].surface_index];
    // Identify which side is the plane and which is the curved surface.
    let (planar_he, planar_face, curved_face) =
        match (surf(f1).surface_type(), surf(f2).surface_type()) {
            (SurfaceKind::Plane, SurfaceKind::Cylinder | SurfaceKind::Cone) => (he1, f1, f2),
            (SurfaceKind::Cylinder | SurfaceKind::Cone, SurfaceKind::Plane) => (he2, f2, f1),
            _ => return None,
        };
    let plane = surf(planar_face).as_any().downcast_ref::<Plane>()?;
    let p = topo.vertices[v].point;

    // Circle from the curved surface's axis and the vertex point.
    let (axis, center, radius) = match surf(curved_face).surface_type() {
        SurfaceKind::Cylinder => {
            let cyl = surf(curved_face)
                .as_any()
                .downcast_ref::<CylinderSurface>()?;
            let axis = *cyl.axis.as_ref();
            let center = cyl.center + axis * (p - cyl.center).dot(axis);
            let radius = (p - center).norm();
            if (radius - cyl.radius).abs() > fit_tol(cyl.radius) {
                return None;
            }
            (axis, center, radius)
        }
        SurfaceKind::Cone => {
            let cone = surf(curved_face).as_any().downcast_ref::<ConeSurface>()?;
            let axis = *cone.axis.as_ref();
            let h = (p - cone.apex).dot(axis);
            let center = cone.apex + axis * h;
            let radius = (p - center).norm();
            let expected = h.abs() * cone.half_angle.tan();
            if (radius - expected).abs() > fit_tol(radius) {
                return None;
            }
            (axis, center, radius)
        }
        _ => return None,
    };
    if radius < 1e-9 {
        return None;
    }
    // The plane must be perpendicular to the axis (else the intersection is
    // an ellipse, not this circle).
    if plane.normal_dir.as_ref().dot(axis).abs() < 1.0 - 1e-9 {
        return None;
    }

    // Orientation. The planar face's loop winds CCW about the face's outward
    // normal when it is the outer bound, CW when it is an inner bound (hole).
    // Derive the canonical half-edge's direction from that convention, since
    // the topology itself carries no curve direction.
    let planar_loop = topo.half_edges[planar_he].loop_id?;
    let face = &topo.faces[planar_face];
    let sign = if face.orientation == Orientation::Forward {
        1.0
    } else {
        -1.0
    };
    let n_out = *plane.normal_dir.as_ref() * sign;
    let planar_dir = if face.outer_loop == planar_loop {
        n_out
    } else {
        -n_out
    };
    // Chain-forward is the canonical half-edge's direction.
    let canonical_normal = if planar_he == topo.edges[eid].half_edge {
        planar_dir
    } else {
        -planar_dir
    };
    // Snap to the exact axis direction.
    let normal = axis * canonical_normal.dot(axis).signum();

    let antipode = center + (center - p);
    Some(ChainPlan {
        edges: vec![eid],
        vertices: vec![v],
        closed: true,
        geom: ChainGeom::Arc {
            center,
            radius,
            normal: Dir3::new_normalize(normal),
        },
        segments: vec![
            Segment {
                start: SegEnd::Topo(v),
                end: SegEnd::Synth(0),
            },
            Segment {
                start: SegEnd::Synth(0),
                end: SegEnd::Topo(v),
            },
        ],
        synth_points: vec![antipode],
    })
}

/// Drop any chain whose edges do not appear as one consecutive (cyclic) run
/// covering the whole chain in every loop that touches it. The writer replaces
/// runs wholesale, so a fragmented appearance would corrupt that loop.
fn validate_loop_consecutiveness(solid: &BRepSolid, plan: &mut EdgeMergePlan) {
    let topo = &solid.topology;
    let mut dead: Vec<bool> = vec![false; plan.chains.len()];

    for (loop_id, _) in &topo.loops {
        let membership: Vec<Option<usize>> = topo
            .loop_half_edges(loop_id)
            .map(|he| {
                topo.half_edges[he]
                    .edge
                    .and_then(|e| plan.edge_chain.get(&e).copied())
            })
            .collect();
        let n = membership.len();
        let mut counts: HashMap<usize, (usize, usize)> = HashMap::new(); // idx -> (count, blocks)
        for i in 0..n {
            if let Some(ci) = membership[i] {
                let entry = counts.entry(ci).or_default();
                entry.0 += 1;
                let prev = membership[(i + n - 1) % n];
                if prev != Some(ci) {
                    entry.1 += 1;
                }
            }
        }
        for (ci, (count, blocks)) in counts {
            let whole_loop = count == n && blocks == 0; // loop is entirely this chain
            if !(whole_loop || blocks == 1) || count != plan.chains[ci].edges.len() {
                dead[ci] = true;
            }
        }
    }

    if dead.iter().any(|d| *d) {
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(plan.chains.len());
        let mut kept = Vec::new();
        for (i, chain) in plan.chains.drain(..).enumerate() {
            if dead[i] {
                remap.push(None);
            } else {
                remap.push(Some(kept.len()));
                kept.push(chain);
            }
        }
        plan.chains = kept;
        plan.edge_chain = plan
            .edge_chain
            .iter()
            .filter_map(|(e, ci)| remap[*ci].map(|ni| (*e, ni)))
            .collect();
    }
}
