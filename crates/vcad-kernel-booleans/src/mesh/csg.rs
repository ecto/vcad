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

/// Nudge distance (mm) for the parity probes. Far above f32 vertex noise,
/// far below feature scale (a 0.5 mm blade wall still separates cleanly).
const PROBE_EPS: f64 = 1e-3;

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
    let plus = point_in_mesh(&(c + PROBE_EPS * n), other);
    let minus = point_in_mesh(&(c - PROBE_EPS * n), other);
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
    polygons_to_mesh(&out)
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
