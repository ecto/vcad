//! Union whose operands are themselves differences — stacked annular
//! rings, the shape every printed rotor/carrier/backplate is made of.
//!
//! Field report (2026-08-27): exported STLs of such parts carried hundreds
//! to thousands of edges not shared by exactly two triangles. Bambu Studio
//! flagged them ("3701 non-manifold edges") and its auto-repair resolved
//! them by FILLING — a printed rotor came out with no shaft hole at all.
//! Vertex welding did not move the count (flat from 1e-4 to 1e-1), because
//! the defect is topological, not numeric.
//!
//! Two splitter bugs, both about a face's HOLES:
//!
//!  1. When a planar face was split by a circle, its existing hole loops
//!     were routed to the disk or the ring sub-face by testing the hole's
//!     CENTROID against the circle. Every concentric loop — whatever its
//!     radius — has its centroid at the shared center, so a hole *larger*
//!     than the splitting circle went to the disk: a face whose hole is
//!     bigger than its own outer loop, which the tessellator draws as the
//!     full disk (a membrane over the bore).
//!  2. A circle lying entirely inside one of the face's holes was treated
//!     as "inside the face" (only the outer loop was consulted), splitting
//!     off a disk over the hole plus a redundant nested hole on the ring.
//!
//! Both produce doubled surface, which is exactly what a slicer reports.
//! The assertions here are on manifoldness AND volume: a membrane over a
//! bore is invisible to a volume check alone only until it is filled.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::{Point3, Transform};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

const SEGMENTS: u32 = 32;

fn translate(brep: &mut BRepSolid, dz: f64) {
    let t = Transform::translation(0.0, 0.0, dz);
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(&t))
        .collect();
}

fn solid(r: BooleanResult) -> BRepSolid {
    r.into_brep().expect("boolean returned no B-rep")
}

/// (edges used once, edges used more than twice) on quantised positions —
/// what an STL consumer sees.
fn bad_edges(mesh: &TriangleMesh) -> (usize, usize) {
    let quantum = 1e-5;
    let key = |vi: usize| -> [i64; 3] {
        [
            (mesh.vertices[vi * 3] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 1] as f64 / quantum).round() as i64,
            (mesh.vertices[vi * 3 + 2] as f64 / quantum).round() as i64,
        ]
    };
    let mut uses: HashMap<([i64; 3], [i64; 3]), usize> = HashMap::new();
    for tri in mesh.indices.chunks(3) {
        let v = [
            key(tri[0] as usize),
            key(tri[1] as usize),
            key(tri[2] as usize),
        ];
        for i in 0..3 {
            let (a, b) = (v[i], v[(i + 1) % 3]);
            if a == b {
                continue;
            }
            let e = if a < b { (a, b) } else { (b, a) };
            *uses.entry(e).or_default() += 1;
        }
    }
    (
        uses.values().filter(|&&n| n == 1).count(),
        uses.values().filter(|&&n| n > 2).count(),
    )
}

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in mesh.indices.chunks(3) {
        let g = |i: u32| {
            let k = i as usize * 3;
            Point3::new(
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        let (a, b, c) = (g(t[0]), g(t[1]), g(t[2]));
        v += a.to_vec().dot(b.to_vec().cross(c.to_vec())) / 6.0;
    }
    v.abs()
}

/// An annular ring of height `h` whose base sits at `z`, built the way a
/// .vcad model builds it: a difference of two cylinders.
fn annulus(r_inner: f64, r_outer: f64, h: f64, z: f64) -> BRepSolid {
    let outer = make_cylinder(r_outer, h, SEGMENTS);
    let mut inner = make_cylinder(r_inner, h + 2.0, SEGMENTS);
    translate(&mut inner, -1.0);
    let mut ring = solid(boolean_op(&outer, &inner, BooleanOp::Difference, SEGMENTS).unwrap());
    translate(&mut ring, z);
    ring
}

fn check(base: BRepSolid, boss: BRepSolid, truth: f64, label: &str) {
    for (part, name) in [(&base, "base"), (&boss, "boss")] {
        let (open, over) = bad_edges(&tessellate_brep(part, SEGMENTS));
        assert_eq!(
            (open, over),
            (0, 0),
            "{label}: operand {name} is already non-manifold"
        );
    }
    let u = solid(boolean_op(&base, &boss, BooleanOp::Union, SEGMENTS).unwrap());
    let mesh = tessellate_brep(&u, SEGMENTS);
    let (open, over) = bad_edges(&mesh);
    assert_eq!(
        (open, over),
        (0, 0),
        "{label}: {open} unpaired + {over} over-used edges — a slicer calls \
         these non-manifold and its auto-repair fills the bore"
    );
    let vol = mesh_volume(&mesh);
    assert!(
        (vol - truth).abs() / truth < 0.01,
        "{label}: volume {vol:.1} vs truth {truth:.1}"
    );
}

/// The reported minimal case: a wide flat ring with a narrow ring stacked
/// on it, their contact planes coincident. Scored 1902 bad edges.
#[test]
fn stacked_rings_face_to_face() {
    let truth = std::f64::consts::PI * (46.0f64.powi(2) - 8.0f64.powi(2)) * 2.5
        + std::f64::consts::PI * (27.5f64.powi(2) - 24.0f64.powi(2)) * 1.5;
    check(
        annulus(8.0, 46.0, 2.5, 0.0),
        annulus(24.0, 27.5, 1.5, 2.5),
        truth,
        "coplanar contact",
    );
}

/// Same pair interpenetrating rather than merely touching — the field
/// report noted the count was identical either way, which is what
/// pointed at the splitter rather than at coplanar-contact handling.
#[test]
fn stacked_rings_interpenetrating() {
    let truth = std::f64::consts::PI * (46.0f64.powi(2) - 8.0f64.powi(2)) * 2.5
        + std::f64::consts::PI * (27.5f64.powi(2) - 24.0f64.powi(2)) * 0.5;
    check(
        annulus(8.0, 46.0, 2.5, 0.0),
        annulus(24.0, 27.5, 1.5, 1.5),
        truth,
        "interpenetrating",
    );
}

/// The boss ring sits over the base's own bore wall (its inner radius is
/// smaller than the base's), so the splitting circles land inside an
/// existing hole — bug 2 above.
#[test]
fn boss_ring_overhanging_the_bore() {
    let truth = std::f64::consts::PI * (46.0f64.powi(2) - 8.0f64.powi(2)) * 2.5
        + std::f64::consts::PI * (12.0f64.powi(2) - 8.0f64.powi(2)) * 3.0;
    check(
        annulus(8.0, 46.0, 2.5, 0.0),
        annulus(8.0, 12.0, 3.0, 2.5),
        truth,
        "boss over the bore",
    );
}
