#![warn(missing_docs)]

//! B-rep to triangle mesh tessellation for the vcad kernel.
//!
//! Converts B-rep faces into triangle meshes by:
//! 1. Sampling face boundaries in parameter space
//! 2. Generating interior sample points
//! 3. Triangulating via ear-clipping
//! 4. Mapping back to 3D via surface evaluation

use std::f64::consts::PI;
use vcad_kernel_geom::{BilinearSurface, GeometryStore, SurfaceKind};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation, Topology};

mod creased_normals;
mod render_bake;

pub use creased_normals::{
    apply_creased_normals, apply_default_creased_normals, DEFAULT_CREASE_ANGLE_RAD,
};
pub use render_bake::{render_bake, render_bake_default, RenderBakeOptions};

/// Per-triangle tag identifying which face kind the triangle came from.
/// Encoded as a single byte per triangle; `FanFill` = 8 marks the
/// synthetic armpit-fan triangles added post-weld.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceKindTag {
    /// Default / unknown.
    Unknown = 0,
    /// Plane.
    Plane = 1,
    /// Cylinder.
    Cylinder = 2,
    /// Sphere.
    Sphere = 3,
    /// Cone.
    Cone = 4,
    /// Bilinear.
    Bilinear = 5,
    /// Torus.
    Torus = 6,
    /// B-spline surface.
    BSpline = 7,
    /// Synthetic fan-fill triangle (added post-weld in
    /// `fill_tiny_boundary_loops` to close armpit gaps). Doesn't
    /// correspond to a BRep face.
    FanFill = 8,
}

impl From<SurfaceKind> for FaceKindTag {
    fn from(kind: SurfaceKind) -> Self {
        match kind {
            SurfaceKind::Plane => FaceKindTag::Plane,
            SurfaceKind::Cylinder => FaceKindTag::Cylinder,
            SurfaceKind::Sphere => FaceKindTag::Sphere,
            SurfaceKind::Cone => FaceKindTag::Cone,
            SurfaceKind::Bilinear => FaceKindTag::Bilinear,
            SurfaceKind::Torus => FaceKindTag::Torus,
            SurfaceKind::BSpline => FaceKindTag::BSpline,
        }
    }
}

/// Output triangle mesh for rendering and export.
#[derive(Debug, Clone)]
pub struct TriangleMesh {
    /// Flat array of vertex positions: `[x0, y0, z0, x1, y1, z1, ...]` (f32).
    pub vertices: Vec<f32>,
    /// Flat array of triangle indices: `[i0, i1, i2, ...]` (u32).
    pub indices: Vec<u32>,
    /// Flat array of vertex normals: `[nx0, ny0, nz0, ...]` (f32). Same length as vertices.
    pub normals: Vec<f32>,
    /// One byte per triangle (same length as `indices / 3`) tagging
    /// which face kind contributed it. Populated by `tessellate_brep`
    /// for debugging / inspection. Empty when the mesh was built
    /// without tagging (legacy paths).
    pub face_kinds: Vec<u8>,
}

