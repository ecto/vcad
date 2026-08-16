//! Mesh-based utilities for boolean operations.

pub mod csg;

use std::collections::HashMap;

use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

/// Build an empty B-rep solid (no faces) with a valid (but empty) outer shell.
///
/// Used for boolean operations whose result is empty — e.g. intersection of
/// non-overlapping solids. Returning an empty `BRepSolid` keeps the result
/// type uniformly B-rep so that downstream code can rely on `as_brep()`
/// returning `Some(_)` without special-casing the empty case.
pub fn empty_brep() -> BRepSolid {
    let mut topology = Topology::new();
    let geometry = GeometryStore::new();
    let shell = topology.add_shell(Vec::new(), ShellType::Outer);
    let solid_id = topology.add_solid(shell);
    BRepSolid {
        topology,
        geometry,
        solid_id,
    }
}

/// Is this solid a triangle-soup B-rep (the [`mesh_to_brep`] stopgap
/// representation): hundreds of faces, every one a bare planar triangle?
///
/// Chained booleans on soup operands skip the B-rep pipeline — its
/// face-pair stages scale quadratically with face count and its splitters
/// gain nothing from anonymous triangles.
pub fn is_triangle_soup(solid: &BRepSolid) -> bool {
    let topo = &solid.topology;
    if topo.faces.len() < 256 {
        return false;
    }
    topo.faces
        .iter()
        .all(|(_, f)| f.inner_loops.is_empty() && topo.loop_len(f.outer_loop) == 3)
}

/// Build a B-rep from a triangle mesh by emitting one planar face per
/// triangle and pairing twin half-edges across shared edges.
///
/// This is a *stopgap* used by the perpendicular cylinder × cylinder
/// Steinmetz fallback: the boolean kernel emits the result as a watertight
/// mesh, and this helper wraps it back as a triangle-soup B-rep so that
/// downstream features that key off `Solid::as_brep()` continue to work.
/// The resulting topology is correct (one face per triangle, twins paired
/// by vertex match) but has no semantic surface grouping — every face is a
/// `Plane`. Callers that need higher-level topology (e.g. recovering the
/// underlying cylindrical surfaces) should still prefer a proper B-rep
/// boolean pipeline.
pub fn mesh_to_brep(mesh: &TriangleMesh) -> BRepSolid {
    if mesh.indices.is_empty() {
        return empty_brep();
    }

    let mut topology = Topology::new();
    let mut geometry = GeometryStore::new();
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();

    fn quantize(p: &Point3) -> [i64; 3] {
        [
            (p.x * 1e6).round() as i64,
            (p.y * 1e6).round() as i64,
            (p.z * 1e6).round() as i64,
        ]
    }

    let mut faces = Vec::new();

    for tri in mesh.indices.chunks(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        let p0 = Point3::new(
            mesh.vertices[i0 * 3] as f64,
            mesh.vertices[i0 * 3 + 1] as f64,
            mesh.vertices[i0 * 3 + 2] as f64,
        );
        let p1 = Point3::new(
            mesh.vertices[i1 * 3] as f64,
            mesh.vertices[i1 * 3 + 1] as f64,
            mesh.vertices[i1 * 3 + 2] as f64,
        );
        let p2 = Point3::new(
            mesh.vertices[i2 * 3] as f64,
            mesh.vertices[i2 * 3 + 1] as f64,
            mesh.vertices[i2 * 3 + 2] as f64,
        );

        let x_dir = p1 - p0;
        let y_dir = p2 - p0;
        if x_dir.norm() < 1e-12 || y_dir.norm() < 1e-12 {
            continue;
        }

        let v0 = *vertex_cache
            .entry(quantize(&p0))
            .or_insert_with(|| topology.add_vertex(p0));
        let v1 = *vertex_cache
            .entry(quantize(&p1))
            .or_insert_with(|| topology.add_vertex(p1));
        let v2 = *vertex_cache
            .entry(quantize(&p2))
            .or_insert_with(|| topology.add_vertex(p2));

        let surface_index = geometry.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)));
        let he0 = topology.add_half_edge(v0);
        let he1 = topology.add_half_edge(v1);
        let he2 = topology.add_half_edge(v2);
        let loop_id = topology.add_loop(&[he0, he1, he2]);
        let face_id = topology.add_face(loop_id, surface_index, Orientation::Forward);
        faces.push(face_id);
    }

    pair_twin_half_edges(&mut topology);

    let shell = topology.add_shell(faces, ShellType::Outer);
    let solid_id = topology.add_solid(shell);
    BRepSolid {
        topology,
        geometry,
        solid_id,
    }
}

/// Pair twin half-edges by matching `(origin, destination)` vertex pairs.
fn pair_twin_half_edges(topology: &mut Topology) {
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();
    let he_ids: Vec<HalfEdgeId> = topology.half_edges.keys().collect();
    for he_id in he_ids {
        let he = &topology.half_edges[he_id];
        let origin = topology.vertices[he.origin].point;
        let next = match he.next {
            Some(n) => n,
            None => continue,
        };
        let dest = topology.vertices[topology.half_edges[next].origin].point;
        let origin_key = [
            (origin.x * 1e6).round() as i64,
            (origin.y * 1e6).round() as i64,
            (origin.z * 1e6).round() as i64,
        ];
        let dest_key = [
            (dest.x * 1e6).round() as i64,
            (dest.y * 1e6).round() as i64,
            (dest.z * 1e6).round() as i64,
        ];
        if let Some(&twin_id) = he_map.get(&(dest_key, origin_key)) {
            if topology.half_edges[he_id].twin.is_none()
                && topology.half_edges[twin_id].twin.is_none()
            {
                topology.add_edge(he_id, twin_id);
            }
        }
        he_map.insert((origin_key, dest_key), he_id);
    }
}

