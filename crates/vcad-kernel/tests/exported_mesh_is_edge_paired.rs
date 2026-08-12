//! Watertightness of the meshes that leave the kernel, asserted the way a
//! slicer sees them: every undirected edge used by exactly two triangles.
//!
//! This is the acceptance criterion for the T-junction stitch in
//! `vcad-kernel-tessellate`. It is deliberately checked here rather than in
//! that crate, because the interesting shapes need booleans and
//! `vcad-kernel-booleans` depends on the tessellator.
//!
//! Volume is asserted alongside the edge count on purpose. The stitch inserts
//! only points that already lie on the edge they split, so it may not move a
//! single position — and a repair that closes a mesh by moving geometry would
//! pass an edge-count check alone. Every historical failure in this kernel was
//! silent and looked correct in a render; a count plus a volume is what
//! catches them.

use std::collections::HashMap;
use vcad_kernel::Solid;

/// Undirected edges used by a number of triangles other than two.
fn defective_edges(verts: &[f32], idx: &[u32]) -> usize {
    // Key by rounded position, not vertex index: the exported mesh is what
    // matters, and two coincident-but-distinct indices are a seam a slicer
    // would still stitch. Matches the tessellator's 0.001 mm weld lattice.
    let key = |i: u32| -> [i64; 3] {
        let k = i as usize * 3;
        [
            (verts[k] as f64 * 1000.0).round() as i64,
            (verts[k + 1] as f64 * 1000.0).round() as i64,
            (verts[k + 2] as f64 * 1000.0).round() as i64,
        ]
    };
    let mut counts: HashMap<([i64; 3], [i64; 3]), u32> = HashMap::new();
    for t in 0..idx.len() / 3 {
        let tri = [idx[3 * t], idx[3 * t + 1], idx[3 * t + 2]];
        for i in 0..3 {
            let (a, b) = (key(tri[i]), key(tri[(i + 1) % 3]));
            let e = if a < b { (a, b) } else { (b, a) };
            *counts.entry(e).or_insert(0) += 1;
        }
    }
    counts.values().filter(|&&n| n != 2).count()
}

fn signed_volume(verts: &[f32], idx: &[u32]) -> f64 {
    let p = |i: u32| -> [f64; 3] {
        let k = i as usize * 3;
        [verts[k] as f64, verts[k + 1] as f64, verts[k + 2] as f64]
    };
    let mut v = 0.0;
    for t in 0..idx.len() / 3 {
        let a = p(idx[3 * t]);
        let b = p(idx[3 * t + 1]);
        let c = p(idx[3 * t + 2]);
        v += (a[0] * (b[1] * c[2] - c[1] * b[2]) - b[0] * (a[1] * c[2] - c[1] * a[2])
            + c[0] * (a[1] * b[2] - b[1] * a[2]))
            / 6.0;
    }
    v
}

fn check(name: &str, solid: &Solid, expected_volume: f64) {
    let mesh = solid.to_mesh(32);
    let bad = defective_edges(&mesh.vertices, &mesh.indices);
    assert_eq!(
        bad, 0,
        "{name}: {bad} undirected edges are not used by exactly 2 triangles"
    );
    let vol = signed_volume(&mesh.vertices, &mesh.indices);
    assert!(
        (vol - expected_volume).abs() / expected_volume < 0.01,
        "{name}: volume {vol} is not within 1% of {expected_volume}"
    );
}

/// A square through-slot in a block. This is the feature whose mouths were
/// where the defect lived in the part that prompted this work: a 5 mm square
/// bolt slot cut clean through, so each end leaves a hole in a planar face.
#[test]
fn square_through_slot_is_edge_paired() {
    let block = Solid::cube(20.0, 20.0, 10.0);
    let slot = Solid::cube(5.0, 5.0, 30.0).translate(7.5, 7.5, -10.0);
    check(
        "square through-slot",
        &block.difference(&slot),
        20.0 * 20.0 * 10.0 - 5.0 * 5.0 * 10.0,
    );
}

/// Two square through-slots cut one after the other. Chaining is what turns a
/// tessellation seam into a compounding one, so the second cut has to land on
/// a face the first already re-triangulated.
#[test]
fn chained_square_through_slots_are_edge_paired() {
    let block = Solid::cube(40.0, 40.0, 10.0);
    let s1 = Solid::cube(5.0, 5.0, 30.0).translate(7.5, 17.5, -10.0);
    let s2 = Solid::cube(5.0, 5.0, 30.0).translate(27.5, 17.5, -10.0);
    check(
        "chained square through-slots",
        &block.difference(&s1).difference(&s2),
        40.0 * 40.0 * 10.0 - 2.0 * 5.0 * 5.0 * 10.0,
    );
}

/// A cylindrical bore through a block — the curved-face counterpart, where
/// the two mouths are circles rather than squares.
#[test]
fn cylindrical_bore_is_edge_paired() {
    let block = Solid::cube(40.0, 40.0, 20.0);
    let segments = 32;
    let bore = Solid::cylinder(6.0, 60.0, segments).translate(20.0, 20.0, -20.0);
    // The bore's cross-section is the inscribed n-gon, not the circle, so the
    // analytic target has to use the polygon's area or the 1% band is a lie
    // about which of the two we are checking.
    let n = segments as f64;
    let plug = 0.5 * n * (2.0 * std::f64::consts::PI / n).sin() * 36.0 * 20.0;
    check(
        "cylindrical bore",
        &block.difference(&bore),
        40.0 * 40.0 * 20.0 - plug,
    );
}
