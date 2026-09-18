//! Reproducers for the rana-60 stator's remaining validity defect: a union
//! whose VOLUME is right and whose fidelity is `Analytic`, but whose shell is
//! not closed.
//!
//! These are `#[ignore]`d — they fail today, and they are here to pin the
//! defect, not to gate CI. `docs/boolean-multilump-union-diagnosis.md` has the
//! full walk-through; the short version:
//!
//! A fillet block's arc is internally TANGENT to the circle that cuts the
//! face it sits on (the stator's root fillets: an r 1.05 arc tangent to the
//! r 24 bore, by construction — the centre distance is 22.95 = 24 − 1.05).
//! Near the touch point the two curves stay within ~1.6e-3 mm of each other
//! over 0.22 mm of boundary, and three different splitters resolve the same
//! corner to three different points, ~7e-3 mm apart:
//!
//! | who | what it computes | where it lands |
//! |---|---|---|
//! | `split_planar_face_by_arc` on the block's cap | circle × arc-POLYLINE crossing | (23.712183, 3.705720), r = 24.000000 |
//! | the block's own geometry | the cube corner the fillet arc ends on | (23.711165, 3.712400), r = 24.000026 |
//! | the bore wall's splitter | that corner projected onto the bore | (23.711138, 3.712400), r = 24.000000 |
//!
//! The cap seam therefore starts at the first, the fillet wall's rim ends at
//! the second, and the bore wall's rim ends at the third; nothing pairs, and
//! a sliver of about 2e-4 mm² is left uncovered on each cap.
//!
//! `repair::split_edges_at_interior_vertices` then hides it rather than
//! fixing it: the arc's last vertices sit ~1e-6 from the chord the cap split
//! left behind, so that chord is subdivided by the cap's OWN vertices and the
//! loop comes back through points it has already visited —
//! `… v77 v78 v79 v175 v79 v78 v77 …`. Triangulated, that covers the sliver
//! twice with opposite winding. The signed volume and the net open-edge count
//! both cancel, which is why the part measures right; the rims do not, which
//! is what these tests see. Removing the pinch (skipping hits already in the
//! loop) does NOT fix it — it unmasks the gap, the union's volume then
//! disagrees with the mesh referee, and `twelve_filleted_posts_stay_analytic`
//! falls to soup. The pinch is load-bearing until the corner is unified.

use vcad_kernel_booleans::{
    boolean_op, boolean_op_reported, mesh_report, BooleanOp, BooleanResult,
};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

const SEGMENTS: u32 = 256;
const Z0: f64 = 11.1;
const H: f64 = 6.0;

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

/// The stator's ring: r 24..28.75, z 11.1..17.1.
fn ring() -> BRepSolid {
    let outer = transformed(
        make_cylinder(28.75, H, SEGMENTS),
        &Transform::translation(0.0, 0.0, Z0),
    );
    let bore = transformed(
        make_cylinder(24.0, H + 0.02, SEGMENTS),
        &Transform::translation(0.0, 0.0, Z0 - 0.01),
    );
    brep(boolean_op(&outer, &bore, BooleanOp::Difference, SEGMENTS).expect("ring"))
}

/// A post-root fillet block: a 1.35 x 1.5124 cube minus an r 1.05 cylinder
/// whose arc is tangent to the post's side AND to the ring's bore.
fn fillet_block() -> BRepSolid {
    let cube = transformed(
        make_cube(1.35, 1.5124, H),
        &Transform::translation(22.6738, 2.2, Z0),
    );
    let cutter = transformed(
        make_cylinder(1.05, H + 0.02, 32),
        &Transform::translation(22.6738, 3.55, Z0 - 0.01),
    );
    brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("fillet block"))
}

/// Midpoint-rule integral of `f` over `[lo, hi]`.
fn integrate(lo: f64, hi: f64, f: impl Fn(f64) -> f64) -> f64 {
    let steps = 20_000;
    let dy = (hi - lo) / steps as f64;
    (0..steps).map(|i| f(lo + (i as f64 + 0.5) * dy) * dy).sum()
}

/// The block's outer corner reaches past r 24; that lens is shared with the
/// ring and must be counted once.
fn block_ring_overlap() -> f64 {
    H * integrate(2.2, 3.7124, |y| 24.0238 - (24.0 * 24.0 - y * y).sqrt())
}

fn volume(solid: &BRepSolid) -> f64 {
    mesh_report(&tessellate_brep(solid, SEGMENTS)).signed_volume
}