/// Test if a point is inside a closed triangle mesh using ray casting with exact predicates.
///
/// Uses Shewchuk's exact orient3d predicate to robustly handle boundary cases where
/// the query point is exactly on a triangle plane. Uses a slightly tilted ray direction
/// to avoid edge/vertex hits in the common case, with exact predicates as fallback.
///
/// Casts a ray along a tilted direction. Odd crossing count = inside, even = outside.
pub fn point_in_mesh(point: &Point3, mesh: &TriangleMesh) -> bool {
    use vcad_kernel_math::predicates::{orient3d, Sign};

    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut crossings = 0u32;

    // Slightly tilted ray direction to avoid hitting edges/vertices exactly
    // The exact predicates handle remaining boundary cases robustly
    let ray_dir = [1.0f64, 1e-7, 1.3e-7];

    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;

        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];

        // Möller-Trumbore ray-triangle intersection
        let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

        // h = ray_dir × edge2
        let h = [
            ray_dir[1] * edge2[2] - ray_dir[2] * edge2[1],
            ray_dir[2] * edge2[0] - ray_dir[0] * edge2[2],
            ray_dir[0] * edge2[1] - ray_dir[1] * edge2[0],
        ];

        let a = edge1[0] * h[0] + edge1[1] * h[1] + edge1[2] * h[2];

        // Use exact orient3d to robustly check for degenerate cases
        if a.abs() < 1e-12 {
            // Ray nearly parallel to triangle - use exact predicate
            let p0 = Point3::new(v0[0], v0[1], v0[2]);
            let p1 = Point3::new(v1[0], v1[1], v1[2]);
            let p2 = Point3::new(v2[0], v2[1], v2[2]);
            let far_pt = Point3::new(
                point.x + ray_dir[0] * 1e10,
                point.y + ray_dir[1] * 1e10,
                point.z + ray_dir[2] * 1e10,
            );

            // Check if query point is coplanar with triangle
            let sign = orient3d(point, &p0, &p1, &p2);
            if matches!(sign, Sign::Zero) {
                // Point is on the triangle plane - check if inside triangle
                if point_in_triangle_coplanar(point, &p0, &p1, &p2) {
                    // Point on boundary - treat as inside (odd crossing)
                    return true;
                }
            }

            // Check if ray pierces the infinite plane containing the triangle
            let sign_far = orient3d(&far_pt, &p0, &p1, &p2);
            if sign == sign_far {
                continue; // Ray doesn't cross plane
            }
            // Would need more robust intersection test here, skip for now
            continue;
        }

        let f = 1.0 / a;
        let s = [point.x - v0[0], point.y - v0[1], point.z - v0[2]];

        let u = f * (s[0] * h[0] + s[1] * h[1] + s[2] * h[2]);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }

        // q = s × edge1
        let q = [
            s[1] * edge1[2] - s[2] * edge1[1],
            s[2] * edge1[0] - s[0] * edge1[2],
            s[0] * edge1[1] - s[1] * edge1[0],
        ];

        let v = f * (ray_dir[0] * q[0] + ray_dir[1] * q[1] + ray_dir[2] * q[2]);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }

        let t = f * (edge2[0] * q[0] + edge2[1] * q[1] + edge2[2] * q[2]);

        // Only count forward intersections (t > 0)
        if t > 1e-10 {
            crossings += 1;
        }
    }

    crossings % 2 == 1
}

/// Check if point p is inside triangle (v0, v1, v2) when all are coplanar.
/// Uses exact orient3d predicates for robust edge tests.
fn point_in_triangle_coplanar(p: &Point3, v0: &Point3, v1: &Point3, v2: &Point3) -> bool {
    use vcad_kernel_math::predicates::orient3d;

    // Compute triangle normal
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let normal = e1.cross(e2);
    let normal_len = normal.norm();
    if normal_len < 1e-15 {
        return false; // Degenerate triangle
    }

    // Create a reference point above the plane
    let ref_pt = Point3::new(
        p.x + normal.x / normal_len,
        p.y + normal.y / normal_len,
        p.z + normal.z / normal_len,
    );

    // Test orientation against each edge
    let s0 = orient3d(p, v0, v1, &ref_pt);
    let s1 = orient3d(p, v1, v2, &ref_pt);
    let s2 = orient3d(p, v2, v0, &ref_pt);

    // Point is inside if all orientations are consistent (all ≥0 or all ≤0)
    let all_non_neg = !s0.is_negative() && !s1.is_negative() && !s2.is_negative();
    let all_non_pos = !s0.is_positive() && !s1.is_positive() && !s2.is_positive();

    all_non_neg || all_non_pos
}
