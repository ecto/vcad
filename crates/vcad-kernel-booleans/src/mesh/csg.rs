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
use vcad_kernel_tessellate::manifold::{make_manifold, DEFAULT_WELD_EPS};
use vcad_kernel_tessellate::TriangleMesh;

use crate::api::BooleanOp;
use crate::mesh::MeshRayIndex;

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

/// Point membership in one operand by ray parity, voted across three ray
/// directions.
///
/// A single ray is only as good as the mesh it crosses, and operands are not
/// always closed: an analytic solid carrying a tangent-contact defect
/// tessellates with a full-height crack (the rana-60 stator's posts, where a
/// fillet block meets the post's side), and a chained fallback hands in
/// t-junction soup. Every probe whose one +X ray threads such a crack reads
/// the wrong parity — measured on the stator, 35 mm² of the ring's top cap,
/// nowhere near the posts, classified as lying on them and was dropped: the
/// union came out 1600 mm³ short, inside its own volume bound.
///
/// The ray direction is fixed inside `mesh_ray` (its exact predicates and its
/// index are built around +X), so the other two directions are had by
/// cyclically permuting coordinates — a proper rotation, so orientation and
/// parity semantics carry over unchanged. A crack fools the rays that cross
/// it; it takes cracks on two of three mutually perpendicular lines through
/// the same probe to fool the vote. On a closed mesh all three agree, so the
/// answer is the single-ray answer.
struct Membership {
    /// The operand with coordinates cyclically shifted 0, 1 and 2 places.
    views: [TriangleMesh; 3],
}

impl Membership {
    fn new(mesh: &TriangleMesh) -> Self {
        let shifted = |k: usize| {
            let mut m = mesh.clone();
            for v in m.vertices.as_chunks_mut::<3>().0 {
                v.rotate_left(k);
            }
            m
        };
        Membership {
            views: [shifted(0), shifted(1), shifted(2)],
        }
    }

    fn index(&self) -> [MeshRayIndex<'_>; 3] {
        [
            MeshRayIndex::new(&self.views[0]),
            MeshRayIndex::new(&self.views[1]),
            MeshRayIndex::new(&self.views[2]),
        ]
    }
}

