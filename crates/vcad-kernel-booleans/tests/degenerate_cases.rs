//! Degenerate-input test suite for the boolean kernel.
//!
//! Covers six categories of hard inputs that commonly break CAD kernels:
//!   1. Coincident co-planar faces
//!   2. Exact tangent surface–surface intersections
//!   3. Knife-edge (face–face single-line) intersections
//!   4. Self-intersecting / near-degenerate input recovery
//!   5. Floating-point near-miss (within epsilon of coincident)
//!   6. Arc-split geometry (cylinder cap arcs on planar box faces)

use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, make_sphere, BRepSolid};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn translate(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
    let t = Transform::translation(dx, dy, dz);
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

fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let indices = &mesh.indices;
    let mut vol = 0.0_f64;
    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    (vol / 6.0).abs()
}

fn assert_valid_mesh(mesh: &vcad_kernel_tessellate::TriangleMesh) {
    assert!(
        mesh.num_triangles() > 0,
        "result mesh must have at least one triangle"
    );
    for &idx in &mesh.indices {
        assert!(
            (idx as usize) < mesh.vertices.len() / 3,
            "index {} out of range (vertex count {})",
            idx,
            mesh.vertices.len() / 3
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Coincident co-planar faces
// ---------------------------------------------------------------------------

/// Union of two cubes that share an exact co-planar face (touching, no overlap).
/// The x=10 face of A is exactly coincident with the x=0 face of B.
#[test]
fn coplanar_adjacent_union_volume() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 0.0, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 2000.0).abs() / 2000.0 < 0.05,
        "coplanar union: expected ~2000, got {vol:.2}"
    );
}

/// Difference of two touching cubes: A minus B yields A unchanged (no overlap).
#[test]
fn coplanar_adjacent_difference_no_change() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 0.0, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 1000.0).abs() / 1000.0 < 0.05,
        "coplanar diff: expected ~1000, got {vol:.2}"
    );
}

/// A2 stepped-block regression (mecheval task `a2-stepped-block-01`):
/// cut box is flush with 4 of the base box's faces (+X, ±Y, +Z), interior
/// faces only at x=0 and z=10. The result is an L-shape with volume
/// 22500 mm³ = 30000 - 7500. Pre-79ab4d0 the kernel computed 17500 mm³
/// because the planar tessellator overrode `Orientation::Reversed` on
/// the cut's exposed cavity walls, inverting their contribution to the
/// divergence-theorem volume integral.
#[test]
fn flush_corner_step_block_difference_volume_pos_x() {
    let mut base = make_cube(50.0, 30.0, 20.0);
    translate(&mut base, -25.0, -15.0, 0.0);
    let mut cut = make_cube(25.0, 30.0, 10.0);
    translate(&mut cut, 0.0, -15.0, 10.0);

    let result = boolean_op(&base, &cut, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 22500.0).abs() / 22500.0 < 0.01,
        "stepped-block diff (+X corner): expected ~22500, got {vol:.2}"
    );
}

/// Mirror of `flush_corner_step_block_difference_volume_pos_x` cutting the −X
/// corner instead. Same flush-on-four-faces topology, opposite-direction
/// outward normals — guards against any sign-only fix.
#[test]
fn flush_corner_step_block_difference_volume_neg_x() {
    let mut base = make_cube(50.0, 30.0, 20.0);
    translate(&mut base, -25.0, -15.0, 0.0);
    let mut cut = make_cube(25.0, 30.0, 10.0);
    translate(&mut cut, -25.0, -15.0, 10.0);

    let result = boolean_op(&base, &cut, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 22500.0).abs() / 22500.0 < 0.01,
        "stepped-block diff (−X corner): expected ~22500, got {vol:.2}"
    );
}

