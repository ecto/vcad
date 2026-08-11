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

fn polygons_to_mesh(polys: &[Polygon]) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let mut cache: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut push = |p: &Point3, mesh: &mut TriangleMesh| -> u32 {
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
            idx
        })
    };
    for poly in polys {
        for k in 1..poly.verts.len() - 1 {
            let a = push(&poly.verts[0], &mut mesh);
            let b = push(&poly.verts[k], &mut mesh);
            let c = push(&poly.verts[k + 1], &mut mesh);
            if a != b && b != c && a != c {
                mesh.indices.push(a);
                mesh.indices.push(b);
                mesh.indices.push(c);
            }
        }
    }
    mesh
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
    let mesh = weld_t_junctions(&polygons_to_mesh(&out));
    orient_outward(mesh)
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

/// Distance (mm) within which a vertex counts as lying on a triangle edge.
/// Mesh coordinates are `f32`, so a shared split point can land ~1e-5 mm
/// apart on two fragments; this sits above that and far below any real
/// feature (0.1 µm).
const WELD_TOL: f64 = 1e-4;

/// Eliminate T-junctions: a vertex lying in the interior of another
/// triangle's edge leaves that edge unpaired, which every watertightness
/// check reads as a hole.
///
/// They are unavoidable upstream: splitting is localized by AABB overlap
/// (without it, every polygon would be cut by every far-away carrier plane
/// — quadratic and explosive), so a fragment may be split by a plane that
/// its edge-sharing neighbour never sees. Repairing after the fact is exact
/// rather than a tolerance fudge: inserting a vertex that already lies on
/// an edge changes no geometry and no volume, only which triangles agree
/// combinatorially along that edge.
///
/// Each triangle that gains edge points is re-emitted as a centroid fan, so
/// every boundary edge appears once and every spoke exactly twice with
/// opposite orientation. The fan preserves the original winding.
///
/// Deliberately a **single** sweep. Fanning multiplies a triangle into one
/// per boundary point, so iterating to a fixed point compounds that growth
/// — four passes pushed several chained-boolean torture cases past a 20 s
/// timeout while removing only a handful more junctions than one pass.
fn weld_t_junctions(mesh: &TriangleMesh) -> TriangleMesh {
    weld_pass(mesh).0
}

/// One welding sweep. Returns the rebuilt mesh and how many edge points it
/// had to insert (zero means the mesh is already junction-free).
fn weld_pass(mesh: &TriangleMesh) -> (TriangleMesh, usize) {
    let nv = mesh.vertices.len() / 3;
    if nv == 0 || mesh.indices.is_empty() {
        return (mesh.clone(), 0);
    }
    let vert = |i: usize| {
        Point3::new(
            mesh.vertices[i * 3] as f64,
            mesh.vertices[i * 3 + 1] as f64,
            mesh.vertices[i * 3 + 2] as f64,
        )
    };

    // Uniform-grid vertex index, so each edge query touches only nearby
    // vertices instead of all of them.
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for i in 0..nv {
        let p = vert(i);
        for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
            min[k] = min[k].min(c);
            max[k] = max[k].max(c);
        }
    }
    let diag = ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2))
        .sqrt()
        .max(1e-9);
    let cell = (diag / 64.0).max(10.0 * WELD_TOL);
    let key_of = |p: &Point3| -> [i64; 3] {
        [
            ((p.x - min[0]) / cell).floor() as i64,
            ((p.y - min[1]) / cell).floor() as i64,
            ((p.z - min[2]) / cell).floor() as i64,
        ]
    };
    let mut grid: std::collections::HashMap<[i64; 3], Vec<usize>> =
        std::collections::HashMap::new();
    for i in 0..nv {
        grid.entry(key_of(&vert(i))).or_default().push(i);
    }

    let mut out = TriangleMesh::new();
    out.vertices = mesh.vertices.clone();
    let mut inserted_total = 0usize;

    for tri in mesh.indices.chunks(3) {
        let corners = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let mut loop_idx: Vec<u32> = Vec::with_capacity(3);
        let mut inserted_here = 0usize;

        for e in 0..3 {
            let a_i = corners[e];
            let b_i = corners[(e + 1) % 3];
            let (a, b) = (vert(a_i), vert(b_i));
            loop_idx.push(a_i as u32);

            let ab = b - a;
            let len2 = ab.dot(ab);
            if len2 < 1e-18 {
                continue;
            }
            let (ka, kb) = (key_of(&a), key_of(&b));
            let mut on_edge: Vec<(f64, usize)> = Vec::new();
            let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for gx in ka[0].min(kb[0]) - 1..=ka[0].max(kb[0]) + 1 {
                for gy in ka[1].min(kb[1]) - 1..=ka[1].max(kb[1]) + 1 {
                    for gz in ka[2].min(kb[2]) - 1..=ka[2].max(kb[2]) + 1 {
                        let Some(bucket) = grid.get(&[gx, gy, gz]) else {
                            continue;
                        };
                        for &v_i in bucket {
                            if v_i == a_i || v_i == b_i || !seen.insert(v_i) {
                                continue;
                            }
                            let v = vert(v_i);
                            let t = (v - a).dot(ab) / len2;
                            if t <= 0.0 || t >= 1.0 {
                                continue;
                            }
                            if (v - (a + t * ab)).norm() > WELD_TOL {
                                continue;
                            }
                            if (v - a).norm() <= WELD_TOL || (v - b).norm() <= WELD_TOL {
                                continue;
                            }
                            on_edge.push((t, v_i));
                        }
                    }
                }
            }
            if on_edge.is_empty() {
                continue;
            }
            on_edge.sort_by(|x, y| x.0.total_cmp(&y.0));
            inserted_here += on_edge.len();
            for (_, v_i) in on_edge {
                loop_idx.push(v_i as u32);
            }
        }

        if inserted_here == 0 {
            out.indices.extend_from_slice(&[
                corners[0] as u32,
                corners[1] as u32,
                corners[2] as u32,
            ]);
            continue;
        }
        inserted_total += inserted_here;

        let c = Point3::from_vec(
            (vert(corners[0]).to_vec() + vert(corners[1]).to_vec() + vert(corners[2]).to_vec())
                / 3.0,
        );
        let ci = (out.vertices.len() / 3) as u32;
        out.vertices.push(c.x as f32);
        out.vertices.push(c.y as f32);
        out.vertices.push(c.z as f32);
        let n = loop_idx.len();
        for k in 0..n {
            let a = loop_idx[k];
            let b = loop_idx[(k + 1) % n];
            if a != b {
                out.indices.extend_from_slice(&[ci, a, b]);
            }
        }
    }
    (out, inserted_total)
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
