//! Regression tests for `Difference` face/shell classification against
//! curved geometry (hemispherical-socket clamp handoff, 2026-08-11).
//!
//! All the failures here were *silent*: the boolean returned a valid-looking
//! mesh of the wrong solid, so assertions must be on volume against the
//! analytic value — mesh-validity checks alone cannot catch them.

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, make_sphere, BRepSolid};

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

fn mesh_bbox(mesh: &vcad_kernel_tessellate::TriangleMesh) -> ([f64; 3], [f64; 3]) {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for v in mesh.vertices.chunks(3) {
        for k in 0..3 {
            min[k] = min[k].min(v[k] as f64);
            max[k] = max[k].max(v[k] as f64);
        }
    }
    (min, max)
}

fn difference(a: &BRepSolid, b: &BRepSolid) -> BooleanResult {
    boolean_op(a, b, BooleanOp::Difference, SEGMENTS).expect("difference should succeed")
}

fn assert_volume_within(actual: f64, expected: f64, tol_frac: f64, label: &str) {
    let rel = (actual - expected).abs() / expected.abs();
    assert!(
        rel <= tol_frac,
        "{label}: volume {actual:.1} differs from analytic {expected:.1} by {:.2}% (allowed {:.2}%)",
        rel * 100.0,
        tol_frac * 100.0
    );
}

/// Case 1 (PRIMARY): hemispherical pocket in a block, then a box cut
/// slicing through the pocket. The second cut used to mis-classify the
/// tool's far side and retain the sphere's lower hemisphere.
#[test]
fn box_cut_through_hemispherical_pocket() {
    let block = make_cube(80.0, 60.0, 29.5);
    let mut ball = make_sphere(25.35, SEGMENTS);
    translate(&mut ball, 40.0, 30.0, 0.0);

    let pocketed = difference(&block, &ball).into_brep().unwrap();
    let pocket_mesh = vcad_kernel_tessellate::tessellate_brep(&pocketed, SEGMENTS);
    assert_volume_within(
        vcad_kernel_booleans::mesh_signed_volume(&pocket_mesh),
        107_481.0,
        0.02,
        "pocket cut",
    );

    let slot = make_cube(80.0, 18.0, 29.5);
    let result = difference(&pocketed, &slot);
    let mesh = result.to_mesh(SEGMENTS);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&mesh);
    assert_volume_within(vol, 70_852.0, 0.02, "slot through pocket");

    // No part of the lower hemisphere belongs in the result.
    let (min, max) = mesh_bbox(&mesh);
    assert!(
        min[2] > -1e-6,
        "result bbox extends below z=0 ({:.2}); lower hemisphere leaked into the result",
        min[2]
    );
    assert!(max[2] < 29.5 + 1e-6);
}

/// Case 2: sphere protruding through two cube faces sharing an edge.
/// Used to be a silent no-op (result identical to the bare cube).
#[test]
fn sphere_through_two_faces_sharing_an_edge() {
    let block = make_cube(80.0, 45.0, 29.5);
    let mut ball = make_sphere(25.35, SEGMENTS);
    translate(&mut ball, 40.0, 12.0, 0.0);

    let result = difference(&block, &ball);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&result.to_mesh(SEGMENTS));
    assert_volume_within(vol, 77_932.0, 0.02, "two-face protrusion");
}

/// Case 3a: sphere minus cylindrical plug entering from below.
/// Used to be a silent no-op (bare sphere returned).
#[test]
fn sphere_minus_cylinder_plug() {
    let ball = make_sphere(30.0, SEGMENTS);
    let mut plug = make_cylinder(10.0, 80.0, SEGMENTS);
    translate(&mut plug, 0.0, 0.0, -40.0);

    let result = difference(&ball, &plug);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&result.to_mesh(SEGMENTS));
    assert_volume_within(vol, 94_782.0, 0.02, "sphere minus cylinder");
}

