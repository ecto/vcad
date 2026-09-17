//! Regressions from the rana-60 stator: prismatic bodies that share their top
//! and bottom planes, unioned one at a time — a 256-segment ring, twelve posts
//! reaching it from the inside, and at every post root two fillet blocks (a
//! cube minus a cylinder tangent to both the post's side and the ring's bore).
//!
//! The part solved to 6600 mm³ against a true 7850 mm³, and six separate
//! defects were behind it. Each gets its smallest reproducer here. As
//! everywhere in this crate the assertions are on VOLUME against a closed
//! form: every one of these failures was a plausible-looking mesh of the
//! wrong solid.
//!
//! 1. `find_line_polygon_crossings` read a face's plane off its first three
//!    vertices. Splitting a neighbouring cap imprints vertices onto the shared
//!    edge, so the loop opened with three collinear points, the face read as
//!    degenerate, and the cut was refused without a word — leaving the
//!    un-notched side face behind as an interior membrane.
//! 2. The mesh CSG classified every fragment with a single +X ray. Through an
//!    operand carrying that membrane, any ray threading it read the wrong
//!    parity, and cap fragments nowhere near the contact were dropped.
//! 3. A circle is not offered as a split to a planar face that already has it
//!    as a boundary arc ("≥3 vertices on the circle"). A fillet tangent to the
//!    circle samples its own rim densely enough that a run of its vertices
//!    sits inside that tolerance, so the bore never cut the fillet block's
//!    caps.
//! 4. A union's volume bound is loose from below; a twelve-lump operand lost
//!    the cap over every overlap region (−1.2%) inside it. A cracked union is
//!    now refereed by the mesh boolean of the same operands.
//! 5. A coplanar patch CONTAINED in a larger face read `OnSame` and won the
//!    union's "operand A keeps the copy" tie-break, while the larger face,
//!    which is not coincident with anything, was kept as `Outside` — so the
//!    overlap was covered twice whenever the small one sat on the A side.
//!    The tie-break now goes to the larger face (`OnSameInner`).
//! 6. That larger face was then cut by a full-width chord for every wall of
//!    the other solid that merely ENDS on its plane. Those chords partition
//!    nothing the tie-break has not already settled, and enough of them
//!    crossing inside one face fragment it past the point where the pieces
//!    classify coherently.

use vcad_kernel_booleans::mesh::csg::mesh_csg;
use vcad_kernel_booleans::{
    boolean_op, boolean_op_reported, mesh_report, mesh_signed_volume, BooleanOp, BooleanResult,
};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

const SEGMENTS: u32 = 256;
const Z0: f64 = 11.1;
const H: f64 = 6.0;
const R_OUT: f64 = 28.75;
const R_IN: f64 = 24.0;

fn transformed(mut brep: BRepSolid, t: &Transform) -> BRepSolid {
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(t))
        .collect();
    brep
}

fn brep(result: BooleanResult) -> BRepSolid {
    let BooleanResult::BRep(b) = result;
    *b
}

fn union(a: &BRepSolid, b: &BRepSolid) -> BRepSolid {
    brep(boolean_op(a, b, BooleanOp::Union, SEGMENTS).expect("union"))
}

fn volume(solid: &BRepSolid) -> f64 {
    mesh_signed_volume(&tessellate_brep(solid, SEGMENTS))
}

/// The stator's ring: r 24..28.75, z 11.1..17.1.
fn ring() -> BRepSolid {
    let outer = transformed(
        make_cylinder(R_OUT, H, SEGMENTS),
        &Transform::translation(0.0, 0.0, Z0),
    );
    let bore = transformed(
        make_cylinder(R_IN, H + 0.02, SEGMENTS),
        &Transform::translation(0.0, 0.0, Z0 - 0.01),
    );
    brep(boolean_op(&outer, &bore, BooleanOp::Difference, SEGMENTS).expect("ring"))
}

/// One post: 8 × 5 × 6, x 16.5..24.5. Its outer end sits 0.5 mm inside the
/// ring wall.
fn post() -> BRepSolid {
    transformed(
        make_cube(8.0, 5.0, H),
        &Transform::translation(16.5, -2.5, Z0),
    )
}

/// A fillet block on the post's +y (`side = 1.0`) or −y side: a 1.35 × 1.5124
/// cube reaching 0.3 mm into the post, minus an r 1.05 cylinder that is
/// tangent to the post's side plane AND to the ring's bore. The arc ends
/// exactly on the block's outer corner, which is exactly on the bore.
fn fillet_block(side: f64) -> BRepSolid {
    let y_lo = if side > 0.0 { 2.2 } else { -3.7124 };
    let cube = transformed(
        make_cube(1.35, 1.5124, H),
        &Transform::translation(22.6738, y_lo, Z0),
    );
    let cutter = transformed(
        make_cylinder(1.05, H + 0.02, 32),
        &Transform::translation(22.6738, 3.55 * side, Z0 - 0.01),
    );
    brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("fillet block"))
}

