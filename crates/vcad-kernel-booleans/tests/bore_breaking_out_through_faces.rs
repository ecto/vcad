//! Regression tests for bores whose opening breaks *out* through a face of
//! the body rather than landing wholly inside one (printed-part handoff,
//! 2026-08-11).
//!
//! Both failures here were silent — the boolean returned a closed,
//! positive-volume, plausible-looking mesh of the wrong solid — so the
//! assertions are on volume against Monte-Carlo/exact ground truth. A
//! "does not panic" test passes on every one of these bugs.
//!
//! Each case also asserts the exported triangle mesh is edge-manifold:
//! every undirected edge used by exactly two triangles. Both bugs left the
//! bore's rim unwelded (hundreds of singly-used edges), which is what a
//! slicer sees as spurious backfaces.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::TriangleMesh;

const SEGMENTS: u32 = 32;

fn apply_transform(brep: &mut BRepSolid, t: &Transform) {
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(t))
        .collect();
}

fn translate(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
    apply_transform(brep, &Transform::translation(dx, dy, dz));
}

fn difference(a: &BRepSolid, b: &BRepSolid) -> BooleanResult {
    boolean_op(a, b, BooleanOp::Difference, SEGMENTS).expect("difference should succeed")
}

fn union(a: &BRepSolid, b: &BRepSolid) -> BooleanResult {
    boolean_op(a, b, BooleanOp::Union, SEGMENTS).expect("union should succeed")
}

fn assert_volume_within(actual: f64, expected: f64, tol_frac: f64, label: &str) {
    let rel = (actual - expected).abs() / expected.abs();
    assert!(
        rel <= tol_frac,
        "{label}: volume {actual:.1} differs from truth {expected:.1} by {:.2}% (allowed {:.2}%)",
        rel * 100.0,
        tol_frac * 100.0
    );
}

/// Every undirected edge of a closed surface is shared by exactly two
/// triangles. Counts violations the way an STL consumer (slicer, renderer)
/// would, on quantised vertex positions.
fn non_manifold_edges(mesh: &TriangleMesh) -> usize {
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
    uses.values().filter(|&&n| n != 2).count()
}

fn assert_edge_manifold(mesh: &TriangleMesh, label: &str) {
    let bad = non_manifold_edges(mesh);
    assert_eq!(
        bad, 0,
        "{label}: {bad} undirected edges are not shared by exactly two triangles \
         (T-junctions on the cut rim — slicers render these as backfaces)"
    );
}

/// The tool cylinder of the boss-and-bore case: axis along X at
/// (y, z) = (15, 47), radius 12.8, spanning x = 6..74.
fn side_bore() -> BRepSolid {
    let mut bore = make_cylinder(12.8, 68.0, SEGMENTS);
    apply_transform(&mut bore, &Transform::rotation_y(90.0_f64.to_radians()));
    translate(&mut bore, 6.0, 15.0, 47.0);
    bore
}

/// A boss union'd onto a base plate, then bored through. The bore's circle
/// BREAKS OUT through the boss's y = 5 wall instead of closing inside it.
///
/// The wall's y > 5 region spans 283° of the cylinder, which the u = 0 seam
/// splits into a 231° piece and a 51° piece. Classification sampled the
/// large piece by assuming a cylindrical face is always the *shorter* of the
/// two arcs its boundary angles bound, so the probe landed in the middle of
/// the 129° complement — inside the neighbouring fragment, on the far side
/// of the wall, where it reads Outside. The whole 231° fragment was dropped
/// and the Difference returned the bare union: 178,471 against a truth of
/// 149,555, with no error raised.
#[test]
fn bore_breaking_out_through_a_boss_side_face() {
    let base = make_cube(80.0, 45.0, 29.5);
    let mut boss = make_cube(60.0, 35.0, 34.5);
    translate(&mut boss, 10.0, 5.0, 29.5);
    let body = union(&base, &boss).into_brep().expect("union is a B-rep");

    // The union alone is the wrong answer this bug used to return.
    let body_vol = vcad_kernel_booleans::mesh_signed_volume(
        &vcad_kernel_tessellate::tessellate_brep(&body, SEGMENTS),
    );
    assert_volume_within(body_vol, 178_650.0, 0.01, "boss union");

    let result = difference(&body, &side_bore());
    let mesh = result.to_mesh(SEGMENTS);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&mesh);

    assert_volume_within(vol, 149_555.0, 0.01, "bore breaking out through y=5");
    assert!(
        vol < body_vol - 1000.0,
        "bore removed only {:.1} mm³ — this is the silent no-op the guard exists for",
        body_vol - vol
    );
    assert_edge_manifold(&mesh, "bore breaking out through y=5");
}

