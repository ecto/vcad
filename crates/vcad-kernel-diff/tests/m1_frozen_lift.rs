//! M1 — frozen tessellation + lift-bridge.
//!
//! Acceptance:
//! 1. Interior-sample dx/dθ on a single generic face matches the FD oracle
//!    to the gate — exercised on a cylinder lateral face (radius = θ) via
//!    the full frozen pipeline, and on a plane (offset = θ) at the
//!    lift-bridge level (planar faces tessellate from boundary vertices
//!    only, so a plane's interior samples don't arise in a full-solid plan).
//! 2. The topology-signature assertion is in place and errors — not lies —
//!    on a deliberately topology-changing perturbation.

use vcad_kernel_diff::{
    compare_velocities, evaluate_with_sensitivity, fd_velocities, lift_surface, DiffError,
    ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, Plane};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::{capture_plan, FrozenError, NodeRecipe};
use vcad_kernel_tessellate::TessellationParams;

const R0: f64 = 5.0;
const HEIGHT: f64 = 8.0;
const H: f64 = 1e-6;
const GATE: f64 = 1e-6;

fn params() -> TessellationParams {
    TessellationParams {
        circle_segments: 32,
        height_segments: 4,
        ..Default::default()
    }
}

/// The parametric model: a cylinder whose radius is θ.
fn build(r: f64) -> BRepSolid {
    make_cylinder(r, HEIGHT, 32)
}

fn seeding(brep: &BRepSolid) -> ParamSeeding {
    let mut seeding = ParamSeeding::new();
    let n = seeding.seed_where(
        &brep.geometry,
        |s| s.as_any().downcast_ref::<CylinderSurface>().is_some(),
        SurfaceSeed::CylinderRadius { rate: 1.0 },
    );
    assert_eq!(n, 1, "primitive cylinder stores one cylindrical surface");
    seeding
}

#[test]
fn m1_cylinder_interior_samples_match_fd() {
    let base = build(R0);
    let plan = capture_plan(&base, &params()).expect("capture");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base)).expect("seam");

    // The plan must contain genuine interior (u, v) samples on the moving
    // face — that's what M1 is about.
    let interior: Vec<usize> = seam
        .recipes
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            NodeRecipe::SurfaceUv { v, .. } if *v > 0.0 && *v < HEIGHT => Some(i),
            _ => None,
        })
        .collect();
    assert!(
        interior.len() >= 3 * 32,
        "expected ≥ 96 interior lateral samples, got {}",
        interior.len()
    );

    // Every interior sample's analytic velocity is the exact radial
    // direction at its frozen angle (Pillar 2 via the lift-bridge).
    for &i in &interior {
        if let NodeRecipe::SurfaceUv { u, .. } = seam.recipes[i] {
            let radial = Vec3::new(u.cos(), u.sin(), 0.0);
            assert!(
                (seam.velocities[i] - radial).norm() < 1e-12,
                "node {i}: analytic velocity {:?} vs exact radial {:?}",
                seam.velocities[i],
                radial
            );
        }
    }

    // FD oracle over the whole mesh (interior samples included).
    let fd = fd_velocities(build, R0, H, &plan).expect("fd velocities");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "dx/dr max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );
}

#[test]
fn m1_plane_offset_interior_samples_match_fd() {
    // A plane whose offset along its normal is θ, sampled on an interior
    // (u, v) grid at frozen parameters. Analytic side: the lift-bridge.
    // FD side: rebuild the surface at θ ± h, evaluate the same grid.
    let build_plane = |theta: f64| {
        Plane::new(
            Point3::new(1.0, -2.0, 0.0) + Vec3::new(0.3, -0.2, 0.9).normalize() * theta,
            Vec3::new(0.9, 0.1, -0.29),
            Vec3::new(-0.1, 0.95, 0.18),
        )
    };
    let theta0 = 1.7;
    let normal_velocity = Vec3::new(0.3, -0.2, 0.9).normalize();
    let base = build_plane(theta0);
    let lifted = lift_surface(
        &base,
        &[SurfaceSeed::Translate {
            velocity: normal_velocity,
        }],
    )
    .expect("lift");

    let mut max_rel = 0.0_f64;
    for iu in 0..7 {
        for iv in 0..7 {
            let uv = Point2::new(-3.0 + iu as f64, -3.0 + iv as f64);
            let (_, vel) = lifted.evaluate_with_velocity(uv);
            let plus = build_plane(theta0 + H).evaluate(uv);
            let minus = build_plane(theta0 - H).evaluate(uv);
            let fd = (plus - minus) / (2.0 * H);
            let rel = (vel - fd).norm() / fd.norm().max(1e-12);
            max_rel = max_rel.max(rel);
        }
    }
    assert!(max_rel <= GATE, "plane dx/dθ max rel err {max_rel:.3e}");
}

#[test]
fn m1_topology_change_is_an_error_not_a_lie() {
    // Capture on a blind-hole block, then perturb the hole depth far enough
    // to punch through: face/loop structure changes, so both the plain
    // frozen evaluation and the seam must refuse.
    use vcad_kernel::Solid;

    let build_block = |depth: f64| -> BRepSolid {
        let block = Solid::cube(10.0, 8.0, 6.0);
        let tool = Solid::cylinder(2.0, depth, 16).translate(5.0, 4.0, 6.0 - depth + 0.5);
        block
            .difference(&tool)
            .as_brep()
            .expect("brep result")
            .clone()
    };

    let blind = build_block(3.0); // hole bottom at z = 3.5 — blind
    let plan = capture_plan(&blind, &params()).expect("capture");

    // Positive control: a small, topology-preserving perturbation evaluates.
    let nearby = build_block(3.0 + 1e-6);
    vcad_kernel_tessellate::frozen::evaluate_plan(&nearby, &plan)
        .expect("small perturbation keeps topology");

    // Punch through: z-extent of the tool now spans the whole block.
    let through = build_block(7.0);
    match vcad_kernel_tessellate::frozen::evaluate_plan(&through, &plan) {
        Err(FrozenError::TopologyChanged { .. }) => {}
        other => panic!("expected TopologyChanged, got {other:?}"),
    }
    match evaluate_with_sensitivity(&through, &plan, &ParamSeeding::new()) {
        Err(DiffError::Frozen(FrozenError::TopologyChanged { .. })) => {}
        other => panic!("expected TopologyChanged from seam, got {other:?}"),
    }
}
