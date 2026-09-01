//! Manifold auditing and repair for triangle meshes.
//!
//! Every mesh that leaves the kernel as an STL is a *claim*: that these
//! triangles bound a solid. Downstream, a slicer re-derives the solid from
//! that surface by ray parity, so any place where the surface is open,
//! doubled, or branching is read as geometry the CAD model never had — a
//! floating island, an interior crack, an inverted region. Those defects
//! are invisible in a viewport and expensive to find by hand (see the
//! four-rewrite history in ecto/vcad#840).
//!
//! This module supplies the two halves that were missing:
//!
//! * [`check_manifold`] — one auditable report, replacing the
//!   copy-pasted "count the boundary edges" idiom scattered across the
//!   test suite. It reports the *four* independent ways a surface stops
//!   bounding a solid, not just the one that is easy to count.
//! * [`make_manifold`] — a deterministic repair pass that welds
//!   near-coincident vertices, drops degenerate triangles, and cancels
//!   double-covered patches.
//!
//! ## Why repair is needed at all
//!
//! A mesh boolean splits both operands' triangles against each other and
//! keeps the surviving fragments. Where two tools' boundaries very nearly
//! (but not exactly) coincide — two cuts meeting along a 0.05° sliver, a
//! chamfer knee landing a micron off a wall — classification is *correct*
//! on both sides yet keeps two fragments covering the same surface. The
//! result is watertight but branches: an edge with four incident
//! triangles. Ray parity through such an edge is orientation-blind, so it
//! is exactly the defect that reads as a crack downstream.
//!
//! Repair is a separate pass rather than something folded into the
//! boolean because callers that reproject vertices onto analytic carriers
//! (quadric surface fidelity) must run before any topology changes; see
//! `vcad_kernel_booleans::mesh::csg`.
//!
//! ## Determinism
//!
//! [`make_manifold`] is a pure function of the input buffers: vertices are
//! welded in index order to the first representative seen, triangles are
//! emitted in input order, and no hash-map iteration order reaches the
//! output. Running it twice on the same input yields byte-identical
//! buffers, which is what makes STL diffing across regenerations a usable
//! review tool.

use std::collections::HashMap;

use crate::TriangleMesh;

/// Default vertex-weld tolerance in millimetres.
///
/// Two orders of magnitude below the smallest feature the kernel's
/// primitives express (a 0.1 mm detent bump is a thousand times this) and
/// an order above the f32 quantisation of a vertex at a 100 mm radius
/// (~8e-6): the band where seam copies of one point live and nothing else
/// does.
///
/// Deliberately tight. Widening it to the boolean's own t-junction
/// tolerance (2e-3) was tried on the rana-60c shell and is *destructive*:
/// at that radius it merges distinct vertices of adjacent facets, which
/// collapses real triangles and tears holes in the surface (measured: 13
/// bad edges appear on a mesh that had none). Defects wider than this
/// tolerance are handled by [`repair_branching`] instead, which merges
/// only the vertices that are actually part of a defect.
pub const DEFAULT_WELD_EPS: f64 = 1e-4;

/// How much wider than the weld tolerance the targeted repair pass may
/// reach when merging the endpoints of a branching edge.
///
/// Boolean seam copies are snapped by the pipeline at up to 2e-3 mm, so a
/// pair that survives welding can sit that far apart; 50× the weld
/// tolerance covers it with margin while staying twenty times under the
/// smallest real feature. Only vertices already incident to a
/// non-manifold edge are eligible, so this reach cannot touch clean
/// geometry however wide it is.
const REPAIR_REACH: f64 = 50.0;

/// Cap on targeted repair rounds. Each round strictly reduces the
/// branching-edge count or stops; in practice the rana-60c shell needs
/// one.
const MAX_REPAIR_ROUNDS: usize = 8;

/// Boundary-loop perimeter (mm) below which a hole is capped outright.
///
/// Matches the mesh boolean's own `HOLE_PERIMETER_EPS`. A loop this short
/// is a pinhole left by a dropped sliver — the rana-60c shell shows a
/// three-edge one, 0.15 mm around, where membrane stripping removes one
/// triangle too many. Real openings in a part are orders of magnitude
/// bigger (this shell's smallest is a 4.4 mm pin entry), so capping here
/// cannot fill a feature.
const MAX_PINHOLE_PERIMETER: f64 = 0.5;