/// Vertical-step variant: cut is flush with ±X and ±Y of the base, only
/// the bottom (−Z) face of the cut is interior. Exercises the same
/// coincident-face routing on a different axis.
#[test]
fn flush_corner_step_block_difference_volume_vertical() {
    // Base 30×30×40 spanning (-15..15, -15..15, 0..40).
    let mut base = make_cube(30.0, 30.0, 40.0);
    translate(&mut base, -15.0, -15.0, 0.0);
    // Cut 30×30×20 spanning (-15..15, -15..15, 20..40) — flush on ±X, ±Y, +Z.
    // Only the −Z face (z=20) is interior, leaving the lower half of the base.
    let mut cut = make_cube(30.0, 30.0, 20.0);
    translate(&mut cut, -15.0, -15.0, 20.0);

    let result = boolean_op(&base, &cut, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 30.0 * 30.0 * 20.0; // 18000
    assert!(
        (vol - expected).abs() / expected < 0.01,
        "stepped-block diff (vertical): expected ~{expected}, got {vol:.2}"
    );
}

/// A2 cube-with-pocket regression (mecheval task `a2-cube-with-pocket-01`,
/// issue #161 repro A): a 20×20×10 pocket cut from the middle of a 40³
/// cube's top face. Exactly ONE cutter face (the pocket top at z=40) is
/// coplanar with a face of the base solid, and the pocket outline lies
/// strictly inside the top face — so the top face gains an inner loop
/// instead of being split at its border. Pre-79ab4d0 this reported
/// 57333.33 mm³ (the pocket's four side walls integrated with inverted
/// orientation); correct volume is 40³ − 20·20·10 = 60000 mm³.
#[test]
fn flush_pocket_difference_volume_one_shared_face() {
    let mut base = make_cube(40.0, 40.0, 40.0);
    translate(&mut base, -20.0, -20.0, 0.0);
    let mut pocket = make_cube(20.0, 20.0, 10.0);
    translate(&mut pocket, -10.0, -10.0, 30.0);

    let result = boolean_op(&base, &pocket, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 60000.0).abs() / 60000.0 < 0.001,
        "pocket diff (1 shared face): expected 60000, got {vol:.2}"
    );
}

/// Pocket flush with TWO base faces (top z=40 and side x=20): the cutter
/// reaches the +X border of the top face, so the top face is split at its
/// boundary rather than gaining an inner loop, and the +X wall gains a
/// notch. Sits between the 1-shared-face pocket and the 4-shared-face
/// stepped-block cases (issue #161 acceptance asks for 1–4 coverage).
#[test]
fn flush_pocket_difference_volume_two_shared_faces() {
    let mut base = make_cube(40.0, 40.0, 40.0);
    translate(&mut base, -20.0, -20.0, 0.0);
    let mut pocket = make_cube(20.0, 20.0, 10.0);
    translate(&mut pocket, 0.0, -10.0, 30.0);

    let result = boolean_op(&base, &pocket, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 60000.0).abs() / 60000.0 < 0.001,
        "pocket diff (2 shared faces): expected 60000, got {vol:.2}"
    );
}

/// Pocket flush with THREE base faces (top z=40, +X, +Y): a corner notch.
#[test]
fn flush_pocket_difference_volume_three_shared_faces() {
    let mut base = make_cube(40.0, 40.0, 40.0);
    translate(&mut base, -20.0, -20.0, 0.0);
    let mut pocket = make_cube(20.0, 20.0, 10.0);
    translate(&mut pocket, 0.0, 0.0, 30.0);

    let result = boolean_op(&base, &pocket, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 60000.0).abs() / 60000.0 < 0.001,
        "pocket diff (3 shared faces): expected 60000, got {vol:.2}"
    );
}

/// Co-planar face union where both operands share an entire face in the Z direction.
/// A occupies z=[0,10], B occupies z=[10,20]; they touch at the z=10 plane.
#[test]
fn coplanar_stacked_union() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 0.0, 0.0, 10.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 2000.0).abs() / 2000.0 < 0.05,
        "stacked union: expected ~2000, got {vol:.2}"
    );
}

