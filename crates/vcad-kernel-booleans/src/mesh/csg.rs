//! Mesh-level CSG: BSP splitting + ray-parity classification.
//!
//! This is the *fallback* boolean: when the B-rep pipeline detects an
//! arrangement its splitters cannot represent (intersecting circle
//! arrangements on a sphere, quadric×quadric crossings with no analytic
//! SSI, …) it must not return a plausible-looking wrong solid. Instead the
//! operands are tessellated and combined here at the triangle level, and
//! the result is wrapped back into a triangle-soup B-rep with
//! [`super::mesh_to_brep`] — the same stopgap contract the Steinmetz
//! cylinder×cylinder path already uses.
//!
//! Each operand's polygons are split along the carrier planes of the other
//! operand's triangles (AABB-localized so an infinite carrier can't
//! shatter far-away geometry), then every fragment is classified by
//! casting rays against the *other operand's actual mesh*
//! ([`super::point_in_mesh`], exact predicates) at two points nudged along
//! ±normal from the fragment centroid. Classification deliberately does
//! NOT use BSP leaf semantics: chained fallbacks feed triangle-soup
//! results (with hairline t-junction seams) back in as operands, and leaf
//! classification on such input misclassifies whole fragments (measured: a
//! chained pocket-and-slot part read 32% high while every parity probe was
//! correct). Ray parity is robust on cracked input and doubles as
//! principled coplanar-face handling:
//!
//! - both probes inside → `In`; both outside → `Out`
//! - split verdict → the fragment lies ON the other boundary, and the
//!   probe pattern tells whether the other surface faces the same way
//!   (`OnAligned`) or opposite (`OnOpposed`)
//!
//! Keep table (B fragments are flipped when kept by a difference):
//!
//! | op           | A keeps            | B keeps    |
//! |--------------|--------------------|------------|
//! | Union        | Out, OnAligned     | Out        |
//! | Intersection | In, OnAligned      | In         |
//! | Difference   | Out, OnOpposed     | In (flip)  |
//!
//! (`OnAligned`/`OnOpposed` duplicates on the B side are always dropped —
//! the A side already decided the shared surface.)
//!
//! All arithmetic is `f64`; splitting and classification are simple loops,
//! so deep tessellations cannot overflow the call stack.

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_tessellate::TriangleMesh;

use crate::api::BooleanOp;
use crate::mesh::point_in_mesh;

/// Plane-side classification tolerance (mm) for splitting.
const EPS: f64 = 1e-5;

/// Upper bound (mm) on the parity-probe nudge. The actual offset scales
/// with the fragment (see [`probe_offset`]).
const PROBE_EPS_MAX: f64 = 1e-3;

/// Lower bound (mm) on the probe nudge: below this, f32 vertex noise makes
/// the probe's side ambiguous.
const PROBE_EPS_MIN: f64 = 2e-6;

/// How far to step off a fragment when classifying it.
///
/// A fixed offset misclassifies sliver fragments: splitting routinely
/// produces pieces only a few microns across, and stepping a flat 1e-3 mm
/// off one lands the probe beyond a *neighbouring* surface rather than in
/// the material the fragment bounds. Dropping the sliver then leaves a
/// hairline hole (measured: a 0.003 mm gap in a 7.6 mm part). Scaling the
/// step to the fragment's own size keeps the probe in the fragment's
/// neighbourhood, with a floor at f32 resolution.
fn probe_offset(poly: &Polygon) -> f64 {
    let mut n = Vec3::new(0.0, 0.0, 0.0);
    let o = poly.verts[0];
    for i in 1..poly.verts.len() - 1 {
        n += (poly.verts[i] - o).cross(poly.verts[i + 1] - o);
    }
    let area = 0.5 * n.norm();
    (0.25 * area.sqrt()).clamp(PROBE_EPS_MIN, PROBE_EPS_MAX)
}

#[derive(Clone)]
struct Polygon {
    verts: Vec<Point3>,
    normal: Vec3,
    w: f64,
}

