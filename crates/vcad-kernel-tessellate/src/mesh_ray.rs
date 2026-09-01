//! Ray-parity point classification and membrane removal for triangle
//! meshes.
//!
//! Moved here from `vcad-kernel-booleans::mesh` so the tessellation repair
//! pipeline can classify its own output; the booleans crate re-exports
//! everything for its callers.

use std::collections::HashMap;

use vcad_kernel_math::Point3;

use crate::TriangleMesh;

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

/// Remove interior membrane triangles — facets with solid material on BOTH
/// sides.
///
/// A boolean that keeps an operand face inside the union of the two solids
/// (measured: `bore ∪ groove` retaining the bore wall segment that runs
/// through the groove cylinder) leaves a wall through solid material. Such
/// a membrane is invisible to volume checks, but any later difference that
/// opens a cavity across it exports it as a face spanning the opening —
/// the "bore looks open but slices closed" defect — and slicers report its
/// rim as non-manifold.
///
/// A triangle is a membrane when probes nudged off both its sides are
/// inside the mesh. The nudge scales with the triangle so a sliver's probe
/// stays in its own neighbourhood, and the whole check uses the same
/// parity classification the mesh boolean itself trusts. Removal can only
/// expose rims that the watertightness repair then re-pairs; triangles on
/// the true boundary always have one probe outside and are never touched.
pub fn remove_interior_membranes(mesh: &mut TriangleMesh) {
    if strip_membranes_once(mesh) {
        crate::repair_watertightness(mesh);
    }
}

/// One membrane-stripping pass; returns whether anything was removed.
/// Exposed at crate level so [`crate::repair_watertightness`] can use it
/// without recursing back into itself.
pub(crate) fn strip_membranes_once(mesh: &mut TriangleMesh) -> bool {
    let ntri = mesh.indices.len() / 3;
    if ntri == 0 {
        return false;
    }
    // A membrane's rim cannot pair cleanly, so a mesh whose every edge is
    // used exactly twice with balanced directions has none — skip the ray
    // casting entirely. (A sealed two-cell complex would slip through,
    // but the defects this pass exists for always break edge pairing.)
    {
        let mut counts: HashMap<(u32, u32), (u32, i32)> = HashMap::new();
        for t in mesh.indices.chunks(3) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a == b {
                    continue;
                }
                let e = counts.entry((a.min(b), a.max(b))).or_default();
                e.0 += 1;
                e.1 += if a < b { 1 } else { -1 };
            }
        }
        if counts.values().all(|&(n, net)| n == 2 && net == 0) {
            return false;
        }
    }
    let mut doomed = vec![false; ntri];
    {
        let index = MeshRayIndex::new(mesh);
        let p = |i: u32| {
            let k = i as usize * 3;
            Point3::new(
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        for (t, doom) in doomed.iter_mut().enumerate() {
            let (a, b, c) = (
                p(mesh.indices[t * 3]),
                p(mesh.indices[t * 3 + 1]),
                p(mesh.indices[t * 3 + 2]),
            );
            let n = (b - a).cross(c - a);
            let nn = n.norm();
            if nn < 1e-12 {
                continue;
            }
            let n = n / nn;
            // Same scaling rationale as the CSG classifier's probe_offset:
            // a fixed nudge overshoots a sliver's neighbourhood.
            let eps = (0.25 * (0.5 * nn).sqrt()).clamp(2e-6, 1e-3);
            let centroid = Point3::new(
                (a.x + b.x + c.x) / 3.0,
                (a.y + b.y + c.y) / 3.0,
                (a.z + b.z + c.z) / 3.0,
            );
            let hi = Point3::new(
                centroid.x + n.x * eps,
                centroid.y + n.y * eps,
                centroid.z + n.z * eps,
            );
            let lo = Point3::new(
                centroid.x - n.x * eps,
                centroid.y - n.y * eps,
                centroid.z - n.z * eps,
            );
            // Both sides solid → a wall through material. Both sides void
            // → a floating flap (measured: an un-notched annulus fragment
            // left spanning a slot mouth). Either way it is not boundary.
            let (hi_in, lo_in) = (index.contains(&hi), index.contains(&lo));
            if hi_in == lo_in {
                *doom = true;
            }
        }
    }
    let doomed_count = doomed.iter().filter(|&&d| d).count();
    if doomed_count == 0 {
        return false;
    }
    // Membranes are a localized defect. When a large share of the mesh
    // classifies as "membrane", the real story is that the mesh is too
    // open for parity to mean anything (a mid-pipeline wall fragment, an
    // imported open shell) — removing anything on that evidence would
    // gut it. Bail instead.
    if doomed_count * 10 > ntri {
        return false;
    }
    let tagged = mesh.face_kinds.len() == ntri;
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut kinds = Vec::new();
    for (t, doom) in doomed.iter().enumerate() {
        if !doom {
            indices.extend_from_slice(&mesh.indices[t * 3..t * 3 + 3]);
            if tagged {
                kinds.push(mesh.face_kinds[t]);
            }
        }
    }
    mesh.indices = indices;
    if tagged {
        mesh.face_kinds = kinds;
    }
    true
}