/// Midpoint-rule integral of `f` over `[lo, hi]`.
fn integrate(lo: f64, hi: f64, f: impl Fn(f64) -> f64) -> f64 {
    let steps = 20_000;
    let dy = (hi - lo) / steps as f64;
    (0..steps).map(|i| f(lo + (i as f64 + 0.5) * dy) * dy).sum()
}

/// Volume a post shares with the ring: its end beyond r 24.
fn post_ring_overlap() -> f64 {
    H * integrate(-2.5, 2.5, |y| 24.5 - (R_IN * R_IN - y * y).sqrt())
}

/// Volume a fillet block shares with the ring: its outer corner beyond r 24.
fn block_ring_overlap() -> f64 {
    H * integrate(2.2, 3.7124, |y| 24.0238 - (R_IN * R_IN - y * y).sqrt())
}

/// Volume of a fillet block outside the post (the 0.3 mm strip inside it is
/// 1.35 × 0.3 × 6).
fn block_outside_post(block: &BRepSolid) -> f64 {
    volume(block) - 1.35 * 0.3 * H
}

fn assert_volume_within(actual: f64, expected: f64, tol_frac: f64, label: &str) {
    let rel = (actual - expected).abs() / expected.abs();
    assert!(
        rel <= tol_frac,
        "{label}: volume {actual:.2} differs from closed form {expected:.2} by {:.3}% (allowed {:.3}%)",
        rel * 100.0,
        tol_frac * 100.0
    );
}

/// Defect 1. `post ∪ A` was exact and `post ∪ (A ∪ B)` was exact, but
/// `(post ∪ A) ∪ B` came out 6.75 mm³ heavy with open edges: the second
/// block's cut of the post's side face was refused.
#[test]
fn second_fillet_block_cuts_a_face_with_imprinted_vertices() {
    let (a, b) = (fillet_block(1.0), fillet_block(-1.0));
    let expected = 240.0 + block_outside_post(&a) + block_outside_post(&b);

    let one_at_a_time = union(&union(&post(), &a), &b);
    let mesh = tessellate_brep(&one_at_a_time, SEGMENTS);
    assert_volume_within(mesh_signed_volume(&mesh), expected, 0.001, "(post ∪ A) ∪ B");
    assert_eq!(
        mesh_report(&mesh).open_edges,
        0,
        "(post ∪ A) ∪ B is not closed — a side-face split was refused"
    );

    let blocks_first = union(&post(), &union(&a, &b));
    assert_volume_within(volume(&blocks_first), expected, 0.001, "post ∪ (A ∪ B)");
}

/// Defect 2. An operand with a hole in it: every +X ray from the far cube
/// enters the open cube through its missing face and leaves through the
/// opposite one — one crossing, "inside" — so a single-ray classifier drops
/// the far cube entirely (1000 mm³). The missing face lies in the plane x = 0,
/// through the origin, so it contributes nothing to the divergence integral
/// and the open cube still reads exactly 1000.
#[test]
fn mesh_classification_survives_a_cracked_operand() {
    let cube = tessellate_brep(&make_cube(10.0, 10.0, 10.0), 16);
    let mut open_cube = TriangleMesh::new();
    open_cube.vertices = cube.vertices.clone();
    for tri in cube.indices.chunks(3) {
        let on_x0 = tri
            .iter()
            .all(|&i| cube.vertices[i as usize * 3].abs() < 1e-6);
        if !on_x0 {
            open_cube.indices.extend_from_slice(tri);
        }
    }
    assert_eq!(open_cube.indices.len(), cube.indices.len() - 6);

    let far = tessellate_brep(
        &transformed(
            make_cube(10.0, 6.0, 6.0),
            &Transform::translation(-20.0, 2.0, 2.0),
        ),
        16,
    );
    let out = mesh_csg(&open_cube, &far, BooleanOp::Union);
    assert_volume_within(
        mesh_signed_volume(&out),
        1360.0,
        0.01,
        "open cube ∪ far cube",
    );
}

/// The mesh CSG itself on coplanar caps: twelve posts into the ring.
#[test]
fn mesh_union_of_ring_and_twelve_posts() {
    let ring = tessellate_brep(&ring(), SEGMENTS);
    let mut posts = post();
    for k in 1..12 {
        let next = transformed(
            post(),
            &Transform::rotation_z((30.0 * k as f64).to_radians()),
        );
        posts = union(&posts, &next);
    }
    let posts = tessellate_brep(&posts, SEGMENTS);
    let expected = mesh_signed_volume(&ring) + 12.0 * (240.0 - post_ring_overlap());
    let out = mesh_csg(&posts, &ring, BooleanOp::Union);
    assert_volume_within(mesh_signed_volume(&out), expected, 0.02, "posts ∪ ring");
}

