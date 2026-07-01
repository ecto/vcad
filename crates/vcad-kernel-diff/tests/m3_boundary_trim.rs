//! M3 — multi-surface boundary recipes: the moving trim lives on a *plane*.
//!
//! This is the exact case the M0–M2 recipe-priority heuristic got wrong (and
//! documented as its known limitation): a cylinder whose **height** is θ.
//! The cap-rim ring is not carried by topology vertices (the primitive's cap
//! loops are degenerate single-vertex circles), and binding rim nodes to the
//! *fixed* cylinder wall at frozen `(u, v)` would freeze them at the old
//! height — the frozen mesh would differentiate a slightly different body,
//! and the FD oracle (replaying the same recipes) would agree with the wrong
//! answer. With `NodeRecipe::Boundary`, rim nodes are Newton-tracked on
//! {cap plane} ∩ {cylinder} and ride up with the moving cap, so both the
//! analytic seam and the FD oracle now measure the true solid:
//! `dV/dh = A_N(r)` (the inscribed N-gon area) exactly.

use vcad_kernel_diff::{
    compare_velocities, evaluate_with_sensitivity, fd_velocities, fd_volume_derivative,
    ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::Plane;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, NodeRecipe};
use vcad_kernel_tessellate::TessellationParams;

const R: f64 = 5.0;
const H0: f64 = 8.0;
const SEGMENTS: u32 = 24;
const H_FD: f64 = 1e-6;
const GATE: f64 = 1e-6;

fn build(h: f64) -> BRepSolid {
    make_cylinder(R, h, SEGMENTS)
}

/// θ = height: the only moving surface is the top cap plane (z = h),
/// translating at ẑ per unit θ. The cylinder wall is fixed.
fn seeding(brep: &BRepSolid, h: f64) -> ParamSeeding {
    let mut seeding = ParamSeeding::new();
    let n = seeding.seed_where(
        &brep.geometry,
        |s| {
            s.as_any()
                .downcast_ref::<Plane>()
                .map(|p| {
                    p.normal_dir.as_ref().cross(Vec3::z()).norm() < 1e-12
                        && p.signed_distance(&Point3::new(0.0, 0.0, h)).abs() < 1e-9
                })
                .unwrap_or(false)
        },
        SurfaceSeed::Translate {
            velocity: Vec3::z(),
        },
    );
    assert_eq!(n, 1, "expected exactly the top cap plane, got {n}");
    seeding
}

#[test]
fn boundary_nodes_track_a_moving_plane_trim() {
    let params = TessellationParams {
        circle_segments: SEGMENTS,
        height_segments: 4,
        ..Default::default()
    };
    let base = build(H0);
    let plan = capture_plan(&base, &params).expect("capture");

    // The cap rims must have been captured as Boundary nodes: one ring per
    // cap, minus the seam anchor vertex that topology carries.
    let boundary_nodes = plan
        .nodes
        .iter()
        .filter(|n| matches!(n, NodeRecipe::Boundary { .. }))
        .count();
    assert_eq!(
        boundary_nodes,
        2 * (SEGMENTS as usize - 1),
        "expected a Boundary ring on each cap"
    );

    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base, H0)).expect("seam");

    // Top-rim boundary nodes ride up with the cap at exactly ẑ; bottom-rim
    // boundary nodes are pinned. Wall interior samples are pinned (the
    // cylinder surface does not move with h).
    for (i, recipe) in seam.recipes.iter().enumerate() {
        match recipe {
            NodeRecipe::Boundary { .. } if seam.positions[i].z > H0 - 1e-9 => {
                assert!(
                    (seam.velocities[i] - Vec3::z()).norm() < 1e-9,
                    "top rim node {i}: velocity {:?} should be ẑ",
                    seam.velocities[i]
                );
            }
            NodeRecipe::Boundary { .. } => {
                assert!(
                    seam.velocities[i].norm() < 1e-9,
                    "bottom rim node {i}: velocity {:?} should be zero",
                    seam.velocities[i]
                );
            }
            _ => {}
        }
    }

    // dV/dh against the discrete closed form A_N(r) = ½·N·sin(2π/N)·r², and
    // against the FD oracle (which now Newton-tracks the true rim at h ± ε).
    let n = SEGMENTS as f64;
    let area = 0.5 * n * (2.0 * std::f64::consts::PI / n).sin() * R * R;
    let (v, dv) = vcad_kernel_diff::volume_with_derivative(&seam);
    assert!((v - area * H0).abs() / (area * H0) < 1e-9);
    let rel_closed = (dv - area).abs() / area;
    assert!(
        rel_closed <= GATE,
        "seam dV/dh = {dv} vs closed form {area} (rel err {rel_closed:.3e})"
    );
    let dv_fd = fd_volume_derivative(build, H0, H_FD, &plan).expect("fd volume");
    let rel_fd = (dv - dv_fd).abs() / area;
    assert!(
        rel_fd <= GATE,
        "seam dV/dh = {dv} vs FD {dv_fd} (rel err {rel_fd:.3e})"
    );

    // Node-wise gate over the whole mesh.
    let fd = fd_velocities(build, H0, H_FD, &plan).expect("fd velocities");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "dx/dh max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );
}