/// Majority vote of the three rays for `p`.
fn contains(index: &[MeshRayIndex<'_>; 3], p: &Point3) -> bool {
    let votes = [
        index[0].contains(p),
        index[1].contains(&Point3::new(p.y, p.z, p.x)),
        index[2].contains(&Point3::new(p.z, p.x, p.y)),
    ];
    votes.iter().filter(|&&v| v).count() >= 2
}

/// Classify a fragment against the other operand by ray parity at two probes
/// nudged off the fragment along ±normal.
fn classify(frag: &Polygon, other: &[MeshRayIndex<'_>; 3]) -> Class {
    if other[0].mesh().indices.is_empty() {
        return Class::Out;
    }
    let c = frag.centroid();
    let n = frag.normal;
    let eps = probe_offset(frag);
    let plus = contains(other, &(c + eps * n));
    let minus = contains(other, &(c - eps * n));
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
    cancel_duplicate_triangles(&mut mesh);
    mesh
}

/// Cancel coincident triangles left by the split/classify pass.
///
/// A coplanar contact between the operands can survive classification
/// twice — once from each operand's fragment — and an opposed pair is a
/// ZERO-THICKNESS FLAP: it adds nothing to the volume (which is why the
/// volume oracle waves it through) but every edge it touches ends up
/// shared by four triangles. Slicers call that a non-manifold edge, and
/// auto-repair resolves it by filling: a printed rotor came back with its
/// shaft bore solid.
///
/// Same-orientation duplicates are simply redundant copies of one facet.
/// Both are removed by keeping, for each vertex triple, |forward −
/// backward| copies of whichever orientation dominates.
fn cancel_duplicate_triangles(mesh: &mut TriangleMesh) {
    let mut seen: std::collections::HashMap<[u32; 3], (i32, usize)> =
        std::collections::HashMap::new();
    for (t, tri) in mesh.indices.chunks(3).enumerate() {
        let mut key = [tri[0], tri[1], tri[2]];
        key.sort_unstable();
        // Orientation relative to the sorted key: even permutation = +1.
        let sign =
            if (tri[0] < tri[1]) as u8 + (tri[1] < tri[2]) as u8 + (tri[0] < tri[2]) as u8 == 2 {
                1
            } else {
                -1
            };
        let e = seen.entry(key).or_insert((0, t));
        e.0 += sign;
    }
    if seen.values().all(|&(net, _)| net.abs() == 1) {
        return;
    }
    let mut out = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks(3) {
        let mut key = [tri[0], tri[1], tri[2]];
        key.sort_unstable();
        let sign =
            if (tri[0] < tri[1]) as u8 + (tri[1] < tri[2]) as u8 + (tri[0] < tri[2]) as u8 == 2 {
                1
            } else {
                -1
            };
        let Some(slot) = seen.get_mut(&key) else {
            continue;
        };
        // Emit while this orientation still has an uncancelled surplus.
        if slot.0 * sign > 0 {
            slot.0 -= sign;
            out.extend_from_slice(tri);
        }
    }
    mesh.indices = out;
}

/// Weld away triangles whose f32-stored vertices are (near-)collinear.
/// Downstream consumers derive per-face planes from the stored coordinates
/// (`mesh_to_brep`, the STEP writer) and a zero-area triangle normalizes to
/// a NaN normal — which the STEP writer then emits verbatim, producing a
/// file that cannot be re-imported. Collapsing the triangle's shortest
/// edge removes it while keeping the neighborhood watertight; the shift is
/// bounded by the sliver's own size.
fn collapse_degenerate_triangles(mesh: &mut TriangleMesh) {
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
            if cross.dot(cross) > 1e-24 {
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
/// geometry at torture-track feature scales.
const HOLE_PERIMETER_EPS: f64 = 0.5;

/// Cap tiny boundary loops with a triangle fan. Boundary edges are chained
/// into directed loops (a hole traverses each missing directed edge), and
/// any loop short enough to be a dropped-sliver artifact is filled.
fn fill_small_holes(mesh: &mut TriangleMesh, reps: &[Point3], open: &[(u32, u32)]) {
    let openset: std::collections::HashSet<(u32, u32)> = open.iter().copied().collect();
    // The surface contains directed edge (a, b) exactly once; the cap must
    // supply (b, a). Chain successor b → a; a duplicate key means a
    // non-manifold boundary vertex — leave those loops alone.
    let mut succ: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut bad: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for tri in mesh.indices.chunks(3) {
        for k in 0..3 {
            let a = tri[k];
            let b = tri[(k + 1) % 3];
            if openset.contains(&(a.min(b), a.max(b))) && succ.insert(b, a).is_some() {
                bad.insert(b);
            }
        }
    }
    let mut done: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // Sorted, not hash order: which loop a chain is walked from decides
    // which loops get capped when two share a vertex, so an unsorted
    // `succ.keys()` made the output differ run to run. Callers diff STLs
    // across regenerations, so the cap order has to be a function of the
    // input alone.
    let mut starts: Vec<u32> = succ.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        if done.contains(&start) {
            continue;
        }
        let mut loop_verts = vec![start];
        let mut perimeter = 0.0;
        let mut ok = false;
        let mut cur = start;
        while let Some(&next) = succ.get(&cur) {
            if bad.contains(&cur) || loop_verts.len() > 16 {
                break;
            }
            perimeter += (reps[next as usize] - reps[cur as usize]).norm();
            if next == start {
                ok = true;
                break;
            }
            loop_verts.push(next);
            cur = next;
        }
        for &v in &loop_verts {
            done.insert(v);
        }
        if !ok || loop_verts.len() < 3 || perimeter > HOLE_PERIMETER_EPS {
            continue;
        }
        for i in 1..loop_verts.len() - 1 {
            mesh.indices
                .extend_from_slice(&[loop_verts[0], loop_verts[i], loop_verts[i + 1]]);
        }
    }
}

/// Smallest coplanar region worth re-merging.
///
/// Below this the split debris is negligible and the merge only risks
/// perturbing a well-formed patch. Sixty-four is about what one cap
/// triangle reaches after a couple of silhouettes have crossed it; the
/// debris this pass exists for runs to thousands on a single cap.
const MERGE_MIN_REGION_TRIS: usize = 64;

/// How far a fragment's vertices may sit off the region's seed plane and
/// still count as lying in it (mm). Matches [`EPS`], the tolerance the
/// splitter itself used to decide which side of a carrier a vertex was on
/// — anything it treated as coplanar is coplanar here. Vertices are stored
/// as `f32`, so the floor is ~2e-6 mm at part scale; 1e-5 clears that
/// without reaching any feature.
const MERGE_PLANE_EPS: f64 = EPS;

/// Re-merge and re-triangulate coplanar regions left by splitting.
///
/// [`split_by_other`] cuts every operand triangle by the carrier planes of
/// the other operand's triangles and never puts the pieces back, so a cap
/// the two operands share comes out as a fan of slivers. That is merely
/// untidy for one boolean and fatal for a chain: the next boolean re-splits
/// the debris and the one after that re-splits *that*. The rana-60 stator's
/// authored 50-step fold (every operand spanning z 11.1..17.1) had not
/// finished after nine minutes, at 2.5 GB and climbing; with the merge it
/// finishes in 66 s at 122 587 triangles. Folding the stator's twelve posts
/// into its ring one at a time goes from 6 146 triangles (and rising with
/// every step) to 2 740 (falling), and the one-shot union of the same
/// twelve from 10 160 to 2 922, with the volume unchanged at 7 406.98 mm³
/// against a closed form of 7 407.22.
///
/// Every boundary vertex survives: a region-boundary edge is shared with a
/// triangle on some other plane, and dropping one of its endpoints would
/// open a t-junction there. Only vertices interior to the region — used by
/// no triangle outside it — disappear, and with them nothing the quadric
/// projector could have pinned (`mesh_fallback` reads each vertex's
/// constraint from its incident triangle normals, and a planar
/// re-triangulation leaves every surviving vertex's incident normal SET
/// unchanged).
///
/// Fail-closed per region: the replacement is taken only when it reproduces
/// the region's SIGNED area in the plane — which is what fixes its
/// contribution to the volume — leaves exactly the region's own unpaired
/// directed edges, changes no edge the rest of the mesh shares, and mints
/// no triangle the rest of the mesh already has. Anything else (a doubled
/// cover, a pinched boundary, an earcut failure) keeps that region's
/// original triangles.
fn merge_coplanar_regions(mesh: &mut TriangleMesh) {
    let before = mesh.clone();
    merge_coplanar_regions_unchecked(mesh);
    if mesh.indices.len() == before.indices.len() {
        return;
    }
    // Whole-mesh backstop. Every region is checked on its own above, but
    // the checks are read against the ORIGINAL mesh, and two regions that
    // meet along a branching edge each judge the other's triangles to be
    // "outside" and immovable — so a pair of individually-sound merges can
    // still interact. Re-merging must not move the enclosed volume (it
    // re-cuts flat patches, nothing else) nor add a defect, and when it
    // does, the un-merged result is simply kept: this is an optimisation,
    // not a repair.
    let v0 = crate::validate::mesh_signed_volume(&before);
    let v1 = crate::validate::mesh_signed_volume(mesh);
    let moved = (v1 - v0).abs() > 1e-6 * v0.abs().max(1.0);
    if moved
        || mesh.boundary_edges().len() > before.boundary_edges().len()
        || mesh.non_manifold_edges().len() > before.non_manifold_edges().len()
    {
        *mesh = before;
    }
}

fn merge_coplanar_regions_unchecked(mesh: &mut TriangleMesh) {
    let ntri = mesh.indices.len() / 3;
    if ntri < MERGE_MIN_REGION_TRIS {
        return;
    }
    // Per-triangle tags would have to be carried through a merge that
    // replaces a whole region with new triangles; the only caller
    // (`mesh_csg`) produces an untagged mesh, so decline rather than
    // invent a provenance.
    if !mesh.face_kinds.is_empty() || !mesh.face_ids.is_empty() || !mesh.normals.is_empty() {
        return;
    }
    let corners = |mesh: &TriangleMesh, t: usize| -> [u32; 3] {
        [
            mesh.indices[t * 3],
            mesh.indices[t * 3 + 1],
            mesh.indices[t * 3 + 2],
        ]
    };
    let mut normal: Vec<Option<Vec3>> = Vec::with_capacity(ntri);
    for t in 0..ntri {
        let c = corners(mesh, t);
        let (a, b, d) = (vertex(mesh, c[0]), vertex(mesh, c[1]), vertex(mesh, c[2]));
        let n = (b - a).cross(d - a);
        let l = n.norm();
        normal.push((l > 1e-18).then(|| n / l));
    }

    // Undirected edge → triangles. Only an edge used by exactly two
    // triangles joins a region: a branching edge's neighbourhood is not a
    // surface patch, and merging across it would guess which sheet the
    // patch continues on.
    let mut edge_tris: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
    for t in 0..ntri {
        let c = corners(mesh, t);
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            if a != b {
                edge_tris.entry((a.min(b), a.max(b))).or_default().push(t);
            }
        }
    }

    // Every triangle by sorted vertex triple. A replacement must not mint
    // a triple that already exists elsewhere in the mesh: `make_manifold`
    // cancels a same-triple pair (opposed ones outright), so an accidental
    // collision deletes BOTH and leaves a hole.
    let mut tri_keys: std::collections::HashMap<[u32; 3], usize> = std::collections::HashMap::new();
    for t in 0..ntri {
        let mut k = corners(mesh, t);
        k.sort_unstable();
        *tri_keys.entry(k).or_default() += 1;
    }

    // Flood-fill regions. Membership is tested against the SEED's plane,
    // not the neighbour's, so a long patch cannot drift off-plane one
    // triangle at a time. Winding is deliberately NOT part of the test: an
    // opposed sliver in the middle of a patch is part of the same planar
    // mess, and excluding it punched a hole the re-triangulation then
    // refused — 38 of the 42 refusals on the twelve-post ring union, which
    // is why that union came out at 10 160 triangles instead of 2 922.
    let mut region = vec![usize::MAX; ntri];
    let mut members: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut replaced: Vec<Option<Vec<[u32; 3]>>> = vec![None; ntri];
    for seed in 0..ntri {
        let Some(n0) = normal[seed] else { continue };
        if region[seed] != usize::MAX {
            continue;
        }
        let p0 = vertex(mesh, corners(mesh, seed)[0]);
        let on_plane = |mesh: &TriangleMesh, t: usize| -> bool {
            corners(mesh, t)
                .iter()
                .all(|&v| (vertex(mesh, v) - p0).dot(n0).abs() <= MERGE_PLANE_EPS)
        };
        region[seed] = seed;
        members.clear();
        stack.clear();
        stack.push(seed);
        while let Some(u) = stack.pop() {
            members.push(u);
            let c = corners(mesh, u);
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                if a == b {
                    continue;
                }
                let Some(nb) = edge_tris.get(&(a.min(b), a.max(b))) else {
                    continue;
                };
                if nb.len() != 2 {
                    continue;
                }
                for &v in nb {
                    if v == u || region[v] != usize::MAX || normal[v].is_none() {
                        continue;
                    }
                    if on_plane(mesh, v) {
                        region[v] = seed;
                        stack.push(v);
                    }
                }
            }
        }
        if members.len() < MERGE_MIN_REGION_TRIS {
            continue;
        }
        members.sort_unstable();
        if let Some(tris) = retriangulate_region(mesh, &members, &n0, &edge_tris, &tri_keys) {
            replaced[members[0]] = Some(tris);
            for &t in &members[1..] {
                replaced[t] = Some(Vec::new());
            }
        }
    }
    if replaced.iter().all(|r| r.is_none()) {
        return;
    }
    let mut indices: Vec<u32> = Vec::with_capacity(mesh.indices.len());
    for (t, slot) in replaced.iter().enumerate() {
        match slot {
            Some(tris) => {
                for tri in tris {
                    indices.extend_from_slice(tri);
                }
            }
            None => indices.extend_from_slice(&mesh.indices[t * 3..t * 3 + 3]),
        }
    }
    mesh.indices = indices;
    compact_vertices(mesh);
}

fn vertex(mesh: &TriangleMesh, i: u32) -> Point3 {
    let k = i as usize * 3;
    Point3::new(
        mesh.vertices[k] as f64,
        mesh.vertices[k + 1] as f64,
        mesh.vertices[k + 2] as f64,
    )
}

/// Drop vertices no triangle references any more (the interiors the merge
/// dissolved), renumbering the survivors in their existing order.
fn compact_vertices(mesh: &mut TriangleMesh) {
    let nv = mesh.vertices.len() / 3;
    let mut used = vec![false; nv];
    for &i in &mesh.indices {
        used[i as usize] = true;
    }
    if used.iter().all(|&u| u) {
        return;
    }
    let mut remap = vec![u32::MAX; nv];
    let mut verts: Vec<f32> = Vec::with_capacity(mesh.vertices.len());
    for (v, &u) in used.iter().enumerate() {
        if u {
            remap[v] = (verts.len() / 3) as u32;
            verts.extend_from_slice(&mesh.vertices[v * 3..v * 3 + 3]);
        }
    }
    mesh.vertices = verts;
    for i in &mut mesh.indices {
        *i = remap[*i as usize];
    }
}

/// Smallest triangle area (mm²) the merge may emit.
///
/// `make_manifold` DROPS a triangle at or below `DEFAULT_WELD_EPS²`, and a
/// dropped triangle is a hole. Ear clipping mints exactly such triangles
/// wherever the region's boundary runs through collinear points — which it
/// routinely does, because a t-junction vertex imprinted by a neighbouring
/// face is collinear with the edge it was imprinted on. Matching the
/// threshold means anything the merge emits survives that pass.
const MERGE_MIN_TRI_AREA: f64 = 1e-8;

/// Fold away the zero-area ears ear clipping leaves on collinear boundary
/// runs.
///
/// For a degenerate triangle `(p, r, q)` whose middle vertex `r` sits on
/// `pq`, the neighbour across `q→p` is some `(p, q, d)`; together they
/// cover the same area as `(p, r, d)` + `(r, q, d)`, which keeps every
/// directed edge of the pair and gives both triangles real area. Returns
/// `false` when an ear cannot be folded — its long edge is on the region
/// boundary, or the fold would be degenerate as well — and the caller then
/// declines the region rather than shipping a hole.
fn fold_degenerate_ears(out: &mut [[u32; 3]], to_2d: &dyn Fn(u32) -> (f64, f64)) -> bool {
    let area = |t: &[u32; 3]| {
        let (a, b, c) = (to_2d(t[0]), to_2d(t[1]), to_2d(t[2]));
        0.5 * ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0))
    };
    let len2 = |a: u32, b: u32| {
        let (p, q) = (to_2d(a), to_2d(b));
        (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)
    };
    let flat = |t: &[u32; 3]| area(t).abs() <= MERGE_MIN_TRI_AREA;
    let mut remaining = out.iter().filter(|t| flat(t)).count();
    // Each accepted fold must remove at least one ear, so the loop cannot
    // cycle; the counter is belt and braces.
    for _ in 0..out.len() + 8 {
        if remaining == 0 {
            return true;
        }
        let Some(t) = out.iter().position(flat) else {
            return true;
        };
        let tri = out[t];
        // The middle vertex is the one opposite the longest edge, so the
        // edge to fold across is the one that does not touch it.
        let k = (0..3)
            .max_by(|&i, &j| {
                len2(tri[i], tri[(i + 1) % 3]).total_cmp(&len2(tri[j], tri[(j + 1) % 3]))
            })
            .unwrap_or(0);
        let (x, y, r) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
        // Neighbour across the long edge, found by its reverse. None means
        // the long edge is on the region's own boundary — nothing to fold
        // into.
        let Some(n) = (0..out.len())
            .find(|&i| i != t && (0..3).any(|j| out[i][j] == y && out[i][(j + 1) % 3] == x))
        else {
            return false;
        };
        let nb = out[n];
        let Some(d) = nb.iter().copied().find(|&v| v != x && v != y) else {
            return false;
        };
        if d == r {
            return false;
        }
        out[t] = [y, r, d];
        out[n] = [r, x, d];
        // A run of collinear boundary points gives ears that only unfold
        // one at a time, so a fold that leaves its own replacement flat is
        // still progress — as long as the total drops.
        let now = out.iter().filter(|t| flat(t)).count();
        if now >= remaining {
            out[t] = tri;
            out[n] = nb;
            return false;
        }
        remaining = now;
    }
    false
}