impl TriangleMesh {
    /// Create an empty mesh.
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            normals: Vec::new(),
            face_kinds: Vec::new(),
        }
    }

    /// Number of triangles.
    pub fn num_triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Number of vertices.
    pub fn num_vertices(&self) -> usize {
        self.vertices.len() / 3
    }

    /// Merge another mesh into this one.
    pub fn merge(&mut self, other: &TriangleMesh) {
        let offset = self.num_vertices() as u32;

        // Validate other mesh indices before merge
        #[cfg(debug_assertions)]
        {
            let other_num_verts = other.num_vertices();
            for (i, &idx) in other.indices.iter().enumerate() {
                debug_assert!(
                    (idx as usize) < other_num_verts,
                    "Other mesh has invalid index {} at position {} (only {} vertices)",
                    idx,
                    i,
                    other_num_verts
                );
            }
        }

        // Keep the normals buffer in lockstep with vertices so the invariant
        // `normals.is_empty() || normals.len() == vertices.len()` survives in
        // release builds — the old check was debug-only and silently let a
        // mismatch through in release, where parallel-buffer consumers
        // (RealityKit/Metal, GLB/STL export, raytracer normal sampling) then
        // read out of bounds or render garbage. In the common case both
        // meshes carry one normal per vertex and this is a plain concat; if
        // either side is missing or partial, recompute that side's normals
        // from its own geometry before appending. Must run before the
        // vertices/indices extends below so it sees `self`'s pre-merge mesh.
        let self_normals_ok = self.normals.len() == self.vertices.len();
        let other_normals_ok = other.normals.len() == other.vertices.len();
        if self_normals_ok && other_normals_ok {
            self.normals.extend_from_slice(&other.normals);
        } else {
            if !self_normals_ok {
                self.normals = computed_vertex_normals(&self.vertices, &self.indices);
            }
            if other_normals_ok {
                self.normals.extend_from_slice(&other.normals);
            } else {
                self.normals
                    .extend_from_slice(&computed_vertex_normals(&other.vertices, &other.indices));
            }
        }

        self.vertices.extend_from_slice(&other.vertices);
        self.indices
            .extend(other.indices.iter().map(|&i| i + offset));
        // face_kinds is per-triangle (one u8 per 3 indices). Pad with
        // Unknown when one side doesn't carry tags so the combined
        // array stays in lockstep with `indices / 3`.
        let self_tri_before = (self.indices.len() - other.indices.len()) / 3;
        let other_tri = other.indices.len() / 3;
        if !other.face_kinds.is_empty() {
            while self.face_kinds.len() < self_tri_before {
                self.face_kinds.push(FaceKindTag::Unknown as u8);
            }
            self.face_kinds.extend_from_slice(&other.face_kinds);
        } else if !self.face_kinds.is_empty() {
            for _ in 0..other_tri {
                self.face_kinds.push(FaceKindTag::Unknown as u8);
            }
        }
    }

    /// Build a map of undirected edge `(min_idx, max_idx)` → number of
    /// adjacent triangles. Used by boundary / non-manifold diagnostics.
    fn edge_use_counts(&self) -> std::collections::HashMap<(u32, u32), u32> {
        let mut counts: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let tri_count = self.indices.len() / 3;
        for t in 0..tri_count {
            let tri = [
                self.indices[3 * t],
                self.indices[3 * t + 1],
                self.indices[3 * t + 2],
            ];
            for i in 0..3 {
                let a = tri[i];
                let b = tri[(i + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Find boundary edges: edges adjacent to exactly one triangle. In a
    /// closed manifold mesh this vector is empty; any entry points at a
    /// hole in the tessellation (e.g. the visible armpit gaps on the
    /// pork-chop fillet).
    ///
    /// Returned as `(vertex_index_a, vertex_index_b)` with `a < b`.
    pub fn boundary_edges(&self) -> Vec<(u32, u32)> {
        self.edge_use_counts()
            .into_iter()
            .filter(|(_, n)| *n == 1)
            .map(|(e, _)| e)
            .collect()
    }

    /// Like `boundary_edges` but returns each edge as a pair of world-
    /// space endpoints `[p_a, p_b]`. Convenient for dumping directly
    /// into a visualizer or log.
    pub fn boundary_edge_positions(&self) -> Vec<[[f32; 3]; 2]> {
        self.boundary_edges()
            .into_iter()
            .map(|(a, b)| {
                let ia = a as usize * 3;
                let ib = b as usize * 3;
                [
                    [
                        self.vertices[ia],
                        self.vertices[ia + 1],
                        self.vertices[ia + 2],
                    ],
                    [
                        self.vertices[ib],
                        self.vertices[ib + 1],
                        self.vertices[ib + 2],
                    ],
                ]
            })
            .collect()
    }

    /// Find non-manifold edges: edges adjacent to 3 or more triangles.
    /// A closed manifold mesh has none; any entry indicates a topology
    /// bug (overlapping faces, welded seams crossing themselves, etc.).
    pub fn non_manifold_edges(&self) -> Vec<(u32, u32, u32)> {
        self.edge_use_counts()
            .into_iter()
            .filter(|(_, n)| *n >= 3)
            .map(|((a, b), n)| (a, b, n))
            .collect()
    }

    /// Group boundary edges into connected loops (chains of vertex
    /// indices). Each loop should close on itself in a well-formed
    /// hole; if it doesn't, the returned chain ends at the first
    /// vertex that has no unvisited boundary neighbor.
    pub fn boundary_loops(&self) -> Vec<Vec<u32>> {
        let edges = self.boundary_edges();
        if edges.is_empty() {
            return Vec::new();
        }
        let mut adj: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        for &(a, b) in &edges {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let mut visited_edges: std::collections::HashSet<(u32, u32)> =
            std::collections::HashSet::new();
        let ek = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut loops: Vec<Vec<u32>> = Vec::new();
        for &(start_a, start_b) in &edges {
            if visited_edges.contains(&ek(start_a, start_b)) {
                continue;
            }
            visited_edges.insert(ek(start_a, start_b));
            let mut chain = vec![start_a, start_b];
            loop {
                let cur = *chain.last().unwrap();
                let prev = chain[chain.len() - 2];
                let next = match adj.get(&cur) {
                    Some(nbrs) => nbrs
                        .iter()
                        .copied()
                        .find(|&n| n != prev && !visited_edges.contains(&ek(cur, n))),
                    None => None,
                };
                match next {
                    Some(n) => {
                        visited_edges.insert(ek(cur, n));
                        if n == start_a {
                            break;
                        }
                        chain.push(n);
                    }
                    None => break,
                }
            }
            loops.push(chain);
        }
        loops
    }
}

impl Default for TriangleMesh {
    fn default() -> Self {
        Self::new()
    }
}

/// Tessellation parameters controlling mesh quality.
///
/// `circle_segments` / `latitude_segments` set a fixed lower bound. When
/// `chord_tolerance` and/or `angular_tolerance` are also set, the per-feature
/// segment count is raised so curvature error stays below the tolerance — a
/// 1mm fillet then gets fewer triangles than a 100mm cylinder, while large
/// cylinders no longer look faceted under polished lighting.
#[derive(Debug, Clone, Copy)]
pub struct TessellationParams {
    /// Minimum number of segments for circular features.
    pub circle_segments: u32,
    /// Number of segments along the height of cylindrical/conical features.
    pub height_segments: u32,
    /// Minimum number of latitude bands for spherical features.
    pub latitude_segments: u32,
    /// Optional sag tolerance in model units (mm). When set, segment counts
    /// are raised so the chord between two adjacent samples never deviates
    /// from the true surface by more than this distance.
    pub chord_tolerance: Option<f64>,
    /// Optional angular tolerance in radians. When set, segment counts are
    /// raised so no single segment subtends more than this angle.
    pub angular_tolerance: Option<f64>,
}

impl Default for TessellationParams {
    fn default() -> Self {
        Self {
            circle_segments: 32,
            height_segments: 1,
            latitude_segments: 16,
            chord_tolerance: None,
            angular_tolerance: None,
        }
    }
}

const ADAPTIVE_SEG_FLOOR: u32 = 3;
const ADAPTIVE_SEG_CEIL: u32 = 256;

impl TessellationParams {
    /// Create params from a segment count hint (used for circular features).
    pub fn from_segments(segments: u32) -> Self {
        Self {
            circle_segments: segments.max(3),
            height_segments: 1,
            latitude_segments: (segments / 2).max(4),
            chord_tolerance: None,
            angular_tolerance: None,
        }
    }

    /// Create params from a chord tolerance (mm) and an angular tolerance
    /// (radians). Both are optional, but at least one must be specified to
    /// drive adaptive subdivision.
    pub fn from_tolerances(chord: Option<f64>, angular: Option<f64>) -> Self {
        Self {
            circle_segments: 16,
            height_segments: 1,
            latitude_segments: 8,
            chord_tolerance: chord,
            angular_tolerance: angular,
        }
    }

    /// Number of segments around the circumference of a feature with the
    /// given radius. Always at least `circle_segments`; raised if a
    /// chord or angular tolerance is set.
    pub fn circle_segments_for_radius(&self, radius: f64) -> u32 {
        let mut n = self.circle_segments.max(ADAPTIVE_SEG_FLOOR);
        if let Some(tol) = self.chord_tolerance {
            // Sag for a chord on a circle of radius r with n segments is
            //   e = r * (1 - cos(π/n))
            // Solving for n given a target e gives
            //   n = π / acos(1 − e/r)
            // Clamp the acos argument so a degenerate (tol >= r) case
            // doesn't blow up — we just fall through to the floor.
            if radius > 0.0 && tol > 0.0 && tol < radius {
                let arg = (1.0 - tol / radius).clamp(-1.0, 1.0);
                let denom = arg.acos();
                if denom > 1e-9 {
                    let segs = (PI / denom).ceil() as u32;
                    n = n.max(segs);
                }
            }
        }
        if let Some(ang) = self.angular_tolerance {
            if ang > 1e-9 {
                let segs = ((2.0 * PI) / ang).ceil() as u32;
                n = n.max(segs);
            }
        }
        n.clamp(ADAPTIVE_SEG_FLOOR, ADAPTIVE_SEG_CEIL)
    }

    /// Number of latitude bands for a sphere of the given radius. The
    /// returned count is roughly half of the longitude segment count, so
    /// triangles stay reasonably square.
    pub fn latitude_segments_for_radius(&self, radius: f64) -> u32 {
        let lon = self.circle_segments_for_radius(radius);
        self.latitude_segments
            .max(ADAPTIVE_SEG_FLOOR)
            .max(lon / 2)
            .clamp(ADAPTIVE_SEG_FLOOR, ADAPTIVE_SEG_CEIL)
    }
}

/// Tessellate an entire B-rep solid into a triangle mesh.
/// Tessellate each face independently, returning one mesh per face along
/// with the face's id and surface kind. Useful for diagnostic overlays
/// that color-code faces by surface type, and for the standalone fillet
/// harness (dumps an OBJ where each face becomes a separate `g` group).
pub fn tessellate_brep_by_face(
    brep: &BRepSolid,
    params: &TessellationParams,
) -> Vec<(FaceId, SurfaceKind, TriangleMesh)> {
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];
    let mut out = Vec::with_capacity(shell.faces.len());
    for &face_id in &shell.faces {
        let surface = &brep.geometry.surfaces[brep.topology.faces[face_id].surface_index];
        let kind = surface.surface_type();
        let face_mesh = tessellate_face(&brep.topology, &brep.geometry, face_id, params);
        out.push((face_id, kind, face_mesh));
    }
    out
}

/// Legacy solid tessellation entry point. Most callers should prefer
/// `tessellate_brep` (which also runs armpit fan-fill). This path is
/// retained for internal use by paths that don't need the post-weld
/// fill (e.g. intermediate meshes produced during boolean operations).
pub fn tessellate_solid(brep: &BRepSolid, params: &TessellationParams) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    for &face_id in &shell.faces {
        let face_mesh = tessellate_face(&brep.topology, &brep.geometry, face_id, params);

        // Validate face mesh before merge
        #[cfg(debug_assertions)]
        {
            let num_verts = face_mesh.num_vertices();
            for (i, &idx) in face_mesh.indices.iter().enumerate() {
                debug_assert!(
                    (idx as usize) < num_verts,
                    "Face {:?} has invalid index {} at position {} (only {} vertices)",
                    face_id,
                    idx,
                    i,
                    num_verts
                );
            }
        }

        mesh.merge(&face_mesh);
    }

    // Each face tessellates independently and `merge` concatenates without
    // deduplicating vertices, so adjacent faces that share a 3D edge emit
    // four boundary edges in the combined mesh instead of two shared ones
    // — the "armpit hole" pattern visible on extruded multi-arc profiles.
    // Weld coincident vertices so the shell is watertight at face seams.
    weld_coincident_vertices(&mut mesh);

    // NOTE: we don't run fill_tiny_boundary_loops here because this
    // path doesn't track sphere-face vertex positions. The primary
    // `tessellate_brep` entrypoint handles armpit-gap fill via the
    // sphere-key gate, and this function is only reached via the
    // legacy tessellate_solid entrypoint which doesn't need it.

    // Validate final mesh
    #[cfg(debug_assertions)]
    {
        let num_verts = mesh.num_vertices();
        for (i, &idx) in mesh.indices.iter().enumerate() {
            debug_assert!(
                (idx as usize) < num_verts,
                "Final mesh has invalid index {} at position {} (only {} vertices)",
                idx,
                i,
                num_verts
            );
        }
    }

    mesh
}

/// Smooth per-vertex normals computed from indexed triangle geometry: each
/// vertex accumulates the area-weighted face normals of the triangles that
/// use it, then is normalized. This is the canonical "fill missing normals"
/// path for an *indexed* mesh — used by [`TriangleMesh::merge`] and
/// [`weld_coincident_vertices`] to repair an empty or partial normals buffer
/// so `normals.len()` always matches `vertices.len()`. (For an unindexed,
/// crease-split result use [`apply_creased_normals`] instead.) Degenerate
/// accumulators fall back to `+Z`, matching `creased_normals`.
fn computed_vertex_normals(vertices: &[f32], indices: &[u32]) -> Vec<f32> {
    let vcount = vertices.len() / 3;
    let mut acc = vec![0.0f32; vertices.len()];
    let tri_count = indices.len() / 3;
    for t in 0..tri_count {
        let i0 = indices[3 * t] as usize;
        let i1 = indices[3 * t + 1] as usize;
        let i2 = indices[3 * t + 2] as usize;
        if i0 >= vcount || i1 >= vcount || i2 >= vcount {
            continue;
        }
        let p = |i: usize| [vertices[3 * i], vertices[3 * i + 1], vertices[3 * i + 2]];
        let (a, b, c) = (p(i0), p(i1), p(i2));
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        // Unnormalized cross product → area-weighted accumulation. Winding
        // matches `creased_normals::face_normal` (cross(p1-p0, p2-p0)).
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &vi in &[i0, i1, i2] {
            acc[3 * vi] += n[0];
            acc[3 * vi + 1] += n[1];
            acc[3 * vi + 2] += n[2];
        }
    }
    for v in 0..vcount {
        let (nx, ny, nz) = (acc[3 * v], acc[3 * v + 1], acc[3 * v + 2]);
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        if len > 1e-12 {
            acc[3 * v] = nx / len;
            acc[3 * v + 1] = ny / len;
            acc[3 * v + 2] = nz / len;
        } else {
            acc[3 * v] = 0.0;
            acc[3 * v + 1] = 0.0;
            acc[3 * v + 2] = 1.0;
        }
    }
    acc
}

/// Collapse mesh vertices that share a position to 3 decimal places.
/// Indices are rewritten; positions kept from the first occurrence.
/// Normals of the winning vertex are kept as-is — downstream smoothing is
/// unchanged. Triangles whose three corners collapse to fewer than three
/// distinct vertices are dropped (degenerate after weld).
fn weld_coincident_vertices(mesh: &mut TriangleMesh) {
    let n = mesh.num_vertices();
    if n == 0 {
        return;
    }
    // Spatial hash at 0.001 mm resolution — tight enough to only weld
    // true coincidences, loose enough to absorb float drift between
    // CylinderSurface::evaluate and our hand-computed arc sample points.
    let key = |i: usize| -> [i64; 3] {
        [
            (mesh.vertices[3 * i] as f64 * 1000.0).round() as i64,
            (mesh.vertices[3 * i + 1] as f64 * 1000.0).round() as i64,
            (mesh.vertices[3 * i + 2] as f64 * 1000.0).round() as i64,
        ]
    };
    // The welded output copies one normal per surviving vertex, so the input
    // must carry either no normals or exactly one per vertex. A partial buffer
    // (e.g. an empty-normal face merged into a normal-bearing one) would
    // otherwise produce `normals.len() < 3 * vertices` — a silent mismatch and
    // an out-of-bounds hazard for parallel-buffer consumers, and only in
    // release builds where merge's check is gone. Recompute from geometry so
    // weld only ever sees an empty-or-complete buffer.
    if !mesh.normals.is_empty() && mesh.normals.len() != mesh.vertices.len() {
        mesh.normals = computed_vertex_normals(&mesh.vertices, &mesh.indices);
    }
    let copy_normals = !mesh.normals.is_empty();

    let mut remap: Vec<u32> = vec![u32::MAX; n];
    let mut dedup: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut new_vertices: Vec<f32> = Vec::with_capacity(mesh.vertices.len());
    let mut new_normals: Vec<f32> = Vec::with_capacity(mesh.normals.len());
    for (i, remap_i) in remap.iter_mut().enumerate() {
        let k = key(i);
        let idx = *dedup.entry(k).or_insert_with(|| {
            let new_i = (new_vertices.len() / 3) as u32;
            new_vertices.extend_from_slice(&mesh.vertices[3 * i..3 * i + 3]);
            // `copy_normals` guarantees `normals.len() == vertices.len()`, so
            // `3 * i + 3 <= normals.len()` for every `i < n` — no underrun.
            if copy_normals {
                new_normals.extend_from_slice(&mesh.normals[3 * i..3 * i + 3]);
            }
            new_i
        });
        *remap_i = idx;
    }
    let mut new_indices: Vec<u32> = Vec::with_capacity(mesh.indices.len());
    let had_face_kinds = mesh.face_kinds.len() == mesh.indices.len() / 3;
    let mut new_face_kinds: Vec<u8> = if had_face_kinds {
        Vec::with_capacity(mesh.face_kinds.len())
    } else {
        Vec::new()
    };
    let tri_count = mesh.indices.len() / 3;
    for t in 0..tri_count {
        let a = remap[mesh.indices[3 * t] as usize];
        let b = remap[mesh.indices[3 * t + 1] as usize];
        let c = remap[mesh.indices[3 * t + 2] as usize];
        if a == b || b == c || c == a {
            continue; // degenerate after weld — drop
        }
        new_indices.extend_from_slice(&[a, b, c]);
        if had_face_kinds {
            new_face_kinds.push(mesh.face_kinds[t]);
        }
    }
    mesh.vertices = new_vertices;
    mesh.normals = new_normals;
    mesh.indices = new_indices;
    if had_face_kinds {
        mesh.face_kinds = new_face_kinds;
    }

    // Invariant restated for release builds: the welded mesh carries either no
    // normals or exactly one per vertex. The recompute above makes this hold;
    // this assert documents it and trips loudly if a future edit regresses it.
    assert!(
        mesh.normals.is_empty() || mesh.normals.len() == mesh.vertices.len(),
        "weld produced {} normals for {} vertices",
        mesh.normals.len(),
        mesh.vertices.len()
    );
}

/// Close any boundary loop with at most `max_verts` vertices that
/// touches a position from `sphere_keys` (a set of quantized positions
/// of verts that came from Sphere faces) by fanning triangles from its
/// centroid.
///
/// Gating on sphere-origin positions makes this selective to convex
/// fillet vertex-blend armpit gaps (which are always adjacent to a
/// sphere patch). Plate-with-hole / counterbore cutouts don't produce
/// sphere faces, so their boundary loops are never touched even if
/// they happen to have ≤ max_verts corners.
fn fill_tiny_boundary_loops(
    mesh: &mut TriangleMesh,
    max_verts: usize,
    sphere_keys: &std::collections::HashSet<[i64; 3]>,
) {
    if sphere_keys.is_empty() {
        return;
    }

    // Undirected boundary loops are reliable. For each loop, check the
    // existing adjacent triangle's vertex normals — use their average
    // as the outward reference and flip the fan winding if needed.
    use std::collections::{HashMap, HashSet};
    let loops = mesh.boundary_loops();
    if loops.is_empty() {
        return;
    }

    // Solid centroid — average of all mesh vertex positions. Used as a
    // robust outward reference: (loop_centroid - solid_centroid) is the
    // outward direction at the loop.
    let vert_count = mesh.num_vertices();
    let mut scx = 0.0_f32;
    let mut scy = 0.0_f32;
    let mut scz = 0.0_f32;
    for vi in 0..vert_count {
        scx += mesh.vertices[3 * vi];
        scy += mesh.vertices[3 * vi + 1];
        scz += mesh.vertices[3 * vi + 2];
    }
    if vert_count > 0 {
        scx /= vert_count as f32;
        scy /= vert_count as f32;
        scz /= vert_count as f32;
    }

    // Map each directed boundary edge (a, b) to the third corner of
    // its adjacent triangle (there's exactly one). We use this third
    // corner's outward-normal side as the "outside" reference.
    let tri_count = mesh.indices.len() / 3;
    let mut dir_counts: HashMap<(u32, u32), u32> = HashMap::new();
    let mut tri_third: HashMap<(u32, u32), u32> = HashMap::new();
    for t in 0..tri_count {
        let a = mesh.indices[3 * t];
        let b = mesh.indices[3 * t + 1];
        let c = mesh.indices[3 * t + 2];
        for &(x, y, z) in &[(a, b, c), (b, c, a), (c, a, b)] {
            *dir_counts.entry((x, y)).or_insert(0) += 1;
            tri_third.entry((x, y)).or_insert(z);
        }
    }
    let is_boundary = |a: u32, b: u32| -> bool {
        let fwd = dir_counts.get(&(a, b)).copied().unwrap_or(0);
        let bwd = dir_counts.get(&(b, a)).copied().unwrap_or(0);
        fwd + bwd == 1
    };
    let _ = is_boundary;
    let _ = HashSet::<u32>::new();

    for loop_verts in &loops {
        let n = loop_verts.len();
        // Skip 3-vert loops: those are the sphere vertex-blend patch
        // triangles themselves — already a single face of the shell,
        // just topologically isolated (all 3 edges unshared). They
        // render fine as-is. Fan-filling them creates a coplanar
        // double cover that causes z-fighting.
        if !(4..=max_verts).contains(&n) {
            continue;
        }
        let positions: Vec<[f32; 3]> = loop_verts
            .iter()
            .map(|&v| {
                [
                    mesh.vertices[3 * v as usize],
                    mesh.vertices[3 * v as usize + 1],
                    mesh.vertices[3 * v as usize + 2],
                ]
            })
            .collect();
        let touches_sphere = positions.iter().any(|p| {
            let q = [
                (p[0] as f64 * 1000.0).round() as i64,
                (p[1] as f64 * 1000.0).round() as i64,
                (p[2] as f64 * 1000.0).round() as i64,
            ];
            sphere_keys.contains(&q)
        });
        if !touches_sphere {
            continue;
        }

        let mut cx = 0.0_f32;
        let mut cy = 0.0_f32;
        let mut cz = 0.0_f32;
        for p in &positions {
            cx += p[0];
            cy += p[1];
            cz += p[2];
        }
        cx /= n as f32;
        cy /= n as f32;
        cz /= n as f32;

        let mut max_r2 = 0.0_f32;
        for p in &positions {
            let dx = p[0] - cx;
            let dy = p[1] - cy;
            let dz = p[2] - cz;
            let r2 = dx * dx + dy * dy + dz * dz;
            if r2 > max_r2 {
                max_r2 = r2;
            }
        }
        let mut total_edge = 0.0_f32;
        for i in 0..n {
            let a = positions[i];
            let b = positions[(i + 1) % n];
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let dz = b[2] - a[2];
            total_edge += (dx * dx + dy * dy + dz * dz).sqrt();
        }
        let avg_edge = total_edge / n as f32;
        if max_r2.sqrt() > avg_edge * 6.0 {
            continue;
        }

        let mut nx = 0.0_f32;
        let mut ny = 0.0_f32;
        let mut nz = 0.0_f32;
        for i in 0..n {
            let a = positions[i];
            let b = positions[(i + 1) % n];
            let ex = a[0] - cx;
            let ey = a[1] - cy;
            let ez = a[2] - cz;
            let fx = b[0] - cx;
            let fy = b[1] - cy;
            let fz = b[2] - cz;
            nx += ey * fz - ez * fy;
            ny += ez * fx - ex * fz;
            nz += ex * fy - ey * fx;
        }
        let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
        if nlen < 1e-6 {
            continue;
        }

        // Each edge of the loop may come from a different adjacent
        // face with its own outward-CCW direction, so we make the
        // winding choice per-edge rather than once for the whole fan.
        // Record whether `(a, b)` or `(b, a)` is the existing boundary
        // direction for each edge so we can emit the fan triangle with
        // the opposite direction — that way the fan's CCW normal
        // aligns with the adjacent existing triangle's outward CCW.
        let mut edge_dirs: Vec<bool> = Vec::with_capacity(n); // true if (a,b) matches existing
        for i in 0..n {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % n];
            let fwd = dir_counts.get(&(a, b)).copied().unwrap_or(0);
            let bwd = dir_counts.get(&(b, a)).copied().unwrap_or(0);
            edge_dirs.push(fwd >= bwd);
        }

        // Robust outward reference: direction from the solid centroid
        // to the loop centroid. Works for any convex-ish solid and
        // doesn't depend on local face tessellation winding, which
        // can be counter-intuitive near fillet blends.
        let mut outward = [cx - scx, cy - scy, cz - scz];
        let olen0 =
            (outward[0] * outward[0] + outward[1] * outward[1] + outward[2] * outward[2]).sqrt();
        if olen0 > 1e-6 {
            outward[0] /= olen0;
            outward[1] /= olen0;
            outward[2] /= olen0;
        }

        // Average the adjacent boundary triangles' VERTEX normals at
        // a, b so cx shades consistently with its neighbours.
        let mut cx_vnormal = [0.0_f32; 3];
        let mut adj_count = 0usize;
        for i in 0..n {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % n];
            let (ea, eb, ec) = if let Some(&c) = tri_third.get(&(a, b)) {
                (a, b, c)
            } else if let Some(&c) = tri_third.get(&(b, a)) {
                (b, a, c)
            } else {
                continue;
            };
            let pa = [
                mesh.vertices[3 * ea as usize],
                mesh.vertices[3 * ea as usize + 1],
                mesh.vertices[3 * ea as usize + 2],
            ];
            let pb = [
                mesh.vertices[3 * eb as usize],
                mesh.vertices[3 * eb as usize + 1],
                mesh.vertices[3 * eb as usize + 2],
            ];
            let pc = [
                mesh.vertices[3 * ec as usize],
                mesh.vertices[3 * ec as usize + 1],
                mesh.vertices[3 * ec as usize + 2],
            ];
            let _ = (pa, pb, pc);

            if mesh.normals.len() >= 3 * ea as usize + 3 {
                cx_vnormal[0] += mesh.normals[3 * ea as usize];
                cx_vnormal[1] += mesh.normals[3 * ea as usize + 1];
                cx_vnormal[2] += mesh.normals[3 * ea as usize + 2];
                cx_vnormal[0] += mesh.normals[3 * eb as usize];
                cx_vnormal[1] += mesh.normals[3 * eb as usize + 1];
                cx_vnormal[2] += mesh.normals[3 * eb as usize + 2];
            }
            adj_count += 1;
        }
        let vnlen = (cx_vnormal[0] * cx_vnormal[0]
            + cx_vnormal[1] * cx_vnormal[1]
            + cx_vnormal[2] * cx_vnormal[2])
            .sqrt();
        if vnlen > 1e-6 {
            cx_vnormal[0] /= vnlen;
            cx_vnormal[1] /= vnlen;
            cx_vnormal[2] /= vnlen;
        } else {
            cx_vnormal = outward;
        }
        let _ = adj_count;

        // Offset the centroid outward by a tiny amount so fan
        // triangles avoid exact degeneracies (e.g. when the loop
        // centroid sits on the V_axial column of an armpit hex).
        // Kept small to avoid visible "fins" protruding from the
        // filleted surface — big offsets made the fan look like
        // spikes sticking out of each junction.
        let offset_scale = avg_edge * 0.05;
        let cx_idx = (mesh.vertices.len() / 3) as u32;
        mesh.vertices.extend_from_slice(&[
            cx + outward[0] * offset_scale,
            cy + outward[1] * offset_scale,
            cz + outward[2] * offset_scale,
        ]);
        mesh.normals.extend_from_slice(&cx_vnormal);

        // Emit each fan triangle with winding chosen so its CCW face
        // normal points roughly outward (away from the solid
        // centroid). For each edge we compute both candidate CCW
        // normals — (cx, a, b) and (cx, b, a) — and pick whichever
        // has a larger dot product with the LOCAL outward direction
        // (fan-triangle-centroid − solid-centroid). This is stable
        // even for non-planar loops where edges come from different
        // adjacent faces with different local outward senses.
        let cx_pos = [
            mesh.vertices[3 * cx_idx as usize],
            mesh.vertices[3 * cx_idx as usize + 1],
            mesh.vertices[3 * cx_idx as usize + 2],
        ];
        for i in 0..n {
            let a = loop_verts[i];
            let b = loop_verts[(i + 1) % n];
            let pa = [
                mesh.vertices[3 * a as usize],
                mesh.vertices[3 * a as usize + 1],
                mesh.vertices[3 * a as usize + 2],
            ];
            let pb = [
                mesh.vertices[3 * b as usize],
                mesh.vertices[3 * b as usize + 1],
                mesh.vertices[3 * b as usize + 2],
            ];
            // Triangle centroid (cx + a + b) / 3.
            let tri_cx = (cx_pos[0] + pa[0] + pb[0]) / 3.0;
            let tri_cy = (cx_pos[1] + pa[1] + pb[1]) / 3.0;
            let tri_cz = (cx_pos[2] + pa[2] + pb[2]) / 3.0;
            let local_outward = [tri_cx - scx, tri_cy - scy, tri_cz - scz];

            // Fan CCW normal for (cx, a, b): (pa - cx) × (pb - cx).
            let e1 = [pa[0] - cx_pos[0], pa[1] - cx_pos[1], pa[2] - cx_pos[2]];
            let e2 = [pb[0] - cx_pos[0], pb[1] - cx_pos[1], pb[2] - cx_pos[2]];
            let fan_n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let dot = fan_n[0] * local_outward[0]
                + fan_n[1] * local_outward[1]
                + fan_n[2] * local_outward[2];
            if dot >= 0.0 {
                mesh.indices.extend_from_slice(&[cx_idx, a, b]);
            } else {
                mesh.indices.extend_from_slice(&[cx_idx, b, a]);
            }
            if !mesh.face_kinds.is_empty() {
                mesh.face_kinds.push(FaceKindTag::FanFill as u8);
            }
        }
    }
}

/// Tessellate a single B-rep face.
fn tessellate_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let reversed = face.orientation == Orientation::Reversed;

    match surface.surface_type() {
        SurfaceKind::Plane => tessellate_planar_face_with_geom(topo, geom, face_id, reversed),
        SurfaceKind::Cylinder => tessellate_cylindrical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Sphere => tessellate_spherical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Cone => tessellate_conical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Bilinear => tessellate_bilinear_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Torus => tessellate_toroidal_face(topo, geom, face_id, params, reversed),
        SurfaceKind::BSpline => tessellate_bspline_face(topo, geom, face_id, params, reversed),
    }
}

/// Tessellate a planar face. Producers (primitives + booleans) wind the
/// loop consistent with the surface's stored normal; `Orientation::Reversed`
/// flips the effective face normal without touching loop ordering — so the
/// volume integrator and renderer see the normal the topology intends.
fn tessellate_planar_face_with_geom(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let surface_normal = surface.normal(Point2::new(0.0, 0.0));

    let outer_verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    if outer_verts.len() < 3 {
        return TriangleMesh::new();
    }

    let mut mesh = if !face.inner_loops.is_empty() {
        tessellate_planar_face_with_holes(topo, face_id, reversed)
    } else {
        tessellate_planar_face_core(&outer_verts, reversed)
    };

    let face_normal = if reversed {
        -surface_normal
    } else {
        surface_normal
    };
    let (nx, ny, nz) = (
        face_normal.x as f32,
        face_normal.y as f32,
        face_normal.z as f32,
    );
    for _ in 0..mesh.num_vertices() {
        mesh.normals.extend_from_slice(&[nx, ny, nz]);
    }

    mesh
}