/// A structural audit of a triangle mesh.
///
/// "Manifold" here means the specific property an STL consumer needs: the
/// triangles form a closed, consistently oriented, non-branching surface,
/// so that ray parity recovers the intended solid. [`is_manifold`] is the
/// conjunction of the four defect counts being zero;
/// [`is_single_manifold_shell`] additionally requires one connected
/// component.
///
/// [`is_manifold`]: ManifoldReport::is_manifold
/// [`is_single_manifold_shell`]: ManifoldReport::is_single_manifold_shell
#[derive(Debug, Clone, PartialEq)]
pub struct ManifoldReport {
    /// Triangle count.
    pub triangles: usize,
    /// Vertex count (as stored; welding is not implied).
    pub vertices: usize,
    /// Undirected edges incident to exactly one triangle — the surface has
    /// a hole there. STL's classic "bad edge".
    pub boundary_edges: usize,
    /// Undirected edges incident to three or more triangles — the surface
    /// branches. Watertight but not a solid boundary.
    pub non_manifold_edges: usize,
    /// Undirected edges incident to exactly two triangles that traverse
    /// them the *same* way, meaning the two neighbours disagree about
    /// which side is outside. Missed entirely by a boundary-edge count.
    pub inconsistent_edges: usize,
    /// Triangles with a repeated vertex index or zero area.
    pub degenerate_triangles: usize,
    /// Triangles sharing a vertex set with an earlier triangle. Includes
    /// both same-orientation duplicates and opposed pairs (internal
    /// membranes).
    pub duplicate_triangles: usize,
    /// Connected components under vertex-index adjacency. A single solid
    /// with no enclosed voids has one.
    pub components: usize,
    /// Signed volume via the divergence theorem. Negative means the whole
    /// surface is wound inside-out.
    pub signed_volume: f64,
}

impl ManifoldReport {
    /// Does this surface bound a solid?
    pub fn is_manifold(&self) -> bool {
        self.boundary_edges == 0
            && self.non_manifold_edges == 0
            && self.inconsistent_edges == 0
            && self.degenerate_triangles == 0
    }

    /// Does this surface bound a solid as exactly one connected shell?
    pub fn is_single_manifold_shell(&self) -> bool {
        self.is_manifold() && self.components == 1
    }
}

impl std::fmt::Display for ManifoldReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} tris / {} verts: boundary={} non_manifold={} inconsistent={} \
             degenerate={} duplicate={} components={} volume={:.4}",
            self.triangles,
            self.vertices,
            self.boundary_edges,
            self.non_manifold_edges,
            self.inconsistent_edges,
            self.degenerate_triangles,
            self.duplicate_triangles,
            self.components,
            self.signed_volume,
        )
    }
}

fn point(mesh: &TriangleMesh, i: u32) -> [f64; 3] {
    let k = i as usize * 3;
    [
        mesh.vertices[k] as f64,
        mesh.vertices[k + 1] as f64,
        mesh.vertices[k + 2] as f64,
    ]
}

