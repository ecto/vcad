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
    let mut crossings = 0u32;
    for t in 0..mesh.indices.len() / 3 {
        match ray_triangle(point, mesh, t) {
            RayHit::OnBoundary => return true,
            RayHit::Cross => crossings += 1,
            RayHit::Miss => {}
        }
    }
    crossings % 2 == 1
}

/// Slightly tilted ray direction, so the cast avoids hitting edges/vertices
/// exactly. The exact predicates in [`ray_triangle`] handle what's left.
const RAY_DIR: [f64; 3] = [1.0, 1e-7, 1.3e-7];

/// Outcome of casting the [`RAY_DIR`] ray from a point at one triangle.
enum RayHit {
    /// No forward crossing.
    Miss,
    /// One forward crossing — flips the parity.
    Cross,
    /// The point lies ON this triangle; the whole query answers `true`.
    OnBoundary,
}

/// Cast the fixed ray from `point` at triangle `tri` of `mesh`.
///
/// Factored out of [`point_in_mesh`] so the indexed path in [`MeshRayIndex`]
/// runs the *same* test on the candidates it keeps: the index may only ever
/// drop triangles that this function would have reported `Miss` for, so
/// crossing parity is identical either way.
#[inline]
fn ray_triangle(point: &Point3, mesh: &TriangleMesh, tri: usize) -> RayHit {
    use vcad_kernel_math::predicates::{orient3d, Sign};

    let verts = &mesh.vertices;
    let idx = |k: usize| mesh.indices[tri * 3 + k] as usize * 3;
    let (i0, i1, i2) = (idx(0), idx(1), idx(2));

    let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
    let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
    let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];

    let ray_dir = RAY_DIR;

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

        // Check if query point is coplanar with triangle
        let sign = orient3d(point, &p0, &p1, &p2);
        if matches!(sign, Sign::Zero) && point_in_triangle_coplanar(point, &p0, &p1, &p2) {
            // Point on boundary - treat as inside (odd crossing)
            return RayHit::OnBoundary;
        }

        // The ray either misses the plane or pierces it somewhere this
        // routine has no robust intersection test for; either way it
        // contributes no counted crossing.
        return RayHit::Miss;
    }

    let f = 1.0 / a;
    let s = [point.x - v0[0], point.y - v0[1], point.z - v0[2]];

    let u = f * (s[0] * h[0] + s[1] * h[1] + s[2] * h[2]);
    if !(0.0..=1.0).contains(&u) {
        return RayHit::Miss;
    }

    // q = s × edge1
    let q = [
        s[1] * edge1[2] - s[2] * edge1[1],
        s[2] * edge1[0] - s[0] * edge1[2],
        s[0] * edge1[1] - s[1] * edge1[0],
    ];

    let v = f * (ray_dir[0] * q[0] + ray_dir[1] * q[1] + ray_dir[2] * q[2]);
    if v < 0.0 || u + v > 1.0 {
        return RayHit::Miss;
    }

    let t = f * (edge2[0] * q[0] + edge2[1] * q[1] + edge2[2] * q[2]);

    // Only count forward intersections (t > 0)
    if t > 1e-10 {
        RayHit::Cross
    } else {
        RayHit::Miss
    }
}

/// A reusable broadphase over one mesh for repeated [`point_in_mesh`] queries.
///
/// `point_in_mesh` scans every triangle, so classifying a whole solid costs
/// O(faces × triangles) — the reason a chain of cuts against one growing
/// subject costs quadratically more than a single batched cut: each cut
/// re-scans the entire accumulated mesh.
///
/// The cast ray is fixed at [`RAY_DIR`], essentially +X, so a triangle can
/// only be hit if the point's (y, z) falls in the triangle's (y, z) extent
/// and the triangle reaches forward of the point in x. Bucketing triangles
/// by their (y, z) box turns each query into a lookup of one or two cells.
///
/// The y/z window is widened by the ray's total transverse drift across the
/// mesh (`1.3e-7` per mm of x-span — about 1e-5 mm on a 100 mm part), so a
/// triangle the ray *does* reach can never be filtered out. Skipped
/// triangles are exactly those [`ray_triangle`] would have called `Miss`,
/// which is why the parity — and so the answer — is unchanged.
pub struct MeshRayIndex<'m> {
    mesh: &'m TriangleMesh,
    /// (y, z) grid origin and inverse cell size.
    origin: (f64, f64),
    inv_cell: (f64, f64),
    dims: (usize, usize),
    /// Triangle indices per cell, row-major over (y, z).
    cells: Vec<Vec<u32>>,
    /// Triangles spanning too many cells to bucket; tested on every query.
    overflow: Vec<u32>,
    /// Largest x reached by each triangle — a triangle entirely behind the
    /// query point cannot produce a forward crossing.
    tri_max_x: Vec<f64>,
    /// Transverse drift of the ray across the mesh's full x-span.
    slack: (f64, f64),
}

/// Above this many cells a single triangle goes to the overflow list rather
/// than being written into every cell it covers.
const MAX_CELLS_PER_TRI: usize = 64;

