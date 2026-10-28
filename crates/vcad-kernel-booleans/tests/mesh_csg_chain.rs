//! Chained mesh booleans over operands that share their caps.
//!
//! `mesh_csg` splits each operand's triangles by the carrier planes of the
//! other's and (before the coplanar re-merge) never put them back. Every
//! operand of the rana-60 stator spans z 11.1..17.1, so each boolean in a
//! chain re-split the accumulated cap debris of all its predecessors: the
//! authored 50-step fold had not finished after 9 minutes at 1.5 GB and
//! climbing, and the part came out ~103k triangles where the analytic
//! equivalent is ~14.7k.
//!
//! The reproducer is the stator's ring with its twelve posts, folded in one
//! at a time. As everywhere in this crate the assertion that matters is on
//! VOLUME against a closed form — a cheap mesh is worthless if it is a mesh
//! of the wrong solid — with a triangle-count ceiling next to it.

use vcad_kernel_booleans::mesh::csg::mesh_csg_remerged;
use vcad_kernel_booleans::{boolean_op, mesh_signed_volume, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

const SEGMENTS: u32 = 256;
const Z0: f64 = 11.1;
const H: f64 = 6.0;
const R_OUT: f64 = 28.75;
const R_IN: f64 = 24.0;
const POST_VOLUME: f64 = 8.0 * 5.0 * H;

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

/// One post: 8 × 5 × 6, x 16.5..24.5, spanning the ring's full height.
fn post(k: usize) -> BRepSolid {
    let p = transformed(
        make_cube(8.0, 5.0, H),
        &Transform::translation(16.5, -2.5, Z0),
    );
    transformed(p, &Transform::rotation_z((30.0 * k as f64).to_radians()))
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

fn tris(mesh: &TriangleMesh) -> usize {
    mesh.indices.len() / 3
}

/// Fold the twelve posts into the ring one at a time through `mesh_csg`,
/// each result fed straight back as operand A.
///
/// Before the coplanar re-merge this chain grew geometrically — each
/// boolean re-split every cap fragment the previous ones had left. The
/// gate is both directions at once: the volume must still be the closed
/// form, and the final mesh must stay within 3× of what the SAME twelve
/// posts cost when unioned in one shot.
#[test]
fn twelve_posts_folded_into_the_ring_one_at_a_time() {
    let ring_solid = ring();
    let ring_mesh = tessellate_brep(&ring_solid, SEGMENTS);
    let expected = mesh_signed_volume(&ring_mesh) + 12.0 * (POST_VOLUME - post_ring_overlap());

    let mut acc = ring_mesh.clone();
    let mut counts = Vec::new();
    for k in 0..12 {
        let p = tessellate_brep(&post(k), SEGMENTS);
        acc = mesh_csg_remerged(&acc, &p, BooleanOp::Union);
        counts.push(tris(&acc));
    }

    // One-shot reference: the twelve posts fused analytically first, then
    // a single mesh boolean against the ring.
    let mut posts = post(0);
    for k in 1..12 {
        posts = brep(boolean_op(&posts, &post(k), BooleanOp::Union, SEGMENTS).expect("posts"));
    }
    let one_shot = mesh_csg_remerged(
        &tessellate_brep(&posts, SEGMENTS),
        &ring_mesh,
        BooleanOp::Union,
    );

    let got = mesh_signed_volume(&acc);
    assert!(
        (got - expected).abs() <= 0.005 * expected,
        "chained ring ∪ 12 posts: {got:.2} mm³, closed form {expected:.2} \
         (triangles per step: {counts:?})"
    );
    assert!(
        tris(&acc) <= 3 * tris(&one_shot),
        "chained fold blew up: {} triangles vs {} for the one-shot union \
         (per step: {counts:?})",
        tris(&acc),
        tris(&one_shot)
    );
}

/// The merge must not cost volume on the single boolean either, and it is
/// where the reduction is largest (one ring cap carved by twelve posts).
#[test]
fn one_shot_ring_and_posts_keeps_its_volume() {
    let ring_mesh = tessellate_brep(&ring(), SEGMENTS);
    let mut posts = post(0);
    for k in 1..12 {
        posts = brep(boolean_op(&posts, &post(k), BooleanOp::Union, SEGMENTS).expect("posts"));
    }
    let out = mesh_csg_remerged(
        &tessellate_brep(&posts, SEGMENTS),
        &ring_mesh,
        BooleanOp::Union,
    );
    let expected = mesh_signed_volume(&ring_mesh) + 12.0 * (POST_VOLUME - post_ring_overlap());
    let got = mesh_signed_volume(&out);
    assert!(
        (got - expected).abs() <= 0.02 * expected,
        "posts ∪ ring: {got:.2} mm³, closed form {expected:.2}, {} triangles",
        tris(&out)
    );
    assert!(
        out.boundary_edges().is_empty(),
        "posts ∪ ring left {} boundary edges",
        out.boundary_edges().len()
    );
}
