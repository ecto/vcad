//! Sub-second reproducers for the rana-60 stator's tangent-seam defects.
//!
//! `#[ignore]`d measurement harness, not a gate: these print a row each
//! rather than asserting, because what they exist for is A/B-ing a change to
//! `repair::collapse_tangency_seams` without paying the stator's 25 s solve.
//! Run them with `VCAD_NO_SEAM_COLLAPSE=1` for the other side of the A/B.
//!
//! ```text
//! cargo test -p vcad-kernel-booleans --test zz_seam_probe -- --ignored --nocapture
//! ```
//!
//! `probe_stator_l2` is the important one. It is the exact operand shape of
//! the union that decides whether the whole part keeps its analytic path —
//! `[union-tree] L2 vol 420.9 ∪ 376.8`, two MULTI-LUMP operands — and it
//! reproduces in 0.4 s what costs 85–125 s to see on the part itself. See
//! `docs/boolean-multilump-union-diagnosis.md`.
#![allow(missing_docs)]

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

/// A post-root fillet block: r 1.05 arc internally tangent to the r 24 bore.
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

/// Loops that pass through the same vertex twice — a pinched, non-simple
/// boundary, which triangulates the sliver it encloses with both windings.
fn loops_revisiting(s: &BRepSolid) -> usize {
    let mut n = 0;
    for (_, face) in &s.topology.faces {
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let mut seen = std::collections::HashSet::new();
            if s.topology
                .loop_half_edges(lid)
                .any(|he| !seen.insert(s.topology.half_edges[he].origin))
            {
                n += 1;
            }
        }
    }
    n
}

fn show(label: &str, s: &BRepSolid) {
    let r = mesh_report(&tessellate_brep(s, SEGMENTS));
    println!(
        "{label:<30} vol {:>11.4}  open {:>4}  over {:>4}  tris {:>6}  faces {:>5}  revisit {}",
        r.signed_volume,
        r.open_edges,
        r.overused_edges,
        r.triangles,
        s.topology.faces.len(),
        loops_revisiting(s),
    );
}

/// The internally-tangent family: ring ∪ post-root fillet block.
///
/// `ring ∪ block` reaches 0 / 0 / 0 with the seam collapse (0 unpaired but 6
/// over-used without it); `block ∪ ring` keeps 6 over-used either way.
#[test]
#[ignore = "measurement harness"]
fn probe_post_root() {
    let (r, b) = (ring(), fillet_block());
    show("ring ∪ block", &union(&r, &b));
    show("block ∪ ring", &union(&b, &r));
}

/// The externally-tangent family: the OD tab group, built one operand at a
/// time. The crack opens on the tab-root fillet blocks, and the collapse
/// closes it — 8 → 0 and 11 → 0 unpaired, volume unchanged.
#[test]
#[ignore = "measurement harness"]
fn probe_tab_group() {
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
    acc = union(&acc, &tab_cube);
    show("ring ∪ cube", &acc);
    acc = union(&acc, &round_end);
    show("∪ round end", &acc);
    acc = union(&acc, &tab_block(1.0));
    show("∪ blk A", &acc);
    acc = union(&acc, &tab_block(-1.0));
    show("∪ blk B", &acc);
}

/// **The gate for any change to the seam collapse.** The exact operand shape
/// of the stator union that decides its fidelity, in 0.4 s.
///
/// Two multi-lump operands: (285° round end ∪ a 0° post cube) against (165°
/// tab group ∪ 285° tab group). The answer must stay `Analytic` at
/// 707.3893 mm³. A collapse that reaches the round end's CYLINDER-PLANE
/// tangency instead emits that cylinder three times over — 948.8 mm³, 1042
/// over-used edges and no open boundary at all — and
/// `validate::union_volume_out_of_bounds` condemns it, which is how the whole
/// part fell to triangle soup on both earlier attempts.
#[test]
#[ignore = "measurement harness"]
fn probe_stator_l2() {
    let rot = |deg: f64| Transform::rotation_z(f64::to_radians(deg));
    // `a.then(&b)` is `a.matrix * b.matrix`, so B acts FIRST: translate-then-
    // rotate is `rot.then(&translation)`. The other order silently builds a
    // different part — and a soup fixture that proves nothing.
    let tab_cube = |deg: f64| {
        transformed(
            make_cube(4.9, 6.2, H),
            &rot(deg).then(&Transform::translation(27.5, -3.1, Z0)),
        )
    };
    let tab_block = |deg: f64, side: f64| {
        let y_lo = if side > 0.0 { 2.8 } else { -4.0038 };
        let cube = transformed(
            make_cube(1.35, 1.2038, H),
            &rot(deg).then(&Transform::translation(28.1596, y_lo, Z0)),
        );
        let cutter = transformed(
            make_cylinder(1.05, H + 0.02, 32),
            &rot(deg).then(&Transform::translation(29.5096, 4.15 * side, Z0 - 0.01)),
        );
        brep(boolean_op(&cube, &cutter, BooleanOp::Difference, SEGMENTS).expect("tab block"))
    };
    let group = |deg: f64| {
        let mut g = tab_cube(deg);
        g = union(&g, &tab_block(deg, 1.0));
        union(&g, &tab_block(deg, -1.0))
    };

    let round_285 = transformed(
        make_cylinder(3.1, H, 64),
        &Transform::translation(8.3857, -31.296, Z0),
    );
    let post_0 = transformed(
        make_cube(8.0, 5.0, H),
        &Transform::translation(16.5, -2.5, Z0),
    );

    let a = union(&round_285, &post_0);
    show("A = round285 ∪ post0", &a);
    let b = union(&group(165.0), &group(285.0));
    show("B = grp165 ∪ grp285", &b);
    let (res, rep) = boolean_op_reported(&a, &b, BooleanOp::Union, SEGMENTS).expect("union");
    println!("  report: {:?} {:?}", rep.fidelity, rep.reason);
    show("A ∪ B", &brep(res));
}
