//! Mesh-based utilities for boolean operations.

pub mod csg;

pub use vcad_kernel_tessellate::mesh_ray::{
    point_in_mesh, remove_interior_membranes, MeshRayIndex,
};

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
