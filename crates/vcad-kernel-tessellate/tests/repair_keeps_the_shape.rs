//! The export repair may close a part; it may not change one.
//!
//! `repair_watertightness` used to optimize a defective-edge count under a
//! single guard — enclosed volume within 1%. On the rana-60 stator that
//! bought 542 defective edges at the price of 0.68 mm of the part: the
//! membrane peeler, which ran unguarded on the reasoning that it can only
//! remove surface that should not exist, deleted 42 triangles of the lead
//! notch's R1.05 fillet walls, and every later pass worked on a torn mesh.
//! 1% of 7850 mm³ is 78 mm³, so nothing noticed.
//!
//! Both tests below fail on the base commit and pass with the shape guard.

use vcad_kernel_tessellate::{
    repair_watertightness, repair_watertightness_reported, TriangleMesh, SHAPE_TOLERANCE,
};

/// A closed axis-aligned box as an indexed mesh.
fn box_mesh(sx: f32, sy: f32, sz: f32) -> TriangleMesh {
    let v = [
        [0.0, 0.0, 0.0],
        [sx, 0.0, 0.0],
        [sx, sy, 0.0],
        [0.0, sy, 0.0],
        [0.0, 0.0, sz],
        [sx, 0.0, sz],
        [sx, sy, sz],
        [0.0, sy, sz],
    ];
    let faces = [
        [0, 3, 2, 1], // z-
        [4, 5, 6, 7], // z+
        [0, 1, 5, 4], // y-
        [1, 2, 6, 5], // x+
        [2, 3, 7, 6], // y+
        [3, 0, 4, 7], // x-
    ];
    let mut mesh = TriangleMesh::new();
    for p in v {
        mesh.vertices.extend_from_slice(&p);
    }
    for f in faces {
        mesh.indices
            .extend_from_slice(&[f[0] as u32, f[1] as u32, f[2] as u32]);
        mesh.indices
            .extend_from_slice(&[f[0] as u32, f[2] as u32, f[3] as u32]);
    }
    mesh
}

/// Every triangle of `mesh`, sampled at its corners and centroid, must lie
/// within `tol` of `other`'s surface: the one-sided Hausdorff the guard uses,
/// recomputed here so the test does not just re-run the implementation.
fn max_deviation(mesh: &TriangleMesh, other: &TriangleMesh) -> f64 {
    let p = |m: &TriangleMesh, i: u32| {
        let k = i as usize * 3;
        [
            m.vertices[k] as f64,
            m.vertices[k + 1] as f64,
            m.vertices[k + 2] as f64,
        ]
    };
    let mut worst: f64 = 0.0;
    for t in mesh.indices.chunks(3) {
        let (a, b, c) = (p(mesh, t[0]), p(mesh, t[1]), p(mesh, t[2]));
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        for q in [a, b, c, centroid] {
            let mut best = f64::INFINITY;
            for u in other.indices.chunks(3) {
                let (x, y, z) = (p(other, u[0]), p(other, u[1]), p(other, u[2]));
                best = best.min(point_tri_dist(q, x, y, z));
            }
            worst = worst.max(best);
        }
    }
    worst
}

/// Distance from a point to a triangle, by dense barycentric sampling —
/// slow, obvious, and independent of the code under test.
fn point_tri_dist(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    const N: usize = 12;
    let mut best = f64::INFINITY;
    for i in 0..=N {
        for j in 0..=(N - i) {
            let (u, v) = (i as f64 / N as f64, j as f64 / N as f64);
            let w = 1.0 - u - v;
            let q = [
                a[0] * w + b[0] * u + c[0] * v,
                a[1] * w + b[1] * u + c[1] * v,
                a[2] * w + b[2] * u + c[2] * v,
            ];
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
            best = best.min(d);
        }
    }
    best
}