impl<'m> MeshRayIndex<'m> {
    /// Build an index over `mesh`. Costs one pass over the triangles.
    pub fn new(mesh: &'m TriangleMesh) -> Self {
        let n_tri = mesh.indices.len() / 3;
        let tri_box = |t: usize| {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for k in 0..3 {
                let i = mesh.indices[t * 3 + k] as usize * 3;
                for c in 0..3 {
                    let x = mesh.vertices[i + c] as f64;
                    lo[c] = lo[c].min(x);
                    hi[c] = hi[c].max(x);
                }
            }
            (lo, hi)
        };

        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut tri_max_x = Vec::with_capacity(n_tri);
        let mut boxes = Vec::with_capacity(n_tri);
        for t in 0..n_tri {
            let (tlo, thi) = tri_box(t);
            for c in 0..3 {
                lo[c] = lo[c].min(tlo[c]);
                hi[c] = hi[c].max(thi[c]);
            }
            tri_max_x.push(thi[0]);
            boxes.push((tlo, thi));
        }

        let span_x = if n_tri == 0 {
            0.0
        } else {
            (hi[0] - lo[0]).max(0.0)
        };
        let slack = (RAY_DIR[1] * span_x + 1e-9, RAY_DIR[2] * span_x + 1e-9);

        // Roughly one triangle per cell, capped so the grid stays small.
        let dim = ((n_tri as f64).sqrt().ceil() as usize).clamp(1, 256);
        let dims = (dim, dim);
        let extent = |c: usize| {
            let e = hi[c] - lo[c];
            if e.is_finite() && e > 0.0 {
                e
            } else {
                1.0
            }
        };
        let inv_cell = (dim as f64 / extent(1), dim as f64 / extent(2));
        let origin = (
            if lo[1].is_finite() { lo[1] } else { 0.0 },
            if lo[2].is_finite() { lo[2] } else { 0.0 },
        );

        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); dim * dim];
        let mut overflow = Vec::new();
        let cell_of = |v: f64, o: f64, inv: f64, n: usize| -> usize {
            (((v - o) * inv).floor().max(0.0) as usize).min(n - 1)
        };
        for (t, (tlo, thi)) in boxes.iter().enumerate() {
            let y0 = cell_of(tlo[1], origin.0, inv_cell.0, dims.0);
            let y1 = cell_of(thi[1], origin.0, inv_cell.0, dims.0);
            let z0 = cell_of(tlo[2], origin.1, inv_cell.1, dims.1);
            let z1 = cell_of(thi[2], origin.1, inv_cell.1, dims.1);
            if (y1 - y0 + 1) * (z1 - z0 + 1) > MAX_CELLS_PER_TRI {
                overflow.push(t as u32);
                continue;
            }
            for cy in y0..=y1 {
                for cz in z0..=z1 {
                    cells[cy * dims.1 + cz].push(t as u32);
                }
            }
        }

        Self {
            mesh,
            origin,
            inv_cell,
            dims,
            cells,
            overflow,
            tri_max_x,
            slack,
        }
    }

    /// The mesh this index was built over.
    pub fn mesh(&self) -> &'m TriangleMesh {
        self.mesh
    }

    /// Test whether `point` is inside the indexed mesh.
    ///
    /// Answers exactly what [`point_in_mesh`] would for the same mesh.
    pub fn contains(&self, point: &Point3) -> bool {
        if self.mesh.indices.is_empty() {
            return false;
        }
        // The ray leaves the point in +y/+z by at most `slack`; widen the
        // window by a hair on the near side too, so a triangle whose box
        // ends exactly at the point still qualifies.
        let cell_of = |v: f64, o: f64, inv: f64, n: usize| -> usize {
            (((v - o) * inv).floor().max(0.0) as usize).min(n - 1)
        };
        let y0 = cell_of(point.y - 1e-9, self.origin.0, self.inv_cell.0, self.dims.0);
        let y1 = cell_of(
            point.y + self.slack.0,
            self.origin.0,
            self.inv_cell.0,
            self.dims.0,
        );
        let z0 = cell_of(point.z - 1e-9, self.origin.1, self.inv_cell.1, self.dims.1);
        let z1 = cell_of(
            point.z + self.slack.1,
            self.origin.1,
            self.inv_cell.1,
            self.dims.1,
        );

        // A triangle bucketed into several of the visited cells must be
        // counted once, or the parity flips. Gather, dedup, then test.
        let mut cand: Vec<u32> = Vec::with_capacity(32);
        for cy in y0..=y1 {
            for cz in z0..=z1 {
                cand.extend_from_slice(&self.cells[cy * self.dims.1 + cz]);
            }
        }
        if y0 != y1 || z0 != z1 {
            cand.sort_unstable();
            cand.dedup();
        }
        cand.extend_from_slice(&self.overflow);

        let mut crossings = 0u32;
        for &t in &cand {
            // Behind the point: no forward crossing is possible.
            if self.tri_max_x[t as usize] < point.x {
                continue;
            }
            match ray_triangle(point, self.mesh, t as usize) {
                RayHit::OnBoundary => return true,
                RayHit::Cross => crossings += 1,
                RayHit::Miss => {}
            }
        }
        crossings % 2 == 1
    }
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
