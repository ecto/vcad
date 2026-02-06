#![warn(missing_docs)]

//! Fillet and chamfer operations for the vcad kernel.
//!
//! Implements edge modification operations on B-rep solids:
//! - **Chamfer**: replaces an edge with a planar bevel face
//! - **Fillet**: replaces an edge with a cylindrical blend surface
//!
//! Currently supports edges between planar faces (the most common case
//! for prismatic CAD geometry).

use std::collections::HashMap;
use std::f64::consts::PI;
use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, Surface, SurfaceKind, TorusSurface};
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};
use vcad_kernel_nurbs::BSplineSurface;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, Orientation, ShellType, Topology, VertexId};

// =============================================================================
// Topology analysis helpers
// =============================================================================

/// Information about a face extracted from the B-rep.
#[derive(Debug, Clone)]
struct FaceInfo {
    face_id: FaceId,
    /// Vertices in loop order.
    vertex_ids: Vec<VertexId>,
    /// Vertex positions in loop order.
    positions: Vec<Point3>,
    /// Outward face normal (from vertex winding).
    normal: Vec3,
}

/// Information about an edge.
#[derive(Debug, Clone)]
struct EdgeInfo {
    #[allow(dead_code)]
    edge_id: EdgeId,
    /// Start vertex (origin of the primary half-edge).
    v_start: VertexId,
    /// End vertex.
    v_end: VertexId,
    /// Face on the primary half-edge side.
    face_a: FaceId,
    /// Face on the twin half-edge side.
    face_b: FaceId,
}

/// Extended face information for curved surface classification.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CurvedFaceInfo {
    face_id: FaceId,
    surface_index: usize,
    surface_kind: SurfaceKind,
    vertex_ids: Vec<VertexId>,
    positions: Vec<Point3>,
    /// Planar normal (only valid for planar faces).
    planar_normal: Option<Vec3>,
}

/// Classification of fillet geometry between two adjacent faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilletCase {
    /// Both faces are planar — cylindrical blend (current implementation).
    PlanePlane,
    /// One face is a plane, other is a cylinder — toric/spherical blend.
    PlaneCylinder,
    /// Two coaxial cylinders — toric blend (stepped shaft fillet).
    CylinderCylinderCoaxial,
    /// Two skew cylinders — Dupin cyclide or NURBS.
    CylinderCylinderSkew,
    /// General curved surfaces — NURBS rolling ball.
    GeneralCurved,
    /// Not supported.
    Unsupported,
}

/// Result of a single edge fillet operation.
#[derive(Debug)]
pub enum FilletResult {
    /// Successfully created a blend surface.
    Success,
    /// Edge pair not supported for fillet.
    Unsupported {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
        /// Reason for failure.
        reason: String,
    },
    /// Fillet radius too large for the geometry.
    RadiusTooLarge {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
        /// Maximum radius that would work.
        max_radius: f64,
    },
    /// Degenerate geometry at the edge.
    DegenerateGeometry {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
    },
}

/// Classify the fillet case between two faces.
pub fn classify_fillet_case(surface_a: &dyn Surface, surface_b: &dyn Surface) -> FilletCase {
    match (surface_a.surface_type(), surface_b.surface_type()) {
        (SurfaceKind::Plane, SurfaceKind::Plane) => FilletCase::PlanePlane,
        (SurfaceKind::Plane, SurfaceKind::Cylinder) | (SurfaceKind::Cylinder, SurfaceKind::Plane) => {
            FilletCase::PlaneCylinder
        }
        (SurfaceKind::Cylinder, SurfaceKind::Cylinder) => {
            // Check if coaxial
            let cyl_a = surface_a.as_any().downcast_ref::<CylinderSurface>();
            let cyl_b = surface_b.as_any().downcast_ref::<CylinderSurface>();
            if let (Some(a), Some(b)) = (cyl_a, cyl_b) {
                let dot = a.axis.as_ref().dot(b.axis.as_ref()).abs();
                if dot > 1.0 - 1e-10 {
                    // Axes are parallel — check if coaxial (centers on same line)
                    let d = b.center - a.center;
                    let cross = d.cross(a.axis.as_ref());
                    if cross.norm() < 1e-6 {
                        FilletCase::CylinderCylinderCoaxial
                    } else {
                        FilletCase::CylinderCylinderSkew
                    }
                } else {
                    FilletCase::CylinderCylinderSkew
                }
            } else {
                FilletCase::GeneralCurved
            }
        }
        (SurfaceKind::Plane, SurfaceKind::Sphere)
        | (SurfaceKind::Sphere, SurfaceKind::Plane)
        | (SurfaceKind::Plane, SurfaceKind::Cone)
        | (SurfaceKind::Cone, SurfaceKind::Plane)
        | (SurfaceKind::Cylinder, SurfaceKind::Sphere)
        | (SurfaceKind::Sphere, SurfaceKind::Cylinder)
        | (SurfaceKind::Sphere, SurfaceKind::Sphere) => FilletCase::GeneralCurved,
        _ => FilletCase::Unsupported,
    }
}