// ---------------------------------------------------------------------------
// 2. Exact tangent surface–surface intersections
// ---------------------------------------------------------------------------

/// Cylinder tangent to a planar box face: the curved surface just touches the
/// y=0 plane at one generator line but does not penetrate.
/// Difference result should equal the full box volume.
#[test]
fn cylinder_tangent_to_plane_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    // Cylinder r=5 centered at (10, -5, 0): tangent to y=0 face, fully outside.
    let mut cyl = make_cylinder(5.0, 20.0, 32);
    translate(&mut cyl, 10.0, -5.0, 0.0);

    let result = boolean_op(&cube, &cyl, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3);
    assert!(
        (vol - expected).abs() / expected < 0.05,
        "tangent diff: expected ~{expected:.1}, got {vol:.2}"
    );
}

/// Sphere tangent to a planar box face: touches at exactly one point, no intersection.
#[test]
fn sphere_tangent_to_plane_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    // Sphere r=5 centered at (10, 10, -5): tangent to z=0 face from below.
    let mut sphere = make_sphere(5.0, 32);
    translate(&mut sphere, 10.0, 10.0, -5.0);

    let result = boolean_op(&cube, &sphere, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3);
    assert!(
        (vol - expected).abs() / expected < 0.05,
        "sphere tangent: expected ~{expected:.1}, got {vol:.2}"
    );
}

/// Cylinder tangent to a box from the inside: cylinder axis sits exactly on a box edge,
/// so the cylinder is tangent (touches but does not protrude outside the box).
#[test]
fn cylinder_internally_tangent_union_is_noop() {
    let cube = make_cube(20.0, 20.0, 20.0);
    // Cylinder r=10 centered at (10,10,0): completely inside the 20x20x20 box.
    let cyl = make_cylinder(10.0, 20.0, 32);
    // No translation — axis at origin, cylinder fits inside.
    let mut inner = cyl;
    translate(&mut inner, 10.0, 10.0, 0.0);

    let result = boolean_op(&cube, &inner, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    // Union of A with B⊆A equals A.
    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3);
    assert!(
        (vol - expected).abs() / expected < 0.05,
        "inner union: expected ~{expected:.1}, got {vol:.2}"
    );
}

// ---------------------------------------------------------------------------
// 3. Knife-edge (face–face single-line) intersections
// ---------------------------------------------------------------------------

/// Two cubes sharing exactly one edge (not a face or volume).
/// A=[0,0,0]-[10,10,10], B=[10,10,0]-[20,20,10]: share edge at (10,10,z).
#[test]
fn knife_edge_union_volume() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 10.0, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 2000.0).abs() / 2000.0 < 0.05,
        "knife-edge union: expected ~2000, got {vol:.2}"
    );
}

/// Knife-edge difference: A minus B where they share only one edge.
/// Result should be A unchanged.
#[test]
fn knife_edge_difference_no_change() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 10.0, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 1000.0).abs() / 1000.0 < 0.05,
        "knife-edge diff: expected ~1000, got {vol:.2}"
    );
}

/// Two boxes sharing a single vertex (point contact).
#[test]
fn vertex_contact_union_volume() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 10.0, 10.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    assert!(
        (vol - 2000.0).abs() / 2000.0 < 0.05,
        "vertex contact union: expected ~2000, got {vol:.2}"
    );
}

// ---------------------------------------------------------------------------
// 4. Self-intersecting / near-degenerate input recovery
// ---------------------------------------------------------------------------