fn tri_area(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// Signed volume of a closed triangle mesh via the divergence theorem.
pub fn signed_volume(mesh: &TriangleMesh) -> f64 {
    let mut vol = 0.0_f64;
    for tri in mesh.indices.chunks(3) {
        let (a, b, c) = (
            point(mesh, tri[0]),
            point(mesh, tri[1]),
            point(mesh, tri[2]),
        );
        vol += a[0] * (b[1] * c[2] - c[1] * b[2]) - b[0] * (a[1] * c[2] - c[1] * a[2])
            + c[0] * (a[1] * b[2] - b[1] * a[2]);
    }
    vol / 6.0
}

/// Canonical (sorted) vertex-set key for a triangle.
fn tri_key(t: &[u32]) -> [u32; 3] {
    let mut k = [t[0], t[1], t[2]];
    k.sort_unstable();
    k
}

/// Is `t` a rotation (rather than a reflection) of its sorted key?
fn tri_positive(t: &[u32; 3]) -> bool {
    let k = tri_key(t);
    [[t[0], t[1], t[2]], [t[1], t[2], t[0]], [t[2], t[0], t[1]]]
        .iter()
        .any(|r| *r == k)
}

/// Audit a triangle mesh's manifoldness.
///
/// Purely diagnostic — the mesh is not modified and no tolerance is
/// applied: vertices are compared by index, so a mesh whose seams are
/// merely *near*-coincident reports as open. Run [`make_manifold`] first
/// if the mesh came out of a boolean.
pub fn check_manifold(mesh: &TriangleMesh) -> ManifoldReport {
    let ntri = mesh.indices.len() / 3;
    let nvert = mesh.num_vertices();

    let mut degenerate = 0usize;
    let mut seen: HashMap<[u32; 3], usize> = HashMap::new();
    let mut duplicate = 0usize;
    // (undirected edge) -> (incidence count, net direction)
    let mut edges: HashMap<(u32, u32), (usize, i32)> = HashMap::new();

    for tri in mesh.indices.chunks(3) {
        let t = [tri[0], tri[1], tri[2]];
        let (a, b, c) = (
            point(mesh, t[0]),
            point(mesh, t[1]),
            point(mesh, t[2]),
        );
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || tri_area(a, b, c) == 0.0 {
            degenerate += 1;
            continue;
        }
        let e = seen.entry(tri_key(&t)).or_insert(0);
        *e += 1;
        if *e > 1 {
            duplicate += 1;
        }
        for k in 0..3 {
            let (u, v) = (t[k], t[(k + 1) % 3]);
            let slot = edges.entry((u.min(v), u.max(v))).or_insert((0, 0));
            slot.0 += 1;
            slot.1 += if u < v { 1 } else { -1 };
        }
    }

    let mut boundary = 0usize;
    let mut non_manifold = 0usize;
    let mut inconsistent = 0usize;
    for &(count, net) in edges.values() {
        match count {
            1 => boundary += 1,
            2 if net != 0 => inconsistent += 1,
            2 => {}
            _ => non_manifold += 1,
        }
    }

    ManifoldReport {
        triangles: ntri,
        vertices: nvert,
        boundary_edges: boundary,
        non_manifold_edges: non_manifold,
        inconsistent_edges: inconsistent,
        degenerate_triangles: degenerate,
        duplicate_triangles: duplicate,
        components: count_components(mesh, nvert),
        signed_volume: signed_volume(mesh),
    }
}

/// Connected components of the triangle adjacency graph, counting only
/// vertices that some triangle actually references.
fn count_components(mesh: &TriangleMesh, nvert: usize) -> usize {
    if mesh.indices.is_empty() {
        return 0;
    }
    let mut parent: Vec<u32> = (0..nvert as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let mut used = vec![false; nvert];
    for tri in mesh.indices.chunks(3) {
        for k in 0..3 {
            used[tri[k] as usize] = true;
            let (a, b) = (find(&mut parent, tri[k]), find(&mut parent, tri[(k + 1) % 3]));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut roots = std::collections::HashSet::new();
    for v in 0..nvert as u32 {
        if used[v as usize] {
            roots.insert(find(&mut parent, v));
        }
    }
    roots.len()
}

/// Deterministically repair a mesh into a manifold surface.
///
/// Three passes, in order:
///
/// 1. **Weld.** Vertices within `eps` of an earlier vertex collapse onto
///    it. Boolean fragment vertices land on shared seams by independent
///    f64 arithmetic and then round to f32, so seam copies disagree in the
///    last few microns; without welding they read as separate boundaries.
/// 2. **Drop degenerates.** Triangles that lost a distinct vertex to
///    welding, or whose area falls to `eps²`, carry no surface.
/// 3. **Cancel double covers.** Triangles sharing a vertex set are summed
///    by orientation: a net of zero is an internal membrane (a patch and
///    its mirror, bounding nothing) and both are dropped; a non-zero net
///    keeps exactly one triangle wound the majority way.
///
/// Step 3 is what removes the branching edges a near-coincident cut pair
/// leaves behind. It is conservative: it only ever *deletes* surface that
/// is covered twice, so it cannot open a hole in a closed mesh, and the
/// enclosed volume is preserved exactly (a cancelling pair contributes
/// zero to the divergence integral).
///
/// Pass [`DEFAULT_WELD_EPS`] unless the caller knows its feature scale.
/// Normals are recomputed as flat per-triangle averages; `face_kinds` and
/// `face_ids` are dropped, since triangles do not survive one-to-one.
pub fn make_manifold(mesh: &TriangleMesh, eps: f64) -> TriangleMesh {
    let eps = eps.max(0.0);
    let mut out = clean_pass(mesh, eps, eps * eps, None);
    for _ in 0..MAX_REPAIR_ROUNDS {
        let Some(next) = repair_branching(&out, eps) else {
            break;
        };
        out = next;
    }
    cap_pinholes(&mut out);
    out
}

/// Fan-fill boundary loops short enough to be dropped-sliver artifacts.
///
/// Removing a double-covered patch or a degenerate triangle occasionally
/// takes one triangle too many and leaves a pinhole a few hundredths of a
/// millimetre across. Capping it restores the closed surface without
/// touching anything at feature scale — see [`MAX_PINHOLE_PERIMETER`].
///
/// Loops are walked from the lowest-indexed vertex outward, so which loops
/// get capped is a function of the input alone.
fn cap_pinholes(mesh: &mut TriangleMesh) {
    // The surface holds each directed boundary edge once; a cap supplies
    // the reverse. Chain b -> a; a repeated key means the boundary pinches
    // at that vertex, and those loops are left alone.
    let mut present: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for tri in mesh.indices.chunks(3) {
        for k in 0..3 {
            present.insert((tri[k], tri[(k + 1) % 3]));
        }
    }
    let mut succ: HashMap<u32, u32> = HashMap::new();
    let mut pinched: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // Sorted, not hash order: on a pinched boundary vertex two candidate
    // predecessors compete for the same key, and whichever wins changes
    // the capping. Sorting keeps the output a function of the input.
    let mut directed: Vec<(u32, u32)> = present.iter().copied().collect();
    directed.sort_unstable();
    for &(a, b) in &directed {
        if a != b && !present.contains(&(b, a)) && succ.insert(b, a).is_some() {
            pinched.insert(b);
        }
    }
    if succ.is_empty() {
        return;
    }
    let mut starts: Vec<u32> = succ.keys().copied().collect();
    starts.sort_unstable();

    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut added: Vec<u32> = Vec::new();
    for start in starts {
        if visited.contains(&start) {
            continue;
        }
        let mut loop_verts = vec![start];
        let mut perimeter = 0.0;
        let mut closed = false;
        let mut cur = start;
        while let Some(&next) = succ.get(&cur) {
            if pinched.contains(&cur) || loop_verts.len() > 32 {
                break;
            }
            let (p, q) = (point(mesh, cur), point(mesh, next));
            perimeter +=
                ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt();
            if next == start {
                closed = true;
                break;
            }
            loop_verts.push(next);
            cur = next;
        }
        for &v in &loop_verts {
            visited.insert(v);
        }
        if !closed || loop_verts.len() < 3 || perimeter > MAX_PINHOLE_PERIMETER {
            continue;
        }
        for i in 1..loop_verts.len() - 1 {
            added.extend_from_slice(&[loop_verts[0], loop_verts[i], loop_verts[i + 1]]);
        }
    }
    if !added.is_empty() {
        mesh.indices.extend_from_slice(&added);
        mesh.normals = vertex_normals(mesh);
    }
}

/// One targeted round against branching edges, or `None` when there is
/// nothing left to do.
///
/// [`clean_pass`] merges vertices only within `eps`, which is tight enough
/// that it can never damage real geometry. That leaves the defects whose
/// two copies of a point sit *further* apart than `eps` — a boolean can
/// snap seam vertices across ten times that distance, so a double-covered
/// sliver strip can survive as an edge with four incident triangles whose
/// endpoints are microns but not nanometres apart.
///
/// Widening the global weld to reach them is not an option: at that radius
/// it also merges genuinely distinct facet corners and tears the surface
/// open. So this pass restricts merging to the vertices that are *already*
/// endpoints of a branching edge — a handful out of thousands — and lets
/// them collapse onto each other at [`REPAIR_REACH`] × `eps`. Collapsing a
/// branching edge turns its incident sliver triangles degenerate, and
/// [`clean_pass`] then drops them, dissolving the branch.
///
/// Returns `None` when the mesh has no branching edges or when the round
/// changed nothing, so the caller's loop terminates.
fn repair_branching(mesh: &TriangleMesh, eps: f64) -> Option<TriangleMesh> {
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for tri in mesh.indices.chunks(3) {
        for k in 0..3 {
            let (u, v) = (tri[k], tri[(k + 1) % 3]);
            if u != v {
                *counts.entry((u.min(v), u.max(v))).or_insert(0) += 1;
            }
        }
    }
    let mut eligible = vec![false; mesh.num_vertices()];
    let mut any = false;
    for (&(u, v), &n) in &counts {
        if n > 2 {
            eligible[u as usize] = true;
            eligible[v as usize] = true;
            any = true;
        }
    }
    if !any {
        return None;
    }
    // The collapse leaves the offending triangles exactly degenerate
    // (a repeated index), so the area threshold stays at the base scale
    // rather than widening with the reach and eating real slivers.
    let next = clean_pass(mesh, eps * REPAIR_REACH, eps * eps, Some(&eligible));
    if next.indices == mesh.indices && next.vertices == mesh.vertices {
        return None;
    }
    Some(next)
}

/// Weld, drop degenerates and cancel double covers in one pass.
///
/// When `eligible` is `Some`, only vertices flagged in it may be merged
/// into one another; everything else is carried through untouched. That is
/// what makes the wide-tolerance repair round safe.
fn clean_pass(
    mesh: &TriangleMesh,
    eps: f64,
    area_eps: f64,
    eligible: Option<&[bool]>,
) -> TriangleMesh {
    let nvert = mesh.num_vertices();

    // --- 1. weld ---------------------------------------------------------
    // Bucket on a grid of side `eps` and probe the 27 neighbouring cells,
    // so a pair straddling a cell boundary still merges. Vertices are
    // visited in index order and always attach to the lowest-index
    // representative found, which makes the mapping input-order-determined
    // rather than hash-order-determined.
    let cell = if eps > 0.0 { eps } else { 1.0 };
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut reps: Vec<[f64; 3]> = Vec::new();
    let mut remap = vec![0u32; nvert];
    for i in 0..nvert {
        let p = point(mesh, i as u32);
        let key = [
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        ];
        let mut hit: Option<u32> = None;
        // A vertex outside the eligibility mask still gets a representative
        // of its own, but never attaches to another vertex.
        let may_merge = eligible.map(|mask| mask[i]).unwrap_or(true);
        if eps > 0.0 && may_merge {
            'probe: for dx in -1..=1i64 {
                for dy in -1..=1i64 {
                    for dz in -1..=1i64 {
                        let Some(bucket) = grid.get(&[key[0] + dx, key[1] + dy, key[2] + dz])
                        else {
                            continue;
                        };
                        for &j in bucket {
                            let q = reps[j as usize];
                            let d2 = (q[0] - p[0]).powi(2)
                                + (q[1] - p[1]).powi(2)
                                + (q[2] - p[2]).powi(2);
                            if d2 <= eps * eps {
                                hit = Some(match hit {
                                    Some(prev) if prev <= j => prev,
                                    _ => j,
                                });
                                break 'probe;
                            }
                        }
                    }
                }
            }
        }
        remap[i] = match hit {
            Some(j) => j,
            None => {
                let id = reps.len() as u32;
                reps.push(p);
                // Only offer this point as a merge target if it is itself
                // eligible — otherwise a defective vertex could drag onto
                // an untouchable one and move clean geometry.
                if may_merge {
                    grid.entry(key).or_default().push(id);
                }
                id
            }
        };
    }

    // --- 2. rebuild, dropping degenerates ---------------------------------
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(mesh.indices.len() / 3);
    for tri in mesh.indices.chunks(3) {
        let t = [
            remap[tri[0] as usize],
            remap[tri[1] as usize],
            remap[tri[2] as usize],
        ];
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        if tri_area(reps[t[0] as usize], reps[t[1] as usize], reps[t[2] as usize]) <= area_eps {
            continue;
        }
        tris.push(t);
    }

    // --- 3. cancel double covers ------------------------------------------
    let mut net: HashMap<[u32; 3], (i32, usize)> = HashMap::new();
    for t in &tris {
        let slot = net.entry(tri_key(t)).or_insert((0, 0));
        slot.0 += if tri_positive(t) { 1 } else { -1 };
        slot.1 += 1;
    }
    let mut emitted: std::collections::HashSet<[u32; 3]> = std::collections::HashSet::new();
    let mut kept: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
    for t in &tris {
        let key = tri_key(t);
        let (net_orient, count) = net[&key];
        if count == 1 {
            kept.push(*t);
            continue;
        }
        if net_orient == 0 || !emitted.insert(key) {
            continue;
        }
        let mut t = *t;
        if (net_orient > 0) != tri_positive(&t) {
            t.swap(1, 2);
        }
        kept.push(t);
    }

    // --- emit --------------------------------------------------------------
    // Only vertices some surviving triangle references are carried over, so
    // welding cannot leave orphans behind. Survivors keep their weld order,
    // so a mesh that needed no repair comes back index-for-index identical.
    let mut used = vec![false; reps.len()];
    for t in &kept {
        for &v in t {
            used[v as usize] = true;
        }
    }
    let mut vremap = vec![u32::MAX; reps.len()];
    let mut out = TriangleMesh::new();
    for (i, p) in reps.iter().enumerate() {
        if !used[i] {
            continue;
        }
        vremap[i] = (out.vertices.len() / 3) as u32;
        out.vertices
            .extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
    }
    out.indices = kept
        .iter()
        .flat_map(|t| t.iter().map(|&v| vremap[v as usize]))
        .collect();
    out.normals = vertex_normals(&out);
    out
}

/// Area-weighted vertex normals, unit length where defined.
fn vertex_normals(mesh: &TriangleMesh) -> Vec<f32> {
    let mut acc = vec![0.0f64; mesh.vertices.len()];
    for tri in mesh.indices.chunks(3) {
        let (a, b, c) = (
            point(mesh, tri[0]),
            point(mesh, tri[1]),
            point(mesh, tri[2]),
        );
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &i in tri {
            let k = i as usize * 3;
            acc[k] += n[0];
            acc[k + 1] += n[1];
            acc[k + 2] += n[2];
        }
    }
    acc.chunks(3)
        .flat_map(|n| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 0.0 {
                [
                    (n[0] / len) as f32,
                    (n[1] / len) as f32,
                    (n[2] / len) as f32,
                ]
            } else {
                [0.0, 0.0, 0.0]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube as an indexed mesh, outward wound.
    fn cube() -> TriangleMesh {
        let v: [[f32; 3]; 8] = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let f: [[u32; 4]; 6] = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut m = TriangleMesh::new();
        m.vertices = v.iter().flatten().copied().collect();
        for q in f {
            m.indices
                .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        m.normals = vec![0.0; m.vertices.len()];
        m
    }

    #[test]
    fn clean_cube_is_a_single_manifold_shell() {
        let r = check_manifold(&cube());
        assert!(r.is_single_manifold_shell(), "{r}");
        assert_eq!(r.triangles, 12);
        assert!((r.signed_volume - 1.0).abs() < 1e-9, "{r}");
    }

    #[test]
    fn a_missing_triangle_shows_up_as_boundary_edges() {
        let mut m = cube();
        m.indices.truncate(m.indices.len() - 3);
        let r = check_manifold(&m);
        assert_eq!(r.boundary_edges, 3, "{r}");
        assert!(!r.is_manifold());
    }

    #[test]
    fn a_flipped_triangle_shows_up_as_inconsistent_edges() {
        let mut m = cube();
        let n = m.indices.len();
        m.indices.swap(n - 2, n - 1);
        let r = check_manifold(&m);
        assert_eq!(r.boundary_edges, 0, "boundary count cannot see this: {r}");
        assert_eq!(r.inconsistent_edges, 3, "{r}");
        assert!(!r.is_manifold());
    }

    #[test]
    fn two_disjoint_cubes_are_two_components() {
        let mut m = cube();
        let mut b = cube();
        for x in b.vertices.iter_mut().step_by(3) {
            *x += 5.0;
        }
        let base = (m.vertices.len() / 3) as u32;
        m.vertices.extend_from_slice(&b.vertices);
        m.normals.extend_from_slice(&b.normals);
        m.indices.extend(b.indices.iter().map(|i| i + base));
        let r = check_manifold(&m);
        assert!(r.is_manifold(), "{r}");
        assert_eq!(r.components, 2, "{r}");
        assert!(!r.is_single_manifold_shell());
    }

    #[test]
    fn make_manifold_welds_a_split_seam() {
        // Duplicate every vertex a hair off and rewire half the faces to
        // the copies: still geometrically a cube, topologically shredded.
        let base = cube();
        let mut m = base.clone();
        let n = (base.vertices.len() / 3) as u32;
        m.vertices
            .extend(base.vertices.iter().map(|x| x + 1e-6));
        m.normals.extend_from_slice(&base.normals);
        for i in m.indices.iter_mut().skip(base.indices.len() / 2) {
            *i += n;
        }
        assert!(check_manifold(&m).boundary_edges > 0);

        let fixed = make_manifold(&m, DEFAULT_WELD_EPS);
        let r = check_manifold(&fixed);
        assert!(r.is_single_manifold_shell(), "{r}");
        assert!((r.signed_volume - 1.0).abs() < 1e-3, "{r}");
    }

    #[test]
    fn make_manifold_cancels_an_internal_membrane() {
        // Two unit cubes stacked face to face and concatenated — the naive
        // "union" a hand-rolled generator emits. The shared z=1 face is
        // covered twice, once from each cube, in opposite directions: an
        // internal face bounding nothing, and every edge around it is
        // 4-incident.
        let mut m = cube();
        let mut top = cube();
        for z in top.vertices.iter_mut().skip(2).step_by(3) {
            *z += 1.0;
        }
        let base = (m.vertices.len() / 3) as u32;
        m.vertices.extend_from_slice(&top.vertices);
        m.normals.extend_from_slice(&top.normals);
        m.indices.extend(top.indices.iter().map(|i| i + base));

        // Index-wise the two cubes are still disjoint, so a plain audit
        // reports two clean shells — the internal face only becomes
        // visible once the coincident seam vertices are welded, which is
        // precisely why the audit and the repair are separate passes.
        let before = check_manifold(&m);
        assert_eq!(before.triangles, 24);
        assert_eq!(before.components, 2, "{before}");

        let fixed = make_manifold(&m, DEFAULT_WELD_EPS);
        let r = check_manifold(&fixed);
        assert!(r.is_single_manifold_shell(), "{r}");
        assert_eq!(r.triangles, 20, "the shared face's 4 triangles vanish: {r}");
        assert!(
            (r.signed_volume - 2.0).abs() < 1e-6,
            "the two cubes become one 1x1x2 box: {r}"
        );
    }

    #[test]
    fn make_manifold_is_a_no_op_on_a_clean_mesh() {
        let m = cube();
        let a = make_manifold(&m, DEFAULT_WELD_EPS);
        assert_eq!(a.indices, m.indices);
        assert_eq!(a.vertices, m.vertices);
    }

    #[test]
    fn make_manifold_is_deterministic() {
        let mut m = cube();
        let base = m.clone();
        let n = (base.vertices.len() / 3) as u32;
        m.vertices.extend(base.vertices.iter().map(|x| x + 1e-6));
        m.normals.extend_from_slice(&base.normals);
        for i in m.indices.iter_mut().skip(base.indices.len() / 2) {
            *i += n;
        }
        let a = make_manifold(&m, DEFAULT_WELD_EPS);
        for _ in 0..8 {
            let b = make_manifold(&m, DEFAULT_WELD_EPS);
            assert_eq!(a.vertices, b.vertices);
            assert_eq!(a.indices, b.indices);
        }
    }
}
