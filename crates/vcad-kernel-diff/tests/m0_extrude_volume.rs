//! M0 — the harness. Extrude distance `d`, QoI = volume.
//!
//! `dV/dd = A` (the profile area) is analytic, so this proves the whole
//! pipeline — parametric rebuild, frozen tessellation, seam sensitivities,
//! dual-number QoI, central-difference oracle — end to end with zero
//! geometric subtlety.

use vcad_kernel_diff::{
    compare_velocities, evaluate_with_sensitivity, fd_velocities, fd_volume_derivative,
    ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::Plane;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_sketch::SketchProfile;
use vcad_kernel_tessellate::frozen::{capture_plan, evaluate_plan};
use vcad_kernel_tessellate::TessellationParams;

const WIDTH: f64 = 4.0;
const HEIGHT: f64 = 3.0;
const D0: f64 = 2.0;
const H: f64 = 1e-6;
const GATE: f64 = 1e-6;

/// The parametric model: a WIDTH × HEIGHT rectangle extruded `d` along +Z.
fn build(d: f64) -> BRepSolid {
    let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), WIDTH, HEIGHT);
    vcad_kernel_sketch::extrude(&profile, Vec3::z() * d).expect("extrude")
}

/// θ → field seeding: the extrude distance moves exactly one stored
/// surface, the top cap plane (its origin translates at ẑ per unit d).
/// Everything else — bottom cap, side walls (infinite planes) — is
/// d-independent.
fn seeding(brep: &BRepSolid, d: f64) -> ParamSeeding {
    let mut seeding = ParamSeeding::new();
    let n = seeding.seed_where(
        &brep.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, d)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1, "expected exactly one moving surface (the top plane)");
    seeding
}

#[test]
fn m0_extrude_volume_derivative_matches_analytic_and_fd() {
    let base = build(D0);
    let plan = capture_plan(&base, &TessellationParams::default()).expect("capture");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base, D0)).expect("seam");

    // Sanity: the frozen mesh is watertight and reproduces the exact volume.
    let mesh = evaluate_plan(&base, &plan).expect("evaluate");
    assert_eq!(mesh.open_edge_count(), 0, "M0 mesh must be watertight");
    let v_exact = WIDTH * HEIGHT * D0;
    assert!((mesh.volume() - v_exact).abs() / v_exact < 1e-12);

    // Analytic gate: dV/dd = profile area, in closed form.
    let (v, dv) = vcad_kernel_diff::volume_with_derivative(&seam);
    let a_exact = WIDTH * HEIGHT;
    assert!((v - v_exact).abs() / v_exact < 1e-12);
    let rel_analytic = (dv - a_exact).abs() / a_exact;
    assert!(
        rel_analytic <= GATE,
        "seam dV/dd = {dv} vs analytic {a_exact} (rel err {rel_analytic:.3e})"
    );

    // FD oracle gate on the QoI.
    let dv_fd = fd_volume_derivative(build, D0, H, &plan).expect("fd volume");
    let rel_fd = (dv - dv_fd).abs() / a_exact;
    assert!(
        rel_fd <= GATE,
        "seam dV/dd = {dv} vs FD {dv_fd} (rel err {rel_fd:.3e})"
    );

    // FD oracle gate node-wise: every node's dx/dd.
    let fd = fd_velocities(build, D0, H, &plan).expect("fd velocities");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "node dx/dd max rel err {:.3e} at node {} (velocity scale {:.3})",
        cmp.max_rel_err,
        cmp.worst_node,
        cmp.velocity_scale
    );

    // The top four corners move at exactly ẑ; the bottom four are pinned.
    let moving = seam
        .velocities
        .iter()
        .filter(|v| (**v - Vec3::z()).norm() < 1e-9)
        .count();
    let pinned = seam.velocities.iter().filter(|v| v.norm() < 1e-12).count();
    assert_eq!(
        (moving, pinned),
        (4, 4),
        "box has 4 moving + 4 pinned nodes"
    );
}