/// Re-triangulate one coplanar region from its boundary loops, or `None`
/// when the region does not admit a clean replacement.
fn retriangulate_region(
    mesh: &TriangleMesh,
    members: &[usize],
    n0: &Vec3,
    edge_tris: &std::collections::HashMap<(u32, u32), Vec<usize>>,
    tri_keys: &std::collections::HashMap<[u32; 3], usize>,
) -> Option<Vec<[u32; 3]>> {
    // Net directed edge use inside the region. A boundary edge is one
    // whose reverse the region does not supply. `uses` is the same tally
    // undirected, needed to keep edges the rest of the mesh also touches
    // at exactly their old use count.
    let mut net: std::collections::HashMap<(u32, u32), i32> = std::collections::HashMap::new();
    let mut uses: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    for &t in members {
        let c = [
            mesh.indices[t * 3],
            mesh.indices[t * 3 + 1],
            mesh.indices[t * 3 + 2],
        ];
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            if a == b {
                return None;
            }
            let (key, s) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *net.entry(key).or_default() += s;
            *uses.entry(key).or_default() += 1;
        }
    }
    // Chain the boundary. A vertex with two outgoing boundary edges means
    // the region pinches (two sub-patches meeting at a point) and its
    // loops are ambiguous — leave it alone.
    let mut succ: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut boundary: Vec<(u32, u32)> = Vec::new();
    for (&(a, b), &n) in &net {
        let (from, to) = match n {
            0 => continue,
            1 => (a, b),
            -1 => (b, a),
            _ => return None, // a doubled cover, not a patch
        };
        if succ.insert(from, to).is_some() {
            return None;
        }
        boundary.push((from, to));
    }
    if boundary.len() < 3 {
        return None;
    }
    // Deterministic walk order: `net` is a HashMap, and which loop a chain
    // is entered from would otherwise decide the vertex order earcut sees.
    boundary.sort_unstable();
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for &(start, _) in &boundary {
        if seen.contains(&start) {
            continue;
        }
        let mut lp = vec![start];
        seen.insert(start);
        let mut cur = start;
        loop {
            let next = *succ.get(&cur)?;
            if next == start {
                break;
            }
            if !seen.insert(next) {
                return None; // the chain runs into another loop
            }
            lp.push(next);
            cur = next;
            if lp.len() > boundary.len() {
                return None;
            }
        }
        if lp.len() < 3 {
            return None;
        }
        loops.push(lp);
    }

    // Project onto the region's plane with a right-handed basis, so a
    // counter-clockwise 2D winding is a +n0-facing triangle.
    let up = if n0.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = (up - *n0 * up.dot(*n0)).normalize();
    let v = n0.cross(u);
    let to_2d = |i: u32| -> (f64, f64) {
        let p = vertex(mesh, i).to_vec();
        (p.dot(u), p.dot(v))
    };
    let signed_area = |r: &[(f64, f64)]| -> f64 {
        let mut s = 0.0;
        for i in 0..r.len() {
            let (x0, y0) = r[i];
            let (x1, y1) = r[(i + 1) % r.len()];
            s += x0 * y1 - x1 * y0;
        }
        0.5 * s
    };
    let tri_area = |t: &[u32; 3]| -> f64 {
        let (a, b, c) = (to_2d(t[0]), to_2d(t[1]), to_2d(t[2]));
        0.5 * ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0))
    };
    let rings: Vec<Vec<(f64, f64)>> = loops
        .iter()
        .map(|lp| lp.iter().map(|&i| to_2d(i)).collect())
        .collect();
    let areas: Vec<f64> = rings.iter().map(|r| signed_area(r)).collect();
    let outer = (0..rings.len()).max_by(|&i, &j| areas[i].abs().total_cmp(&areas[j].abs()))?;
    if areas[outer] <= 0.0 {
        return None;
    }
    // Every other loop must be a hole. The gate is on MAGNITUDE, not sign:
    // a near-degenerate loop around a sliver comes out with either sign and
    // encloses nothing, while a loop enclosing real area the wrong way
    // round means this is two patches, not one patch with holes.
    if (0..rings.len()).any(|i| i != outer && areas[i] > 1e-6 * areas[outer]) {
        return None;
    }
    // Signed, not unsigned: the volume a planar patch contributes is
    // (n·p₀)/3 times its signed area, so reproducing the signed area in
    // this plane reproduces the patch's contribution exactly — and a fold,
    // a double cover or a dropped sliver shows up as a mismatch. All three
    // areas are measured in THIS 2D frame, and the shoelace of the
    // boundary equals the sum of the triangles it bounds exactly, so the
    // tolerance is f64 rounding over the region (1e-9 relative leaves two
    // orders of magnitude of headroom at 100k triangles) — not a physical
    // slack. Loosening it to 1e-6 let earcut drop a zero-area ear at a
    // t-junction and opened a three-edge hole in the rana-60 shell.
    let members_area: f64 = members
        .iter()
        .map(|&t| {
            tri_area(&[
                mesh.indices[t * 3],
                mesh.indices[t * 3 + 1],
                mesh.indices[t * 3 + 2],
            ])
        })
        .sum();
    let loop_area: f64 = areas.iter().sum();
    let area_eps = 1e-9 * loop_area.abs() + 1e-12;
    if (loop_area - members_area).abs() > area_eps {
        return None;
    }

    let holes: Vec<Vec<(f64, f64)>> = (0..rings.len())
        .filter(|&i| i != outer)
        .map(|i| rings[i].clone())
        .collect();
    let tris = vcad_kernel_tessellate::triangulate_polygon_2d(&rings[outer], &holes)?;
    // Combined index space earcut reports into: the outer ring, then the
    // holes in the order they were handed over.
    let mut flat: Vec<u32> = loops[outer].clone();
    for i in (0..rings.len()).filter(|&i| i != outer) {
        flat.extend_from_slice(&loops[i]);
    }
    let mut out: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
    for t in &tris {
        let tri = [
            *flat.get(t[0] as usize)?,
            *flat.get(t[1] as usize)?,
            *flat.get(t[2] as usize)?,
        ];
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            continue;
        }
        out.push(tri);
    }
    if out.is_empty() || !fold_degenerate_ears(&mut out, &to_2d) {
        return None;
    }
    let mut check: std::collections::HashMap<(u32, u32), i32> = std::collections::HashMap::new();
    let mut new_uses: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    let mut new_area = 0.0;
    for tri in &out {
        new_area += tri_area(tri);
        for k in 0..3 {
            let (x, y) = (tri[k], tri[(k + 1) % 3]);
            let (key, s) = if x < y { ((x, y), 1) } else { ((y, x), -1) };
            *check.entry(key).or_default() += s;
            *new_uses.entry(key).or_default() += 1;
        }
    }
    if (new_area - members_area).abs() > area_eps {
        return None;
    }
    // Watertightness is preserved exactly when the replacement leaves the
    // same unpaired directed edges the region did — no more, no fewer. The
    // interior edges differ (that is the point), so only the unpaired ones
    // are compared, through a sorted map so the comparison is order-free.
    let unpaired = |m: &std::collections::HashMap<(u32, u32), i32>| {
        m.iter()
            .filter(|&(_, &n)| n != 0)
            .map(|(&k, &v)| (k, v))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    if unpaired(&check) != unpaired(&net) {
        return None;
    }
    // …and manifoldness is preserved only if the replacement leaves every
    // edge the REST of the mesh also touches in the state it found it.
    // Earcut is free to run a diagonal between two boundary vertices, and
    // on a chained soup that diagonal can already exist on a neighbouring
    // plane: it would then be used by four triangles, which a slicer reads
    // as an interior crack. The test is on the edge's TOTAL use count,
    // counting the triangles outside the region that do not change:
    // a healthy edge must stay at two users, and no edge may be left with
    // exactly one (a hole) that did not already have one. An edge that is
    // already over-used may drop back towards two — that is an
    // improvement, and refusing it cost the twelve-post ring union its
    // whole merge (19 regions declined, 10 160 triangles instead of
    // 2 922).
    let mut touched: std::collections::BTreeSet<(u32, u32)> = uses.keys().copied().collect();
    touched.extend(new_uses.keys().copied());
    for e in touched {
        let inside_old = uses.get(&e).copied().unwrap_or(0);
        let inside_new = new_uses.get(&e).copied().unwrap_or(0);
        let outside = edge_tris
            .get(&e)
            .map(|t| t.len())
            .unwrap_or(inside_old)
            .saturating_sub(inside_old);
        let (old_total, new_total) = (outside + inside_old, outside + inside_new);
        // `outside > 0`: an edge only the region touches is ours to
        // re-cut — dropping it is the whole point of the merge.
        if (outside > 0 && old_total == 2 && new_total != 2) || (new_total == 1 && old_total != 1) {
            return None;
        }
    }
    // Finally, no new triangle may repeat a vertex triple that survives
    // outside the region. `make_manifold` cancels same-triple pairs — an
    // opposed pair outright — so minting a collision would delete the
    // outside copy along with ours and tear a hole. Nothing in the checks
    // above sees it: the triple's three edges can each keep their old use
    // counts.
    let mut own: std::collections::HashMap<[u32; 3], usize> = std::collections::HashMap::new();
    for &t in members {
        let mut k = [
            mesh.indices[t * 3],
            mesh.indices[t * 3 + 1],
            mesh.indices[t * 3 + 2],
        ];
        k.sort_unstable();
        *own.entry(k).or_default() += 1;
    }
    for tri in &out {
        let mut k = *tri;
        k.sort_unstable();
        if tri_keys.get(&k).copied().unwrap_or(0) > own.get(&k).copied().unwrap_or(0) {
            return None;
        }
    }
    Some(out)
}