/// Core tessellation logic for a planar polygon without holes.
fn tessellate_planar_face_core(outer_verts: &[Point3], reversed: bool) -> TriangleMesh {
    // Find the best fan center vertex index.
    // For faces with curved boundaries (like quarter disks), we need to pick a vertex
    // that's at the junction of straight edges, not on the curved portion.
    // Heuristic: find a vertex where consecutive edges form a significant angle (corner vertex).
    // Returns None if the polygon is too concave for fan triangulation.
    match find_best_fan_center(outer_verts) {
        Some(fan_center) => {
            // Fan triangulation is valid for this polygon
            let mut mesh = TriangleMesh::new();
            let n = outer_verts.len();

            // Add all vertices (rotated so fan_center is at index 0)
            for i in 0..n {
                let v = &outer_verts[(fan_center + i) % n];
                mesh.vertices.push(v.x as f32);
                mesh.vertices.push(v.y as f32);
                mesh.vertices.push(v.z as f32);
            }

            // Fan triangulation from vertex 0 (which is now the best fan center)
            for i in 1..(n - 1) {
                if reversed {
                    mesh.indices.push(0);
                    mesh.indices.push((i + 1) as u32);
                    mesh.indices.push(i as u32);
                } else {
                    mesh.indices.push(0);
                    mesh.indices.push(i as u32);
                    mesh.indices.push((i + 1) as u32);
                }
            }

            mesh
        }
        None => {
            // Polygon is too concave for fan triangulation - use ear clipping
            tessellate_concave_polygon(outer_verts, reversed)
        }
    }
}

/// Tessellate a concave polygon using ear clipping algorithm.
/// This is the fallback when fan triangulation cannot produce valid triangles.
fn tessellate_concave_polygon(verts: &[Point3], reversed: bool) -> TriangleMesh {
    let n = verts.len();
    if n < 3 {
        return TriangleMesh::new();
    }

    // Build 2D projection for ear clipping.
    // Compute the face plane from first 3 non-collinear vertices.
    let e1 = verts[1] - verts[0];
    let e2 = verts[2] - verts[0];
    let mut face_normal = e1.cross(e2);
    for i in 3..n {
        if face_normal.norm() > 1e-12 {
            break;
        }
        let ei = verts[i] - verts[0];
        face_normal = e1.cross(ei);
    }

    if face_normal.norm() < 1e-12 {
        // Degenerate polygon - all points collinear
        return TriangleMesh::new();
    }

    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(e1).normalize();
    let origin = verts[0];

    // Project 3D points to 2D
    let verts_2d: Vec<(f64, f64)> = verts
        .iter()
        .map(|p| {
            let d = *p - origin;
            (d.dot(u_axis), d.dot(v_axis))
        })
        .collect();

    // Build mesh with all 3D vertices
    let mut mesh = TriangleMesh::new();
    for v in verts {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    // Run ear clipping on the 2D projection
    let indices: Vec<usize> = (0..n).collect();
    ear_clip_triangulate(&verts_2d, &indices, &mut mesh.indices, reversed);

    mesh
}

/// Find the best vertex to use as a fan triangulation center.
/// Returns Some(index) if a valid fan center is found, None if the polygon is too concave.
///
/// For simple convex polygons, any vertex works. But for polygons with curved
/// sections (like quarter disks), we should pick a "corner" vertex where two
/// straight edges meet, not a vertex on the curved portion.
///
/// CRITICAL: For concave polygons, we must verify that fan triangulation from
/// the chosen vertex produces correctly-wound triangles. If a vertex is in a
/// concave region, its fan triangles may flip.
fn find_best_fan_center(verts: &[Point3]) -> Option<usize> {
    let n = verts.len();
    if n <= 4 {
        return Some(0); // Simple polygons are fine with vertex 0
    }

    // Compute polygon winding (signed area) to know expected triangle orientation
    let polygon_signed_area: f64 = (0..n)
        .map(|i| {
            let j = (i + 1) % n;
            verts[i].x * verts[j].y - verts[j].x * verts[i].y
        })
        .sum();

    // Helper: check if a fan center produces valid triangles
    // A valid fan center is one where ALL fan triangles have the same winding as the polygon
    let is_valid_fan_center = |center_idx: usize| -> bool {
        let center = &verts[center_idx];
        for i in 1..(n - 1) {
            let v1_idx = (center_idx + i) % n;
            let v2_idx = (center_idx + i + 1) % n;
            let v1 = &verts[v1_idx];
            let v2 = &verts[v2_idx];
            // Compute signed area of triangle (center, v1, v2)
            let tri_area =
                (v1.x - center.x) * (v2.y - center.y) - (v2.x - center.x) * (v1.y - center.y);
            // Triangle should have same sign as polygon (both positive or both negative)
            // Use a small tolerance to avoid issues with degenerate triangles
            if tri_area.abs() > 1e-10 && (tri_area > 0.0) != (polygon_signed_area > 0.0) {
                return false; // This fan center produces a flipped triangle
            }
        }
        true
    };

    // First, find candidates with good geometry (sharp angles, long edges)
    let mut candidates: Vec<(usize, f64)> = Vec::new();

    for i in 0..n {
        let prev = &verts[(i + n - 1) % n];
        let curr = &verts[i];
        let next = &verts[(i + 1) % n];

        // Vectors from current to neighbors
        let to_prev = *prev - *curr;
        let to_next = *next - *curr;

        let len_prev = to_prev.norm();
        let len_next = to_next.norm();

        if len_prev < 1e-10 || len_next < 1e-10 {
            continue;
        }

        // Compute angle using dot product
        let cos_angle = to_prev.dot(to_next) / (len_prev * len_next);
        let angle = cos_angle.clamp(-1.0, 1.0).acos();

        // Also consider edge lengths - prefer vertices adjacent to longer edges
        // (curved portions tend to have many short edges)
        let edge_factor = 1.0 / (len_prev + len_next + 0.001);

        // Score: lower is better. Prefer sharp angles with longer adjacent edges.
        // Sharp angle = small angle value, so we want to minimize (angle * edge_factor).
        let score = angle * edge_factor;

        candidates.push((i, score));
    }

    // Sort candidates by score (lower is better)
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Find the first candidate that produces valid triangles
    if let Some(&(idx, _)) = candidates.iter().find(|(idx, _)| is_valid_fan_center(*idx)) {
        return Some(idx);
    }

    // If no candidate is valid, try all vertices as a fallback
    // No valid fan center exists (polygon is too concave for fan triangulation)
    (0..n).find(|&i| is_valid_fan_center(i))
}

/// Tessellate a bilinear surface face using the surface's normal method.
/// This enables smooth shading when corner normals are provided.
fn tessellate_bilinear_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Try to downcast to BilinearSurface
    if let Some(bilinear) = surface.as_any().downcast_ref::<BilinearSurface>() {
        let n_u = params.circle_segments.max(2) as usize;
        let n_v = params.height_segments.max(2) as usize;

        let mut mesh = TriangleMesh::new();

        // Generate grid of vertices with surface normals
        for j in 0..=n_v {
            let v = j as f64 / n_v as f64;
            for i in 0..=n_u {
                let u = i as f64 / n_u as f64;
                let uv = Point2::new(u, v);
                let pt: Point3 = bilinear.evaluate(uv);
                let normal = bilinear.normal(uv);

                mesh.vertices.push(pt.x as f32);
                mesh.vertices.push(pt.y as f32);
                mesh.vertices.push(pt.z as f32);

                let (nx, ny, nz) = if reversed {
                    (-normal.x as f32, -normal.y as f32, -normal.z as f32)
                } else {
                    (normal.x as f32, normal.y as f32, normal.z as f32)
                };
                mesh.normals.push(nx);
                mesh.normals.push(ny);
                mesh.normals.push(nz);
            }
        }

        // Generate triangles
        let stride = (n_u + 1) as u32;
        for j in 0..n_v {
            for i in 0..n_u {
                let bl = j as u32 * stride + i as u32;
                let br = bl + 1;
                let tl = bl + stride;
                let tr = tl + 1;

                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
                }
            }
        }

        mesh
    } else {
        // Fallback to simple quad tessellation
        TriangleMesh::new()
    }
}

/// Tessellate a planar face with inner loops (holes).
/// Uses a ring-based approach for better triangle quality: adds intermediate
/// Steiner points around each hole to prevent long thin triangles.
fn tessellate_planar_face_with_holes(
    topo: &Topology,
    face_id: FaceId,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];

    // Get outer loop vertices
    let mut outer_verts: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    if outer_verts.len() < 3 {
        return TriangleMesh::new();
    }

    // Get all inner loop vertices
    let mut inner_loops: Vec<Vec<Point3>> = Vec::new();
    for &inner_loop in &face.inner_loops {
        let inner_verts: Vec<Point3> = topo
            .loop_half_edges(inner_loop)
            .map(|he| topo.vertices[topo.half_edges[he].origin].point)
            .collect();
        if inner_verts.len() >= 3 {
            inner_loops.push(inner_verts);
        }
    }

    if inner_loops.is_empty() {
        // No valid inner loops, fall back to simple triangulation
        return tessellate_simple_polygon(&outer_verts, reversed);
    }

    // Build a 2D projection for triangulation
    // Compute the face plane from first 3 vertices
    let e1 = outer_verts[1] - outer_verts[0];
    let e2 = outer_verts[2] - outer_verts[0];
    let face_normal = e1.cross(e2);
    if face_normal.norm() < 1e-12 {
        return TriangleMesh::new();
    }

    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(e1).normalize();
    let origin = outer_verts[0];

    // Project 3D points to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(u_axis), d.dot(v_axis))
    };

    // Project outer loop
    let mut outer_2d: Vec<(f64, f64)> = outer_verts.iter().map(&project).collect();

    // Project inner loops
    let mut inner_2d: Vec<Vec<(f64, f64)>> = inner_loops
        .iter()
        .map(|loop_verts| loop_verts.iter().map(&project).collect())
        .collect();

    // Normalize winding: outer loop must be CCW (positive area),
    // inner loops must be CW (negative area). STEP files may have
    // inconsistent winding depending on the exporter.
    let outer_area = polygon_area_2d(&outer_2d);
    if outer_area < 0.0 {
        outer_verts.reverse();
        outer_2d.reverse();
    }
    for (i, hole_2d) in inner_2d.iter_mut().enumerate() {
        let hole_area = polygon_area_2d(hole_2d);
        if hole_area > 0.0 {
            inner_loops[i].reverse();
            hole_2d.reverse();
        }
    }

    // Merge overlapping inner loops (e.g., semicircular arcs from STEP
    // that together form a full circle at the same position).
    merge_overlapping_holes(&mut inner_2d, &mut inner_loops);

    // After merging overlapping arcs, use bridge+ear-clip directly.
    // The merged holes are well-shaped (no more overlapping semicircles),
    // so bridge construction works reliably.
    triangulate_polygon_with_holes(&outer_2d, &inner_2d, &outer_verts, &inner_loops, reversed)
}

/// Merge inner loops that overlap (e.g., two semicircular arcs forming a full circle).
/// Loops are merged when their centroids are closer than the sum of their average radii.
fn merge_overlapping_holes(inner_2d: &mut Vec<Vec<(f64, f64)>>, inner_3d: &mut Vec<Vec<Point3>>) {
    loop {
        let mut merge_pair = None;
        'search: for i in 0..inner_2d.len() {
            let ci = centroid_2d(&inner_2d[i]);
            let ri = avg_radius_2d(&inner_2d[i], ci);
            #[allow(clippy::needless_range_loop)]
            for j in (i + 1)..inner_2d.len() {
                let cj = centroid_2d(&inner_2d[j]);
                let rj = avg_radius_2d(&inner_2d[j], cj);
                let dist = ((ci.0 - cj.0).powi(2) + (ci.1 - cj.1).powi(2)).sqrt();
                // Only merge loops whose centroids are very close — i.e., they
                // represent split arcs (semicircles) of the same hole.
                if dist < ri.min(rj) * 0.5 {
                    merge_pair = Some((i, j));
                    break 'search;
                }
            }
        }

        let Some((i, j)) = merge_pair else { break };

        // Merge loop j into loop i by combining vertices and sorting by angle
        let mut combined_2d = std::mem::take(&mut inner_2d[i]);
        combined_2d.extend_from_slice(&inner_2d[j]);
        let mut combined_3d = std::mem::take(&mut inner_3d[i]);
        combined_3d.extend_from_slice(&inner_3d[j]);

        // Compute combined centroid
        let c = centroid_2d(&combined_2d);

        // Sort by angle from centroid, removing near-duplicate angles
        let mut indexed: Vec<(f64, (f64, f64), Point3)> = combined_2d
            .iter()
            .zip(combined_3d.iter())
            .map(|(&p2, &p3)| {
                let angle = (p2.1 - c.1).atan2(p2.0 - c.0);
                (angle, p2, p3)
            })
            .collect();
        indexed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Remove near-duplicate angles (from shared seam vertices)
        let mut deduped_2d = Vec::new();
        let mut deduped_3d = Vec::new();
        for &(angle, p2, p3) in &indexed {
            if deduped_2d.is_empty() {
                deduped_2d.push(p2);
                deduped_3d.push(p3);
            } else {
                let last = *deduped_2d.last().unwrap();
                let dist = ((p2.0 - last.0).powi(2) + (p2.1 - last.1).powi(2)).sqrt();
                if dist > 0.01 {
                    deduped_2d.push(p2);
                    deduped_3d.push(p3);
                }
            }
            let _ = angle;
        }
        // Also check wrap-around duplicate
        if deduped_2d.len() > 1 {
            let first = deduped_2d[0];
            let last = *deduped_2d.last().unwrap();
            if ((first.0 - last.0).powi(2) + (first.1 - last.1).powi(2)).sqrt() < 0.01 {
                deduped_2d.pop();
                deduped_3d.pop();
            }
        }

        // Ensure merged loop has CW winding (negative area = hole)
        if polygon_area_2d(&deduped_2d) > 0.0 {
            deduped_2d.reverse();
            deduped_3d.reverse();
        }

        inner_2d[i] = deduped_2d;
        inner_3d[i] = deduped_3d;

        // Remove the merged loop
        inner_2d.remove(j);
        inner_3d.remove(j);
    }
}

fn centroid_2d(pts: &[(f64, f64)]) -> (f64, f64) {
    let n = pts.len() as f64;
    pts.iter()
        .fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n))
}

fn avg_radius_2d(pts: &[(f64, f64)], c: (f64, f64)) -> f64 {
    let n = pts.len() as f64;
    pts.iter()
        .map(|p| ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt())
        .sum::<f64>()
        / n
}

/// Compute signed area of a 2D polygon.
fn polygon_area_2d(pts: &[(f64, f64)]) -> f64 {
    let mut area = 0.0;
    let n = pts.len();
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i].0 * pts[j].1 - pts[j].0 * pts[i].1;
    }
    area / 2.0
}