/// The coordinator's synthetic: a closed box carrying a second, coincident
/// copy of one wall. The duplicate is a real defect — every edge of that wall
/// is used four times — and the repair is welcome to take it away. What it
/// may not do is take the wall.
#[test]
fn a_duplicated_wall_may_be_peeled_but_the_wall_must_stay() {
    let clean = box_mesh(10.0, 8.0, 6.0);
    let mut mesh = clean.clone();

    // Duplicate the x+ wall (vertices 1,2,6,5), as its own two triangles on
    // fresh vertices — a doubled sheet, not a repeated index.
    let base = (mesh.vertices.len() / 3) as u32;
    for i in [1u32, 2, 6, 5] {
        let k = i as usize * 3;
        let (x, y, z) = (mesh.vertices[k], mesh.vertices[k + 1], mesh.vertices[k + 2]);
        mesh.vertices.extend_from_slice(&[x, y, z]);
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

    let before_tris = mesh.indices.len() / 3;
    repair_watertightness(&mut mesh);

    // The wall is still there: every point of the clean box is on the result.
    let lost = max_deviation(&clean, &mesh);
    assert!(
        lost <= SHAPE_TOLERANCE,
        "the repair opened the box — a point of the original surface is now \
         {lost:.4} mm from anything the repair left (tolerance {SHAPE_TOLERANCE})"
    );
    // ...and nothing was invented either.
    let gained = max_deviation(&mesh, &clean);
    assert!(
        gained <= SHAPE_TOLERANCE,
        "the repair added surface {gained:.4} mm away from the original box"
    );
    assert!(
        mesh.indices.len() / 3 <= before_tris,
        "peeling a duplicate should not grow the mesh"
    );
}

/// A mesh the repair cannot close must come back WHOLE, and must say so.
///
/// The lid is removed, leaving a 10 x 8 hole — the shape of an open import or
/// a mid-pipeline fragment. The repair's carve-and-refill pass answers that by
/// gutting the mesh, which scores a perfect defect count on nothing at all;
/// the guard declines it because it would move the surface infinitely far.
/// What the caller gets back is the five walls it handed in, plus the verdict
/// that they are not watertight (native-app friction log item 30: a degraded
/// solid used to look like a good one).
#[test]
fn a_mesh_that_cannot_be_closed_comes_back_whole_and_says_so() {
    let clean = box_mesh(10.0, 8.0, 6.0);
    let mut mesh = clean.clone();
    // Drop the z+ face (triangles 2 and 3 in `box_mesh`'s emission order).
    let keep: Vec<u32> = mesh
        .indices
        .chunks(3)
        .enumerate()
        .filter(|(t, _)| *t != 2 && *t != 3)
        .flat_map(|(_, tri)| tri.to_vec())
        .collect();
    mesh.indices = keep;
    let open = mesh.clone();

    let outcome = repair_watertightness_reported(&mut mesh);
    assert!(
        outcome.defects_before > 0,
        "the open box should read as defective"
    );

    // Whatever it decided, the five walls it was given are still there.
    let lost = max_deviation(&open, &mesh);
    assert!(
        lost <= SHAPE_TOLERANCE,
        "the repair removed surface it was given: a point of the open box is \
         now {lost:.4} mm from anything that came back"
    );
    // ...and it did not invent any.
    let gained_off_box = max_deviation(&mesh, &clean);
    assert!(
        gained_off_box <= SHAPE_TOLERANCE,
        "the repair added surface {gained_off_box:.4} mm off the box"
    );

    // And the caller is told, rather than left holding a part that looks fine.
    if !outcome.is_watertight() {
        let warning = outcome.warning().expect("a non-watertight mesh warns");
        assert!(
            warning.contains("not watertight"),
            "unhelpful warning: {warning}"
        );
    }
}

/// The verdict has to reach the caller: a mesh that comes out still defective
/// says so, with the distance the declined repair would have moved the part.
#[test]
fn the_outcome_reports_a_mesh_it_could_not_close() {
    let mut mesh = box_mesh(10.0, 8.0, 6.0);
    let outcome = repair_watertightness_reported(&mut mesh);
    assert_eq!(outcome.defects_before, 0, "a plain box is closed");
    assert!(outcome.is_watertight());
    assert_eq!(outcome.warning(), None, "nothing to warn about");
}
