//! M2 — the first true boolean seam: a through-hole's diameter.
//!
//! A cylinder of radius `r` is boolean-subtracted from a block; θ = r and
//! the QoI is volume. The moving surface is the simplest possible and the
//! continuum derivative is closed-form (`dV/dr = −2πrt`), so this validates
//! the *framework*, not the geometry:
//!
//! - interior hole-wall samples are Pillar 2 (exact via the lift-bridge);
//! - the rim — the moving trim boundary {on cap plane} ∩ {on cylinder(r)} —
//!   is the first Pillar-3 exercise: rim vertices are differentiated
//!   implicitly through the two-surface system with the tangential DOF
//!   frozen.
//!
//! The frozen mesh carries the rim as an N-gon (the boolean polygonizes the
//! intersection circle at `segments`), so the *discrete* closed form it
//! must match exactly is `dV/dr = −N·sin(2π/N)·r·t` — the derivative of
//! `V(r) = L·W·t − ½·N·sin(2π/N)·r²·t`. The continuum `−2πrt` differs from
//! this by the polygonization factor `sin(x)/x`, `x = 2π/N` (relative gap
//! `x²/6 + O(x⁴)`); the test gates the discrete form at 1e-6 and the
//! continuum form at its discretization bound.

use vcad_kernel::Solid;
use vcad_kernel_diff::{
    compare_velocities, evaluate_with_sensitivity, fd_velocities, fd_volume_derivative,
    ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{capture_plan, evaluate_plan, NodeRecipe};
use vcad_kernel_tessellate::TessellationParams;

const L: f64 = 10.0;
const W: f64 = 8.0;
const T: f64 = 5.0;
const R0: f64 = 2.5;
const SEGMENTS: u32 = 32;
const H: f64 = 1e-6;
const GATE: f64 = 1e-6;

/// Points on the through-hole's rim circle in the frozen mesh.
///
/// NOT `SEGMENTS`: boolean-result rims ride the kernel's sag-adaptive
/// canonical grid, which is at least as fine as the requested resolution
/// (112 for r=2.5 here, against `circle_segments = 32`). Ask the kernel
/// rather than assuming, so this tracks the convention instead of pinning a
/// stale copy of it.
fn rim_segments() -> u32 {
    vcad_kernel::vcad_kernel_booleans::split::arc_segments(R0, SEGMENTS)
}

/// The parametric model: block minus a through-hole of radius `r` centered
/// at (L/2, W/2), tool overshooting both faces.
fn build(r: f64) -> BRepSolid {
    let block = Solid::cube(L, W, T);
    let tool = Solid::cylinder(r, T + 2.0, SEGMENTS).translate(L / 2.0, W / 2.0, -1.0);
    block
        .difference(&tool)
        .as_brep()
        .expect("boolean should stay BRep")
        .clone()
}

/// θ → field seeding: r touches exactly the hole-wall cylinder surface.
fn seeding(brep: &BRepSolid) -> ParamSeeding {
    let mut seeding = ParamSeeding::new();
    let n = seeding.seed_where(
        &brep.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<CylinderSurface>()
                .map(|c| (c.radius - R0).abs() < 1e-9)
                .unwrap_or(false)
        },
        SurfaceSeed::CylinderRadius { rate: 1.0 },
    );
    assert_eq!(n, 1, "expected exactly one hole-wall surface, got {n}");
    seeding
}