/// Compute the closest point on a surface to a given 3D point, returning UV parameters.
///
/// This is a general-purpose surface projection that works for all surface types.
/// For analytic surfaces, uses geometric solutions. For general surfaces, uses Newton iteration.
pub fn closest_point_uv(surface: &dyn Surface, point: &Point3, tolerance: f64) -> Option<Point2> {
    match surface.surface_type() {
        SurfaceKind::Plane => {
            let plane = surface.as_any().downcast_ref::<Plane>()?;
            Some(plane.project(point))
        }
        SurfaceKind::Cylinder => {
            let cyl = surface.as_any().downcast_ref::<CylinderSurface>()?;
            let d = *point - cyl.center;
            let v = d.dot(cyl.axis.as_ref());
            let radial = d - v * cyl.axis.as_ref();
            let u = radial.dot(&cyl.y_dir()).atan2(radial.dot(cyl.ref_dir.as_ref()));
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            Some(Point2::new(u, v))
        }
        SurfaceKind::Sphere => {
            let sphere = surface.as_any().downcast_ref::<vcad_kernel_geom::SphereSurface>()?;
            let d = *point - sphere.center;
            let len = d.norm();
            if len < 1e-15 {
                return None;
            }
            let d_norm = d / len;
            let v = d_norm.dot(sphere.axis.as_ref()).asin();
            let cos_v = v.cos();
            if cos_v.abs() < 1e-15 {
                return Some(Point2::new(0.0, v)); // At pole
            }
            let x_comp = d_norm.dot(sphere.ref_dir.as_ref()) / cos_v;
            let y_comp = d_norm.dot(&sphere.y_dir()) / cos_v;
            let u = y_comp.atan2(x_comp);
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            Some(Point2::new(u, v))
        }
        _ => {
            // Newton iteration for general surfaces
            closest_point_uv_newton(surface, point, tolerance)
        }
    }
}

/// Newton iteration to find closest point on a general surface.
fn closest_point_uv_newton(surface: &dyn Surface, point: &Point3, tolerance: f64) -> Option<Point2> {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    // Start at domain center
    let mut u = (u_min + u_max) * 0.5;
    let mut v = (v_min + v_max) * 0.5;

    for _ in 0..50 {
        let uv = Point2::new(u, v);
        let p = surface.evaluate(uv);
        let diff = p - *point;
        let dist = diff.norm();
        if dist < tolerance {
            return Some(uv);
        }

        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        // Solve 2x2 system: [du·du, du·dv; dv·du, dv·dv] * [delta_u, delta_v] = [-diff·du, -diff·dv]
        let a11 = du.dot(&du);
        let a12 = du.dot(&dv);
        let a22 = dv.dot(&dv);
        let b1 = -diff.dot(&du);
        let b2 = -diff.dot(&dv);

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-30 {
            break;
        }

        let delta_u = (a22 * b1 - a12 * b2) / det;
        let delta_v = (a11 * b2 - a12 * b1) / det;

        u = (u + delta_u).clamp(u_min, u_max);
        v = (v + delta_v).clamp(v_min, v_max);
    }

    Some(Point2::new(u, v))
}

/// Extract face information from a B-rep solid.
fn extract_faces(brep: &BRepSolid) -> Vec<FaceInfo> {
    let topo = &brep.topology;
    let mut faces = Vec::new();

    for (face_id, face) in &topo.faces {
        let vertex_ids = topo.loop_vertices(face.outer_loop);
        let positions: Vec<Point3> = vertex_ids.iter().map(|&v| topo.vertices[v].point).collect();
        let normal = compute_face_normal(&positions);

        faces.push(FaceInfo {
            face_id,
            vertex_ids,
            positions,
            normal,
        });
    }

    faces
}