/// Defect 3. The bore circle has to cut the block's caps even though the
/// block's own fillet arc hugs that circle near the tangent point. Unsplit,
/// the cap over block ∩ ring went missing: +0.64 mm³ and 20 open edges.
#[test]
fn bore_circle_cuts_a_cap_whose_fillet_is_tangent_to_it() {
    let (ring, block) = (ring(), fillet_block(1.0));
    let expected = volume(&ring) + volume(&block) - block_ring_overlap();
    let got = volume(&union(&ring, &block));
    assert!(
        (got - expected).abs() < 0.2,
        "ring ∪ tangent fillet block: {got:.3} mm³, closed form {expected:.3} — \
         the bore circle did not split the block's caps"
    );
}

/// Defect 5. Operand order. `ring ∪ post` was exact and closed while
/// `post ∪ ring` came out 4951.99 mm³ against 4946.55 with 30 open and 24
/// over-used edges. The excess is exactly the flux of one doubled cap pair —
/// 2.717 mm² of overlap at z = 11.1 and z = 17.1 integrate to 5.44 mm³ —
/// because the post's cap patch is contained in the ring's annular cap but
/// not the other way round: the patch read `OnSame` and won the union's
/// "operand A keeps the coplanar copy" tie-break, while the annulus, which
/// covers the same ground and more, read `Outside` and was kept too. The
/// tie-break now goes to the LARGER face (`OnSameInner`), which is what made
/// the other order right.
#[test]
fn a_contained_coplanar_patch_does_not_double_the_larger_face() {
    let (ring, post) = (ring(), post());
    let expected = volume(&ring) + volume(&post) - post_ring_overlap();
    for (label, x, y) in [("ring ∪ post", &ring, &post), ("post ∪ ring", &post, &ring)] {
        let (result, report) =
            boolean_op_reported(x, y, BooleanOp::Union, SEGMENTS).expect("union");
        assert_eq!(report.reason, None, "{label} left the analytic path");
        let mesh = tessellate_brep(&brep(result), SEGMENTS);
        assert_volume_within(mesh_signed_volume(&mesh), expected, 0.001, label);
        assert_eq!(
            mesh_report(&mesh).open_edges,
            0,
            "{label} is not closed — the overlap cap is covered twice"
        );
    }
}

/// Defect 6. The same twelve-lump operand as defect 4, but held to the
/// ANALYTIC result — `reason == None` says the mesh referee never had to
/// overrule, so this stands whether or not `VCAD_NO_UNION_REFEREE` is set.
///
/// A post's side and end planes each offered the ring's cap a chord spanning
/// its full width, though the post reaches only 0.5 mm into a 4.75 mm wall.
/// Two posts 30° apart are enough — their end planes cross at r 25.36, inside
/// the cap — and twelve of them fragmented the cap and the bore wall past the
/// point where the pieces classify coherently: the union lost 1.17% and kept
/// 1358 open edges. Those chords partition nothing that matters (the coplanar
/// tie-break already settles the caps) and are no longer recorded.
#[test]
fn twelve_filleted_posts_stay_analytic() {
    let ring = ring();
    let (a, b) = (fillet_block(1.0), fillet_block(-1.0));
    let filleted = union(&union(&post(), &a), &b);
    let per_post = volume(&filleted) - post_ring_overlap() - 2.0 * block_ring_overlap();

    let mut posts = filleted.clone();
    for k in 1..12 {
        let t = Transform::rotation_z((30.0 * k as f64).to_radians());
        posts = union(&posts, &transformed(filleted.clone(), &t));
    }
    let expected = volume(&ring) + 12.0 * per_post;

    for (label, x, y) in [
        ("ring ∪ posts", &ring, &posts),
        ("posts ∪ ring", &posts, &ring),
    ] {
        let (result, report) =
            boolean_op_reported(x, y, BooleanOp::Union, SEGMENTS).expect("union");
        assert_eq!(
            report.reason, None,
            "{label} fell off the analytic path onto the mesh fallback"
        );
        assert_volume_within(volume(&brep(result)), expected, 0.005, label);
    }
}

/// Defect 4. Twelve filleted posts as ONE operand. The analytic union kept
/// 1358 open edges and lost the caps over the overlap regions (−1.2%), inside
/// the union volume bound; the watertight mesh referee now overrules it.
#[test]
fn twelve_filleted_posts_into_the_ring() {
    let ring = ring();
    let (a, b) = (fillet_block(1.0), fillet_block(-1.0));
    let filleted = union(&union(&post(), &a), &b);
    let per_post = volume(&filleted) - post_ring_overlap() - 2.0 * block_ring_overlap();

    let mut posts = filleted.clone();
    for k in 1..12 {
        let t = Transform::rotation_z((30.0 * k as f64).to_radians());
        posts = union(&posts, &transformed(filleted.clone(), &t));
    }
    let expected = volume(&ring) + 12.0 * per_post;

    for (label, x, y) in [
        ("ring ∪ posts", &ring, &posts),
        ("posts ∪ ring", &posts, &ring),
    ] {
        let (result, _report) =
            boolean_op_reported(x, y, BooleanOp::Union, SEGMENTS).expect("union");
        assert_volume_within(volume(&brep(result)), expected, 0.005, label);
    }
}