/// Case 3b: cylinder minus perpendicular cylinder (cross-drilled hole).
/// Tool surface used to be merged into the result (volume GREW).
#[test]
fn cylinder_minus_perpendicular_cylinder() {
    let bar = make_cylinder(30.0, 61.0, SEGMENTS);
    let mut drill = make_cylinder(18.5, 40.0, SEGMENTS);
    apply_transform(&mut drill, &Transform::rotation_x(90.0_f64.to_radians()));

    let bar_vol = vcad_kernel_booleans::mesh_signed_volume(
        &vcad_kernel_tessellate::tessellate_brep(&bar, SEGMENTS),
    );
    let result = difference(&bar, &drill);
    let vol = vcad_kernel_booleans::mesh_signed_volume(&result.to_mesh(SEGMENTS));
    assert!(
        vol < bar_vol,
        "difference must remove material: result {vol:.1} >= bar {bar_vol:.1}"
    );
}

/// Chained box cuts through a spherical pocket must stay bounded: this
/// two-cut chain used to blow up to 246k triangles in 21 s (garbage
/// intersection curves feeding downstream booleans). The second cut is
/// symmetric to the first about the sphere's equator, so its expected
/// volume follows from Case 1's.
#[test]
fn chained_cuts_through_pocket_stay_bounded() {
    let block = make_cube(80.0, 60.0, 29.5);
    let mut ball = make_sphere(25.35, SEGMENTS);
    translate(&mut ball, 40.0, 30.0, 0.0);
    let pocketed = difference(&block, &ball).into_brep().unwrap();

    let slot_near = make_cube(80.0, 18.0, 29.5);
    let cut1 = difference(&pocketed, &slot_near).into_brep().unwrap();

    let mut slot_far = make_cube(80.0, 18.0, 29.5);
    translate(&mut slot_far, 0.0, 42.0, 0.0);
    let result = difference(&cut1, &slot_far);
    let mesh = result.to_mesh(SEGMENTS);

    // 70,852 − (107,481 − 70,852): the far slot removes the same volume as
    // the near one by symmetry about the sphere's y=30 equator plane.
    assert_volume_within(
        vcad_kernel_booleans::mesh_signed_volume(&mesh),
        34_223.0,
        0.02,
        "chained slots",
    );
    let tris = mesh.indices.len() / 3;
    assert!(
        tris < 20_000,
        "chained cuts exploded to {tris} triangles (correct output is a few thousand)"
    );
}

// ---------------------------------------------------------------------------
// Regression guard: cases that already worked and must keep working.
// ---------------------------------------------------------------------------

#[test]
fn guard_cube_minus_interior_sphere() {
    let block = make_cube(80.0, 60.0, 60.0);
    let mut ball = make_sphere(25.35, SEGMENTS);
    translate(&mut ball, 40.0, 30.0, 30.0);
    let vol =
        vcad_kernel_booleans::mesh_signed_volume(&difference(&block, &ball).to_mesh(SEGMENTS));
    assert_volume_within(vol, 219_763.0, 0.02, "interior sphere");
}

#[test]
fn guard_cube_minus_one_face_protruding_sphere() {
    let block = make_cube(80.0, 60.0, 29.5);
    let mut ball = make_sphere(25.35, SEGMENTS);
    translate(&mut ball, 40.0, 30.0, 0.0);
    let vol =
        vcad_kernel_booleans::mesh_signed_volume(&difference(&block, &ball).to_mesh(SEGMENTS));
    assert_volume_within(vol, 107_481.0, 0.02, "one-face protrusion");
}

#[test]
fn guard_sphere_minus_concentric_sphere() {
    let outer = make_sphere(30.0, SEGMENTS);
    let inner = make_sphere(25.0, SEGMENTS);
    let vol =
        vcad_kernel_booleans::mesh_signed_volume(&difference(&outer, &inner).to_mesh(SEGMENTS));
    // 4/3·π·(30³−25³) = 47_647.5
    assert_volume_within(vol, 47_647.0, 0.03, "concentric spheres");
}