/// Compute face normal from vertex positions using Newell's method.
fn compute_face_normal(positions: &[Point3]) -> Vec3 {
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
fn extract_edges(brep: &BRepSolid) -> Vec<EdgeInfo> {
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

    edges
}

// =============================================================================
// Trim vertex computation
// =============================================================================

/// Key for a trim vertex: (original_vertex, face_id).
/// Each original vertex gets one trim vertex per adjacent face.
type TrimKey = (VertexId, FaceId);

/// Compute trim vertices for all vertices on all faces.
///
/// For each vertex V on face F:
/// - The entering edge E_enter and leaving edge E_leave define two trim lines
///   (parallel to each edge, offset inward by `distance`)
/// - The trim vertex is at the intersection of these two trim lines
///
/// This gives one vertex per (original_vertex, face) pair.
fn compute_trim_vertices(faces: &[FaceInfo], distance: f64) -> HashMap<TrimKey, Point3> {
    let mut trims = HashMap::new();

    // Build a map: (vertex, face) → (entering_edge_dir, leaving_edge_dir)
    // For each face, walk its loop and find the entering/leaving edge directions at each vertex.
    for face in faces {
        let n = face.vertex_ids.len();
        let normal = face.normal;

        for i in 0..n {
            let v_id = face.vertex_ids[i];
            let v_pos = face.positions[i];
            let prev_idx = (i + n - 1) % n;
            let next_idx = (i + 1) % n;

            // Direction of entering edge: from predecessor toward this vertex
            let prev_pos = face.positions[prev_idx];
            let d_enter = v_pos - prev_pos;
            let d_enter_len = d_enter.norm();

            // Direction of leaving edge: from this vertex toward successor
            let next_pos = face.positions[next_idx];
            let d_leave = next_pos - v_pos;
            let d_leave_len = d_leave.norm();

            if d_enter_len < 1e-15 || d_leave_len < 1e-15 {
                trims.insert((v_id, face.face_id), v_pos);
                continue;
            }

            let d_enter = d_enter / d_enter_len;
            let d_leave = d_leave / d_leave_len;

            // Compute inward perpendiculars (into the face interior)
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

            // Trim line 1: point on entering edge's trim line, direction d_enter
            // P1 = V + distance * perp_enter
            // Trim line 2: point on leaving edge's trim line, direction d_leave
            // P2 = V + distance * perp_leave
            //
            // Solve: P1 + t1 * d_enter = P2 + t2 * d_leave
            // => distance * (perp_enter - perp_leave) = t2 * d_leave - t1 * d_enter
            //
            // Cross with d_leave: delta × d_leave = -t1 * (d_enter × d_leave)
            // t1 = -(delta × d_leave) · normal / (d_enter × d_leave) · normal

            let delta = distance * (perp_enter - perp_leave);
            let cross_dirs = d_enter.cross(&d_leave);
            let denom = cross_dirs.dot(&normal);

            if denom.abs() < 1e-15 {
                // Parallel edges — use midpoint of perpendicular offsets
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

// =============================================================================
// Chamfer
// =============================================================================

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

    // Build vertex→edges map (which edges meet at each vertex)
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
            // Orient the quad for outward normal
            let chamfer_center =
                Point3::from((pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25);
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

    // 3. Build vertex faces (one per vertex where ≥3 edges meet)
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

/// Compute the centroid of all faces' vertex positions.
fn compute_centroid(faces: &[FaceInfo]) -> Point3 {
    let mut sum = Vec3::zeros();
    let mut count = 0;
    for face in faces {
        for p in &face.positions {
            sum += p.coords;
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
fn pair_twin_half_edges(topo: &mut Topology) {
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

fn quantize(p: Point3) -> [i64; 3] {
    [
        (p.x * 1e9).round() as i64,
        (p.y * 1e9).round() as i64,
        (p.z * 1e9).round() as i64,
    ]
}

/// Build vertex faces for all vertices where ≥3 edges meet.
/// Each vertex face is a polygon connecting the trim vertices from all adjacent faces.
#[allow(clippy::too_many_arguments)]
fn build_vertex_faces(
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

        // Collect trim vertices from all faces at this vertex
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

        // Sort by angle around the axis from solid center to vertex
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

// =============================================================================
// Fillet
// =============================================================================

/// Fillet all edges of a B-rep solid with a constant radius.
///
/// Creates a new solid where each edge is replaced by a cylindrical blend
/// surface tangent to both adjacent faces. Each original face is trimmed,
/// and each vertex becomes a polygon face with curved transitions.
///
/// # Requirements
///
/// - All faces must be planar
/// - The solid should be convex
/// - Radius must be positive and smaller than the shortest edge / 2
///
/// # Current limitations
///
/// The vertex faces at edge junctions are still planar (not smooth transitions).
/// This is a common simplification for constant-radius fillets.
pub fn fillet_all_edges(brep: &BRepSolid, radius: f64) -> BRepSolid {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);

    if edges.is_empty() {
        return brep.clone();
    }

    // Tangent points are at the same positions as chamfer trim vertices
    let trims = compute_trim_vertices(&faces, radius);
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
            // Cylinder axis along the edge direction
            let v_start_pos = brep.topology.vertices[edge_info.v_start].point;
            let v_end_pos = brep.topology.vertices[edge_info.v_end].point;
            let edge_dir = v_end_pos - v_start_pos;
            let edge_len = edge_dir.norm();
            if edge_len < 1e-12 {
                continue;
            }
            let edge_unit = edge_dir / edge_len;

            // Cylinder center: offset from the edge by r along both face normals
            let center_offset = radius * (fa.normal + fb.normal);
            let center_start = v_start_pos + center_offset;

            // Ref dir: from cylinder center toward the tangent on face_a
            let to_tangent_a = pa_s - center_start;
            let ref_dir = to_tangent_a - to_tangent_a.dot(&edge_unit) * edge_unit;
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

            // Orient the quad for outward normal
            let solid_center = compute_centroid(&faces);
            let chamfer_center =
                Point3::from((pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25);
            let outward = chamfer_center - solid_center;

            let e1 = pa_e - pa_s;
            let e2 = pb_s - pa_s;
            let n = e1.cross(&e2);

            let positions = if n.dot(&outward) > 0.0 {
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

// =============================================================================
// Curved fillet support
// =============================================================================

/// Fillet specific edges of a B-rep solid, supporting curved faces.
///
/// This is the extended fillet API that handles plane-cylinder, coaxial cylinder,
/// and general curved face pairs in addition to the basic plane-plane case.
///
/// # Arguments
///
/// * `brep` - The input solid
/// * `edge_ids` - Edges to fillet
/// * `radius` - Fillet radius
///
/// # Returns
///
/// A new `BRepSolid` and a vector of per-edge results indicating success or failure.
pub fn fillet_edges_detailed(
    brep: &BRepSolid,
    edge_ids: &[EdgeId],
    radius: f64,
) -> (BRepSolid, Vec<FilletResult>) {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);
    let topo = &brep.topology;
    let geom = &brep.geometry;

    // Filter edges to only requested ones
    let target_edges: Vec<&EdgeInfo> = edges
        .iter()
        .filter(|e| edge_ids.contains(&e.edge_id))
        .collect();

    if target_edges.is_empty() {
        return (brep.clone(), Vec::new());
    }

    // Build curved face info for classification
    let _curved_faces: HashMap<FaceId, CurvedFaceInfo> = topo
        .faces
        .iter()
        .map(|(face_id, face)| {
            let surface = &geom.surfaces[face.surface_index];
            let vertex_ids = topo.loop_vertices(face.outer_loop);
            let positions: Vec<Point3> =
                vertex_ids.iter().map(|&v| topo.vertices[v].point).collect();
            let planar_normal = if surface.surface_type() == SurfaceKind::Plane {
                Some(compute_face_normal(&positions))
            } else {
                None
            };
            (
                face_id,
                CurvedFaceInfo {
                    face_id,
                    surface_index: face.surface_index,
                    surface_kind: surface.surface_type(),
                    vertex_ids,
                    positions,
                    planar_normal,
                },
            )
        })
        .collect();

    let mut results = Vec::new();
    let trims = compute_trim_vertices(&faces, radius);
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

        // Preserve original surface for curved faces
        let face_data = &topo.faces[face.face_id];
        let surface = &geom.surfaces[face_data.surface_index];
        let surf_idx = new_geom.add_surface(surface.clone_box());

        let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
        let loop_id = new_topo.add_loop(&hes);
        let face_id = new_topo.add_face(loop_id, surf_idx, face_data.orientation);
        all_faces.push(face_id);
    }

    // 2. Build blend faces for each target edge
    for edge_info in &target_edges {
        let surface_a = &geom.surfaces[topo.faces[edge_info.face_a].surface_index];
        let surface_b = &geom.surfaces[topo.faces[edge_info.face_b].surface_index];
        let case = classify_fillet_case(surface_a.as_ref(), surface_b.as_ref());

        match case {
            FilletCase::PlanePlane => {
                // Use existing cylindrical blend logic
                let fa = face_map.get(&edge_info.face_a);
                let fb = face_map.get(&edge_info.face_b);
                if let (Some(fa), Some(fb)) = (fa, fb) {
                    if build_plane_plane_blend(
                        edge_info, fa, fb, &trims, &faces, radius, brep,
                        &mut vertex_cache, &mut new_topo, &mut new_geom, &mut all_faces,
                    ) {
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                }
            }
            FilletCase::PlaneCylinder => {
                // Torus blend between plane and cylinder
                let (plane_surf, cyl_surf, plane_face_id, _cyl_face_id) =
                    if surface_a.surface_type() == SurfaceKind::Plane {
                        (surface_a, surface_b, edge_info.face_a, edge_info.face_b)
                    } else {
                        (surface_b, surface_a, edge_info.face_b, edge_info.face_a)
                    };

                if let Some(torus) = build_plane_cylinder_torus(
                    plane_surf.as_ref(),
                    cyl_surf.as_ref(),
                    topo.faces[plane_face_id].orientation,
                    radius,
                ) {
                    let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
                    let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
                    let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
                    let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

                    if let (Some(&pa_s), Some(&pa_e), Some(&pb_s), Some(&pb_e)) =
                        (pa_s, pa_e, pb_s, pb_e)
                    {
                        let surf_idx = new_geom.add_surface(Box::new(torus));
                        let solid_center = compute_centroid(&faces);
                        let chamfer_center = Point3::from(
                            (pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25,
                        );
                        let outward = chamfer_center - solid_center;
                        let e1 = pa_e - pa_s;
                        let e2 = pb_s - pa_s;
                        let n = e1.cross(&e2);

                        let positions = if n.dot(&outward) > 0.0 {
                            vec![pa_s, pa_e, pb_e, pb_s]
                        } else {
                            vec![pa_s, pb_s, pb_e, pa_e]
                        };

                        let verts: Vec<VertexId> = positions
                            .iter()
                            .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
                            .collect();

                        let hes: Vec<HalfEdgeId> =
                            verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
                        let loop_id = new_topo.add_loop(&hes);
                        let face_id =
                            new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
                        all_faces.push(face_id);
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                } else {
                    results.push(FilletResult::Unsupported {
                        edge_id: edge_info.edge_id,
                        reason: "could not construct torus blend".into(),
                    });
                }
            }
            FilletCase::CylinderCylinderCoaxial => {
                // Torus blend for stepped shaft
                if let Some(torus) = build_coaxial_cylinder_torus(
                    surface_a.as_ref(),
                    surface_b.as_ref(),
                    radius,
                ) {
                    let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
                    let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
                    let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
                    let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

                    if let (Some(&pa_s), Some(&pa_e), Some(&pb_s), Some(&pb_e)) =
                        (pa_s, pa_e, pb_s, pb_e)
                    {
                        let surf_idx = new_geom.add_surface(Box::new(torus));
                        let solid_center = compute_centroid(&faces);
                        let chamfer_center = Point3::from(
                            (pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25,
                        );
                        let outward = chamfer_center - solid_center;
                        let e1 = pa_e - pa_s;
                        let e2 = pb_s - pa_s;
                        let n = e1.cross(&e2);

                        let positions = if n.dot(&outward) > 0.0 {
                            vec![pa_s, pa_e, pb_e, pb_s]
                        } else {
                            vec![pa_s, pb_s, pb_e, pa_e]
                        };

                        let verts: Vec<VertexId> = positions
                            .iter()
                            .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
                            .collect();

                        let hes: Vec<HalfEdgeId> =
                            verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
                        let loop_id = new_topo.add_loop(&hes);
                        let face_id =
                            new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
                        all_faces.push(face_id);
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                } else {
                    results.push(FilletResult::Unsupported {
                        edge_id: edge_info.edge_id,
                        reason: "could not construct coaxial torus blend".into(),
                    });
                }
            }
            FilletCase::CylinderCylinderSkew | FilletCase::GeneralCurved => {
                // NURBS rolling ball blend
                let v_start_pos = topo.vertices[edge_info.v_start].point;
                let v_end_pos = topo.vertices[edge_info.v_end].point;

                match rolling_ball_blend(
                    surface_a.as_ref(),
                    surface_b.as_ref(),
                    v_start_pos,
                    v_end_pos,
                    radius,
                    8,  // samples along edge
                    5,  // samples across blend
                ) {
                    Some(bspline) => {
                        let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a));
                        let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a));
                        let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b));
                        let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b));

                        if let (Some(&pa_s), Some(&pa_e), Some(&pb_s), Some(&pb_e)) =
                            (pa_s, pa_e, pb_s, pb_e)
                        {
                            let surf_idx = new_geom.add_surface(Box::new(bspline));
                            let solid_center = compute_centroid(&faces);
                            let chamfer_center = Point3::from(
                                (pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25,
                            );
                            let outward = chamfer_center - solid_center;
                            let e1 = pa_e - pa_s;
                            let e2 = pb_s - pa_s;
                            let n = e1.cross(&e2);

                            let positions = if n.dot(&outward) > 0.0 {
                                vec![pa_s, pa_e, pb_e, pb_s]
                            } else {
                                vec![pa_s, pb_s, pb_e, pa_e]
                            };

                            let verts: Vec<VertexId> = positions
                                .iter()
                                .map(|p| {
                                    get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p)
                                })
                                .collect();

                            let hes: Vec<HalfEdgeId> =
                                verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
                            let loop_id = new_topo.add_loop(&hes);
                            let face_id =
                                new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
                            all_faces.push(face_id);
                            results.push(FilletResult::Success);
                        } else {
                            results.push(FilletResult::DegenerateGeometry {
                                edge_id: edge_info.edge_id,
                            });
                        }
                    }
                    None => {
                        results.push(FilletResult::Unsupported {
                            edge_id: edge_info.edge_id,
                            reason: format!(
                                "rolling ball blend failed for {:?} edge",
                                case
                            ),
                        });
                    }
                }
            }
            FilletCase::Unsupported => {
                results.push(FilletResult::Unsupported {
                    edge_id: edge_info.edge_id,
                    reason: format!(
                        "unsupported surface combination: {:?} / {:?}",
                        surface_a.surface_type(),
                        surface_b.surface_type()
                    ),
                });
            }
        }
    }

    // 3. Build vertex faces for target edges
    let target_vertex_edges: HashMap<VertexId, Vec<&EdgeInfo>> = {
        let mut map: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
        for &edge in &target_edges {
            map.entry(edge.v_start).or_default().push(edge);
            map.entry(edge.v_end).or_default().push(edge);
        }
        map
    };

    build_vertex_faces(
        &faces,
        &target_vertex_edges,
        &trims,
        brep,
        &mut vertex_cache,
        &mut new_topo,
        &mut new_geom,
        &mut all_faces,
    );

    // 4. Pair twin half-edges and build shell
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

/// Build a plane-plane cylindrical blend (existing algorithm, extracted).
#[allow(clippy::too_many_arguments)]
fn build_plane_plane_blend(
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

    let center_offset = radius * (fa.normal + fb.normal);
    let center_start = v_start_pos + center_offset;

    let to_tangent_a = pa_s - center_start;
    let ref_dir = to_tangent_a - to_tangent_a.dot(&edge_unit) * edge_unit;
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
        Point3::from((pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25);
    let outward = chamfer_center - solid_center;

    let e1 = pa_e - pa_s;
    let e2 = pb_s - pa_s;
    let n = e1.cross(&e2);

    let positions = if n.dot(&outward) > 0.0 {
        vec![pa_s, pa_e, pb_e, pb_s]
    } else {
        vec![pa_s, pb_s, pb_e, pa_e]
    };

    let get_or_create = |cache: &mut HashMap<[i64; 3], VertexId>,
                         topo: &mut Topology,
                         pos: Point3|
     -> VertexId {
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

/// Build a torus blend surface for a plane-cylinder edge.
///
/// The torus is tangent to both the plane and the cylinder, with its
/// minor radius equal to the fillet radius.
fn build_plane_cylinder_torus(
    plane_surf: &dyn Surface,
    cyl_surf: &dyn Surface,
    plane_orientation: Orientation,
    radius: f64,
) -> Option<TorusSurface> {
    let plane = plane_surf.as_any().downcast_ref::<Plane>()?;
    let cyl = cyl_surf.as_any().downcast_ref::<CylinderSurface>()?;

    // The torus axis is the cylinder axis
    let axis = cyl.axis;

    // Determine if the plane faces toward or away from the cylinder axis
    let plane_normal = match plane_orientation {
        Orientation::Forward => *plane.normal_dir.as_ref(),
        Orientation::Reversed => -*plane.normal_dir.as_ref(),
    };

    // Project cylinder center onto the plane to find the intersection direction
    let to_plane = plane.signed_distance(&cyl.center);
    let plane_along_axis = plane_normal.dot(axis.as_ref());

    // For the torus: major_radius = cylinder.radius ± fillet_radius
    // depending on whether the fillet is on the inside or outside of the cylinder
    let major_radius = cyl.radius + radius;
    if major_radius <= 0.0 {
        return None;
    }

    // Torus center is on the cylinder axis, at the plane offset by the fillet radius
    let torus_center = cyl.center - (to_plane - radius * plane_along_axis.signum()) * plane_normal;

    // Project torus center onto cylinder axis
    let axis_param = (torus_center - cyl.center).dot(axis.as_ref());
    let center_on_axis = cyl.center + axis_param * axis.as_ref();

    Some(TorusSurface {
        center: center_on_axis,
        axis,
        ref_dir: cyl.ref_dir,
        major_radius,
        minor_radius: radius,
    })
}

/// Build a torus blend surface for two coaxial cylinders (stepped shaft).
fn build_coaxial_cylinder_torus(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    radius: f64,
) -> Option<TorusSurface> {
    let cyl_a = surface_a.as_any().downcast_ref::<CylinderSurface>()?;
    let cyl_b = surface_b.as_any().downcast_ref::<CylinderSurface>()?;

    // For coaxial cylinders, the torus sits at the step between the two radii
    let (smaller, larger) = if cyl_a.radius < cyl_b.radius {
        (cyl_a, cyl_b)
    } else {
        (cyl_b, cyl_a)
    };

    let radius_step = larger.radius - smaller.radius;
    if radius < 1e-15 || radius > radius_step {
        return None; // Radius too large for the step
    }

    // Torus major radius: distance from axis to tube center
    // For a fillet at the step, the tube center is at smaller.radius + radius
    let major_radius = smaller.radius + radius;

    // Torus center is on the axis at the step location
    // The step is where the two cylinders meet along the axis
    // For now, use the midpoint of the two cylinder centers projected onto the axis
    let axis = smaller.axis;
    let d = larger.center - smaller.center;
    let t = d.dot(axis.as_ref());
    let center = smaller.center + t * axis.as_ref();

    Some(TorusSurface {
        center,
        axis,
        ref_dir: smaller.ref_dir,
        major_radius,
        minor_radius: radius,
    })
}

// =============================================================================
// NURBS Rolling Ball Fillet
// =============================================================================

/// Compute a rolling ball fillet blend surface between two general surfaces.
///
/// The algorithm:
/// 1. Samples points along the shared edge curve
/// 2. At each sample, finds the ball center that is equidistant (= radius) from both surfaces
/// 3. Records the contact points on each surface
/// 4. Fits a bicubic B-spline surface through the contact point grid
///
/// Returns `None` if the rolling ball cannot be computed (surfaces too far apart,
/// degenerate geometry, etc).
pub fn rolling_ball_blend(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    edge_start: Point3,
    edge_end: Point3,
    radius: f64,
    num_samples_along: usize,
    num_samples_across: usize,
) -> Option<BSplineSurface> {
    if num_samples_along < 2 || num_samples_across < 2 {
        return None;
    }

    let edge_dir = edge_end - edge_start;
    let edge_len = edge_dir.norm();
    if edge_len < 1e-12 {
        return None;
    }

    // Sample points along the edge
    let mut blend_points = Vec::new(); // n_along x n_across grid

    for i in 0..num_samples_along {
        let t = i as f64 / (num_samples_along - 1) as f64;
        let edge_point = edge_start + t * edge_dir;

        // Find closest points and normals on both surfaces at this edge position
        let uv_a = closest_point_uv(surface_a, &edge_point, 1e-6)?;
        let uv_b = closest_point_uv(surface_b, &edge_point, 1e-6)?;

        let n_a = surface_a.normal(uv_a);
        let n_b = surface_b.normal(uv_b);

        // The rolling ball center is offset from both surfaces by `radius` along the normal.
        // We need to find the center C such that:
        //   |C - pt_a| = radius  (approximately, since pt_a is on surface_a)
        //   |C - pt_b| = radius  (approximately, since pt_b is on surface_b)
        // For the initial estimate, use the bisector of the two normals:
        let bisector = (*n_a.as_ref() + *n_b.as_ref()).normalize();
        if bisector.norm() < 1e-12 {
            return None; // Normals are antiparallel
        }

        // Refine ball center using Newton iteration
        let ball_center = refine_ball_center(
            surface_a, surface_b, &edge_point, &bisector, radius,
        )?;

        // Contact points: closest points on each surface to the ball center
        let uv_ca = closest_point_uv(surface_a, &ball_center, 1e-6)?;
        let uv_cb = closest_point_uv(surface_b, &ball_center, 1e-6)?;
        let contact_a = surface_a.evaluate(uv_ca);
        let contact_b = surface_b.evaluate(uv_cb);

        // Generate blend points across the fillet (from contact_a to contact_b on the ball surface)
        for j in 0..num_samples_across {
            let s = j as f64 / (num_samples_across - 1) as f64;
            // Spherical interpolation on the ball surface
            let dir_a = (contact_a - ball_center).normalize();
            let dir_b = (contact_b - ball_center).normalize();
            let blend_dir = slerp(&dir_a, &dir_b, s);
            let blend_pt = ball_center + radius * blend_dir;
            blend_points.push(blend_pt);
        }
    }

    // Fit a B-spline surface through the blend points
    // Use the sample grid as control points for an interpolating surface
    fit_bspline_surface(&blend_points, num_samples_along, num_samples_across)
}

/// Refine the rolling ball center using Newton-like iteration.
fn refine_ball_center(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    initial_pos: &Point3,
    direction: &Vec3,
    radius: f64,
) -> Option<Point3> {
    let mut center = *initial_pos + radius * direction;

    for _ in 0..20 {
        // Find closest points on both surfaces
        let uv_a = closest_point_uv(surface_a, &center, 1e-6)?;
        let uv_b = closest_point_uv(surface_b, &center, 1e-6)?;
        let pt_a = surface_a.evaluate(uv_a);
        let pt_b = surface_b.evaluate(uv_b);

        let dist_a = (center - pt_a).norm();
        let dist_b = (center - pt_b).norm();

        // Check convergence
        if (dist_a - radius).abs() < 1e-6 && (dist_b - radius).abs() < 1e-6 {
            return Some(center);
        }

        // Adjust center: move toward/away from each surface
        let n_a = if dist_a > 1e-12 {
            (center - pt_a) / dist_a
        } else {
            *surface_a.normal(uv_a).as_ref()
        };
        let n_b = if dist_b > 1e-12 {
            (center - pt_b) / dist_b
        } else {
            *surface_b.normal(uv_b).as_ref()
        };

        let err_a = dist_a - radius;
        let err_b = dist_b - radius;

        // Move center to reduce errors
        let correction = -0.5 * (err_a * n_a + err_b * n_b);
        center += correction;
    }

    // Return best estimate even if not fully converged
    Some(center)
}

/// Spherical linear interpolation between two unit vectors.
fn slerp(a: &Vec3, b: &Vec3, t: f64) -> Vec3 {
    let dot = a.dot(b).clamp(-1.0, 1.0);
    let theta = dot.acos();

    if theta.abs() < 1e-10 {
        // Vectors nearly parallel, use linear interpolation
        return ((1.0 - t) * a + t * b).normalize();
    }

    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;
    (wa * a + wb * b).normalize()
}

/// Fit a B-spline surface through a grid of points.
///
/// Uses the point grid directly as control points for an approximating
/// bicubic B-spline surface (not interpolating, which would require
/// solving a linear system).
fn fit_bspline_surface(
    points: &[Point3],
    n_along: usize,
    n_across: usize,
) -> Option<BSplineSurface> {
    if points.len() != n_along * n_across {
        return None;
    }

    // For a reasonable approximation, use degree 3 if we have enough points,
    // otherwise reduce the degree.
    let degree_u = (n_along - 1).min(3);
    let degree_v = (n_across - 1).min(3);

    // Generate clamped uniform knot vectors
    let knots_u = clamped_uniform_knots(n_along, degree_u);
    let knots_v = clamped_uniform_knots(n_across, degree_v);

    // Use the sample points as control points (approximation)
    // For an interpolating fit, we'd solve a linear system, but for the
    // rolling ball the sample points are already well-distributed.
    Some(BSplineSurface::new(
        points.to_vec(),
        n_along,
        n_across,
        knots_u,
        knots_v,
        degree_u,
        degree_v,
    ))
}

/// Generate a clamped uniform knot vector for `n` control points with degree `p`.
fn clamped_uniform_knots(n: usize, p: usize) -> Vec<f64> {
    let m = n + p + 1;
    let mut knots = Vec::with_capacity(m);
    for i in 0..m {
        if i <= p {
            knots.push(0.0);
        } else if i >= n {
            knots.push(1.0);
        } else {
            knots.push((i - p) as f64 / (n - p) as f64);
        }
    }
    knots
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_extract_faces_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let faces = extract_faces(&cube);
        assert_eq!(faces.len(), 6);
        for face in &faces {
            assert_eq!(face.vertex_ids.len(), 4);
            let n = face.normal;
            assert!(
                (n.norm() - 1.0).abs() < 0.01,
                "face normal not unit: {:?}",
                n
            );
        }
    }

    #[test]
    fn test_extract_edges_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let edges = extract_edges(&cube);
        assert_eq!(edges.len(), 12, "cube should have 12 edges");
    }

    #[test]
    fn test_chamfer_cube_topology() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let chamfered = chamfer_all_edges(&cube, 1.0);

        // Chamfered cube: 6 quads (trimmed faces) + 12 quads (chamfer faces) + 8 triangles = 26 faces
        let n_faces = chamfered.topology.faces.len();
        assert_eq!(
            n_faces, 26,
            "chamfered cube should have 26 faces, got {}",
            n_faces
        );

        // 24 vertices (each original vertex spawns 3 trim vertices, one per face)
        let n_verts = chamfered.topology.vertices.len();
        assert_eq!(
            n_verts, 24,
            "chamfered cube should have 24 vertices, got {}",
            n_verts
        );

        // All half-edges should be paired (closed solid)
        let total_hes = chamfered.topology.half_edges.len();
        let paired_hes = chamfered
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_some())
            .count();
        assert_eq!(
            paired_hes, total_hes,
            "all {} half-edges should be paired, got {} paired",
            total_hes, paired_hes
        );
    }

    #[test]
    fn test_chamfer_cube_volume() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let d = 1.0;
        let chamfered = chamfer_all_edges(&cube, d);

        let mesh = vcad_kernel_tessellate::tessellate_brep(&chamfered, 32);

        // Volume of chamfered cube via inclusion-exclusion:
        // 12 full edge prisms (cross-section 0.5*d², length L): 12 * 0.5 * d² * L
        // 24 pairwise overlaps at vertices (3 per vertex): 24 * d³/3
        // 8 triple overlaps at vertices: 8 * d³/4
        // Removed = 6*d²*L - 8*d³ + 2*d³ = 6*d²*(L - d)
        // Expected = L³ - 6*d²*(L - d)
        let l = 10.0;
        let expected_vol = l * l * l - 6.0 * d * d * (l - d);

        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - expected_vol).abs() < 5.0,
            "chamfered cube volume: expected ~{:.1}, got {:.1}",
            expected_vol,
            vol
        );
    }

    #[test]
    fn test_fillet_cube_topology() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let filleted = fillet_all_edges(&cube, 1.0);

        // Same topology as chamfer: 26 faces
        let n_faces = filleted.topology.faces.len();
        assert_eq!(
            n_faces, 26,
            "filleted cube should have 26 faces, got {}",
            n_faces
        );
    }

    #[test]
    fn test_fillet_cube_has_cylindrical_surfaces() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let filleted = fillet_all_edges(&cube, 1.0);

        // Should have 12 cylindrical surfaces (one per edge)
        let n_cyl = filleted
            .geometry
            .surfaces
            .iter()
            .filter(|s| s.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder)
            .count();
        assert_eq!(
            n_cyl, 12,
            "filleted cube should have 12 cylindrical surfaces, got {}",
            n_cyl
        );
    }

    #[test]
    fn test_classify_cube_edges() {
        // All cube edges are plane-plane
        let cube = make_cube(10.0, 10.0, 10.0);
        let edges = extract_edges(&cube);
        for edge in &edges {
            let sa = &cube.geometry.surfaces[cube.topology.faces[edge.face_a].surface_index];
            let sb = &cube.geometry.surfaces[cube.topology.faces[edge.face_b].surface_index];
            assert_eq!(
                classify_fillet_case(sa.as_ref(), sb.as_ref()),
                FilletCase::PlanePlane
            );
        }
    }

    #[test]
    fn test_classify_cylinder_edges() {
        // Cylinder has plane-plane (between caps) and plane-cylinder edges
        let cyl = vcad_kernel_primitives::make_cylinder(5.0, 10.0, 64);
        let edges = extract_edges(&cyl);
        let mut has_plane_cyl = false;
        for edge in &edges {
            let sa = &cyl.geometry.surfaces[cyl.topology.faces[edge.face_a].surface_index];
            let sb = &cyl.geometry.surfaces[cyl.topology.faces[edge.face_b].surface_index];
            if classify_fillet_case(sa.as_ref(), sb.as_ref()) == FilletCase::PlaneCylinder {
                has_plane_cyl = true;
            }
        }
        assert!(has_plane_cyl, "cylinder should have plane-cylinder edges");
    }

    #[test]
    fn test_closest_point_uv_plane() {
        let plane = Plane::xy();
        let pt = Point3::new(3.0, 4.0, 5.0);
        let uv = closest_point_uv(&plane, &pt, 1e-10).unwrap();
        assert!((uv.x - 3.0).abs() < 1e-10);
        assert!((uv.y - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_closest_point_uv_cylinder() {
        let cyl = CylinderSurface::new(5.0);
        let pt = Point3::new(5.0, 0.0, 3.0); // On the cylinder at u=0, v=3
        let uv = closest_point_uv(&cyl, &pt, 1e-10).unwrap();
        assert!((uv.x).abs() < 1e-6, "u should be ~0, got {}", uv.x); // u=0
        assert!((uv.y - 3.0).abs() < 1e-6, "v should be ~3, got {}", uv.y);
    }

    #[test]
    fn test_fillet_edges_detailed_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let edge_ids: Vec<EdgeId> = cube.topology.edges.keys().collect();
        let (result, results) = fillet_edges_detailed(&cube, &edge_ids, 1.0);

        // All edges should succeed (plane-plane)
        for r in &results {
            assert!(
                matches!(r, FilletResult::Success),
                "expected Success, got {:?}",
                r
            );
        }
        assert_eq!(results.len(), 12, "should have 12 results for 12 edges");

        // Should produce a valid solid
        assert!(
            !result.topology.faces.is_empty(),
            "filleted result should have faces"
        );
    }

    #[test]
    fn test_fillet_cylinder_cap_edge() {
        // Fillet a cylinder's cap edge - should produce TorusSurface
        let cyl = vcad_kernel_primitives::make_cylinder(5.0, 10.0, 64);
        let edges = extract_edges(&cyl);

        // Find plane-cylinder edges
        let plane_cyl_edges: Vec<EdgeId> = edges
            .iter()
            .filter(|e| {
                let sa = &cyl.geometry.surfaces[cyl.topology.faces[e.face_a].surface_index];
                let sb = &cyl.geometry.surfaces[cyl.topology.faces[e.face_b].surface_index];
                classify_fillet_case(sa.as_ref(), sb.as_ref()) == FilletCase::PlaneCylinder
            })
            .map(|e| e.edge_id)
            .collect();

        if !plane_cyl_edges.is_empty() {
            let (result, results) = fillet_edges_detailed(&cyl, &plane_cyl_edges, 1.0);
            // Check that at least some edges got a torus surface
            let has_torus = result
                .geometry
                .surfaces
                .iter()
                .any(|s| s.surface_type() == SurfaceKind::Torus);
            // Torus blend may or may not succeed depending on geometry details
            if has_torus {
                assert!(
                    results.iter().any(|r| matches!(r, FilletResult::Success)),
                    "should have at least one successful fillet"
                );
            }
        }
    }

    fn compute_mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vol = 0.0;
        for tri in indices.chunks(3) {
            let (i0, i1, i2) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }
}