#[test]
fn m2_through_hole_dv_dr_matches_closed_form_and_fd() {
    let base = build(R0);
    let params = TessellationParams {
        circle_segments: SEGMENTS,
        height_segments: 3,
        ..Default::default()
    };
    let plan = capture_plan(&base, &params).expect("capture");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base)).expect("seam");

    // Discrete closed form for the frozen mesh: hole cross-section is the
    // inscribed N-gon of the rim circle. N is the kernel's canonical rim
    // count, NOT `SEGMENTS` — boolean-result rims are sag-adaptive, so the
    // hole wall is finer than the requested resolution (112 vs 32 here).
    let n = rim_segments() as f64;
    let sector = 2.0 * std::f64::consts::PI / n;
    let v_exact = L * W * T - 0.5 * n * sector.sin() * R0 * R0 * T;
    let dv_exact = -n * sector.sin() * R0 * T;

    let mesh = evaluate_plan(&base, &plan).expect("evaluate");
    assert!(
        (mesh.volume() - v_exact).abs() / v_exact < 1e-12,
        "frozen volume {} vs discrete closed form {v_exact}",
        mesh.volume()
    );

    // Gate 1: seam dV/dr vs the discrete closed form.
    let (v, dv) = vcad_kernel_diff::volume_with_derivative(&seam);
    assert!((v - v_exact).abs() / v_exact < 1e-12);
    let rel_closed = (dv - dv_exact).abs() / dv_exact.abs();
    assert!(
        rel_closed <= GATE,
        "seam dV/dr = {dv} vs discrete closed form {dv_exact} (rel err {rel_closed:.3e})"
    );

    // Gate 2: seam dV/dr vs the FD oracle under the same frozen plan.
    let dv_fd = fd_volume_derivative(build, R0, H, &plan).expect("fd volume");
    let rel_fd = (dv - dv_fd).abs() / dv_exact.abs();
    assert!(
        rel_fd <= GATE,
        "seam dV/dr = {dv} vs FD {dv_fd} (rel err {rel_fd:.3e})"
    );

    // Continuum closed form −2πrt: differs by the polygonization factor
    // sin(x)/x only. Verify the gap is exactly the discretization error,
    // within 10% slack of the leading term x²/6, x = 2π/N.
    let dv_continuum = -2.0 * std::f64::consts::PI * R0 * T;
    let gap = (dv - dv_continuum).abs() / dv_continuum.abs();
    let bound = sector.powi(2) / 6.0;
    assert!(
        gap <= bound * 1.1,
        "continuum gap {gap:.3e} exceeds discretization bound {bound:.3e}"
    );
}

#[test]
fn m2_rim_nodes_implicit_diff_matches_fd() {
    let base = build(R0);
    let params = TessellationParams {
        circle_segments: SEGMENTS,
        height_segments: 3,
        ..Default::default()
    };
    let plan = capture_plan(&base, &params).expect("capture");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base)).expect("seam");
    let fd = fd_velocities(build, R0, H, &plan).expect("fd velocities");

    // Rim nodes: topology vertices lying on the hole wall. Their velocity
    // comes from the implicit two-surface system {cap plane, cylinder(r)}
    // with the tangential DOF frozen — it must be the exact radial
    // direction, and must match the FD oracle node-wise.
    let center = Vec3::new(L / 2.0, W / 2.0, 0.0);
    let mut rim_nodes = 0;
    for (i, recipe) in seam.recipes.iter().enumerate() {
        if !matches!(recipe, NodeRecipe::TopoVertex { .. }) {
            continue;
        }
        let p = seam.positions[i];
        let radial_offset = Vec3::new(p.x - center.x, p.y - center.y, 0.0);
        if (radial_offset.norm() - R0).abs() > 1e-9 {
            continue; // block corner, not a rim vertex
        }
        rim_nodes += 1;
        let radial = radial_offset / radial_offset.norm();
        assert!(
            (seam.velocities[i] - radial).norm() < 1e-9,
            "rim node {i}: implicit velocity {:?} vs radial {:?}",
            seam.velocities[i],
            radial
        );
        let rel = (seam.velocities[i] - fd[i]).norm() / fd[i].norm();
        assert!(
            rel <= GATE,
            "rim node {i}: implicit vs FD rel err {rel:.3e}"
        );
    }
    assert_eq!(
        rim_nodes,
        2 * rim_segments() as usize,
        "expected a full rim ring on each cap"
    );

    // And the whole mesh — interior wall samples (Pillar 2) included —
    // passes the node-wise gate.
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "dx/dr max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );
}