/// Add Steiner points to the outer polygon to improve triangulation quality.
///
/// This function:
/// 1. Subdivides long outer edges into smaller segments (max ~20 units)
/// 2. Adds additional points near each hole centroid
///
/// This prevents very long bridges that cause thin, degenerate triangles.
fn refine_outer_polygon_for_holes(
    outer_2d: &[(f64, f64)],
    outer_3d: &[Point3],
    inner_2d: &[Vec<(f64, f64)>],
) -> (Vec<(f64, f64)>, Vec<Point3>) {
    if outer_2d.len() < 3 {
        return (outer_2d.to_vec(), outer_3d.to_vec());
    }

    // Maximum edge length before subdivision.
    // Using a small value to ensure good quality triangles near holes.
    const MAX_EDGE_LENGTH: f64 = 8.0;

    // First pass: subdivide long edges
    let mut result_2d: Vec<(f64, f64)> = Vec::new();
    let mut result_3d: Vec<Point3> = Vec::new();

    for i in 0..outer_2d.len() {
        let j = (i + 1) % outer_2d.len();
        let a_2d = outer_2d[i];
        let b_2d = outer_2d[j];
        let a_3d = outer_3d[i];
        let b_3d = outer_3d[j];

        // Add the start vertex
        result_2d.push(a_2d);
        result_3d.push(a_3d);

        // Calculate edge length
        let edge_len = ((b_2d.0 - a_2d.0).powi(2) + (b_2d.1 - a_2d.1).powi(2)).sqrt();

        // If edge is long, subdivide it
        if edge_len > MAX_EDGE_LENGTH {
            let num_segments = (edge_len / MAX_EDGE_LENGTH).ceil() as usize;
            for k in 1..num_segments {
                let t = k as f64 / num_segments as f64;
                let new_2d = (
                    a_2d.0 + t * (b_2d.0 - a_2d.0),
                    a_2d.1 + t * (b_2d.1 - a_2d.1),
                );
                let new_3d = Point3::new(
                    a_3d.x + t * (b_3d.x - a_3d.x),
                    a_3d.y + t * (b_3d.y - a_3d.y),
                    a_3d.z + t * (b_3d.z - a_3d.z),
                );
                result_2d.push(new_2d);
                result_3d.push(new_3d);
            }
        }
    }

    // If no holes, we're done with just edge subdivision
    if inner_2d.is_empty() {
        return (result_2d, result_3d);
    }

    // Second pass: add points near each hole centroid
    // Collect insertion points: (edge_index, t_param, 2d_point, 3d_point)
    let mut insertions: Vec<(usize, f64, (f64, f64), Point3)> = Vec::new();

    for hole in inner_2d {
        if hole.is_empty() {
            continue;
        }

        // Find centroid of hole
        let centroid: (f64, f64) = hole
            .iter()
            .fold((0.0, 0.0), |acc, p| (acc.0 + p.0, acc.1 + p.1));
        let n = hole.len() as f64;
        let centroid = (centroid.0 / n, centroid.1 / n);

        // Find closest point on outer polygon edges to the hole centroid
        let mut best_edge = 0;
        let mut best_t = 0.5;
        let mut best_dist = f64::INFINITY;

        for i in 0..result_2d.len() {
            let j = (i + 1) % result_2d.len();
            let a = result_2d[i];
            let b = result_2d[j];

            // Project centroid onto edge a-b
            let ab = (b.0 - a.0, b.1 - a.1);
            let len2 = ab.0 * ab.0 + ab.1 * ab.1;
            if len2 < 1e-12 {
                continue;
            }

            let ap = (centroid.0 - a.0, centroid.1 - a.1);
            let t = (ap.0 * ab.0 + ap.1 * ab.1) / len2;

            // Only consider points on the edge interior (not at endpoints)
            if t <= 0.1 || t >= 0.9 {
                continue;
            }

            let proj = (a.0 + t * ab.0, a.1 + t * ab.1);
            let dist = ((centroid.0 - proj.0).powi(2) + (centroid.1 - proj.1).powi(2)).sqrt();

            if dist < best_dist {
                best_dist = dist;
                best_edge = i;
                best_t = t;
            }
        }

        // Check if the best point is significantly closer than existing vertices
        let mut min_vertex_dist = f64::INFINITY;
        for &v in &result_2d {
            let d = ((centroid.0 - v.0).powi(2) + (centroid.1 - v.1).powi(2)).sqrt();
            min_vertex_dist = min_vertex_dist.min(d);
        }

        // Only add if the edge point is at least 30% closer than any existing vertex
        if best_dist < min_vertex_dist * 0.7 && best_dist < f64::INFINITY {
            let j = (best_edge + 1) % result_2d.len();
            let a_2d = result_2d[best_edge];
            let b_2d = result_2d[j];
            let new_2d = (
                a_2d.0 + best_t * (b_2d.0 - a_2d.0),
                a_2d.1 + best_t * (b_2d.1 - a_2d.1),
            );

            let a_3d = result_3d[best_edge];
            let b_3d = result_3d[j];
            let new_3d = Point3::new(
                a_3d.x + best_t * (b_3d.x - a_3d.x),
                a_3d.y + best_t * (b_3d.y - a_3d.y),
                a_3d.z + best_t * (b_3d.z - a_3d.z),
            );

            insertions.push((best_edge, best_t, new_2d, new_3d));
        }
    }

    if insertions.is_empty() {
        return (result_2d, result_3d);
    }

    // Sort insertions by edge index (descending) then by t (descending within same edge)
    insertions.sort_by(|a, b| {
        if a.0 != b.0 {
            b.0.cmp(&a.0)
        } else {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    for (edge_idx, _, pt_2d, pt_3d) in insertions {
        result_2d.insert(edge_idx + 1, pt_2d);
        result_3d.insert(edge_idx + 1, pt_3d);
    }

    (result_2d, result_3d)
}

/// Triangulate a polygon with holes using ear-clipping with bridge construction.
fn triangulate_polygon_with_holes(
    outer_2d: &[(f64, f64)],
    inner_2d: &[Vec<(f64, f64)>],
    outer_3d: &[Point3],
    inner_3d: &[Vec<Point3>],
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // First, refine the outer polygon by adding Steiner points near each hole.
    // This prevents very long bridges that cause thin triangles.
    let (refined_outer_2d, refined_outer_3d) =
        refine_outer_polygon_for_holes(outer_2d, outer_3d, inner_2d);

    // Collect all vertices
    let mut all_verts_3d: Vec<Point3> = refined_outer_3d.clone();
    let mut all_verts_2d: Vec<(f64, f64)> = refined_outer_2d.clone();

    // Track where each inner loop starts
    let mut inner_starts: Vec<usize> = Vec::new();
    for (inner_loop_3d, inner_loop_2d) in inner_3d.iter().zip(inner_2d.iter()) {
        inner_starts.push(all_verts_3d.len());
        all_verts_3d.extend_from_slice(inner_loop_3d);
        all_verts_2d.extend_from_slice(inner_loop_2d);
    }

    // Add all vertices to mesh
    for v in &all_verts_3d {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    // Build a merged polygon by bridging outer to each inner loop.
    // Sort holes by the angle of their centroid relative to the outer polygon
    // centroid. This prevents bridges from crossing when holes are spread
    // around a circle (e.g., bolt patterns on cylinder caps).
    let outer_centroid = {
        let n = refined_outer_2d.len() as f64;
        let (sx, sy) = refined_outer_2d
            .iter()
            .fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
        (sx / n, sy / n)
    };
    let mut hole_order: Vec<usize> = (0..inner_starts.len()).collect();
    hole_order.sort_by(|&a, &b| {
        let ca = {
            let n = inner_2d[a].len() as f64;
            let (sx, sy) = inner_2d[a]
                .iter()
                .fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
            (sx / n, sy / n)
        };
        let cb = {
            let n = inner_2d[b].len() as f64;
            let (sx, sy) = inner_2d[b]
                .iter()
                .fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
            (sx / n, sy / n)
        };
        let angle_a = (ca.1 - outer_centroid.1).atan2(ca.0 - outer_centroid.0);
        let angle_b = (cb.1 - outer_centroid.1).atan2(cb.0 - outer_centroid.0);
        angle_a
            .partial_cmp(&angle_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut poly_indices: Vec<usize> = (0..refined_outer_2d.len()).collect();

    // Track which vertices have been used as bridge endpoints
    let mut used_bridge_vertices: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for &hole_idx in &hole_order {
        let inner_start = inner_starts[hole_idx];
        let inner_len = inner_2d[hole_idx].len();

        // Find the pair of (outer vertex, inner vertex) with minimum distance
        // Avoid vertices already used as bridge endpoints
        let mut candidates: Vec<(f64, usize, usize)> = Vec::new(); // (dist, inner_idx, outer_poly_idx)

        for i in 0..inner_len {
            let inner_pt = all_verts_2d[inner_start + i];
            for (j, &outer_idx) in poly_indices.iter().enumerate() {
                let outer_pt = all_verts_2d[outer_idx];
                let dist = (outer_pt.0 - inner_pt.0).powi(2) + (outer_pt.1 - inner_pt.1).powi(2);
                candidates.push((dist, i, j));
            }
        }

        // Sort by distance
        candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Find the best candidate that doesn't reuse a bridge vertex
        let mut best_inner = 0;
        let mut best_outer_idx = 0;

        for (_, inner_idx, outer_poly_idx) in &candidates {
            let outer_vertex_idx = poly_indices[*outer_poly_idx];
            // For the first hole, allow any vertex. For subsequent holes,
            // prefer vertices that haven't been used, but fall back if needed.
            if !used_bridge_vertices.contains(&outer_vertex_idx) || hole_idx == 0 {
                best_inner = *inner_idx;
                best_outer_idx = *outer_poly_idx;
                used_bridge_vertices.insert(outer_vertex_idx);
                break;
            }
        }

        // If all vertices are used (shouldn't happen with reasonable input), use closest anyway
        if candidates.is_empty() {
            continue;
        }

        let rightmost_inner = best_inner;

        // Insert bridge: outer -> hole -> back to outer
        let inner_global_start = inner_start;
        let hole_indices: Vec<usize> = (0..inner_len)
            .map(|i| inner_global_start + ((rightmost_inner + i) % inner_len))
            .collect();

        // Insert after best_outer_idx:
        // poly[0..=best_outer_idx] + hole_indices + [hole_indices[0], poly[best_outer_idx]] + poly[best_outer_idx+1..]
        // Simplified: insert hole loop with bridge vertices
        let bridge_outer = poly_indices[best_outer_idx];
        let bridge_inner = hole_indices[0];

        let mut new_poly = Vec::new();
        new_poly.extend_from_slice(&poly_indices[..=best_outer_idx]);
        new_poly.extend_from_slice(&hole_indices);
        new_poly.push(bridge_inner);
        new_poly.push(bridge_outer);
        new_poly.extend_from_slice(&poly_indices[best_outer_idx + 1..]);

        poly_indices = new_poly;
    }

    // Now triangulate the merged polygon using ear clipping
    ear_clip_triangulate(&all_verts_2d, &poly_indices, &mut mesh.indices, reversed);

    mesh
}

/// Simple ear-clipping triangulation for a polygon (defined by indices into a vertex array).
fn ear_clip_triangulate(
    verts_2d: &[(f64, f64)],
    indices: &[usize],
    out_indices: &mut Vec<u32>,
    reversed: bool,
) {
    if indices.len() < 3 {
        return;
    }

    let mut remaining: Vec<usize> = indices.to_vec();

    while remaining.len() > 3 {
        let n = remaining.len();
        let mut found_ear = false;

        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = (i + 1) % n;

            let a = verts_2d[remaining[prev]];
            let b = verts_2d[remaining[i]];
            let c = verts_2d[remaining[next]];

            // Check if this is a convex vertex (ear candidate)
            let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            let is_convex = if reversed { cross < 0.0 } else { cross > 0.0 };

            if !is_convex {
                continue;
            }

            // Check if any other vertex is inside this triangle
            let mut is_ear = true;
            for j in 0..n {
                if j == prev || j == i || j == next {
                    continue;
                }
                let p = verts_2d[remaining[j]];
                if point_in_triangle_2d(p, a, b, c) {
                    is_ear = false;
                    break;
                }
            }

            // Check that the ear diagonal (a→c) doesn't cross any polygon edge
            if is_ear {
                for j in 0..n {
                    let j_next = (j + 1) % n;
                    if j == prev || j_next == prev || j == next || j_next == next {
                        continue;
                    }
                    let p1 = verts_2d[remaining[j]];
                    let p2 = verts_2d[remaining[j_next]];
                    if segments_intersect(a, c, p1, p2) {
                        is_ear = false;
                        break;
                    }
                }
            }

            if is_ear {
                if reversed {
                    out_indices.push(remaining[prev] as u32);
                    out_indices.push(remaining[next] as u32);
                    out_indices.push(remaining[i] as u32);
                } else {
                    out_indices.push(remaining[prev] as u32);
                    out_indices.push(remaining[i] as u32);
                    out_indices.push(remaining[next] as u32);
                }
                remaining.remove(i);
                found_ear = true;
                break;
            }
        }

        if !found_ear {
            break;
        }
    }

    // Final triangle
    if remaining.len() == 3 {
        if reversed {
            out_indices.push(remaining[0] as u32);
            out_indices.push(remaining[2] as u32);
            out_indices.push(remaining[1] as u32);
        } else {
            out_indices.push(remaining[0] as u32);
            out_indices.push(remaining[1] as u32);
            out_indices.push(remaining[2] as u32);
        }
    }
}

/// Check if a point is inside a triangle in 2D using barycentric coordinates.
fn point_in_triangle_2d(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let v0 = (c.0 - a.0, c.1 - a.1);
    let v1 = (b.0 - a.0, b.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);

    let dot00 = v0.0 * v0.0 + v0.1 * v0.1;
    let dot01 = v0.0 * v1.0 + v0.1 * v1.1;
    let dot02 = v0.0 * v2.0 + v0.1 * v2.1;
    let dot11 = v1.0 * v1.0 + v1.1 * v1.1;
    let dot12 = v1.0 * v2.0 + v1.1 * v2.1;

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    // Use small epsilon to avoid boundary issues
    let eps = 1e-10;
    u > eps && v > eps && (u + v) < 1.0 - eps
}

/// Check if two line segments (a1→a2) and (b1→b2) properly intersect.
/// Returns true only for proper crossings (not shared endpoints or collinear overlap).
fn segments_intersect(a1: (f64, f64), a2: (f64, f64), b1: (f64, f64), b2: (f64, f64)) -> bool {
    let d1 = (a2.0 - a1.0, a2.1 - a1.1);
    let d2 = (b2.0 - b1.0, b2.1 - b1.1);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-12 {
        return false; // Parallel or collinear
    }
    let d = (b1.0 - a1.0, b1.1 - a1.1);
    let t = (d.0 * d2.1 - d.1 * d2.0) / denom;
    let u = (d.0 * d1.1 - d.1 * d1.0) / denom;
    // Proper intersection: both parameters strictly in (0, 1)
    let eps = 1e-8;
    t > eps && t < 1.0 - eps && u > eps && u < 1.0 - eps
}

/// Point-in-polygon test using ray casting (winding number).
fn point_in_polygon_2d(px: f64, py: f64, polygon: &[(f64, f64)]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut crossings = 0;
    for i in 0..n {
        let j = (i + 1) % n;
        let (ax, ay) = polygon[i];
        let (bx, by) = polygon[j];
        // Check if the horizontal ray from (px, py) going right crosses edge (a, b)
        if (ay <= py && by > py) || (by <= py && ay > py) {
            let t = (py - ay) / (by - ay);
            let x_intersect = ax + t * (bx - ax);
            if px < x_intersect {
                crossings += 1;
            }
        }
    }
    crossings % 2 == 1
}

/// Triangulate a planar polygon with holes using Delaunay triangulation
/// Bowyer-Watson incremental Delaunay triangulation.
/// Returns triangle indices into the input point array.
fn bowyer_watson_2d(points: &[(f64, f64)]) -> Vec<(usize, usize, usize)> {
    if points.len() < 3 {
        return Vec::new();
    }

    // Compute bounding box
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let d = dx.max(dy).max(1e-6);
    let cx = (min_x + max_x) / 2.0;
    let cy = (min_y + max_y) / 2.0;

    // Super-triangle vertices (indices: n, n+1, n+2)
    let n = points.len();
    let margin = 10.0 * d;
    let super_verts = [
        (cx - margin, cy - margin),
        (cx + margin, cy - margin),
        (cx, cy + margin),
    ];

    // All vertices: original points + super-triangle
    let mut all_pts: Vec<(f64, f64)> = points.to_vec();
    all_pts.extend_from_slice(&super_verts);

    // Start with the super-triangle
    let mut tris: Vec<(usize, usize, usize)> = vec![(n, n + 1, n + 2)];

    // Insert each point
    for pi in 0..n {
        let (px, py) = all_pts[pi];

        // Find triangles whose circumcircle contains this point
        let mut bad_tris: Vec<usize> = Vec::new();
        for (ti, &(a, b, c)) in tris.iter().enumerate() {
            if in_circumcircle(px, py, all_pts[a], all_pts[b], all_pts[c]) {
                bad_tris.push(ti);
            }
        }

        // Find boundary edges of the hole (edges not shared between bad triangles)
        let mut edge_count: std::collections::HashMap<(usize, usize), usize> =
            std::collections::HashMap::new();
        for &ti in &bad_tris {
            let (a, b, c) = tris[ti];
            for &(e1, e2) in &[(a, b), (b, c), (c, a)] {
                let key = if e1 < e2 { (e1, e2) } else { (e2, e1) };
                *edge_count.entry(key).or_insert(0) += 1;
            }
        }
        let boundary_edges: Vec<(usize, usize)> = edge_count
            .into_iter()
            .filter(|&(_, count)| count == 1)
            .map(|(edge, _)| edge)
            .collect();

        // Remove bad triangles (in reverse order to preserve indices)
        bad_tris.sort_unstable();
        for &ti in bad_tris.iter().rev() {
            tris.swap_remove(ti);
        }

        // Create new triangles connecting the point to boundary edges
        for &(e1, e2) in &boundary_edges {
            tris.push((pi, e1, e2));
        }
    }

    // Remove triangles that reference super-triangle vertices
    tris.retain(|&(a, b, c)| a < n && b < n && c < n);

    // Ensure consistent winding (CCW)
    tris.iter_mut().for_each(|(a, b, c)| {
        let (ax, ay) = all_pts[*a];
        let (bx, by) = all_pts[*b];
        let (cx, cy) = all_pts[*c];
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if cross < 0.0 {
            std::mem::swap(b, c);
        }
    });

    tris
}

/// Check if point (px, py) is inside the circumcircle of triangle (a, b, c).
fn in_circumcircle(px: f64, py: f64, a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    // Using the determinant method
    let ax = a.0 - px;
    let ay = a.1 - py;
    let bx = b.0 - px;
    let by = b.1 - py;
    let cx = c.0 - px;
    let cy = c.1 - py;

    let det = ax * (by * (cx * cx + cy * cy) - cy * (bx * bx + by * by))
        - bx * (ay * (cx * cx + cy * cy) - cy * (ax * ax + ay * ay))
        + cx * (ay * (bx * bx + by * by) - by * (ax * ax + ay * ay));

    // For CCW triangle, det > 0 means inside circumcircle
    // Ensure the triangle is CCW first
    let tri_cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if tri_cross > 0.0 {
        det > 0.0
    } else {
        det < 0.0
    }
}

/// Specialized triangulation for circular caps with holes.
/// Uses concentric ring Steiner points + Delaunay + post-filtering.
/// This avoids the co-circular degeneracy of pure boundary-point Delaunay
/// and the large-triangle problem of bridge+ear-clip.
#[allow(clippy::too_many_arguments)]
fn triangulate_circular_cap_with_holes(
    center: Point3,
    radius: f64,
    x_dir: Vec3,
    y_dir: Vec3,
    outer_2d: &[(f64, f64)],
    inner_2d: &[Vec<(f64, f64)>],
    outer_3d: &[Point3],
    inner_3d: &[Vec<Point3>],
    reversed: bool,
) -> TriangleMesh {
    let mut all_2d: Vec<(f64, f64)> = outer_2d.to_vec();
    let mut all_3d: Vec<Point3> = outer_3d.to_vec();

    // Add inner loop points
    for (loop_2d, loop_3d) in inner_2d.iter().zip(inner_3d.iter()) {
        all_2d.extend_from_slice(loop_2d);
        all_3d.extend_from_slice(loop_3d);
    }

    // Add concentric ring Steiner points to break up large triangles.
    // These interior points give Delaunay proper non-co-circular vertices.
    let num_rings = 3;
    let pts_per_ring = 16;
    for ring in 1..=num_rings {
        let r_frac = ring as f64 / (num_rings + 1) as f64;
        let ring_r = radius * r_frac;
        for j in 0..pts_per_ring {
            let theta = 2.0 * PI * (j as f64 + 0.5 * ring as f64) / pts_per_ring as f64;
            let (sin_t, cos_t) = theta.sin_cos();
            let pt_2d = (ring_r * cos_t, ring_r * sin_t);

            // Skip if inside any hole
            let mut in_hole = false;
            for hole in inner_2d {
                if point_in_polygon_2d(pt_2d.0, pt_2d.1, hole) {
                    in_hole = true;
                    break;
                }
            }
            // Also skip if too close to any hole boundary (to avoid near-degenerate tris)
            if !in_hole {
                for hole in inner_2d {
                    for &(hx, hy) in hole {
                        let dist = ((pt_2d.0 - hx).powi(2) + (pt_2d.1 - hy).powi(2)).sqrt();
                        if dist < radius * 0.02 {
                            in_hole = true;
                            break;
                        }
                    }
                    if in_hole {
                        break;
                    }
                }
            }

            if !in_hole {
                all_2d.push(pt_2d);
                let pt_3d = center + pt_2d.0 * x_dir + pt_2d.1 * y_dir;
                all_3d.push(pt_3d);
            }
        }
    }

    // Also add center point
    let center_2d = (0.0, 0.0);
    let mut center_in_hole = false;
    for hole in inner_2d {
        if point_in_polygon_2d(0.0, 0.0, hole) {
            center_in_hole = true;
            break;
        }
    }
    if !center_in_hole {
        all_2d.push(center_2d);
        all_3d.push(center);
    }

    // Perturb points slightly for Delaunay robustness
    let perturbed: Vec<(f64, f64)> = all_2d
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let eps = 1e-8;
            let angle = i as f64 * 2.654_435_769;
            (x + eps * angle.sin(), y + eps * angle.cos())
        })
        .collect();

    let triangles = bowyer_watson_2d(&perturbed);

    // Build mesh
    let mut mesh = TriangleMesh::new();
    for v in &all_3d {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    // Collect constrained edges (hole boundaries)
    let mut constrained_edges: Vec<((f64, f64), (f64, f64))> = Vec::new();
    for hole in inner_2d {
        for i in 0..hole.len() {
            let j = (i + 1) % hole.len();
            constrained_edges.push((hole[i], hole[j]));
        }
    }

    // Filter triangles
    for &(i, j, k) in &triangles {
        let (a, b, c) = (all_2d[i], all_2d[j], all_2d[k]);
        let cx = (a.0 + b.0 + c.0) / 3.0;
        let cy = (a.1 + b.1 + c.1) / 3.0;

        // Must be inside outer polygon
        if !point_in_polygon_2d(cx, cy, outer_2d) {
            continue;
        }

        // Must not be inside any hole
        let mut in_hole = false;
        for hole in inner_2d {
            if point_in_polygon_2d(cx, cy, hole) {
                in_hole = true;
                break;
            }
        }
        if in_hole {
            continue;
        }

        // Check no edge crosses a hole boundary
        let tri_edges = [(a, b), (b, c), (c, a)];
        let mut crosses = false;
        'check: for &(e1, e2) in &tri_edges {
            for &(h1, h2) in &constrained_edges {
                if segments_intersect(e1, e2, h1, h2) {
                    crosses = true;
                    break 'check;
                }
            }
        }
        if crosses {
            continue;
        }

        if reversed {
            mesh.indices.push(i as u32);
            mesh.indices.push(k as u32);
            mesh.indices.push(j as u32);
        } else {
            mesh.indices.push(i as u32);
            mesh.indices.push(j as u32);
            mesh.indices.push(k as u32);
        }
    }

    mesh
}

/// Simple fan triangulation for a convex polygon.
fn tessellate_simple_polygon(verts: &[Point3], reversed: bool) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    for v in verts {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    for i in 1..(verts.len() - 1) {
        if reversed {
            mesh.indices.push(0);
            mesh.indices.push((i + 1) as u32);
            mesh.indices.push(i as u32);
        } else {
            mesh.indices.push(0);
            mesh.indices.push(i as u32);
            mesh.indices.push((i + 1) as u32);
        }
    }

    mesh
}

/// Tessellate a cylindrical face (lateral surface of a cylinder).
fn tessellate_cylindrical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    // `n_circ` is the lower bound; once we extract the cylinder's actual
    // radius below, `circle_segments_for_radius` may raise it so that the
    // chord/angular tolerance is respected.
    let mut n_circ = params.circle_segments.max(3) as usize;
    let n_height = params.height_segments.max(1) as usize;

    // Determine the v (height) parameter range by projecting seam vertices
    // onto the cylinder axis. This works correctly after any transform.
    let verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    let mut radius = None;
    let mut u_min = 0.0;
    let mut u_max = 2.0 * PI;
    let mut unique_angles: Vec<f64> = Vec::new();
    let (v_min, v_max) = if let Some(cyl) = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        let r = cyl.radius.abs().max(1e-6);
        radius = Some(r);
        n_circ = (params.circle_segments_for_radius(r) as usize).max(3);
        // Project vertices onto axis to get v parameter and compute U angles
        let mut vmin = f64::MAX;
        let mut vmax = f64::MIN;

        // Compute U (angle) for each vertex to find the angular range
        let ref_dir = cyl.ref_dir.as_ref();
        let y_dir = cyl.axis.as_ref().cross(ref_dir);
        let mut angles: Vec<f64> = Vec::new();

        for pt in &verts {
            let d = *pt - cyl.center;
            let v = d.dot(cyl.axis.as_ref());
            vmin = vmin.min(v);
            vmax = vmax.max(v);

            // Compute angle for this vertex
            let dot_y = d.dot(y_dir);
            let dot_ref = d.dot(ref_dir);
            let u = dot_y.atan2(dot_ref);
            // Normalize to [0, 2π). Use a small epsilon to handle -0.0 and tiny negative values
            // that should be treated as 0. Also snap values very close to 2π back to 0.
            let u_normalized = if u < -1e-12 {
                u + 2.0 * PI
            } else if u < 1e-12 || (u - 2.0 * PI).abs() < 1e-12 {
                0.0 // Snap -0.0, tiny negatives, and ~2π to exactly 0
            } else {
                u
            };

            angles.push(u_normalized);
        }

        // Determine U range from the face vertices
        // For a partial face, we need to find the angular extent
        // Get unique angles (vertices at same angle but different heights)
        for &a in &angles {
            if !unique_angles.iter().any(|&ua| (ua - a).abs() < 0.01) {
                unique_angles.push(a);
            }
        }

        // Sort unique angles and check if they cover the full circle
        unique_angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Detect full cylinder: check if vertices are distributed around
        // the entire circle by looking at the largest angular gap.
        // For a full cylinder with arc-sampled edges, vertices are evenly
        // spaced and no gap exceeds ~40°. For a partial cylinder, the
        // "missing" section creates a gap > 90°.
        let is_full = if unique_angles.len() <= 1 {
            true
        } else {
            let mut max_gap = 0.0f64;
            for i in 0..unique_angles.len() - 1 {
                let gap = unique_angles[i + 1] - unique_angles[i];
                max_gap = max_gap.max(gap);
            }
            // Wrap-around gap from last angle back to first + 2π
            let wrap_gap = 2.0 * PI - (unique_angles[unique_angles.len() - 1] - unique_angles[0]);
            max_gap = max_gap.max(wrap_gap);
            max_gap < PI / 2.0
        };

        if is_full {
            u_min = 0.0;
            u_max = 2.0 * PI;
        } else {
            // Determine angular direction from loop vertex order
            let mut first_dir = 0.0;
            for i in 0..angles.len() {
                let a1 = angles[i];
                let a2 = angles[(i + 1) % angles.len()];
                let mut diff = a2 - a1;
                if diff > PI {
                    diff -= 2.0 * PI;
                } else if diff < -PI {
                    diff += 2.0 * PI;
                }
                if diff.abs() > 0.1 {
                    first_dir = diff;
                    break;
                }
            }

            // The face occupies the complement of the largest angular gap
            // between consecutive unique vertex angles. Considering ALL
            // gaps (not just the wrap-around one) handles faces that
            // straddle the u = 0 seam with the seam point itself among
            // the vertices — e.g. the concave (clockwise-arc) lateral
            // face of an extruded annular sector, whose ref_dir anchors
            // u = 0 at one end while the remaining samples descend from
            // 2π. The previous min/max heuristic collapsed such faces to
            // the last sliver before the seam.
            let n_u = unique_angles.len();
            let wrap_gap = 2.0 * PI - (unique_angles[n_u - 1] - unique_angles[0]);
            let mut gap_idx = n_u - 1; // wrap-around gap → contiguous face
            let mut max_gap = wrap_gap;
            for i in 0..n_u - 1 {
                let gap = unique_angles[i + 1] - unique_angles[i];
                if gap > max_gap {
                    max_gap = gap;
                    gap_idx = i;
                }
            }
            // Preserve the old ambiguity tie-break: with only two unique
            // angles the two gaps can be indistinguishable — fall back to
            // the loop winding direction (clockwise ⇒ crosses the seam).
            if n_u == 2 {
                let direct_gap = unique_angles[1] - unique_angles[0];
                if (wrap_gap - direct_gap).abs() <= 0.1 {
                    gap_idx = if first_dir < 0.0 { 0 } else { n_u - 1 };
                }
            }

            if gap_idx == n_u - 1 {
                // Empty region crosses the seam → face is contiguous.
                u_min = unique_angles[0];
                u_max = unique_angles[n_u - 1];
            } else {
                // Face crosses the seam: it runs from just after the gap
                // around through 2π to just before it.
                u_min = unique_angles[gap_idx + 1];
                u_max = unique_angles[gap_idx] + 2.0 * PI;
            }
        }

        (vmin, vmax)
    } else {
        // Fallback: use z coordinates, full angle
        let z_min = verts.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        let z_max = verts.iter().map(|v| v.z).fold(f64::MIN, f64::max);
        (z_min, z_max)
    };

    let height = v_max - v_min;
    let u_range = u_max - u_min;

    // Compute the u-sample schedule for the grid. For a partial face
    // (e.g. one arc-extrude cylinder per arc of a kidney profile) we
    // anchor samples at every unique_angle on the face's outer loop so
    // the lateral-face tessellation shares its boundary vertices with
    // adjacent cap-face vertices at the exact same 3D points. Between
    // consecutive anchor angles we subdivide to maintain the density
    // the caller asked for (n_circ samples around a full 2π), keeping
    // boolean-cut cylinder meshes from going coarse. Full cylinders
    // keep the original evenly-spaced schedule.
    let mut u_samples: Vec<f64> = Vec::new();
    let is_partial = u_range < 2.0 * PI - 0.01;
    if is_partial && unique_angles.len() >= 2 {
        // Map unique angles into the (possibly wrap-extended) u range.
        let offset = if u_max > 2.0 * PI { 2.0 * PI } else { 0.0 };
        let mut anchors: Vec<f64> = Vec::new();
        for a in &unique_angles {
            let adj = if *a < u_min - 1e-6 { *a + offset } else { *a };
            if adj >= u_min - 1e-6 && adj <= u_max + 1e-6 {
                anchors.push(adj.clamp(u_min, u_max));
            }
        }
        anchors.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        anchors.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if anchors.first().map(|&x| x - u_min > 1e-6).unwrap_or(true) {
            anchors.insert(0, u_min);
        }
        if anchors.last().map(|&x| u_max - x > 1e-6).unwrap_or(true) {
            anchors.push(u_max);
        }

        // Subdivide each anchor-to-anchor segment to maintain the
        // caller's requested density across the full 2π circle — but
        // only when the native anchor spacing is coarser than the
        // target. Arc-extruded cylinder faces carry dense anchors along
        // the bottom/top arcs (one per tessellation segment of the
        // source arc); introducing subdivision samples there just
        // creates new mid-edge vertices that aren't present on the cap
        // boundary, breaking the weld at the cap-lateral seam. Faces
        // with sparse anchors (e.g. a boolean-cut cylinder strip with
        // only two endpoints) still get subdivided so the shell
        // doesn't go visibly coarse.
        let target_density = n_circ as f64 / (2.0 * PI);
        let mean_span = u_range / ((anchors.len() - 1) as f64).max(1.0);
        let anchor_density_ratio = if mean_span > 1e-12 {
            (1.0 / mean_span) / target_density
        } else {
            0.0
        };
        // Even anchors that are almost-but-not-quite at target density
        // are still preferable to subdividing, since the cap tessellation
        // can only match samples at the anchor positions (anything in
        // between creates an unweldable mid-edge vertex). Only subdivide
        // when anchors are clearly sparse (e.g. a 2-vertex boolean-cut
        // strip) rather than merely 5-10% below target.
        let anchors_are_dense = anchor_density_ratio >= 0.5;
        u_samples.push(anchors[0]);
        for w in anchors.windows(2) {
            let span = w[1] - w[0];
            let subdivs = if anchors_are_dense {
                1
            } else {
                ((span * target_density).ceil() as usize).max(1)
            };
            for i in 1..=subdivs {
                u_samples.push(w[0] + span * (i as f64 / subdivs as f64));
            }
        }
    }
    if u_samples.len() < 2 {
        // Evenly-spaced fallback (full cylinder or loop with <2 unique angles).
        let effective_n_circ = if is_partial {
            let fraction = u_range / (2.0 * PI);
            (n_circ as f64 * fraction).ceil().max(2.0) as usize
        } else {
            n_circ
        };
        u_samples = (0..=effective_n_circ)
            .map(|i| u_min + u_range * (i as f64 / effective_n_circ as f64))
            .collect();
    }
    let effective_n_circ = u_samples.len() - 1;

    // n_height stays at the caller's value (default 1). A cylinder is
    // ruled in the v direction — no curvature, no shading benefit from
    // intermediate v samples. The prior formula drove n_height from
    // `2π·radius / n_circ`, which varies per cylinder: adjacent arc-
    // extrude cylinders with different radii ended up with different
    // n_height → different v samples along their shared vertical seam
    // → the weld stage couldn't collapse the duplicate seam vertices,
    // leaving a long strip hole in the middle of the pork-chop. Keeping
    // n_height = 1 gives every cylinder the same {v_min, v_max} pair
    // along the seam, which always welds.
    let _ = radius;

    let mut mesh = TriangleMesh::new();

    // Generate grid of vertices using surface.evaluate. Each row shares
    // its u-schedule with the face's outer loop boundary, so adjacent
    // cap/lateral faces produce coincident seam vertices (welded away by
    // `weld_coincident_vertices` in `tessellate_brep`).
    for j in 0..=n_height {
        let v = v_min + height * (j as f64 / n_height as f64);
        for u_raw in &u_samples {
            // Normalize u to [0, 2π) for surface evaluation
            let u_eval = u_raw.rem_euclid(2.0 * PI);
            let uv = Point2::new(u_eval, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
        }
    }

    // Generate triangles
    let stride = (effective_n_circ + 1) as u32;
    for j in 0..n_height {
        for i in 0..effective_n_circ {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a spherical face.
/// Uses a single vertex at each pole to avoid normal computation artifacts.
/// For split caps (from boolean operations), uses boundary-aware tessellation.
fn tessellate_spherical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Count edges in the face loop to detect split caps
    let loop_verts: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    // A normal closed-sphere B-rep has exactly 4 edges (seam cycle).
    // Faces with 3 or >4 loop vertices are partial spherical caps —
    // either boolean-split caps or fillet vertex-blend patches — and
    // should be tessellated as a bounded cap, not a full sphere.
    if loop_verts.len() == 3 || loop_verts.len() > 4 {
        return tessellate_spherical_cap(surface.as_ref(), &loop_verts, params, reversed);
    }

    let sphere_radius = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::SphereSurface>()
        .map(|s| s.radius.abs().max(1e-6));
    let (n_lon, n_lat) = if let Some(r) = sphere_radius {
        (
            params.circle_segments_for_radius(r) as usize,
            params.latitude_segments_for_radius(r) as usize,
        )
    } else {
        (
            params.circle_segments as usize,
            params.latitude_segments as usize,
        )
    };

    let mut mesh = TriangleMesh::new();

    // Helper to push a normal
    let push_normal = |mesh: &mut TriangleMesh, normal: Vec3, reversed: bool| {
        let (nx, ny, nz) = if reversed {
            (-normal.x as f32, -normal.y as f32, -normal.z as f32)
        } else {
            (normal.x as f32, normal.y as f32, normal.z as f32)
        };
        mesh.normals.extend_from_slice(&[nx, ny, nz]);
    };

    // South pole - single vertex (index 0)
    let south_uv = Point2::new(0.0, -PI / 2.0);
    let south = surface.evaluate(south_uv);
    mesh.vertices.push(south.x as f32);
    mesh.vertices.push(south.y as f32);
    mesh.vertices.push(south.z as f32);
    push_normal(&mut mesh, *surface.normal(south_uv), reversed);

    // Middle latitude bands (j = 1 to n_lat - 1)
    for j in 1..n_lat {
        let v = -PI / 2.0 + PI * (j as f64 / n_lat as f64);
        for i in 0..=n_lon {
            let u = 2.0 * PI * (i as f64 / n_lon as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            push_normal(&mut mesh, *surface.normal(uv), reversed);
        }
    }

    // North pole - single vertex (last index)
    let north_uv = Point2::new(0.0, PI / 2.0);
    let north = surface.evaluate(north_uv);
    mesh.vertices.push(north.x as f32);
    mesh.vertices.push(north.y as f32);
    mesh.vertices.push(north.z as f32);
    push_normal(&mut mesh, *surface.normal(north_uv), reversed);

    let south_idx = 0u32;
    let north_idx = mesh.num_vertices() as u32 - 1;
    let stride = (n_lon + 1) as u32;

    // South pole triangles (fan from south pole to first latitude band)
    let first_band_start = 1u32;
    for i in 0..n_lon {
        let v1 = first_band_start + i as u32;
        let v2 = first_band_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[south_idx, v1, v2]);
        } else {
            mesh.indices.extend_from_slice(&[south_idx, v2, v1]);
        }
    }

    // Middle bands (quads between latitude bands)
    for j in 0..(n_lat - 2) {
        let band_start = 1 + j as u32 * stride;
        let next_band_start = band_start + stride;
        for i in 0..n_lon {
            let bl = band_start + i as u32;
            let br = band_start + (i + 1) as u32;
            let tl = next_band_start + i as u32;
            let tr = next_band_start + (i + 1) as u32;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            }
        }
    }

    // North pole triangles (fan from last latitude band to north pole)
    let last_band_start = 1 + (n_lat - 2) as u32 * stride;
    for i in 0..n_lon {
        let v1 = last_band_start + i as u32;
        let v2 = last_band_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[north_idx, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[north_idx, v1, v2]);
        }
    }

    mesh
}

/// Tessellate a spherical cap defined by a boundary loop.
/// Used for split faces from boolean operations.
fn tessellate_spherical_cap(
    surface: &dyn vcad_kernel_geom::Surface,
    loop_verts: &[Point3],
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    use vcad_kernel_geom::SphereSurface;

    let mesh = TriangleMesh::new();

    if loop_verts.len() < 3 {
        return mesh;
    }

    // Get sphere center and radius
    let (center, radius) = if let Some(sphere) = surface.as_any().downcast_ref::<SphereSurface>() {
        (sphere.center, sphere.radius)
    } else {
        // Fallback: estimate center from boundary
        let centroid: Point3 = loop_verts.iter().fold(Point3::origin(), |acc, p| {
            Point3::new(acc.x + p.x, acc.y + p.y, acc.z + p.z)
        });
        let n = loop_verts.len() as f64;
        let centroid = Point3::new(centroid.x / n, centroid.y / n, centroid.z / n);
        let r = (loop_verts[0] - centroid).norm();
        (centroid, r)
    };

    // Direction to the cap pole, derived from the loop winding's
    // solid-angle integrand. For a closed loop on a sphere, summing
    // (p_i - c) × (p_{i+1} - c) over consecutive vertices yields a
    // vector along the spherical cap's axis pointing toward the bounded
    // cap's pole. This is robust to the two split caps sharing an
    // identical boundary centroid (the previous `boundary_centroid -
    // center` formulation collapsed both caps to the same direction).
    let mut sa_normal = Vec3::zeros();
    for i in 0..loop_verts.len() {
        let j = (i + 1) % loop_verts.len();
        let a = loop_verts[i] - center;
        let b = loop_verts[j] - center;
        sa_normal += a.cross(b);
    }
    let sa_len = sa_normal.norm();
    let cap_dir = if sa_len > 1e-12 {
        sa_normal / sa_len
    } else {
        // Degenerate: fall back to the boundary centroid direction.
        let boundary_centroid: Point3 = loop_verts.iter().fold(Point3::origin(), |acc, p| {
            Point3::new(acc.x + p.x, acc.y + p.y, acc.z + p.z)
        });
        let n = loop_verts.len() as f64;
        let boundary_centroid = Point3::new(
            boundary_centroid.x / n,
            boundary_centroid.y / n,
            boundary_centroid.z / n,
        );
        (boundary_centroid - center).normalize()
    };

    // Compute angle from cap direction to each boundary vertex
    let boundary_angles: Vec<f64> = loop_verts
        .iter()
        .map(|p| {
            let v = (*p - center).normalize();
            v.dot(cap_dir).clamp(-1.0, 1.0).acos()
        })
        .collect();

    let min_angle = boundary_angles
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    let avg_angle = boundary_angles.iter().sum::<f64>() / boundary_angles.len() as f64;

    // Use the small-cap path whenever the boundary stays clear of the
    // antipode of the cap pole (phi < ~0.85·π, ≈153°). The small-cap
    // slerp `(cap_dir·sin((1-t)·phi) + b_dir·sin(t·phi))/sin(phi)` is
    // numerically robust through this range, including the hemispherical
    // phi = π/2 case where the previous `> π/2` threshold flipped on
    // floating-point noise. The large-cap path is reserved for caps
    // that genuinely wrap toward the antipode (e.g. big-sphere minus
    // small-bite from a Difference op).
    let is_large_cap = avg_angle > 0.85 * PI;

    if is_large_cap {
        // `tessellate_large_spherical_cap` was written under the legacy
        // convention where `cap_dir` points to the boundary side and
        // `anti_pole = -cap_dir` is the cap interior pole. Our winding
        // -derived `cap_dir` points to the cap interior, so flip it (and
        // recompute the min angle in the flipped frame) for that path.
        let flipped_cap_dir = -cap_dir;
        let min_angle_flipped = loop_verts
            .iter()
            .map(|p| {
                let v = (*p - center).normalize();
                v.dot(flipped_cap_dir).clamp(-1.0, 1.0).acos()
            })
            .fold(f64::INFINITY, f64::min);
        tessellate_large_spherical_cap(
            loop_verts,
            center,
            radius,
            flipped_cap_dir,
            min_angle_flipped,
            reversed,
        )
    } else {
        let _ = min_angle;
        tessellate_small_spherical_cap(loop_verts, center, radius, cap_dir, params, reversed)
    }
}

/// Tessellate a small spherical cap with latitude rings between the cap
/// pole and the boundary loop. A 3-vertex boundary (cube-corner fillet
/// blend) used to fan to a single triangle per pair, producing the
/// visible 3-facet "diamond" at every corner. The ring-based path
/// gives a smooth dome and stitches to the boundary the same way the
/// large-cap path does.
fn tessellate_small_spherical_cap(
    loop_verts: &[Point3],
    center: Point3,
    radius: f64,
    cap_dir: vcad_kernel_math::Vec3,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    let sphere_normal = |pt: Point3| -> Vec3 {
        let n = (pt - center).normalize();
        if reversed {
            -n
        } else {
            n
        }
    };

    // Densify the boundary along great-circle arcs so the sphere's
    // boundary samples match the adjacent cylinder fillets' v-arc
    // cap samples 3D-point-for-3D-point. When the kernel welds
    // identical positions, the cylinder/sphere seam disappears and
    // computeVertexNormals averages cleanly across the boundary.
    //
    // `samples_per_arc` follows `circle_segments`: a 90° great-circle
    // (cube case) gets `circle_segments / 4` segments, which is the
    // same density a cylinder fillet uses around its 90° v-arc cap.
    let dense_loop_verts: Vec<Point3> = if loop_verts.len() >= 3 {
        let n = loop_verts.len();
        let mut out: Vec<Point3> = Vec::new();
        for i in 0..n {
            let p_i = loop_verts[i];
            let p_j = loop_verts[(i + 1) % n];
            let v_i = (p_i - center).normalize();
            let v_j = (p_j - center).normalize();
            let cos_theta = v_i.dot(v_j).clamp(-1.0, 1.0);
            let theta = cos_theta.acos();
            // Per-arc sample count scales with the arc angle so a 90°
            // boundary gets ~circle_segments/4, a 180° boundary
            // ~circle_segments/2, etc.
            let segments = ((params.circle_segments as f64) * theta / (2.0 * PI))
                .ceil()
                .max(1.0) as usize;
            out.push(p_i);
            if theta < 1e-9 {
                continue;
            }
            let v_perp_raw = v_j - v_i * cos_theta;
            let v_perp_len = v_perp_raw.norm();
            if v_perp_len < 1e-9 {
                continue;
            }
            let v_perp = v_perp_raw / v_perp_len;
            for k in 1..segments {
                let alpha = k as f64 * theta / segments as f64;
                let dir = v_i * alpha.cos() + v_perp * alpha.sin();
                out.push(center + radius * dir);
            }
        }
        out
    } else {
        loop_verts.to_vec()
    };
    let loop_verts = dense_loop_verts.as_slice();
    let boundary_count = loop_verts.len();

    // Cap pole — the outermost point of the cap along cap_dir.
    let pole = center + radius * cap_dir;
    mesh.vertices.push(pole.x as f32);
    mesh.vertices.push(pole.y as f32);
    mesh.vertices.push(pole.z as f32);
    let n = sphere_normal(pole);
    mesh.normals
        .extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);

    // Boundary-shaped rings: each interior ring has the SAME topology
    // as the boundary loop — vertex i of every ring is the slerp of
    // the corresponding boundary vertex toward the pole. This means
    // the boundary's wavy "spherical-triangle" shape (3 great-circle
    // arcs that dip closer to the pole between the trim points) is
    // preserved at every depth, so the band stitch between adjacent
    // rings is a clean quad strip with no angle-match slivers.
    //
    // It's the same idea Catia/NX use for their N-sided patches:
    // walk the cap inward following the boundary's parameterization
    // instead of imposing a constant-latitude grid that has to be
    // reconciled with the outline.
    let n_rings = ((params.latitude_segments as usize) / 2).max(3);

    // Boundary directions and their angular distance from the pole.
    let boundary_dirs: Vec<vcad_kernel_math::Vec3> = loop_verts
        .iter()
        .map(|p| (*p - center).normalize())
        .collect();
    let boundary_phis: Vec<f64> = boundary_dirs
        .iter()
        .map(|d| d.dot(cap_dir).clamp(-1.0, 1.0).acos())
        .collect();

    let pole_idx = 0u32;
    let stride = boundary_count as u32;

    // Generate `n_rings` interior rings + the boundary itself as the
    // outermost ring (so `n_rings + 1` total rings of `boundary_count`
    // vertices each, after the pole). Ring r ∈ [1, n_rings] sits at
    // fraction r / n_rings along the slerp from pole to boundary.
    for ring in 1..=n_rings {
        let t = ring as f64 / n_rings as f64;
        for (i, &b_dir) in boundary_dirs.iter().enumerate() {
            let dir = if t >= 1.0 {
                // Last ring IS the boundary — use the exact boundary
                // position so weld/normal-smoothing match the cylinder.
                b_dir
            } else {
                // Slerp(pole, boundary_i, t).
                let phi = boundary_phis[i];
                if phi < 1e-9 {
                    cap_dir
                } else {
                    let s = phi.sin();
                    (cap_dir * ((1.0 - t) * phi).sin() + b_dir * (t * phi).sin()) / s
                }
            };
            let pt = if t >= 1.0 {
                loop_verts[i]
            } else {
                center + radius * dir
            };
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let n = sphere_normal(pt);
            mesh.normals
                .extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
        }
    }

    let first_ring_start = 1u32;

    // Pole fan to the first interior ring.
    for i in 0..boundary_count {
        let v1 = first_ring_start + i as u32;
        let v2 = first_ring_start + ((i + 1) % boundary_count) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[pole_idx, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[pole_idx, v1, v2]);
        }
    }

    // Bands between consecutive rings (last band lands on the
    // boundary, since the boundary IS the last ring).
    for ring in 0..(n_rings - 1) {
        let ring_start = 1 + ring as u32 * stride;
        let next_ring_start = ring_start + stride;
        for i in 0..boundary_count {
            let i_next = (i + 1) % boundary_count;
            let bl = ring_start + i as u32;
            let br = ring_start + i_next as u32;
            let tl = next_ring_start + i as u32;
            let tr = next_ring_start + i_next as u32;
            if reversed {
                mesh.indices.extend_from_slice(&[bl, tr, tl]);
                mesh.indices.extend_from_slice(&[bl, br, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, tl, tr]);
                mesh.indices.extend_from_slice(&[bl, tr, br]);
            }
        }
    }

    mesh
}