/// The control that isolates the trigger: move the same bore 5 mm in +y so
/// its circle is fully contained in the boss's y faces. This case was always
/// correct, and must stay correct — it is what proves the defect is the
/// face-edge crossing and not the bore itself.
#[test]
fn bore_contained_in_the_boss_side_faces_is_unaffected() {
    let base = make_cube(80.0, 45.0, 29.5);
    let mut boss = make_cube(60.0, 35.0, 34.5);
    translate(&mut boss, 10.0, 5.0, 29.5);
    let body = union(&base, &boss).into_brep().expect("union is a B-rep");

    let mut bore = make_cylinder(12.8, 68.0, SEGMENTS);
    apply_transform(&mut bore, &Transform::rotation_y(90.0_f64.to_radians()));
    translate(&mut bore, 6.0, 20.0, 47.0);

    let result = difference(&body, &bore);
    let mesh = result.to_mesh(SEGMENTS);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&mesh);

    // 178,650 − π·12.8²·60 (a full-length bore through the boss).
    assert_volume_within(vol, 147_764.0, 0.01, "contained bore");
    assert_edge_manifold(&mesh, "contained bore");
}

/// A bore crossing a 45° face that is itself *derived* — the face exists
/// only because a rotated block was cut out of a box, so by the time the
/// bore arrives the face has already been split once and its first three
/// loop vertices are colinear.
///
/// Two independent defects met here. The elliptical mouth on the 45° face
/// enters and leaves through the same boundary edge, which the curve
/// splitter rejected outright as "no real cut" — a rule that is right for a
/// straight chord and wrong for an arc, which bulges into the interior and
/// bounds a genuine sub-face. And the circular mouth on the y = −44 wall
/// went unsplit because the arc splitter derived its plane frame from the
/// first three loop vertices, which that already-split face left colinear.
/// Result: 84,845 against a truth of 83,317, with the rim left unwelded.
#[test]
fn bore_crossing_a_derived_forty_five_degree_face() {
    let mut block = make_cube(92.0, 70.0, 24.0);
    translate(&mut block, -46.0, -44.0, 0.0);

    let mut wedge = make_cube(60.0, 60.0, 60.0);
    translate(&mut wedge, -30.0, -30.0, -30.0);
    apply_transform(&mut wedge, &Transform::rotation_x(45.0_f64.to_radians()));
    translate(&mut wedge, 0.0, 0.0, -7.4955);

    let body = difference(&block, &wedge)
        .into_brep()
        .expect("grooved body is a B-rep");
    let body_vol = vcad_kernel_booleans::mesh_signed_volume(
        &vcad_kernel_tessellate::tessellate_brep(&body, SEGMENTS),
    );
    assert_volume_within(body_vol, 90_912.0, 0.01, "45° grooved body");

    let mut bore = make_cylinder(17.2, 28.0, SEGMENTS);
    apply_transform(&mut bore, &Transform::rotation_x(-90.0_f64.to_radians()));
    translate(&mut bore, 0.0, -47.0, 0.0);

    let result = difference(&body, &bore);
    let mesh = result.to_mesh(SEGMENTS);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&mesh);

    assert_volume_within(vol, 83_317.0, 0.01, "bore across a 45° derived face");
    assert_edge_manifold(&mesh, "bore across a 45° derived face");
}