impl Polygon {
    fn new(verts: Vec<Point3>) -> Option<Self> {
        if verts.len() < 3 {
            return None;
        }
        // Newell's method: stable normal for sliver fragments where two
        // edges are nearly parallel.
        let mut n = Vec3::new(0.0, 0.0, 0.0);
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            n.x += (a.y - b.y) * (a.z + b.z);
            n.y += (a.z - b.z) * (a.x + b.x);
            n.z += (a.x - b.x) * (a.y + b.y);
        }
        let len = n.norm();
        if len < 1e-12 {
            return None;
        }
        let normal = n / len;
        let w = normal.dot(verts[0].to_vec());
        Some(Polygon { verts, normal, w })
    }

    fn flip(&mut self) {
        self.verts.reverse();
        self.normal = -self.normal;
        self.w = -self.w;
    }

    fn centroid(&self) -> Point3 {
        let mut c = Vec3::new(0.0, 0.0, 0.0);
        for v in &self.verts {
            c += v.to_vec();
        }
        Point3::from_vec(c / self.verts.len() as f64)
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// Split `poly` by the plane `(normal, w)` into the four csg.js buckets.
fn split_polygon(
    normal: &Vec3,
    w: f64,
    poly: &Polygon,
    coplanar_front: &mut Vec<Polygon>,
    coplanar_back: &mut Vec<Polygon>,
    front: &mut Vec<Polygon>,
    back: &mut Vec<Polygon>,
) {
    let mut poly_type = 0u8;
    let mut types = Vec::with_capacity(poly.verts.len());
    for v in &poly.verts {
        let t = normal.dot(v.to_vec()) - w;
        let ty = if t < -EPS {
            BACK
        } else if t > EPS {
            FRONT
        } else {
            COPLANAR
        };
        poly_type |= ty;
        types.push(ty);
    }

    match poly_type {
        COPLANAR => {
            if normal.dot(poly.normal) > 0.0 {
                coplanar_front.push(poly.clone());
            } else {
                coplanar_back.push(poly.clone());
            }
        }
        FRONT => front.push(poly.clone()),
        BACK => back.push(poly.clone()),
        _ => {
            let mut f: Vec<Point3> = Vec::new();
            let mut b: Vec<Point3> = Vec::new();
            let n = poly.verts.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let ti = types[i];
                let tj = types[j];
                let vi = poly.verts[i];
                let vj = poly.verts[j];
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let denom = normal.dot(vj - vi);
                    if denom.abs() > 1e-15 {
                        let t = (w - normal.dot(vi.to_vec())) / denom;
                        let v = vi + t * (vj - vi);
                        f.push(v);
                        b.push(v);
                    }
                }
            }
            if let Some(p) = Polygon::new(f) {
                front.push(p);
            }
            if let Some(p) = Polygon::new(b) {
                back.push(p);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Aabb {
    min: [f64; 3],
    max: [f64; 3],
}

impl Aabb {
    fn of(verts: &[Point3]) -> Aabb {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in verts {
            let c = [v.x, v.y, v.z];
            for k in 0..3 {
                min[k] = min[k].min(c[k]);
                max[k] = max[k].max(c[k]);
            }
        }
        Aabb { min, max }
    }

    fn overlaps(&self, other: &Aabb, pad: f64) -> bool {
        (0..3).all(|k| self.min[k] <= other.max[k] + pad && other.min[k] <= self.max[k] + pad)
    }
}

/// Cut every polygon of `polys` along the carrier planes of the other
/// operand's triangles, localized by AABB overlap: a fragment is only split
/// by a triangle's plane while it overlaps that triangle's bounding box, so
/// an infinite carrier can't shatter geometry far from the actual surface.
/// Every fragment that crosses the other operand's surface must cross one
/// of its triangles — and therefore gets split by that triangle's plane —
/// so each final fragment lies entirely inside, outside, or on the other
/// operand (up to tolerance). Nothing is classified or dropped here.
fn split_by_other(polys: Vec<Polygon>, other: &[Polygon]) -> Vec<Polygon> {
    let other_boxes: Vec<Aabb> = other.iter().map(|t| Aabb::of(&t.verts)).collect();
    let pad = 10.0 * EPS;
    let mut out = Vec::new();
    let mut frags: Vec<Polygon> = Vec::new();
    let mut next: Vec<Polygon> = Vec::new();
    for poly in polys {
        let poly_box = Aabb::of(&poly.verts);
        frags.clear();
        frags.push(poly);
        for (tri, tri_box) in other.iter().zip(&other_boxes) {
            if !poly_box.overlaps(tri_box, pad) {
                continue;
            }
            next.clear();
            for f in frags.drain(..) {
                if !Aabb::of(&f.verts).overlaps(tri_box, pad) {
                    next.push(f);
                    continue;
                }
                // Coplanar fragments need no split by this plane; the
                // coplanar buckets receive an unmodified clone.
                let before = next.len();
                let mut cf = Vec::new();
                let mut cb = Vec::new();
                let mut front = Vec::new();
                let mut back = Vec::new();
                split_polygon(
                    &tri.normal,
                    tri.w,
                    &f,
                    &mut cf,
                    &mut cb,
                    &mut front,
                    &mut back,
                );
                next.extend(cf);
                next.extend(cb);
                next.extend(front);
                next.extend(back);
                debug_assert!(next.len() > before);
            }
            std::mem::swap(&mut frags, &mut next);
        }
        out.append(&mut frags);
    }
    out
}

/// Where a fragment sits relative to the other operand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Out,
    In,
    /// On the other operand's boundary, surfaces facing the same way.
    OnAligned,
    /// On the other operand's boundary, surfaces facing opposite ways.
    OnOpposed,
}

/// Classify a fragment against the other operand's mesh by ray parity at
/// two probes nudged off the fragment along ±normal.
fn classify(frag: &Polygon, other: &TriangleMesh) -> Class {
    if other.indices.is_empty() {
        return Class::Out;
    }
    let c = frag.centroid();
    let n = frag.normal;
    let eps = probe_offset(frag);
    let plus = point_in_mesh(&(c + eps * n), other);
    let minus = point_in_mesh(&(c - eps * n), other);
    match (plus, minus) {
        (false, false) => Class::Out,
        (true, true) => Class::In,
        // Other's material only on the −normal side: its surface at the
        // fragment faces +normal, same as the fragment.
        (false, true) => Class::OnAligned,
        (true, false) => Class::OnOpposed,
    }
}

fn mesh_polygons(mesh: &TriangleMesh) -> Vec<Polygon> {
    let mut polys = Vec::with_capacity(mesh.indices.len() / 3);
    for tri in mesh.indices.chunks(3) {
        let p = |k: usize| {
            let i = tri[k] as usize * 3;
            Point3::new(
                mesh.vertices[i] as f64,
                mesh.vertices[i + 1] as f64,
                mesh.vertices[i + 2] as f64,
            )
        };
        if let Some(poly) = Polygon::new(vec![p(0), p(1), p(2)]) {
            polys.push(poly);
        }
    }
    polys
}

/// Distance (mm) within which a stray vertex counts as lying ON a polygon
/// edge during t-junction healing. Far above f32 vertex noise, far below
/// the split tolerance that created the junction.
const TJUNCTION_EPS: f64 = 2e-3;

/// Triangulate the kept fragments, inserting any of `stitch` that lies on a
/// polygon edge (t-junction healing). Fragments are convex — they start as
/// triangles and are only ever cut by planes — so a fan triangulation stays
/// valid after inserting (collinear) points.
fn triangulate(polys: &[Polygon], stitch: &[Point3]) -> (TriangleMesh, Vec<Point3>) {
    let mut mesh = TriangleMesh::new();
    // Exact f64 representative per emitted vertex index: healing must hand
    // the *original* coordinates back into the next pass — reading the f32
    // mesh back loses more precision than the 1e-7 dedup cell, so a
    // stitched point would no longer merge with the crack vertex it heals.
    let mut reps: Vec<Point3> = Vec::new();
    let mut cache: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut push = |p: &Point3, mesh: &mut TriangleMesh, reps: &mut Vec<Point3>| -> u32 {
        let key = [
            (p.x * 1e7).round() as i64,
            (p.y * 1e7).round() as i64,
            (p.z * 1e7).round() as i64,
        ];
        *cache.entry(key).or_insert_with(|| {
            let idx = (mesh.vertices.len() / 3) as u32;
            mesh.vertices.push(p.x as f32);
            mesh.vertices.push(p.y as f32);
            mesh.vertices.push(p.z as f32);
            reps.push(*p);
            idx
        })
    };
    let mut refined: Vec<Point3> = Vec::new();
    let mut on_edge: Vec<(f64, Point3)> = Vec::new();
    for poly in polys {
        let verts: &[Point3] = if stitch.is_empty() {
            &poly.verts
        } else {
            refined.clear();
            let n = poly.verts.len();
            for i in 0..n {
                let a = poly.verts[i];
                let b = poly.verts[(i + 1) % n];
                refined.push(a);
                let ab = b - a;
                let len2 = ab.dot(ab);
                if len2 < 1e-18 {
                    continue;
                }
                on_edge.clear();
                for p in stitch {
                    let ap = *p - a;
                    let t = ap.dot(ab) / len2;
                    if !(1e-9..=1.0 - 1e-9).contains(&t) {
                        continue;
                    }
                    let d = ap - t * ab;
                    if d.dot(d) < TJUNCTION_EPS * TJUNCTION_EPS {
                        on_edge.push((t, *p));
                    }
                }
                on_edge.sort_by(|x, y| x.0.total_cmp(&y.0));
                refined.extend(on_edge.iter().map(|&(_, p)| p));
            }
            &refined
        };
        if verts.len() > poly.verts.len() {
            // Points were stitched in. A vertex fan would emit exactly
            // degenerate triangles (fan origin collinear with an inserted
            // run), whose zero normal turns into NaN in the STEP writer —
            // fan from the centroid instead so every triangle has area.
            // The spoke edges pair up inside the polygon's own fan, so
            // watertightness is unaffected.
            let center = Point3::from_vec(
                verts.iter().map(|v| v.to_vec()).sum::<Vec3>() / verts.len() as f64,
            );
            let o = push(&center, &mut mesh, &mut reps);
            for k in 0..verts.len() {
                let a = push(&verts[k], &mut mesh, &mut reps);
                let b = push(&verts[(k + 1) % verts.len()], &mut mesh, &mut reps);
                if o != a && a != b && o != b {
                    mesh.indices.push(o);
                    mesh.indices.push(a);
                    mesh.indices.push(b);
                }
            }
        } else {
            for k in 1..verts.len().saturating_sub(1) {
                let a = push(&verts[0], &mut mesh, &mut reps);
                let b = push(&verts[k], &mut mesh, &mut reps);
                let c = push(&verts[k + 1], &mut mesh, &mut reps);
                if a != b && b != c && a != c {
                    mesh.indices.push(a);
                    mesh.indices.push(b);
                    mesh.indices.push(c);
                }
            }
        }
    }
    (mesh, reps)
}

fn polygons_to_mesh(polys: &[Polygon]) -> TriangleMesh {
    let (mut mesh, mut reps) = triangulate(polys, &[]);
    // Heal t-junctions: AABB-localized splitting legitimately leaves one
    // side of a shared edge split where the other is not, opening hairline
    // cracks. Every crack vertex is an endpoint of some open boundary edge,
    // so re-triangulate with those points stitched into any edge they lie
    // on. Iterate: an insertion can expose a finer junction on the next
    // pass. Structure stays advisory for *validity* (see validate.rs), but
    // consumers (and the torture track) reasonably prefer watertight output.
    let mut open = mesh.boundary_edges();
    for _ in 0..3 {
        if open.is_empty() {
            break;
        }
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut stitch: Vec<Point3> = Vec::new();
        for &(a, b) in &open {
            for idx in [a, b] {
                if seen.insert(idx) {
                    stitch.push(reps[idx as usize]);
                }
            }
        }
        let (healed, healed_reps) = triangulate(polys, &stitch);
        let healed_open = healed.boundary_edges();
        if healed_open.len() >= open.len() {
            break;
        }
        mesh = healed;
        reps = healed_reps;
        open = healed_open;
    }
    if !open.is_empty() {
        snap_boundary_vertices(&mut mesh, &reps, &open);
        let open = mesh.boundary_edges();
        if !open.is_empty() {
            fill_small_holes(&mut mesh, &reps, &open);
        }
    }
    collapse_degenerate_triangles(&mut mesh);
    collapse_sliver_triangles(&mut mesh);
    drop_coincident_internal_faces(&mut mesh);
    for _ in 0..3 {
        let before = mesh.indices.len();
        drop_four_use_slivers(&mut mesh);
        collapse_sliver_triangles(&mut mesh);
        if mesh.indices.len() == before {
            break;
        }
    }
    // Sliver collapse can reopen a sub-mm hole the first fill missed
    // (measured: a 0.78 mm pentagon on a 64-segment helical slot). Cap
    // whatever boundary remains, using the post-collapse vertex buffer.
    close_hairline_holes(&mut mesh);
    mesh
}

/// Cap residual sub-mm boundary loops. Used after vertex projection, which
/// can reopen a pinhole that `polygons_to_mesh` had already closed, and
/// after welding so slicer-visible T-junctions become real holes we can
/// fill.
pub(crate) fn close_hairline_holes(mesh: &mut TriangleMesh) {
    weld_coincident(mesh);
    collapse_degenerate_triangles(mesh);
    let open = mesh.boundary_edges();
    if !open.is_empty() {
        let reps = mesh_vertex_reps(mesh);
        fill_small_holes(mesh, &reps, &open);
        collapse_degenerate_triangles(mesh);
    }
    drop_four_use_slivers(mesh);
    collapse_sliver_triangles(mesh);
}

/// Merge vertices that share a 0.001 mm lattice cell: the same criterion
/// [`TriangleMesh::welded_defective_edge_count`] uses. Near-coincident
/// corners from plane splits otherwise look closed by index and open to a
/// slicer.
fn weld_coincident(mesh: &mut TriangleMesh) {
    let n = mesh.vertices.len() / 3;
    if n == 0 {
        return;
    }
    let key = |i: usize| -> [i64; 3] {
        [
            (mesh.vertices[3 * i] as f64 * 1000.0).round() as i64,
            (mesh.vertices[3 * i + 1] as f64 * 1000.0).round() as i64,
            (mesh.vertices[3 * i + 2] as f64 * 1000.0).round() as i64,
        ]
    };
    let copy_normals = !mesh.normals.is_empty() && mesh.normals.len() == mesh.vertices.len();
    let mut remap = vec![0u32; n];
    let mut dedup: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut new_vertices: Vec<f32> = Vec::with_capacity(mesh.vertices.len());
    let mut new_normals: Vec<f32> =
        Vec::with_capacity(if copy_normals { mesh.normals.len() } else { 0 });
    for (i, slot) in remap.iter_mut().enumerate() {
        let idx = *dedup.entry(key(i)).or_insert_with(|| {
            let ni = (new_vertices.len() / 3) as u32;
            new_vertices.extend_from_slice(&mesh.vertices[3 * i..3 * i + 3]);
            if copy_normals {
                new_normals.extend_from_slice(&mesh.normals[3 * i..3 * i + 3]);
            }
            ni
        });
        *slot = idx;
    }
    if new_vertices.len() == mesh.vertices.len() {
        return;
    }
    let mut new_indices = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks(3) {
        let a = remap[tri[0] as usize];
        let b = remap[tri[1] as usize];
        let c = remap[tri[2] as usize];
        if a != b && b != c && a != c {
            new_indices.extend_from_slice(&[a, b, c]);
        }
    }
    mesh.vertices = new_vertices;
    mesh.indices = new_indices;
    if copy_normals {
        mesh.normals = new_normals;
    } else {
        mesh.normals.clear();
    }
}

fn mesh_vertex_reps(mesh: &TriangleMesh) -> Vec<Point3> {
    (0..mesh.vertices.len() / 3)
        .map(|i| {
            Point3::new(
                mesh.vertices[3 * i] as f64,
                mesh.vertices[3 * i + 1] as f64,
                mesh.vertices[3 * i + 2] as f64,
            )
        })
        .collect()
}

/// Drop pairs of coincident triangles with opposite winding.
///
/// Mesh CSG can keep both sides of a coplanar overlap as an internal
/// face: two triangles occupy the same three vertices and cancel in
/// signed volume, but a slicer sees a non-manifold edge (4 uses) or a
/// zero-thickness internal sheet. Hand-rolled meshes hit this constantly
/// at construction-body seams; a real CSG result must not.
fn drop_coincident_internal_faces(mesh: &mut TriangleMesh) {
    if mesh.indices.len() < 6 {
        return;
    }
    let vkey = |i: u32| -> [i64; 3] {
        let k = i as usize * 3;
        [
            (mesh.vertices[k] as f64 * 1000.0).round() as i64,
            (mesh.vertices[k + 1] as f64 * 1000.0).round() as i64,
            (mesh.vertices[k + 2] as f64 * 1000.0).round() as i64,
        ]
    };
    let sort_key = |a: [i64; 3], b: [i64; 3], c: [i64; 3]| -> ([[i64; 3]; 3], i8) {
        let mut pts = [a, b, c];
        let mut sign: i8 = 1;
        if pts[0] > pts[1] {
            pts.swap(0, 1);
            sign = -sign;
        }
        if pts[1] > pts[2] {
            pts.swap(1, 2);
            sign = -sign;
        }
        if pts[0] > pts[1] {
            pts.swap(0, 1);
            sign = -sign;
        }
        if pts[0] == pts[1] || pts[1] == pts[2] || pts[0] == pts[2] {
            (pts, 0)
        } else {
            (pts, sign)
        }
    };

    let n = mesh.indices.len() / 3;
    let mut keys: Vec<([[i64; 3]; 3], i8)> = Vec::with_capacity(n);
    let mut counts: std::collections::HashMap<[[i64; 3]; 3], (u32, u32)> =
        std::collections::HashMap::new();
    for t in 0..n {
        let a = vkey(mesh.indices[3 * t]);
        let b = vkey(mesh.indices[3 * t + 1]);
        let c = vkey(mesh.indices[3 * t + 2]);
        let (key, sign) = sort_key(a, b, c);
        keys.push((key, sign));
        let entry = counts.entry(key).or_insert((0, 0));
        if sign > 0 {
            entry.0 += 1;
        } else if sign < 0 {
            entry.1 += 1;
        }
    }

    let mut keep_budget: std::collections::HashMap<[[i64; 3]; 3], (u32, u32)> =
        std::collections::HashMap::new();
    for (key, (plus, minus)) in counts {
        // Opposite coincident pair: drop both. Same-winding duplicates:
        // keep one.
        let (kp, km) = {
            let paired = plus.min(minus);
            ((plus - paired).min(1), (minus - paired).min(1))
        };
        keep_budget.insert(key, (kp, km));
    }

    let mut indices = Vec::with_capacity(mesh.indices.len());
    for (t, &(key, sign)) in keys.iter().enumerate() {
        let budget = keep_budget.get_mut(&key);
        let keep = match (budget, sign) {
            (Some(b), s) if s > 0 && b.0 > 0 => {
                b.0 -= 1;
                true
            }
            (Some(b), s) if s < 0 && b.1 > 0 => {
                b.1 -= 1;
                true
            }
            _ => false,
        };
        if keep {
            indices.extend_from_slice(&mesh.indices[3 * t..3 * t + 3]);
        }
    }
    mesh.indices = indices;
}

/// Weld away triangles whose f32-stored vertices are (near-)collinear.
/// Downstream consumers derive per-face planes from the stored coordinates
/// (`mesh_to_brep`, the STEP writer) and a zero-area triangle normalizes to
/// a NaN normal — which the STEP writer then emits verbatim, producing a
/// file that cannot be re-imported. Collapsing the triangle's shortest
/// edge removes it while keeping the neighborhood watertight; the shift is
/// bounded by the sliver's own size.
fn collapse_degenerate_triangles(mesh: &mut TriangleMesh) {
    collapse_small_triangles(mesh, 1e-24);
}

/// Collapse triangles whose area is below a feature-scale sliver threshold.
///
/// `collapse_degenerate_triangles` only catches numerically zero-area
/// faces (NaN normals). Mesh CSG of helical cuts routinely leaves
/// 1e-4 mm² slivers that sit on an edge with four uses: closed, but not
/// a manifold. Collapsing the shortest edge of those slivers drops them
/// while the neighbourhood stays watertight.
fn collapse_sliver_triangles(mesh: &mut TriangleMesh) {
    // area = 0.5 |cross|; 1e-3 mm² → |cross|² = 4e-6. A 1 mm × 0.002 mm
    // sliver is still far below feature scale on printed parts.
    collapse_small_triangles(mesh, 4e-6);
}

fn collapse_small_triangles(mesh: &mut TriangleMesh, min_cross2: f64) {
    for _ in 0..4 {
        let v = |i: u32| {
            let k = i as usize * 3;
            Point3::new(
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for tri in mesh.indices.chunks(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            let (pa, pb, pc) = (v(a), v(b), v(c));
            let cross = (pb - pa).cross(pc - pa);
            if cross.dot(cross) > min_cross2 {
                continue;
            }
            // Merge the two closest corners, always dropping the higher
            // index into the lower so remap chains only ever descend —
            // no cycles, and resolve() reaches a fixpoint.
            let pairs = [(a, b, pb - pa), (b, c, pc - pb), (c, a, pa - pc)];
            if let Some(&(x, y, _)) = pairs
                .iter()
                .min_by(|x, y| x.2.dot(x.2).total_cmp(&y.2.dot(y.2)))
            {
                let (keep, drop) = (x.min(y), x.max(y));
                let entry = remap.entry(drop).or_insert(keep);
                *entry = (*entry).min(keep);
            }
        }
        if remap.is_empty() {
            return;
        }
        // Resolve chains (b→a, c→b) so every index maps to a terminal.
        let resolve = |mut i: u32| {
            while let Some(&j) = remap.get(&i) {
                if j >= i {
                    break;
                }
                i = j;
            }
            i
        };
        let mut indices = Vec::with_capacity(mesh.indices.len());
        for tri in mesh.indices.chunks(3) {
            let (a, b, c) = (resolve(tri[0]), resolve(tri[1]), resolve(tri[2]));
            if a != b && b != c && a != c {
                indices.extend_from_slice(&[a, b, c]);
            }
        }
        mesh.indices = indices;
    }
}

/// Drop the two smallest triangles on any 4-use edge when those two are
/// slivers. Four triangles on one edge is the leftover of a collapsed
/// internal face; the two larger triangles are the surface.
fn drop_four_use_slivers(mesh: &mut TriangleMesh) {
    let n = mesh.indices.len() / 3;
    if n < 4 {
        return;
    }
    let vkey = |i: u32| -> [i64; 3] {
        let k = i as usize * 3;
        [
            (mesh.vertices[k] as f64 * 1000.0).round() as i64,
            (mesh.vertices[k + 1] as f64 * 1000.0).round() as i64,
            (mesh.vertices[k + 2] as f64 * 1000.0).round() as i64,
        ]
    };
    let area = |t: usize| -> f64 {
        let i0 = mesh.indices[3 * t] as usize * 3;
        let i1 = mesh.indices[3 * t + 1] as usize * 3;
        let i2 = mesh.indices[3 * t + 2] as usize * 3;
        let ax = mesh.vertices[i0] as f64;
        let ay = mesh.vertices[i0 + 1] as f64;
        let az = mesh.vertices[i0 + 2] as f64;
        let bx = mesh.vertices[i1] as f64 - ax;
        let by = mesh.vertices[i1 + 1] as f64 - ay;
        let bz = mesh.vertices[i1 + 2] as f64 - az;
        let cx = mesh.vertices[i2] as f64 - ax;
        let cy = mesh.vertices[i2 + 1] as f64 - ay;
        let cz = mesh.vertices[i2 + 2] as f64 - az;
        let nx = by * cz - bz * cy;
        let ny = bz * cx - bx * cz;
        let nz = bx * cy - by * cx;
        0.5 * (nx * nx + ny * ny + nz * nz).sqrt()
    };
    let mut adj: std::collections::HashMap<([i64; 3], [i64; 3]), Vec<usize>> =
        std::collections::HashMap::new();
    for t in 0..n {
        let tri = [
            mesh.indices[3 * t],
            mesh.indices[3 * t + 1],
            mesh.indices[3 * t + 2],
        ];
        for i in 0..3 {
            let a = vkey(tri[i]);
            let b = vkey(tri[(i + 1) % 3]);
            if a == b {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            adj.entry(key).or_default().push(t);
        }
    }
    let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for tris in adj.values() {
        let mut ranked: Vec<(f64, usize)> = tris
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .map(|t| (area(t), t))
            .collect();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        match ranked.len() {
            3 if ranked[0].0 < 0.01 => {
                drop.insert(ranked[0].1);
            }
            4 if ranked[1].0 < 0.01 || (ranked[2].0 > 0.0 && ranked[1].0 < ranked[2].0 * 0.1) => {
                drop.insert(ranked[0].1);
                drop.insert(ranked[1].1);
            }
            n if n > 4 && ranked[0].0 < 0.01 => {
                drop.insert(ranked[0].1);
            }
            _ => {}
        }
    }
    if drop.is_empty() {
        return;
    }
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for t in 0..n {
        if !drop.contains(&t) {
            indices.extend_from_slice(&mesh.indices[3 * t..3 * t + 3]);
        }
    }
    mesh.indices = indices;
}

/// Last-resort closure for cracks stitching can't reach: near-coincident
/// boundary vertex pairs (e.g. a sliver fragment dropped by `Polygon::new`
/// leaving a hairline hole). Merge boundary vertices closer than the
/// stitch tolerance and drop the triangles that degenerate; the paired
/// boundary edges then cancel. Only boundary vertices move, so closed
/// regions of the mesh are untouched.
fn snap_boundary_vertices(mesh: &mut TriangleMesh, reps: &[Point3], open: &[(u32, u32)]) {
    let mut verts: Vec<u32> = open.iter().flat_map(|&(a, b)| [a, b]).collect();
    verts.sort_unstable();
    verts.dedup();
    // Map each boundary vertex to the lowest-index boundary vertex within
    // tolerance (tiny sets: a handful of edges by the time we get here).
    let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for (i, &vi) in verts.iter().enumerate() {
        if remap.contains_key(&vi) {
            continue;
        }
        for &vj in &verts[i + 1..] {
            if remap.contains_key(&vj) {
                continue;
            }
            let d = reps[vj as usize] - reps[vi as usize];
            if d.dot(d) < TJUNCTION_EPS * TJUNCTION_EPS {
                remap.insert(vj, vi);
            }
        }
    }
    if remap.is_empty() {
        return;
    }
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks(3) {
        let m = |i: u32| *remap.get(&i).unwrap_or(&i);
        let (a, b, c) = (m(tri[0]), m(tri[1]), m(tri[2]));
        if a != b && b != c && a != c {
            indices.extend_from_slice(&[a, b, c]);
        }
    }
    mesh.indices = indices;
}

/// Hairline hole perimeter (mm) below which a boundary loop is capped
/// outright. Holes this small come from sliver fragments dropped during
/// splitting (degenerate `Polygon::new` rejections), never from real
/// geometry at torture-track feature scales. A 2 mm perimeter is a
/// ~0.6 mm equivalent diameter: below a typical FDM nozzle, far below
/// the 4 mm helical-slot width.
const HOLE_PERIMETER_EPS: f64 = 2.0;

/// Cap tiny boundary loops with a triangle fan. Uses *undirected* loops so
/// a hole whose triangles disagree on winding (indegree-2 at one vertex)
/// still fills. The fan is wound opposite any adjacent surface triangle.
fn fill_small_holes(mesh: &mut TriangleMesh, reps: &[Point3], open: &[(u32, u32)]) {
    if open.len() < 3 {
        return;
    }
    let mut open: Vec<(u32, u32)> = open.to_vec();
    open.sort_unstable();
    let mut adj: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for &(a, b) in &open {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }
    for nbrs in adj.values_mut() {
        nbrs.sort_unstable();
        nbrs.dedup();
    }
    let ek = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    let mut visited: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut caps: Vec<[u32; 3]> = Vec::new();
    for &(start_a, start_b) in &open {
        if visited.contains(&ek(start_a, start_b)) {
            continue;
        }
        visited.insert(ek(start_a, start_b));
        let mut chain = vec![start_a, start_b];
        let mut closed = false;
        loop {
            let cur = *chain.last().unwrap();
            let prev = chain[chain.len() - 2];
            let next = adj.get(&cur).and_then(|nbrs| {
                nbrs.iter()
                    .copied()
                    .find(|&n| n != prev && !visited.contains(&ek(cur, n)))
            });
            match next {
                Some(n) => {
                    visited.insert(ek(cur, n));
                    if n == start_a {
                        closed = true;
                        break;
                    }
                    chain.push(n);
                    if chain.len() > 16 {
                        break;
                    }
                }
                None => break,
            }
        }
        if !closed || chain.len() < 3 {
            continue;
        }
        let mut perimeter = 0.0;
        for i in 0..chain.len() {
            let a = chain[i];
            let b = chain[(i + 1) % chain.len()];
            perimeter += (reps[b as usize] - reps[a as usize]).norm();
        }
        if perimeter > HOLE_PERIMETER_EPS {
            continue;
        }
        // Existing surface triangle using a chain edge a→b means the cap
        // must use b→a (opposite winding). Reverse the chain if needed.
        let mut reverse = false;
        'orient: for i in 0..chain.len() {
            let a = chain[i];
            let b = chain[(i + 1) % chain.len()];
            for tri in mesh.indices.chunks(3) {
                for k in 0..3 {
                    if tri[k] == a && tri[(k + 1) % 3] == b {
                        reverse = true;
                        break 'orient;
                    }
                }
            }
        }
        if reverse {
            chain.reverse();
        }
        for i in 1..chain.len() - 1 {
            caps.push([chain[0], chain[i], chain[i + 1]]);
        }
    }
    for cap in caps {
        if cap[0] != cap[1] && cap[1] != cap[2] && cap[0] != cap[2] {
            mesh.indices.extend_from_slice(&cap);
        }
    }
}

/// Boolean of two closed triangle meshes. Returns the result surface as a
/// triangle mesh; empty results give an empty mesh.
pub fn mesh_csg(mesh_a: &TriangleMesh, mesh_b: &TriangleMesh, op: BooleanOp) -> TriangleMesh {
    let pa = mesh_polygons(mesh_a);
    let pb = mesh_polygons(mesh_b);
    let frags_a = split_by_other(pa.clone(), &pb);
    let frags_b = split_by_other(pb, &pa);

    let keep_a = |c: Class| match op {
        BooleanOp::Union => matches!(c, Class::Out | Class::OnAligned),
        BooleanOp::Intersection => matches!(c, Class::In | Class::OnAligned),
        BooleanOp::Difference => matches!(c, Class::Out | Class::OnOpposed),
    };
    let keep_b = |c: Class| match op {
        BooleanOp::Union => matches!(c, Class::Out),
        BooleanOp::Intersection => matches!(c, Class::In),
        BooleanOp::Difference => matches!(c, Class::In),
    };

    let mut out: Vec<Polygon> = frags_a
        .into_iter()
        .filter(|f| keep_a(classify(f, mesh_b)))
        .collect();
    for mut f in frags_b {
        if keep_b(classify(&f, mesh_a)) {
            if op == BooleanOp::Difference {
                // Kept B fragments bound the carved cavity; they face
                // inward in the result.
                f.flip();
            }
            out.push(f);
        }
    }
    // `polygons_to_mesh` already heals t-junctions, snaps near-coincident
    // boundary vertices, caps residual pinholes and collapses degenerate
    // triangles; all that remains is to pin the global orientation.
    orient_outward(polygons_to_mesh(&out))
}

/// Pin the global orientation: a bounded solid — outer shells minus any
/// enclosed voids — always has positive signed volume, so a negative total
/// means every fragment is wound inside-out and flipping all of them is the
/// unique correction.
///
/// Needed because fragment orientation is inherited from the operand
/// tessellations, and a few configurations (measured: a torus-like
/// intersection whose kept fragments came out at −28.42 against a
/// Monte-Carlo truth of +28.84) invert wholesale. Ray-parity checks cannot
/// catch this — crossing counts are orientation-blind — so signed volume is
/// the only available guard. This cannot repair a *partially* inconsistent
/// surface; that shows up downstream as a watertightness or volume failure
/// rather than being silently accepted.
fn orient_outward(mut mesh: TriangleMesh) -> TriangleMesh {
    let mut vol = 0.0_f64;
    for tri in mesh.indices.chunks(3) {
        let p = |k: usize| {
            let i = tri[k] as usize * 3;
            [
                mesh.vertices[i] as f64,
                mesh.vertices[i + 1] as f64,
                mesh.vertices[i + 2] as f64,
            ]
        };
        let (a, b, c) = (p(0), p(1), p(2));
        vol += a[0] * (b[1] * c[2] - c[1] * b[2]) - b[0] * (a[1] * c[2] - c[1] * a[2])
            + c[0] * (a[1] * b[2] - b[1] * a[2]);
    }
    if vol < 0.0 {
        for tri in mesh.indices.chunks_mut(3) {
            tri.swap(1, 2);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;
    use vcad_kernel_tessellate::tessellate_brep;

    fn volume(mesh: &TriangleMesh) -> f64 {
        let mut vol = 0.0_f64;
        for tri in mesh.indices.chunks(3) {
            let p = |k: usize| {
                let i = tri[k] as usize * 3;
                [
                    mesh.vertices[i] as f64,
                    mesh.vertices[i + 1] as f64,
                    mesh.vertices[i + 2] as f64,
                ]
            };
            let (v0, v1, v2) = (p(0), p(1), p(2));
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        vol / 6.0
    }

    fn shifted(mesh: &TriangleMesh, d: [f32; 3]) -> TriangleMesh {
        let mut m = mesh.clone();
        for v in m.vertices.chunks_mut(3) {
            v[0] += d[0];
            v[1] += d[1];
            v[2] += d[2];
        }
        m
    }

    #[test]
    fn overlapping_cubes_difference() {
        let a = tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16);
        let b = shifted(
            &tessellate_brep(&make_cube(4.0, 4.0, 12.0), 16),
            [3.0, 3.0, -1.0],
        );
        let vol = volume(&mesh_csg(&a, &b, BooleanOp::Difference));
        assert!((vol - 840.0).abs() < 1.0, "expected 840, got {vol}");
    }

    #[test]
    fn overlapping_cubes_union_and_intersection() {
        let a = tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16);
        let b = shifted(
            &tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16),
            [5.0, 0.0, 0.0],
        );
        let uni = volume(&mesh_csg(&a, &b, BooleanOp::Union));
        assert!(
            (uni - 1500.0).abs() < 1.0,
            "union: expected 1500, got {uni}"
        );
        let inter = volume(&mesh_csg(&a, &b, BooleanOp::Intersection));
        assert!(
            (inter - 500.0).abs() < 1.0,
            "intersection: expected 500, got {inter}"
        );
    }

    #[test]
    fn identical_cubes() {
        let a = tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16);
        let uni = volume(&mesh_csg(&a, &a.clone(), BooleanOp::Union));
        assert!((uni - 1000.0).abs() < 1.0, "self-union: {uni}");
        let diff = volume(&mesh_csg(&a, &a.clone(), BooleanOp::Difference));
        assert!(diff.abs() < 1.0, "self-difference should be empty: {diff}");
    }

    #[test]
    fn boundary_coplanar_slot() {
        // Full-height slot spanning the cube's footprint in x and z: five
        // of the tool's six faces are coplanar with the target's.
        let a = tessellate_brep(&make_cube(80.0, 60.0, 29.5), 16);
        let b = shifted(
            &tessellate_brep(&make_cube(80.0, 18.0, 29.5), 16),
            [0.0, 42.0, 0.0],
        );
        let out = mesh_csg(&a, &b, BooleanOp::Difference);
        let vol = volume(&out);
        assert!((vol - 99120.0).abs() < 5.0, "expected 99120, got {vol}");
        assert_eq!(
            out.welded_defective_edge_count(),
            0,
            "slot difference must be a closed manifold"
        );
    }
}