/// Tessellate a large spherical cap using latitude rings with boundary stitching.
fn tessellate_large_spherical_cap(
    loop_verts: &[Point3],
    center: Point3,
    radius: f64,
    cap_dir: vcad_kernel_math::Vec3,
    min_angle: f64,
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Helper to compute sphere normal at a point
    let sphere_normal = |pt: Point3| -> vcad_kernel_math::Vec3 {
        let n = (pt - center).normalize();
        if reversed {
            -n
        } else {
            n
        }
    };

    // Antipodal pole (opposite to cap center)
    let anti_pole = center - radius * cap_dir;
    mesh.vertices.push(anti_pole.x as f32);
    mesh.vertices.push(anti_pole.y as f32);
    mesh.vertices.push(anti_pole.z as f32);
    let n = sphere_normal(anti_pole);
    mesh.normals
        .extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);

    // Create local coordinate system for longitude
    let up = cap_dir;
    let right = if up.x.abs() < 0.9 {
        vcad_kernel_math::Vec3::new(1.0, 0.0, 0.0)
            .cross(up)
            .normalize()
    } else {
        vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0)
            .cross(up)
            .normalize()
    };
    let forward = up.cross(right);

    // Number of rings between pole and boundary
    let n_rings = 8;
    let n_lon = 32;

    // Generate latitude rings from antipodal pole toward boundary
    let ring_stop = min_angle * 0.98;
    for ring in 1..=n_rings {
        let t = ring as f64 / (n_rings + 1) as f64;
        let angle_from_pole = PI - (PI - ring_stop) * (1.0 - t);
        let sin_a = angle_from_pole.sin();
        let cos_a = angle_from_pole.cos();

        for i in 0..=n_lon {
            let lon = 2.0 * PI * (i as f64 / n_lon as f64);
            let x = sin_a * lon.cos();
            let y = sin_a * lon.sin();
            let z = cos_a;

            let local = x * right + y * forward - z * up;
            let pt = center + radius * local;
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let n = sphere_normal(pt);
            mesh.normals
                .extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
        }
    }

    // Add boundary vertices
    let boundary_start = mesh.num_vertices();
    for p in loop_verts {
        mesh.vertices.push(p.x as f32);
        mesh.vertices.push(p.y as f32);
        mesh.vertices.push(p.z as f32);
        let n = sphere_normal(*p);
        mesh.normals
            .extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
    }

    let pole_idx = 0u32;
    let stride = (n_lon + 1) as u32;

    // Pole fan to first ring
    let first_ring_start = 1u32;
    for i in 0..n_lon {
        let v1 = first_ring_start + i as u32;
        let v2 = first_ring_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[pole_idx, v1, v2]);
        } else {
            mesh.indices.extend_from_slice(&[pole_idx, v2, v1]);
        }
    }

    // Bands between rings
    for ring in 0..(n_rings - 1) {
        let ring_start = 1 + ring as u32 * stride;
        let next_ring_start = ring_start + stride;
        for i in 0..n_lon {
            let bl = ring_start + i as u32;
            let br = ring_start + (i + 1) as u32;
            let tl = next_ring_start + i as u32;
            let tr = next_ring_start + (i + 1) as u32;
            if reversed {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            } else {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            }
        }
    }

    // Stitch last ring to boundary
    let last_ring_start = 1 + (n_rings - 1) as u32 * stride;
    let boundary_start = boundary_start as u32;
    let boundary_len = loop_verts.len();

    let last_ring_angles: Vec<f64> = (0..=n_lon)
        .map(|i| 2.0 * PI * (i as f64 / n_lon as f64))
        .collect();

    let boundary_angles: Vec<f64> = loop_verts
        .iter()
        .map(|p| {
            let v = (*p - center).normalize();
            let x = v.dot(right);
            let y = v.dot(forward);
            y.atan2(x).rem_euclid(2.0 * PI)
        })
        .collect();

    stitch_ring_to_boundary(
        &mut mesh,
        last_ring_start,
        n_lon,
        &last_ring_angles,
        boundary_start,
        boundary_len,
        &boundary_angles,
        reversed,
    );

    mesh
}