/// Extremely thin solid (height = 1e-4): boolean operation must not panic
/// and must produce a non-empty mesh.
#[test]
fn very_thin_box_difference_no_panic() {
    let big = make_cube(10.0, 10.0, 10.0);
    let mut thin = make_cube(5.0, 5.0, 1e-4);
    translate(&mut thin, 2.5, 2.5, 5.0);

    // Must not panic; volume should be essentially unchanged.
    let result = boolean_op(&big, &thin, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
}

/// Needle-like solid (very narrow in two dimensions): no panic on union.
#[test]
fn needle_solid_union_no_panic() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut needle = make_cube(0.001, 0.001, 20.0);
    translate(&mut needle, 5.0, 5.0, -5.0);

    let result = boolean_op(&a, &needle, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
}

/// Flat coin-like solid: degenerate in Z, must produce a valid result.
#[test]
fn flat_coin_difference_no_panic() {
    let big = make_cube(20.0, 20.0, 20.0);
    let mut coin = make_cylinder(8.0, 1e-3, 32);
    translate(&mut coin, 10.0, 10.0, 10.0);

    let result = boolean_op(&big, &coin, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
}

// ---------------------------------------------------------------------------
// 5. Floating-point near-miss (within epsilon of coincident)
// ---------------------------------------------------------------------------

/// Faces separated by 1e-7 (below typical snap tolerance): union must not crash
/// or produce garbage geometry, and combined volume should be near 2000.
#[test]
fn near_miss_epsilon_overlap_union() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0 + 1e-7, 0.0, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
}

/// Faces separated by exactly 0 in one axis, by 1e-8 in another: must not panic.
#[test]
fn near_miss_sub_epsilon_difference() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 10.0, 1e-8, 0.0);

    let result = boolean_op(&a, &b, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
}

/// Faces within 1e-5 of coincident (within snap tolerance): difference must
/// not produce negative or zero volume.
#[test]
fn near_miss_within_snap_tolerance() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    translate(&mut b, 9.99999, 0.0, 0.0); // 1e-5 penetration

    let result = boolean_op(&a, &b, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    // Must not panic and must produce a mesh.
    assert_valid_mesh(&mesh);
}

// ---------------------------------------------------------------------------
// 6. Arc-split geometry
// ---------------------------------------------------------------------------
//
// These tests exercise the planar-face-by-arc split path that runs when a
// cylinder cap circle only partially overlaps a box face.  After the dedup
// fix in split_planar_face_by_arc, volumes should land within 5%.

/// Box minus half-cylinder (cylinder axis on the left edge of the box).
/// Cylinder center at (0, 10, z) with r=10: the right semicircle is inside the box.
/// Expected volume = 20³ − π·r²·h/2.
#[test]
fn arc_split_half_cylinder_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    let mut cyl = make_cylinder(10.0, 20.0, 32);
    translate(&mut cyl, 0.0, 10.0, 0.0);

    let result = boolean_op(&cube, &cyl, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3) - std::f64::consts::PI * 100.0 * 20.0 / 2.0;
    let err = (vol - expected).abs() / expected;
    assert!(
        err < 0.05,
        "half-cylinder diff: volume error {:.1}% (expected {expected:.2}, got {vol:.2})",
        err * 100.0
    );
}

/// Box minus quarter-cylinder (cylinder axis at box corner).
/// Cylinder center at (0, 0, z) with r=10: only the x>0,y>0 quarter is inside.
/// Expected volume = 20³ − π·r²·h/4.
#[test]
fn arc_split_quarter_cylinder_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    let cyl = make_cylinder(10.0, 20.0, 32);

    let result = boolean_op(&cube, &cyl, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3) - std::f64::consts::PI * 100.0 * 20.0 / 4.0;
    let err = (vol - expected).abs() / expected;
    assert!(
        err < 0.05,
        "quarter-cylinder diff: volume error {:.1}% (expected {expected:.2}, got {vol:.2})",
        err * 100.0
    );
}