/// Boolean of two closed triangle meshes. Returns the result surface as a
/// triangle mesh; empty results give an empty mesh.
pub fn mesh_csg(mesh_a: &TriangleMesh, mesh_b: &TriangleMesh, op: BooleanOp) -> TriangleMesh {
    orient_outward(mesh_csg_split(mesh_a, mesh_b, op))
}

/// [`mesh_csg`] with the split debris put back together
/// ([`merge_coplanar_regions`]).
///
/// For CHAINED booleans only — the ones whose operands are themselves
/// fallback results. Their debris compounds: every boolean re-splits what
/// the last one left, and the rana-60 stator's authored 50-step fold (each
/// operand spanning z 11.1..17.1) had not finished after nine minutes at
/// 2.5 GB and climbing.
///
/// A FIRST-generation boolean does not get this, on purpose. The merge is
/// sound on its own terms — it re-cuts flat patches, moves no vertex and
/// preserves the signed area, the unpaired edges and the volume — but it
/// hands downstream passes a coarser mesh, and those carry tolerances of
/// their own: a sheet-metal U-channel came out 1.29% light and the
/// shell-ring reproducer lost its edge-manifoldness, with nothing wrong in
/// the merged mesh itself. Those parts have no debris problem to trade
/// against, so they keep the fine triangulation they were built with.
pub fn mesh_csg_remerged(
    mesh_a: &TriangleMesh,
    mesh_b: &TriangleMesh,
    op: BooleanOp,
) -> TriangleMesh {
    let mut mesh = mesh_csg_split(mesh_a, mesh_b, op);
    merge_coplanar_regions(&mut mesh);
    orient_outward(mesh)
}