/// Stitch a latitude ring to an arbitrary boundary loop.
#[allow(clippy::too_many_arguments)]
fn stitch_ring_to_boundary(
    mesh: &mut TriangleMesh,
    ring_start: u32,
    ring_len: usize,
    ring_angles: &[f64],
    boundary_start: u32,
    boundary_len: usize,
    boundary_angles: &[f64],
    reversed: bool,
) {
    // For each ring edge, connect to nearest boundary vertex
    for i in 0..ring_len {
        let ring_curr = ring_start + i as u32;
        let ring_next = ring_start + ((i + 1) % (ring_len + 1)) as u32;

        let ring_angle = (ring_angles[i] + ring_angles[(i + 1) % (ring_len + 1)]) / 2.0;
        let mut closest_boundary = 0usize;
        let mut closest_dist = f64::INFINITY;
        for (j, &ba) in boundary_angles.iter().enumerate() {
            let dist = (ba - ring_angle)
                .abs()
                .min(2.0 * PI - (ba - ring_angle).abs());
            if dist < closest_dist {
                closest_dist = dist;
                closest_boundary = j;
            }
        }
        let boundary_idx = boundary_start + closest_boundary as u32;

        if reversed {
            mesh.indices
                .extend_from_slice(&[ring_curr, boundary_idx, ring_next]);
        } else {
            mesh.indices
                .extend_from_slice(&[ring_curr, ring_next, boundary_idx]);
        }
    }

    // For each boundary edge, connect to nearest ring vertex
    for i in 0..boundary_len {
        let b_curr = boundary_start + i as u32;
        let b_next = boundary_start + ((i + 1) % boundary_len) as u32;

        let b_angle = (boundary_angles[i] + boundary_angles[(i + 1) % boundary_len]) / 2.0;
        let mut closest_ring = 0usize;
        let mut closest_dist = f64::INFINITY;
        for (j, &ra) in ring_angles.iter().enumerate().take(ring_len + 1) {
            let dist = (ra - b_angle).abs().min(2.0 * PI - (ra - b_angle).abs());
            if dist < closest_dist {
                closest_dist = dist;
                closest_ring = j;
            }
        }
        let ring_idx = ring_start + closest_ring as u32;

        if reversed {
            mesh.indices.extend_from_slice(&[b_curr, ring_idx, b_next]);
        } else {
            mesh.indices.extend_from_slice(&[b_curr, b_next, ring_idx]);
        }
    }
}