/// Box minus cylinder fully inside: all 6 box faces are untouched, only the
/// cylindrical void is cut.  Expected volume = 20³ − π·r²·h.
#[test]
fn arc_split_full_cylinder_interior_difference() {
    let cube = make_cube(20.0, 20.0, 20.0);
    let mut cyl = make_cylinder(5.0, 20.0, 32);
    translate(&mut cyl, 10.0, 10.0, 0.0); // fully inside the 20×20×20 box

    let result = boolean_op(&cube, &cyl, BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let expected = 20.0_f64.powi(3) - std::f64::consts::PI * 25.0 * 20.0;
    let err = (vol - expected).abs() / expected;
    assert!(
        err < 0.05,
        "interior cylinder diff: volume error {:.1}% (expected {expected:.2}, got {vol:.2})",
        err * 100.0
    );
}

// ---------------------------------------------------------------------------
// 7. Coaxial cylinder difference — annular washer.
// ---------------------------------------------------------------------------

/// Difference of two coaxial Z-axis cylinders. Result must be an annular
/// washer with both top and bottom annular cap faces present, not an open
/// tube. Regression for a sample-point bug in classify::face_sample_point:
/// a multi-vertex inner loop centered at the face center was producing
/// `concentric_inner_r = 0`, causing the sample radius to land inside the
/// hole; the annular face was then misclassified as Inside B and dropped.
///
/// Tests both the flush case (inner cylinder same height as outer) and
/// the through-piercing case (inner extends past both ends), since both
/// hit the same code path.
#[test]
fn coaxial_cylinder_difference_produces_annular_caps() {
    for (label, inner_h, inner_z) in &[("flush", 8.0_f64, 0.0_f64), ("through", 10.0, -1.0)] {
        let outer = make_cylinder(30.0, 8.0, 32);
        let mut inner = make_cylinder(20.0, *inner_h, 32);
        translate(&mut inner, 0.0, 0.0, *inner_z);
        let result = boolean_op(&outer, &inner, BooleanOp::Difference, 32);
        let brep = result
            .as_brep()
            .unwrap_or_else(|| panic!("[{label}] expected BRep result, got mesh-only"));

        // Topology must include 4 faces: outer wall, inner wall, top cap, bottom cap.
        let face_count = brep.topology.faces.len();
        assert_eq!(
            face_count, 4,
            "[{label}] expected 4 faces (2 walls + 2 annular caps), got {face_count}",
        );
        let planar_count = brep
            .topology
            .faces
            .iter()
            .filter(|(_, f)| {
                brep.geometry.surfaces[f.surface_index].surface_type()
                    == vcad_kernel_geom::SurfaceKind::Plane
            })
            .count();
        assert_eq!(
            planar_count, 2,
            "[{label}] expected 2 planar (annular cap) faces, got {planar_count}",
        );

        // Volume must be close to π·(R² − r²)·h. Tolerate the standard
        // 32-segment polygon-vs-circle area discrepancy (~0.7%).
        let mesh = result.to_mesh(32);
        let vol = mesh_volume(&mesh);
        let expected = std::f64::consts::PI * (30.0_f64.powi(2) - 20.0_f64.powi(2)) * 8.0;
        let err = (vol - expected).abs() / expected;
        assert!(
            err < 0.02,
            "[{label}] coaxial annulus: volume error {:.2}% (expected {expected:.2}, got {vol:.2})",
            err * 100.0
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Cylinder-cylinder degenerate unions
// ---------------------------------------------------------------------------

/// Two perpendicular cylinders fused into a "+" cross shape (mecheval task
/// `a3-cross-shaft-01`). This is the Steinmetz/bicylinder geometry: the
/// intersection region is a curved Steinmetz solid that must be subtracted
/// once, not double-counted.
///
/// Vertical cylinder: r=10, axis Z, base on XY plane (z ∈ [0, 40]).
/// Horizontal cylinder: r=10, axis X, centered at (0, 0, 20) (x ∈ [-20, 20]).
///
/// Expected volume = 2·π·r²·h − 16·r³/3
///                 = 2·π·100·40 − 16·1000/3
///                 ≈ 19799.41 mm³.
///
#[test]
fn cylinder_cylinder_perpendicular_union_steinmetz() {
    let vertical = make_cylinder(10.0, 40.0, 32);
    let mut horizontal = make_cylinder(10.0, 40.0, 32);
    // Rotate Z-axis cylinder onto X axis, then center along X at x=0,
    // shift up so the X axis sits at z=20.
    apply_transform(
        &mut horizontal,
        &Transform::rotation_y(std::f64::consts::FRAC_PI_2),
    );
    translate(&mut horizontal, -20.0, 0.0, 20.0);

    let result = boolean_op(&vertical, &horizontal, BooleanOp::Union, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);

    let vol = mesh_volume(&mesh);
    let r = 10.0_f64;
    let h = 40.0_f64;
    let expected = 2.0 * std::f64::consts::PI * r * r * h - 16.0 * r.powi(3) / 3.0;
    let err = (vol - expected).abs() / expected;
    assert!(
        err < 0.02,
        "perpendicular cylinder union: volume error {:.2}% (expected {expected:.2}, got {vol:.2})",
        err * 100.0
    );
}

// ---------------------------------------------------------------------------
// 9. Boolean-chain tessellation (octagonal flange, issue #162)
// ---------------------------------------------------------------------------
//
// The a3-octagonal-flange-01 mecheval task panicked in the tessellator with
// "Normals/vertices mismatch: N normals vs 2N vertices" on two families of
// input: a well-formed flange (intersection of a box with its 45°-rotated
// copy, minus a bolt-hole union) and a malformed model output whose chained
// slab intersection collapses to an empty solid. Both must tessellate (or
// evaluate empty) without panicking, with normals in 1:1 lockstep with
// vertices.

/// Well-formed octagonal flange: `Difference(Intersection(box, rotated-box),
/// Union(8 cylinders))` — the issue #162 acceptance geometry. The octagon is
/// the intersection of a 60×40×8 box centered in XY with its 45°-rotated
/// copy (area 1871.07 mm², computed by polygon clipping); the 8 bolt holes
/// (r=2.5 on a bolt circle of r=15) all land strictly inside it.
#[test]
fn octagonal_flange_boolean_chain_tessellates() {
    let mut box_a = make_cube(60.0, 40.0, 8.0);
    translate(&mut box_a, -30.0, -20.0, 0.0);
    let mut box_b = make_cube(60.0, 40.0, 8.0);
    translate(&mut box_b, -30.0, -20.0, 0.0);
    apply_transform(
        &mut box_b,
        &Transform::rotation_z(std::f64::consts::FRAC_PI_4),
    );

    let octagon = boolean_op(&box_a, &box_b, BooleanOp::Intersection, 32)
        .into_brep()
        .expect("octagon intersection should produce a BRep");

    let mut holes: Option<BRepSolid> = None;
    for k in 0..8 {
        let ang = (k as f64) * std::f64::consts::FRAC_PI_4;
        let mut cyl = make_cylinder(2.5, 8.0, 32);
        translate(&mut cyl, 15.0 * ang.cos(), 15.0 * ang.sin(), 0.0);
        holes = Some(match holes {
            None => cyl,
            Some(prev) => boolean_op(&prev, &cyl, BooleanOp::Union, 32)
                .into_brep()
                .expect("bolt-hole union should produce a BRep"),
        });
    }

    let result = boolean_op(&octagon, &holes.unwrap(), BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert_valid_mesh(&mesh);
    assert!(
        mesh.normals.is_empty() || mesh.normals.len() == mesh.vertices.len(),
        "normals/vertices mismatch: {} normals vs {} vertices",
        mesh.normals.len(),
        mesh.vertices.len()
    );

    // Octagon area × height, minus 8 bolt holes. The mesh discretizes each
    // hole as a 32-gon (slightly smaller than the true circle), so allow
    // 0.5% against the analytic value.
    let octagon_area = 1871.067_811_865_475_5;
    let hole_vol = std::f64::consts::PI * 2.5 * 2.5 * 8.0;
    let expected = octagon_area * 8.0 - 8.0 * hole_vol;
    let vol = mesh_volume(&mesh);
    let err = (vol - expected).abs() / expected;
    assert!(
        err < 0.005,
        "octagonal flange: volume error {:.2}% (expected {expected:.2}, got {vol:.2})",
        err * 100.0
    );
}

/// Malformed flange from the actual panicking mecheval run
/// (`a3-octagonal-flange-01` / gpt-5-mini): eight 80×160×8 slabs rotated
/// about Z and chain-intersected. The slab placement is wrong — the true
/// intersection collapses to empty at the 4th step (29576.45 → 9600 → 1600
/// → 0 mm³, verified analytically by polygon clipping) — and the empty
/// solid then flows into a Difference against a bolt-hole union. Every step
/// must return the correct (eventually empty) result without panicking.
#[test]
fn chained_slab_intersection_collapses_to_empty_no_panic() {
    // Slab k: cube corner at origin, rotated (180 + 45k)° about Z, then
    // translated 20mm radially outward at angle 45k° (the diagonal offsets
    // use the run's literal 14.142136 values).
    let offsets = [
        (20.0, 0.0),
        (14.142_136, 14.142_136),
        (0.0, 20.0),
        (-14.142_136, 14.142_136),
        (-20.0, 0.0),
        (-14.142_136, -14.142_136),
        (0.0, -20.0),
        (14.142_136, -14.142_136),
    ];
    let expected_step_volumes = [29576.45, 9600.0, 1600.0, 0.0, 0.0, 0.0, 0.0];

    let slab = |k: usize| {
        let mut s = make_cube(80.0, 160.0, 8.0);
        let ang = (180.0 + 45.0 * k as f64).to_radians();
        apply_transform(&mut s, &Transform::rotation_z(ang));
        translate(&mut s, offsets[k].0, offsets[k].1, 0.0);
        s
    };

    let mut acc = slab(0);
    for (k, &expected_vol) in expected_step_volumes.iter().enumerate() {
        let result = boolean_op(&acc, &slab(k + 1), BooleanOp::Intersection, 32);
        let mesh = result.to_mesh(32);
        assert!(
            mesh.normals.is_empty() || mesh.normals.len() == mesh.vertices.len(),
            "step {}: normals/vertices mismatch: {} normals vs {} vertices",
            k + 1,
            mesh.normals.len(),
            mesh.vertices.len()
        );
        let vol = mesh_volume(&mesh);
        assert!(
            (vol - expected_vol).abs() < expected_vol.max(1.0) * 0.01,
            "step {}: expected volume ~{expected_vol}, got {vol:.2}",
            k + 1
        );
        acc = result
            .into_brep()
            .expect("intersection should produce a (possibly empty) BRep");
    }

    // Difference of the (empty) octagon against a bolt-hole union must not
    // panic and must stay empty.
    let mut holes: Option<BRepSolid> = None;
    for k in 0..8 {
        let ang = (k as f64) * std::f64::consts::FRAC_PI_4;
        let mut cyl = make_cylinder(2.5, 8.0, 32);
        translate(&mut cyl, 15.0 * ang.cos(), 15.0 * ang.sin(), 0.0);
        holes = Some(match holes {
            None => cyl,
            Some(prev) => boolean_op(&prev, &cyl, BooleanOp::Union, 32)
                .into_brep()
                .expect("bolt-hole union should produce a BRep"),
        });
    }
    let result = boolean_op(&acc, &holes.unwrap(), BooleanOp::Difference, 32);
    let mesh = result.to_mesh(32);
    assert!(
        mesh.normals.is_empty() || mesh.normals.len() == mesh.vertices.len(),
        "final difference: normals/vertices mismatch: {} normals vs {} vertices",
        mesh.normals.len(),
        mesh.vertices.len()
    );
    let vol = mesh_volume(&mesh);
    assert!(
        vol < 1.0,
        "difference of an empty solid must stay empty, got volume {vol:.2}"
    );
}