/// The smallest case: one fillet block onto the ring. The volume has been
/// right since #901; the shell has not.
///
/// Measured on `claude/cam-roadmap` (8daf2aa6): 4726.99 mm³ against a closed
/// form of 4726.99 — and 8 unpaired plus 9 over-used edges, all of them
/// within 0.007 mm of the tangency corner, four per cap plane.
///
/// **Half closed (`cam/w2d-union-seam-3`).** `repair::collapse_tangency_seams`
/// puts the seam's three rails back on the one line they all belong to, and
/// `ring ∪ block` now scores 0 unpaired and 0 over-used at 4726.9913 mm³.
/// `block ∪ ring` still scores 6 over-used, so the test stays ignored: the
/// two operand orders reach the corner through different splitters and only
/// one of them is healed. That asymmetry is the remaining work here.
#[test]
#[ignore = "known defect: block ∪ ring still doubles the tangency sliver (6 over-used)"]
fn ring_union_tangent_fillet_block_is_watertight() {
    let (r, b) = (ring(), fillet_block());
    let expected = volume(&r) + volume(&b) - block_ring_overlap();

    for (label, x, y) in [("ring ∪ block", &r, &b), ("block ∪ ring", &b, &r)] {
        let out = union(x, y);
        let report = mesh_report(&tessellate_brep(&out, SEGMENTS));
        let rel = (report.signed_volume - expected).abs() / expected;
        assert!(
            rel < 1e-4,
            "{label}: {:.4} mm³ against {expected:.4}",
            report.signed_volume
        );
        assert_eq!(
            report.open_edges, 0,
            "{label} is not closed — the fillet arc, the cap seam and the \
             bore wall resolve the tangency corner to three different points"
        );
        assert_eq!(
            report.overused_edges, 0,
            "{label} covers the tangency sliver twice — the repair pass \
             pinched the cap's loop instead of closing the gap"
        );
    }
}

/// The pinch itself, stated as topology rather than as a mesh symptom: a
/// face's outer loop must be simple. Where the cap's seam chord is collinear
/// (to ~1e-6) with the fillet arc it just came from,
/// `repair::split_edges_at_interior_vertices` subdivides that chord at the
/// arc's own vertices, and the loop revisits them in reverse.
///
/// **Closed.** Passes since `repair::collapse_twin_pair_spurs` learned to see
/// a retraced pair by its VERTICES rather than by `first.twin == second`, and
/// to marry the two outside partners a nested flap strands. Kept
/// non-ignored: it is the topology statement of the defect, and it must not
/// come back.
#[test]
fn no_face_loop_visits_a_vertex_twice() {
    let out = union(&ring(), &fillet_block());
    for (fid, face) in &out.topology.faces {
        let mut seen = std::collections::HashSet::new();
        for he in out.topology.loop_half_edges(face.outer_loop) {
            let v = out.topology.half_edges[he].origin;
            assert!(
                seen.insert(v),
                "face {fid:?} passes through {v:?} twice — its loop is pinched, \
                 so the sliver it encloses is triangulated with both windings"
            );
        }
    }
}

/// The same defect on the OD tabs, which is where most of the stator's 642
/// unpaired edges live. The tab is a cube plus a round end; on its own that
/// union is exact and closed. Adding the two root fillet blocks — tangent to
/// the ring's OD at r 28.75 — leaves the round end's cap disc covered twice:
/// 6 unpaired and 43 OVER-USED edges, the over-use being the doubled fan.
///
/// **Unpaired half closed (`cam/w2d-union-seam-3`).** The seam collapse takes
/// this group from 11 unpaired / 35 over-used / 3 pinched loops to **0
/// unpaired**, 31 over-used and 1 pinched loop, with the volume unchanged at
/// 4953.4179 mm³. The doubled round-end cap is what is left.
#[test]
#[ignore = "known defect: tangent tab-root fillets double the round end's cap (31 over-used)"]
fn tab_group_leaves_no_doubled_cap() {
    let tab_cube = transformed(
        make_cube(4.9, 6.2, H),
        &Transform::translation(27.5, -3.1, Z0),
    );
    let round_end = transformed(
        make_cylinder(3.1, H, 64),
        &Transform::translation(32.4, 0.0, Z0),
    );
    let tab_block = |side: f64| {
        let y_lo = if side > 0.0 { 2.8 } else { -4.0038 };
        let cube = transformed(
            make_cube(1.35, 1.2038, H),
            &Transform::translation(28.1596, y_lo, Z0),
        );
        let cutter = transformed(
            make_cylinder(1.05, H + 0.02, 32),
            &Transform::translation(29.5096, 4.15 * side, Z0 - 0.01),
        );
        brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("tab block"))
    };

    let mut acc = ring();
    // The ring plus the bare stadium tab is closed; assert that, so a
    // regression there is told apart from the fillet defect below.
    acc = union(&acc, &tab_cube);
    acc = union(&acc, &round_end);
    let bare = mesh_report(&tessellate_brep(&acc, SEGMENTS));
    assert_eq!(bare.open_edges, 0, "ring ∪ stadium tab is closed today");
    assert_eq!(bare.overused_edges, 0, "ring ∪ stadium tab is closed today");

    acc = union(&acc, &tab_block(1.0));
    acc = union(&acc, &tab_block(-1.0));
    let report = mesh_report(&tessellate_brep(&acc, SEGMENTS));
    assert_eq!(
        report.overused_edges, 0,
        "the tab's round-end cap is covered twice once the tangent root \
         fillets are unioned in"
    );
    assert_eq!(report.open_edges, 0, "the tab group is not closed");
}