/// Tessellate a conical face (lateral surface of a cone/frustum).
fn tessellate_conical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let mut n_circ = params.circle_segments as usize;
    let n_height = params.height_segments as usize;

    // Get seam vertices to determine the cone extent
    let verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    // Extract cone geometry for axis-aware parameterization
    let (axis, apex, ref_dir, half_angle) = if let Some(cone) = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::ConeSurface>(
    ) {
        (
            *cone.axis.as_ref(),
            cone.apex,
            *cone.ref_dir.as_ref(),
            cone.half_angle,
        )
    } else {
        // Fallback: assume Z-axis cone at origin
        let z_min = verts.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        let z_max = verts.iter().map(|v| v.z).fold(f64::MIN, f64::max);
        let r_min = verts
            .iter()
            .filter(|v| (v.z - z_min).abs() < 1e-6)
            .map(|v| (v.x * v.x + v.y * v.y).sqrt())
            .next()
            .unwrap_or(0.0);
        return tessellate_cone_direct(&verts, z_min, z_max, r_min, n_circ, n_height, reversed);
    };

    // Project vertices onto axis to get v parameter range (distance from apex)
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for pt in &verts {
        let d = pt - apex;
        let v = d.dot(axis) / half_angle.cos();
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    // Pick the widest ring's radius for adaptive sampling so the base of
    // a wide frustum doesn't end up as faceted as its tip.
    let max_radius = v_max.abs().max(v_min.abs()) * half_angle.sin().abs();
    if max_radius > 1e-9 {
        n_circ = (params.circle_segments_for_radius(max_radius) as usize).max(3);
    }

    // Generate mesh using surface.evaluate()
    let y_dir = axis.cross(ref_dir);
    let mut mesh = TriangleMesh::new();
    let mut rows: Vec<Vec<u32>> = Vec::new();

    // Helper to push normal for cone surface
    let push_cone_normal = |mesh: &mut TriangleMesh,
                            u: f64,
                            ref_dir: Vec3,
                            y_dir: Vec3,
                            axis: Vec3,
                            half_angle: f64,
                            reversed: bool| {
        // Cone outward normal: radial direction rotated by (π/2 - half_angle) toward axis
        let radial = u.cos() * ref_dir + u.sin() * y_dir;
        let normal = half_angle.cos() * radial - half_angle.sin() * axis;
        let (nx, ny, nz) = if reversed {
            (-normal.x as f32, -normal.y as f32, -normal.z as f32)
        } else {
            (normal.x as f32, normal.y as f32, normal.z as f32)
        };
        mesh.normals.extend_from_slice(&[nx, ny, nz]);
    };

    for j in 0..=n_height {
        let t = j as f64 / n_height as f64;
        let v = v_min + (v_max - v_min) * t;
        let r = v * half_angle.sin();

        let mut row = Vec::new();

        if r.abs() < 1e-12 {
            // Apex point - use average normal (axis direction)
            let pt = apex + v * half_angle.cos() * axis;
            let idx = mesh.num_vertices() as u32;
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            // At apex, normal is along the axis (degenerate point)
            let (nx, ny, nz) = if reversed {
                (axis.x as f32, axis.y as f32, axis.z as f32)
            } else {
                (-axis.x as f32, -axis.y as f32, -axis.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
            row.push(idx);
        } else {
            let center = apex + v * half_angle.cos() * axis;
            for i in 0..=n_circ {
                let u = 2.0 * PI * (i as f64 / n_circ as f64);
                let pt = center + r * (u.cos() * ref_dir + u.sin() * y_dir);
                let idx = mesh.num_vertices() as u32;
                mesh.vertices.push(pt.x as f32);
                mesh.vertices.push(pt.y as f32);
                mesh.vertices.push(pt.z as f32);
                push_cone_normal(&mut mesh, u, ref_dir, y_dir, axis, half_angle, reversed);
                row.push(idx);
            }
        }

        rows.push(row);
    }

    // Generate triangles between adjacent rows
    for j in 0..n_height {
        let bot = &rows[j];
        let top = &rows[j + 1];

        if bot.len() == 1 {
            let apex_idx = bot[0];
            for i in 0..(top.len() - 1) {
                if reversed {
                    mesh.indices
                        .extend_from_slice(&[apex_idx, top[i + 1], top[i]]);
                } else {
                    mesh.indices
                        .extend_from_slice(&[apex_idx, top[i], top[i + 1]]);
                }
            }
        } else if top.len() == 1 {
            let apex_idx = top[0];
            for i in 0..(bot.len() - 1) {
                if reversed {
                    mesh.indices
                        .extend_from_slice(&[bot[i], apex_idx, bot[i + 1]]);
                } else {
                    mesh.indices
                        .extend_from_slice(&[bot[i], bot[i + 1], apex_idx]);
                }
            }
        } else {
            for i in 0..n_circ {
                let bl = bot[i];
                let br = bot[i + 1];
                let tl = top[i];
                let tr = top[i + 1];
                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br]);
                    mesh.indices.extend_from_slice(&[br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl]);
                    mesh.indices.extend_from_slice(&[br, tr, tl]);
                }
            }
        }
    }

    mesh
}

/// Fallback cone tessellation using direct z-axis coordinates.
fn tessellate_cone_direct(
    verts: &[Point3],
    z_min: f64,
    z_max: f64,
    r_at_zmin: f64,
    n_circ: usize,
    n_height: usize,
    reversed: bool,
) -> TriangleMesh {
    let r_at_zmax = verts
        .iter()
        .filter(|v| (v.z - z_max).abs() < 1e-6)
        .map(|v| (v.x * v.x + v.y * v.y).sqrt())
        .next()
        .unwrap_or(0.0);

    let mut mesh = TriangleMesh::new();
    let mut rows: Vec<Vec<u32>> = Vec::new();

    for j in 0..=n_height {
        let t = j as f64 / n_height as f64;
        let z = z_min + (z_max - z_min) * t;
        let r = r_at_zmin + (r_at_zmax - r_at_zmin) * t;

        let mut row = Vec::new();
        if r < 1e-12 {
            let idx = mesh.num_vertices() as u32;
            mesh.vertices.extend_from_slice(&[0.0f32, 0.0f32, z as f32]);
            row.push(idx);
        } else {
            for i in 0..=n_circ {
                let u = 2.0 * PI * (i as f64 / n_circ as f64);
                let idx = mesh.num_vertices() as u32;
                mesh.vertices.extend_from_slice(&[
                    (r * u.cos()) as f32,
                    (r * u.sin()) as f32,
                    z as f32,
                ]);
                row.push(idx);
            }
        }
        rows.push(row);
    }

    for j in 0..n_height {
        let bot = &rows[j];
        let top = &rows[j + 1];
        if bot.len() == 1 {
            let a = bot[0];
            for i in 0..(top.len() - 1) {
                if reversed {
                    mesh.indices.extend_from_slice(&[a, top[i + 1], top[i]]);
                } else {
                    mesh.indices.extend_from_slice(&[a, top[i], top[i + 1]]);
                }
            }
        } else if top.len() == 1 {
            let a = top[0];
            for i in 0..(bot.len() - 1) {
                if reversed {
                    mesh.indices.extend_from_slice(&[bot[i], a, bot[i + 1]]);
                } else {
                    mesh.indices.extend_from_slice(&[bot[i], bot[i + 1], a]);
                }
            }
        } else {
            for i in 0..n_circ {
                let bl = bot[i];
                let br = bot[i + 1];
                let tl = top[i];
                let tr = top[i + 1];
                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br]);
                    mesh.indices.extend_from_slice(&[br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl]);
                    mesh.indices.extend_from_slice(&[br, tr, tl]);
                }
            }
        }
    }

    mesh
}