/// [`mesh_csg`] up to (but not including) the orientation pin, shared with
/// [`mesh_csg_remerged`].
fn mesh_csg_split(mesh_a: &TriangleMesh, mesh_b: &TriangleMesh, op: BooleanOp) -> TriangleMesh {
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

    let (member_a, member_b) = (Membership::new(mesh_a), Membership::new(mesh_b));
    let (in_a, in_b) = (member_a.index(), member_b.index());
    let mut out: Vec<Polygon> = frags_a
        .into_iter()
        .filter(|f| keep_a(classify(f, &in_b)))
        .collect();
    for mut f in frags_b {
        if keep_b(classify(&f, &in_a)) {
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
    // triangles. What it cannot reach are *redundant patches*: whole
    // sliver strips that double-cover the surface along seam circles
    // (classification keeps both operands' fragments of the chord-vs-arc
    // lune where one operand's cap crosses the other's chordal wall).
    // Peel those, then pin the global orientation.
    // No repair passes run here — deliberately. The caller's quadric
    // projection decides per-vertex constraints from incident triangle
    // normals, so even a pure deletion (peeling a zero-area flap) before
    // it flips pinning decisions and strands seam vertices off their
    // carriers (measured 0.36 mm off a R25 sphere). `mesh_fallback` runs
    // the repair pipeline between two projection passes instead.
    polygons_to_mesh(&out)
}

/// Boolean of two closed triangle meshes, repaired into a manifold shell.
///
/// [`mesh_csg`] deliberately runs no repair passes, because callers that
/// reproject vertices onto analytic carriers must see the raw fragment
/// topology. Callers who just want a solid to export want the opposite,
/// and got neither: the raw result is watertight but *branches* wherever
/// two tool boundaries nearly coincide — classification correctly keeps a
/// fragment from each operand covering the same surface, leaving edges
/// with four incident triangles. A slicer's ray parity reads those as
/// interior cracks, which is the failure mode ecto/vcad#840 was filed for.
///
/// This is the export-facing entry point: [`mesh_csg`] followed by
/// [`make_manifold`], which welds the seam copies, drops slivers and
/// cancels the double covers. It is deterministic and volume-preserving —
/// a cancelling patch pair contributes nothing to the divergence integral,
/// so repair cannot quietly change the part.
///
/// Chain it directly for multi-tool parts: differencing N tools one at a
/// time keeps every intermediate a valid solid, so a failure is localised
/// to the tool that caused it rather than surfacing at the end.
pub fn manifold_csg(mesh_a: &TriangleMesh, mesh_b: &TriangleMesh, op: BooleanOp) -> TriangleMesh {
    let mut out = mesh_csg(mesh_a, mesh_b, op);
    // Strip double covers that `make_manifold` cannot see. Its cancellation
    // matches triangles by vertex set, which catches a patch and its exact
    // mirror but not two patches covering the same surface with *different*
    // triangulations — the shape a difference leaves where a tool's face
    // grazes an existing wall. This pass classifies by ray casting instead,
    // so the triangulations need not agree.
    super::remove_interior_membranes(&mut out);
    make_manifold(&out, DEFAULT_WELD_EPS)
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
        let vol = volume(&mesh_csg(&a, &b, BooleanOp::Difference));
        assert!((vol - 99120.0).abs() < 5.0, "expected 99120, got {vol}");
    }
}