/// Tangency is the trigger, and the window is narrow. Sliding the fillet
/// block 0.01 mm inwards — so its arc no longer touches the bore — closes the
/// shell; at 0 it does not. Kept non-ignored: it is the control that says the
/// machinery works away from the tangency, and it passes today.
#[test]
fn a_fillet_clear_of_the_bore_is_closed() {
    let r = ring();
    for d in [-0.20f64, -0.05, -0.01] {
        let cube = transformed(
            make_cube(1.35, 1.5124, H),
            &Transform::translation(22.6738 + d, 2.2, Z0),
        );
        let cutter = transformed(
            make_cylinder(1.05, H + 0.02, 32),
            &Transform::translation(22.6738 + d, 3.55, Z0 - 0.01),
        );
        let block =
            brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("block"));
        let out = union(&r, &block);
        let report = mesh_report(&tessellate_brep(&out, SEGMENTS));
        assert_eq!(
            report.open_edges, 0,
            "fillet pulled {d} mm off the bore: shell not closed"
        );
        assert_eq!(
            report.overused_edges, 0,
            "fillet pulled {d} mm off the bore: doubled coverage"
        );
    }
}

/// The near miss that must not be fused into a tangency — and whose volume
/// must not be silently wrong.
///
/// Sliding the fillet block 0.01 mm OUTWARD breaks the tangency: the centre
/// distance misses `R − r` by 9.9e-3 mm, a hundred times the 1e-4 mm at which
/// `tangency::TANGENCY_EPS` declares two carriers to be touching, so nothing
/// in the tangency machinery may fire here. It does not. Good.
///
/// The union still reports 4734.78 mm³ against a closed form of 4726.91 —
/// +0.167 %, `Analytic`, 6 unpaired edges.
///
/// Located (`docs/boolean-multilump-union-diagnosis.md`): the SHAPE is right.
/// Sectioned against the 2D CSG of its own source at six heights, the outline
/// agrees to **0.00499 mm** and the area to 0.055 mm². What is wrong is that
/// one wall face is absent — the 0.164 mm stretch from the fillet arc's apex
/// (23.7338, 3.5500), which sits 0.0022 mm inside the bore, to where the bore
/// meets the block's top edge at (23.7111, 3.7124). The arc crosses r = 24
/// mid-stretch, so the boundary has to hand over from arc to bore there, and
/// both sides of the handover are dropped.
///
/// The volume error is that hole, billed by the divergence theorem:
/// 0.16397 × 6 = 0.9838 mm² of missing wall at r = 24 contributes
/// (1/3)·24·0.9838 = 7.8706 mm³, against an observed 7.869. So this test's
/// assertion is sound — a user reading mass or a quote gets the wrong number
/// — but the defect is a missing face at a near-tangent crossing, not a
/// missing trim. Neither the volume bound nor the buried-face check can see
/// it: the bound is on volume, and volume is precisely what the hole
/// corrupts.
#[test]
fn a_near_miss_fillet_is_not_silently_wrong() {
    let r = ring();
    let d = 0.01;
    let cube = transformed(
        make_cube(1.35, 1.5124, H),
        &Transform::translation(22.6738 + d, 2.2, Z0),
    );
    let cutter = transformed(
        make_cylinder(1.05, H + 0.02, 32),
        &Transform::translation(22.6738 + d, 3.55, Z0 - 0.01),
    );
    let block = brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("block"));

    // The lens the block shares with the ring, 0.01 mm further out.
    let expected = volume(&r) + volume(&block)
        - H * integrate(2.2, 3.7124, |y| 24.0338 - (24.0 * 24.0 - y * y).sqrt());

    let (result, report) = boolean_op_reported(&r, &block, BooleanOp::Union, SEGMENTS).expect("u");
    let got = volume(&brep(result));
    let rel = (got - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "ring ∪ near-miss block: {got:.4} mm³ against {expected:.4} ({:.3}% out), \
         reported {:?} — a wrong solid that calls itself Analytic",
        rel * 100.0,
        report.reason
    );
}