/// Tessellate a toroidal face.
///
/// Uses UV grid sampling similar to sphere tessellation.
fn tessellate_toroidal_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    // Use the wider of the major and tube radii to size the segmentation.
    // A small fillet torus (tiny tube) doesn't need many around-the-major-
    // axis samples; the major arc is the dominant curvature.
    let torus_radius = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::TorusSurface>()
        .map(|t| t.major_radius.abs().max(t.minor_radius.abs()).max(1e-6));
    let n_circ = if let Some(r) = torus_radius {
        (params.circle_segments_for_radius(r) as usize).max(3)
    } else {
        params.circle_segments.max(3) as usize
    };

    let mut mesh = TriangleMesh::new();

    // Derive the face's actual u/v range from its outer loop vertices
    // instead of tessellating the entire torus donut. A plane↔cylinder
    // fillet torus blend only covers a small rectangular patch in (u, v)
    // space — rendering the full 2π×2π domain produced the overhanging
    // ring we saw in the pork-chop render that poked 4 units above the
    // top cap and below the bottom cap. Falls back to the full domain
    // when the face doesn't expose a `TorusSurface` we can invert.
    let ((default_u_min, default_u_max), (default_v_min, default_v_max)) = surface.domain();
    let (u_min, u_max, v_min, v_max) = if let Some(torus) = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::TorusSurface>(
    ) {
        let verts: Vec<_> = topo
            .loop_half_edges(face.outer_loop)
            .map(|he| topo.vertices[topo.half_edges[he].origin].point)
            .collect();
        let y = torus.axis.as_ref().cross(torus.ref_dir.as_ref());
        let mut us = Vec::new();
        let mut vs = Vec::new();
        for pt in &verts {
            let d = *pt - torus.center;
            // u: angle around torus major axis from ref_dir → y
            let ru = d.dot(torus.ref_dir.as_ref());
            let yu = d.dot(y);
            let u = yu.atan2(ru);
            let u = if u < 0.0 { u + 2.0 * PI } else { u };
            // v: angle around tube. project d onto (tube_center_dir, axis) plane.
            let tube_dir = *torus.ref_dir.as_ref() * u.cos() + y * u.sin();
            let in_tube_plane = d - tube_dir * torus.major_radius;
            let cv = in_tube_plane.dot(tube_dir);
            let sv = in_tube_plane.dot(torus.axis.as_ref());
            let v = sv.atan2(cv);
            let v = if v < 0.0 { v + 2.0 * PI } else { v };
            us.push(u);
            vs.push(v);
        }
        if us.len() >= 2 {
            us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            vs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (us[0], us[us.len() - 1], vs[0], vs[vs.len() - 1])
        } else {
            (default_u_min, default_u_max, default_v_min, default_v_max)
        }
    } else {
        (default_u_min, default_u_max, default_v_min, default_v_max)
    };

    // Scale n_u / n_v by the fraction of the full torus the face covers.
    let u_frac = (u_max - u_min) / (2.0 * PI);
    let v_frac = (v_max - v_min) / (2.0 * PI);
    let n_u = ((n_circ as f64 * u_frac).ceil() as usize).max(2);
    let n_v = ((n_circ as f64 * v_frac).ceil() as usize).max(2);

    // Generate grid of vertices with analytical normals
    for j in 0..=n_v {
        let v = v_min + (v_max - v_min) * (j as f64 / n_v as f64);
        for i in 0..=n_u {
            let u = u_min + (u_max - u_min) * (i as f64 / n_u as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
        }
    }

    // Generate triangles with per-quad winding consistency: compare
    // each quad's candidate CCW normal (from its own (bl, br, tl)
    // vertex order in the generated mesh) against the torus surface's
    // analytical outward normal at the quad's (u, v) midpoint. Flip
    // the winding when they disagree. Needed for plane-cyl blend-torus
    // faces near armpit junctions, where the torus's (u, v)
    // parameterization winds CCW-inward in some u-ranges after trim
    // snapping — the vertex normals are already outward (set from
    // `surface.normal(uv)` in the loop above), but the face winding
    // Three.js's `computeVertexNormals` derives for rendering would
    // point the opposite way.
    let torus_ref = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::TorusSurface>();
    let stride = (n_u + 1) as u32;
    for j in 0..n_v {
        for i in 0..n_u {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            let mut flip = false;
            if let Some(torus) = torus_ref {
                let du = (u_max - u_min) / n_u as f64;
                let dv = (v_max - v_min) / n_v as f64;
                let u_mid = u_min + du * (i as f64 + 0.5);
                let v_mid = v_min + dv * (j as f64 + 0.5);
                let p_bl_x = mesh.vertices[3 * bl as usize] as f64;
                let p_bl_y = mesh.vertices[3 * bl as usize + 1] as f64;
                let p_bl_z = mesh.vertices[3 * bl as usize + 2] as f64;
                let p_br_x = mesh.vertices[3 * br as usize] as f64;
                let p_br_y = mesh.vertices[3 * br as usize + 1] as f64;
                let p_br_z = mesh.vertices[3 * br as usize + 2] as f64;
                let p_tl_x = mesh.vertices[3 * tl as usize] as f64;
                let p_tl_y = mesh.vertices[3 * tl as usize + 1] as f64;
                let p_tl_z = mesh.vertices[3 * tl as usize + 2] as f64;
                let ex = p_br_x - p_bl_x;
                let ey = p_br_y - p_bl_y;
                let ez = p_br_z - p_bl_z;
                let fx = p_tl_x - p_bl_x;
                let fy = p_tl_y - p_bl_y;
                let fz = p_tl_z - p_bl_z;
                let nx = ey * fz - ez * fy;
                let ny = ez * fx - ex * fz;
                let nz = ex * fy - ey * fx;
                let outward = *torus.normal(Point2::new(u_mid, v_mid));
                let dot = nx * outward.x + ny * outward.y + nz * outward.z;
                if reversed {
                    flip = dot > 0.0;
                } else {
                    flip = dot < 0.0;
                }
            }

            let effective_reversed = reversed ^ flip;
            if effective_reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a B-spline or NURBS face.
///
/// Uses adaptive UV grid sampling.
fn tessellate_bspline_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Use higher resolution for B-splines since they can be complex
    let n_u = (params.circle_segments * 2).max(16) as usize;
    let n_v = (params.circle_segments * 2).max(16) as usize;

    let mut mesh = TriangleMesh::new();

    // Get UV domain
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();

    // Generate grid of vertices with normals
    for j in 0..=n_v {
        let v = v_min + (v_max - v_min) * (j as f64 / n_v as f64);
        for i in 0..=n_u {
            let u = u_min + (u_max - u_min) * (i as f64 / n_u as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);

            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);

            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.push(nx);
            mesh.normals.push(ny);
            mesh.normals.push(nz);
        }
    }

    // Generate triangles
    let stride = (n_u + 1) as u32;
    for j in 0..n_v {
        for i in 0..n_u {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a planar disk with arbitrary orientation.
/// `x_dir` and `y_dir` define the disk plane.
fn tessellate_disk_general(
    center: Point3,
    radius: f64,
    x_dir: vcad_kernel_math::Vec3,
    y_dir: vcad_kernel_math::Vec3,
    segments: u32,
    flip: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let n = segments as usize;

    // Center vertex
    mesh.vertices.push(center.x as f32);
    mesh.vertices.push(center.y as f32);
    mesh.vertices.push(center.z as f32);

    // Rim vertices
    for i in 0..=n {
        let u = 2.0 * PI * (i as f64 / n as f64);
        let pt = center + radius * (u.cos() * x_dir + u.sin() * y_dir);
        mesh.vertices.push(pt.x as f32);
        mesh.vertices.push(pt.y as f32);
        mesh.vertices.push(pt.z as f32);
    }

    // Fan triangles
    for i in 0..n {
        let v0 = 0u32;
        let v1 = (i + 1) as u32;
        let v2 = (i + 2) as u32;
        if flip {
            mesh.indices.extend_from_slice(&[v0, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[v0, v1, v2]);
        }
    }

    mesh
}

/// Tessellate a planar disk (cap face) with a circular boundary.
/// Used for cylinder and cone caps.
pub fn tessellate_disk(
    center: Point3,
    radius: f64,
    z: f64,
    segments: u32,
    flip: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let n = segments as usize;

    // Center vertex
    mesh.vertices.push(center.x as f32);
    mesh.vertices.push(center.y as f32);
    mesh.vertices.push(z as f32);

    // Rim vertices
    for i in 0..=n {
        let u = 2.0 * PI * (i as f64 / n as f64);
        mesh.vertices.push((radius * u.cos()) as f32);
        mesh.vertices.push((radius * u.sin()) as f32);
        mesh.vertices.push(z as f32);
    }

    // Fan triangles
    for i in 0..n {
        let v0 = 0u32; // center
        let v1 = (i + 1) as u32;
        let v2 = (i + 2) as u32;
        if flip {
            mesh.indices.extend_from_slice(&[v0, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[v0, v1, v2]);
        }
    }

    mesh
}

/// Full tessellation of a B-rep solid, using `segments` as a quality hint.
///
/// This is the main entry point for converting a B-rep to a triangle mesh.
///
/// Output format:
/// - `vertices`: flat `Vec<f32>` of `[x, y, z, x, y, z, ...]`
/// - `indices`: flat `Vec<u32>` of triangle vertex indices
pub fn tessellate(brep: &BRepSolid, segments: u32) -> TriangleMesh {
    let params = TessellationParams::from_segments(segments);
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    let mut mesh = TriangleMesh::new();

    for &face_id in &shell.faces {
        let face = &brep.topology.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];
        let reversed = face.orientation == Orientation::Reversed;

        match surface.surface_type() {
            SurfaceKind::Plane => {
                // Use winding-aware tessellation to handle faces with mismatched loop winding
                let face_mesh = tessellate_planar_face_with_geom(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cylinder => {
                let face_mesh = tessellate_cylindrical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);

                // Also tessellate the caps
                // (Caps are separate faces and will be handled as planar faces
                //  if they have enough vertices. But our cylinder caps only have
                //  1 vertex in the loop, so we generate disks directly.)
            }
            SurfaceKind::Sphere => {
                let face_mesh = tessellate_spherical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cone => {
                let face_mesh = tessellate_conical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            _ => {
                // Fallback for tessellate(): use winding-aware tessellation
                let face_mesh = tessellate_planar_face_with_geom(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
        }
    }

    mesh
}

/// Tessellate a B-rep solid with special handling for cap faces that
/// have degenerate (single-vertex) loops.
///
/// This is the primary tessellation function used by the facade crate.
pub fn tessellate_brep(brep: &BRepSolid, segments: u32) -> TriangleMesh {
    let params = TessellationParams::from_segments(segments);
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    let mut mesh = TriangleMesh::new();
    // Positions (quantized to 0.001mm, same resolution as the welder)
    // of every vertex contributed by a Sphere face. After post-weld
    // fan-fill, we only fill boundary loops that touch at least one of
    // these positions — this makes the fill selective to convex fillet
    // vertex-blend armpit gaps, not e.g. plate-with-hole cutouts.
    let mut sphere_vert_keys: std::collections::HashSet<[i64; 3]> =
        std::collections::HashSet::new();

    for &face_id in &shell.faces {
        let face = &brep.topology.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];
        let reversed = face.orientation == Orientation::Reversed;
        let loop_len = brep.topology.loop_len(face.outer_loop);
        // Track sphere AND torus face vertex positions. Only loops that
        // touch one of these get filled — enough to cover armpit gaps
        // at filleted junctions (always near sphere + torus blend),
        // while leaving plate-with-hole / boolean-cut rectangular
        // cutouts alone (those have no sphere OR torus faces).
        let is_fillet_face = matches!(
            surface.surface_type(),
            SurfaceKind::Sphere | SurfaceKind::Torus
        );
        let pre_face_verts = mesh.num_vertices();
        let pre_face_tris = mesh.indices.len() / 3;
        let face_kind_tag: u8 = FaceKindTag::from(surface.surface_type()) as u8;

        match surface.surface_type() {
            SurfaceKind::Plane => {
                if loop_len <= 1 {
                    // Degenerate cap face with ≤1 vertex — single degenerate edge
                    // forming a full circle. Sample the circle into a proper polygon.
                    let verts: Vec<_> = brep
                        .topology
                        .loop_half_edges(face.outer_loop)
                        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                        .collect();
                    if let Some(&v) = verts.first() {
                        let plane = &brep.geometry.surfaces[face.surface_index];
                        let center = plane.evaluate(Point2::origin());
                        let r = (v - center).norm();
                        let x_dir = if r > 1e-12 {
                            (v - center).normalize()
                        } else {
                            plane.d_du(Point2::origin()).normalize()
                        };
                        let normal = plane.normal(Point2::origin());
                        let y_dir = normal.as_ref().cross(x_dir);

                        if face.inner_loops.is_empty() {
                            let disk = tessellate_disk_general(
                                center,
                                r,
                                x_dir,
                                y_dir,
                                params.circle_segments,
                                reversed,
                            );
                            mesh.merge(&disk);
                        } else {
                            // Degenerate outer loop WITH inner loops (holes) —
                            // e.g. a cylinder cap after boolean subtraction.
                            // Sample the outer circle into a polygon and use
                            // hole-aware CDT tessellation.
                            let n = params.circle_segments as usize;
                            let mut outer_3d = Vec::with_capacity(n);
                            for i in 0..n {
                                let theta = 2.0 * PI * (i as f64) / (n as f64);
                                let p = center + r * (theta.cos() * x_dir + theta.sin() * y_dir);
                                outer_3d.push(p);
                            }

                            let u_axis = x_dir;
                            let v_axis = y_dir;
                            let project = |p: &Point3| -> (f64, f64) {
                                let d = *p - center;
                                (d.dot(u_axis), d.dot(v_axis))
                            };

                            let outer_2d: Vec<(f64, f64)> = outer_3d.iter().map(&project).collect();

                            let mut inner_loops_3d: Vec<Vec<Point3>> = Vec::new();
                            let mut inner_loops_2d: Vec<Vec<(f64, f64)>> = Vec::new();
                            for &inner_loop in &face.inner_loops {
                                let iv: Vec<Point3> = brep
                                    .topology
                                    .loop_half_edges(inner_loop)
                                    .map(|he| {
                                        brep.topology.vertices[brep.topology.half_edges[he].origin]
                                            .point
                                    })
                                    .collect();
                                if iv.len() >= 3 {
                                    let iv_2d: Vec<(f64, f64)> = iv.iter().map(&project).collect();
                                    inner_loops_3d.push(iv);
                                    inner_loops_2d.push(iv_2d);
                                }
                            }

                            // Normalize winding: outer CCW, inner CW
                            let outer_area = polygon_area_2d(&outer_2d);
                            let (outer_2d, outer_3d) = if outer_area < 0.0 {
                                let mut o2 = outer_2d;
                                let mut o3 = outer_3d;
                                o2.reverse();
                                o3.reverse();
                                (o2, o3)
                            } else {
                                (outer_2d, outer_3d)
                            };
                            for (i, hole_2d) in inner_loops_2d.iter_mut().enumerate() {
                                let hole_area = polygon_area_2d(hole_2d);
                                if hole_area > 0.0 {
                                    inner_loops_3d[i].reverse();
                                    hole_2d.reverse();
                                }
                            }

                            merge_overlapping_holes(&mut inner_loops_2d, &mut inner_loops_3d);

                            let mut face_mesh = triangulate_circular_cap_with_holes(
                                center,
                                r,
                                x_dir,
                                y_dir,
                                &outer_2d,
                                &inner_loops_2d,
                                &outer_3d,
                                &inner_loops_3d,
                                reversed,
                            );

                            // Add planar normals
                            let face_normal = if reversed { -normal } else { normal };
                            let (nx, ny, nz) = (
                                face_normal.x as f32,
                                face_normal.y as f32,
                                face_normal.z as f32,
                            );
                            for _ in 0..face_mesh.num_vertices() {
                                face_mesh.normals.extend_from_slice(&[nx, ny, nz]);
                            }

                            mesh.merge(&face_mesh);
                        }
                    }
                } else {
                    // Use winding-aware tessellation to handle faces with mismatched loop winding
                    let face_mesh = tessellate_planar_face_with_geom(
                        &brep.topology,
                        &brep.geometry,
                        face_id,
                        reversed,
                    );
                    mesh.merge(&face_mesh);
                }
            }
            SurfaceKind::Cylinder => {
                let face_mesh = tessellate_cylindrical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Sphere => {
                let face_mesh = tessellate_spherical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cone => {
                let face_mesh = tessellate_conical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            _ => {
                // Fallback for tessellate_brep(): use winding-aware tessellation
                let face_mesh = tessellate_planar_face_with_geom(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
        }

        if is_fillet_face {
            let n_verts = mesh.num_vertices();
            for vi in pre_face_verts..n_verts {
                sphere_vert_keys.insert([
                    (mesh.vertices[3 * vi] as f64 * 1000.0).round() as i64,
                    (mesh.vertices[3 * vi + 1] as f64 * 1000.0).round() as i64,
                    (mesh.vertices[3 * vi + 2] as f64 * 1000.0).round() as i64,
                ]);
            }
        }

        // Tag every triangle this face added with its surface kind.
        // Use OVERWRITE (not just extend) because `mesh.merge` may
        // have already padded these slots with Unknown when the face's
        // sub-mesh didn't carry tags of its own (degenerate-cap path
        // uses `tessellate_disk_general`, which returns an untagged
        // mesh).
        let post_face_tris = mesh.indices.len() / 3;
        while mesh.face_kinds.len() < post_face_tris {
            mesh.face_kinds.push(FaceKindTag::Unknown as u8);
        }
        for t in pre_face_tris..post_face_tris {
            mesh.face_kinds[t] = face_kind_tag;
        }
    }

    weld_coincident_vertices(&mut mesh);
    fill_tiny_boundary_loops(&mut mesh, 6, &sphere_vert_keys);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::{make_cone, make_cube, make_cylinder, make_sphere};

    fn assert_normals_consistent(mesh: &TriangleMesh, ctx: &str) {
        assert!(
            mesh.normals.is_empty() || mesh.normals.len() == mesh.vertices.len(),
            "{ctx}: {} normals for {} vertices (invariant: empty or 1:1)",
            mesh.normals.len(),
            mesh.vertices.len()
        );
    }

    /// Merging a face that came back with empty normals into a normal-bearing
    /// mesh must not leave a partial normals buffer — the invariant
    /// `normals.len() == vertices.len()` has to hold in release builds, where
    /// merge's old check was compiled out. Regression guard for the silent
    /// buffer underrun that hit RealityKit/Metal, GLB/STL export, and the
    /// raytracer.
    #[test]
    fn merge_empty_normals_keeps_buffers_consistent() {
        // A mesh with full per-vertex normals.
        let mut a = TriangleMesh {
            vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            face_kinds: Vec::new(),
        };
        // A second mesh with vertices but NO normals (the bug trigger). Placed
        // at z=1 so weld won't collapse it into `a`.
        let b = TriangleMesh {
            vertices: vec![0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0],
            indices: vec![0, 1, 2],
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        a.merge(&b);
        assert_normals_consistent(&a, "after merge");
        assert_eq!(a.num_vertices(), 6);

        // ...and it survives weld without truncating the normals buffer.
        weld_coincident_vertices(&mut a);
        assert_normals_consistent(&a, "after weld");

        // Every normal is unit length — no zeroed/garbage slots.
        for v in 0..a.num_vertices() {
            let n = [a.normals[3 * v], a.normals[3 * v + 1], a.normals[3 * v + 2]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "vertex {v} normal not unit: {n:?}"
            );
        }
    }

    /// A mesh whose normals buffer is shorter than its vertex buffer (partial)
    /// must be repaired by weld rather than copied slice-by-slice — otherwise
    /// the welded output keeps the shortfall.
    #[test]
    fn weld_recomputes_partial_normals() {
        // Two well-separated triangles (6 distinct verts) but normals for only
        // the first three vertices.
        let mut mesh = TriangleMesh {
            vertices: vec![
                0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, // tri 0
                5.0, 0.0, 0.0, 7.0, 0.0, 0.0, 5.0, 2.0, 0.0, // tri 1
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0], // 3 of 6
            face_kinds: Vec::new(),
        };

        weld_coincident_vertices(&mut mesh);
        assert_normals_consistent(&mesh, "weld of partial-normal mesh");
        assert_eq!(mesh.num_vertices(), 6);
    }

    /// Merging into an empty accumulator (the per-face loop's first iteration)
    /// keeps things consistent regardless of which side carries normals.
    #[test]
    fn merge_into_empty_accumulator() {
        let face = TriangleMesh {
            vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            face_kinds: Vec::new(),
        };
        let mut acc = TriangleMesh::new();
        acc.merge(&face);
        assert_normals_consistent(&acc, "empty.merge(full)");
        assert_eq!(acc.normals.len(), acc.vertices.len());
    }

    #[test]
    fn test_tessellate_cube() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        // A cube should have at least 12 triangles (2 per face × 6 faces)
        assert!(
            mesh.num_triangles() >= 12,
            "expected >= 12 triangles, got {}",
            mesh.num_triangles()
        );
        assert!(mesh.num_vertices() > 0);
    }

    #[test]
    fn test_closed_cube_has_no_boundary_edges() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        let boundary = mesh.boundary_edges();
        assert!(
            boundary.is_empty(),
            "closed cube should have zero boundary edges, got {}",
            boundary.len()
        );
        let nm = mesh.non_manifold_edges();
        assert!(
            nm.is_empty(),
            "closed cube should have zero non-manifold edges, got {}",
            nm.len()
        );
    }

    #[test]
    fn test_tessellate_cylinder() {
        let brep = make_cylinder(5.0, 10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        // Cylinder: lateral (32 quads = 64 tris) + 2 caps (32 tris each) = ~128
        // Use 120 as threshold to ensure caps are actually present
        assert!(
            mesh.num_triangles() >= 120,
            "expected >= 120 triangles (lateral + 2 caps), got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_tessellate_sphere() {
        let brep = make_sphere(10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        assert!(
            mesh.num_triangles() >= 100,
            "expected >= 100 triangles, got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_tessellate_cone() {
        let brep = make_cone(5.0, 0.0, 10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        assert!(
            mesh.num_triangles() >= 32,
            "expected >= 32 triangles, got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_cube_volume_from_mesh() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        let vol = compute_mesh_volume(&mesh);
        assert!((vol - 1000.0).abs() < 1.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_cube_surface_area_from_mesh() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        let area = compute_mesh_surface_area(&mesh);
        assert!((area - 600.0).abs() < 1.0, "expected ~600, got {area}");
    }

    #[test]
    fn test_cylinder_volume_from_mesh() {
        let brep = make_cylinder(5.0, 10.0, 64);
        let mesh = tessellate_brep(&brep, 64);
        let expected = PI * 25.0 * 10.0; // π r² h
        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - expected).abs() < expected * 0.05,
            "expected ~{expected}, got {vol}"
        );
    }

    #[test]
    fn test_sphere_volume_from_mesh() {
        let brep = make_sphere(10.0, 64);
        let mesh = tessellate_brep(&brep, 64);
        let expected = (4.0 / 3.0) * PI * 1000.0; // (4/3)πr³
        let vol = compute_mesh_volume(&mesh);
        // Sphere tessellation is less accurate, allow 5% error
        assert!(
            (vol - expected).abs() < expected * 0.05,
            "expected ~{expected}, got {vol}"
        );
    }

    /// Compute signed volume of a triangle mesh using the divergence theorem.
    fn compute_mesh_volume(mesh: &TriangleMesh) -> f64 {
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

    /// Compute surface area of a triangle mesh.
    fn compute_mesh_surface_area(mesh: &TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut area = 0.0;
        for tri in indices.chunks(3) {
            let (i0, i1, i2) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            area += (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() / 2.0;
        }
        area
    }

    /// Regression test: degenerate cap face (1 half-edge outer loop) with an
    /// inner loop (hole from boolean subtraction) must still produce triangles.
    /// Previously, the code routed this to `tessellate_planar_face_with_geom()`
    /// which returned an empty mesh because outer_verts.len() < 3.
    #[test]
    fn test_degenerate_cap_with_inner_loop() {
        use vcad_kernel_geom::{GeometryStore, Plane};
        use vcad_kernel_math::{Point3, Vec3};
        use vcad_kernel_topo::{Orientation, ShellType, Topology};

        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();

        // Outer circle: radius 10, degenerate 1-vertex loop (full circle)
        let v_outer = topo.add_vertex(Point3::new(10.0, 0.0, 0.0));
        let he_outer = topo.add_half_edge(v_outer);
        let outer_loop = topo.add_loop(&[he_outer]);

        // Inner circle: radius 3, centered at origin, 8 vertices (a hole)
        let n = 8;
        let mut inner_he_ids = Vec::new();
        for i in 0..n {
            let theta = 2.0 * PI * (i as f64) / (n as f64);
            let v = topo.add_vertex(Point3::new(3.0 * theta.cos(), 3.0 * theta.sin(), 0.0));
            inner_he_ids.push(topo.add_half_edge(v));
        }
        // Link inner half-edges into a chain
        for i in 0..n {
            let next = (i + 1) % n;
            topo.half_edges[inner_he_ids[i]].next = Some(inner_he_ids[next]);
        }
        let inner_loop = topo.add_loop(&inner_he_ids);

        // Plane at z=0, normal +Z
        let plane = Plane::new(
            Point3::origin(),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let surf_idx = geom.add_surface(Box::new(plane));

        let face_id = topo.add_face(outer_loop, surf_idx, Orientation::Forward);
        topo.faces[face_id].inner_loops.push(inner_loop);

        let shell = topo.add_shell(vec![face_id], ShellType::Outer);
        let solid_id = topo.add_solid(shell);

        let brep = BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        };

        let mesh = tessellate_brep(&brep, 32);
        assert!(
            mesh.num_triangles() > 0,
            "degenerate cap with inner loop must produce triangles, got 0"
        );
        // The annular region area = π(10² - 3²) ≈ 285.88
        let area = compute_mesh_surface_area(&mesh);
        let expected = PI * (100.0 - 9.0);
        assert!(
            (area - expected).abs() < expected * 0.1,
            "expected area ~{expected:.1}, got {area:.1}"
        );
    }

    #[test]
    fn test_triangulate_square_with_circular_hole() {
        // Test the triangulation of a square with a circular hole in the center
        // This is what happens when a cylinder cuts through a planar face
        use vcad_kernel_math::Point3;

        // Square: 10x10 in XY plane at Z=0 (CCW winding)
        let outer_2d: Vec<(f64, f64)> = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let outer_3d: Vec<Point3> = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        // Circular hole: radius 2, center at (5, 5), 8 segments
        // CW winding (opposite to outer) - this is how B-rep stores inner loops
        let n_seg = 8usize;
        let hole_2d: Vec<(f64, f64)> = (0..n_seg)
            .rev() // CW winding: reverse the order
            .map(|i| {
                let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n_seg as f64);
                (5.0 + 2.0 * theta.cos(), 5.0 + 2.0 * theta.sin())
            })
            .collect();
        let hole_3d: Vec<Point3> = hole_2d
            .iter()
            .map(|&(x, y)| Point3::new(x, y, 0.0))
            .collect();

        let inner_2d = vec![hole_2d];
        let inner_3d = vec![hole_3d];

        let mesh =
            triangulate_polygon_with_holes(&outer_2d, &inner_2d, &outer_3d, &inner_3d, false);

        println!(
            "Square with hole: {} triangles, {} vertices",
            mesh.num_triangles(),
            mesh.num_vertices()
        );

        // Should have triangles
        assert!(mesh.num_triangles() > 0, "Should produce triangles");

        // Compute mesh area - should be square area minus circle area
        let area = compute_mesh_surface_area(&mesh);
        let expected_area = 100.0 - std::f64::consts::PI * 4.0; // 100 - 4π ≈ 87.4
        println!("Mesh area: {:.2}, expected: {:.2}", area, expected_area);

        // Allow some tolerance due to polygon approximation of circle
        assert!(
            (area - expected_area).abs() < 5.0,
            "Area should be ~{:.1}, got {:.1}",
            expected_area,
            area
        );
    }
}
