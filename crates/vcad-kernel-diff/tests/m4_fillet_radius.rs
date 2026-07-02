//! M4 — differentiable fillet radius.
//!
//! The model is a cube with **all edges filleted** (the kernel's supported
//! fillet path): 6 inset planes, 12 quarter-cylinder edge blends, 8
//! sphere-octant corners. θ = the fillet radius, and it moves *twenty*
//! surfaces at once, each with a **composite seed**: every blend's radius
//! grows at rate 1 while its axis/center simultaneously retreats from the
//! edge (velocity ±1 per non-axial coordinate), which is exactly what the
//! composite `ParamSeeding` exists for.
//!
//! What makes fillets the hard case (and why this test exists):
//!
//! - Blend surfaces meet their support faces **tangentially**, so the
//!   two-surface Boundary Newton system is singular exactly on the moving
//!   trim. The seam handles this without a special case: a tangent line is
//!   `u = const` on the blend, so a frozen-`(u,v)` sample with the
//!   composite seed already tracks it exactly (Pillar 2), and the Boundary
//!   upgrade's tangency check routes those nodes away from the singular
//!   solve.
//! - The fillet kernel rebuilds blend frames **nondeterministically**
//!   (axis signs flip between builds), so the FD oracle transports frozen
//!   samples through each rebuilt surface's frame (`FaceFrame` /
//!   `transport_uv`) instead of trusting raw parameters.
//!
//! Volume gates: the seam derivative must match the FD oracle at 1e-6 (the
//! framework test), and sit within the documented polygonization band of
//! the continuum closed form — the Minkowski sum of the shrunken cube with
//! a ball: `V(r) = a³ + 6a²r + 3πar² + (4/3)πr³`, `a = L − 2r`.

use vcad_kernel_diff::{
    compare_velocities, evaluate_with_sensitivity, fd_velocities, fd_volume_derivative, minimize,
    volume_with_derivative, OptimizeOptions, ParamSeeding, SeamMesh, StopReason, SurfaceSeed,
};
use vcad_kernel_geom::{CylinderSurface, SphereSurface};
use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::{make_cube, BRepSolid};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

const L: f64 = 10.0;
const R0: f64 = 1.5;
const H: f64 = 1e-6;
const GATE: f64 = 1e-6;

fn build(theta: &[f64]) -> BRepSolid {
    vcad_kernel_fillet::fillet_all_edges(&make_cube(L, L, L), theta[0])
}

fn minkowski(r: f64) -> f64 {
    let a = L - 2.0 * r;
    a * a * a
        + 6.0 * a * a * r
        + 3.0 * std::f64::consts::PI * a * r * r
        + 4.0 / 3.0 * std::f64::consts::PI * r * r * r
}

/// Retreat velocity of a fillet-surface center: for every coordinate
/// pinned at `r` off a face of the cube, +1; at `L − r`, −1; otherwise 0
/// (the blend's axial direction). Same rule covers edge cylinders (two
/// pinned coordinates) and corner spheres (three).
fn retreat_velocity(center: vcad_kernel_math::Point3, r: f64) -> Vec3 {
    let component = |c: f64| {
        if (c - r).abs() < 1e-9 {
            1.0
        } else if (c - (L - r)).abs() < 1e-9 {
            -1.0
        } else {
            0.0
        }
    };
    Vec3::new(
        component(center.x),
        component(center.y),
        component(center.z),
    )
}

/// θ = fillet radius: composite seeds on all 12 blend cylinders and all 8
/// corner spheres (radius rate 1 + center retreat); the 6 planes are
/// θ-independent.
fn seeding(brep: &BRepSolid, r: f64) -> ParamSeeding {
    let mut seeding = ParamSeeding::new();
    let mut cylinders = 0;
    let mut spheres = 0;
    for (i, s) in brep.geometry.surfaces.iter().enumerate() {
        if let Some(c) = s.as_any().downcast_ref::<CylinderSurface>() {
            assert!((c.radius - r).abs() < 1e-9, "unexpected blend radius");
            seeding.seed(i, SurfaceSeed::CylinderRadius { rate: 1.0 });
            seeding.seed(
                i,
                SurfaceSeed::Translate {
                    velocity: retreat_velocity(c.center, r),
                },
            );
            cylinders += 1;
        } else if let Some(sp) = s.as_any().downcast_ref::<SphereSurface>() {
            assert!((sp.radius - r).abs() < 1e-9, "unexpected corner radius");
            seeding.seed(i, SurfaceSeed::SphereRadius { rate: 1.0 });
            seeding.seed(
                i,
                SurfaceSeed::Translate {
                    velocity: retreat_velocity(sp.center, r),
                },
            );
            spheres += 1;
        }
    }
    assert_eq!((cylinders, spheres), (12, 8), "rounded cube surface census");
    seeding
}

fn tess_params() -> TessellationParams {
    TessellationParams {
        circle_segments: 16,
        height_segments: 2,
        ..Default::default()
    }
}

#[test]
fn m4_fillet_radius_derivative_matches_fd_and_continuum() {
    let build1 = |r: f64| build(&[r]);
    let base = build1(R0);
    let plan = capture_plan(&base, &tess_params()).expect("capture");
    let seam = evaluate_with_sensitivity(&base, &plan, &seeding(&base, R0)).expect("seam");

    // Gate 1 (the framework test): seam dV/dr vs the FD oracle across
    // twenty simultaneously-moving surfaces with composite seeds, under
    // frame-transported correspondence.
    let (v, dv) = volume_with_derivative(&seam);
    let dv_fd = fd_volume_derivative(build1, R0, H, &plan).expect("fd volume");
    let rel_fd = (dv - dv_fd).abs() / dv_fd.abs();
    assert!(
        rel_fd <= GATE,
        "seam dV/dr = {dv} vs FD {dv_fd} (rel err {rel_fd:.3e})"
    );

    // Gate 2: node-wise dx/dr across the whole mesh.
    let fd = fd_velocities(build1, R0, H, &plan).expect("fd velocities");
    let cmp = compare_velocities(&seam.velocities, &fd);
    assert!(
        cmp.max_rel_err <= GATE,
        "dx/dr max rel err {:.3e} at node {}",
        cmp.max_rel_err,
        cmp.worst_node
    );

    // Exact spot-checks on the moving tangent lines: a node on the top
    // face's front tangent line (z = L, y = r) slides at exactly +ŷ — the
    // frozen u = const sample composed of the blend's retreat (0, 1, −1)
    // and its radial growth (0, 0, 1).
    let mut tangent_nodes = 0;
    let mut corner_nodes = 0;
    for (i, p) in seam.positions.iter().enumerate() {
        if (p.z - L).abs() < 1e-9 && (p.y - R0).abs() < 1e-9 {
            if p.x > R0 + 1e-9 && p.x < L - R0 - 1e-9 {
                tangent_nodes += 1;
                assert!(
                    (seam.velocities[i] - Vec3::new(0.0, 1.0, 0.0)).norm() < 1e-9,
                    "tangent-line node {i}: velocity {:?}",
                    seam.velocities[i]
                );
            } else if (p.x - R0).abs() < 1e-9 {
                // The corner where two tangent lines cross: it slides
                // diagonally along the top face — the rank-deficient case
                // the tangency-completion rows exist for.
                corner_nodes += 1;
                assert!(
                    (seam.velocities[i] - Vec3::new(1.0, 1.0, 0.0)).norm() < 1e-9,
                    "tangent-corner node {i}: velocity {:?}",
                    seam.velocities[i]
                );
            }
        }
    }
    assert!(
        tangent_nodes >= 1,
        "expected tangent-line nodes on the top face"
    );
    assert!(
        corner_nodes >= 1,
        "expected a tangent-corner vertex on the top face"
    );

    // Gate 3: continuum closed form within the polygonization band. The
    // frozen mesh carries 16-segment blends and coarse sphere octants, so
    // the discrete derivative differs from −(dV_minkowski/dr) by a few
    // percent — bounded here at 10% and expected to shrink with segment
    // count. (The exact framework agreement is Gate 1; discretization is a
    // property of the mesh, not the seam.)
    let dmink = {
        let h = 1e-6;
        (minkowski(R0 + h) - minkowski(R0 - h)) / (2.0 * h)
    };
    let gap = (dv - dmink).abs() / dmink.abs();
    assert!(
        gap < 0.10,
        "continuum gap {gap:.3} exceeds the documented polygonization band"
    );
    assert!((v - minkowski(R0)).abs() / minkowski(R0) < 0.01);
}

#[test]
fn m4_fillet_radius_optimized_to_target_volume() {
    // Close the loop on the fillet parameter itself: pick the radius whose
    // rounded cube matches a target volume, by gradient descent through
    // the seam. Target = volume at r* = 2.2 (computed with the same frozen
    // discretization so the optimum is exact for the discrete objective).
    let r_star = 2.2;
    let target = {
        let brep = build(&[r_star]);
        let plan = capture_plan(&brep, &tess_params()).expect("capture target");
        let seam =
            evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new()).expect("seam target");
        volume_with_derivative(&seam).0
    };

    let objective = move |seam: &SeamMesh| -> (f64, f64) {
        let (v, dv) = volume_with_derivative(seam);
        let miss = (v - target) / target;
        (miss * miss, 2.0 * miss * dv / target)
    };

    let result = minimize(
        &build,
        &|brep, _k| {
            seeding(brep, {
                // The blend radius IS θ; read it back from the built model so
                // the seeding stays honest at every iterate.
                brep.geometry
                    .surfaces
                    .iter()
                    .find_map(|s| {
                        s.as_any()
                            .downcast_ref::<CylinderSurface>()
                            .map(|c| c.radius)
                    })
                    .expect("rounded cube has blends")
            })
        },
        &objective,
        &[1.0],
        &tess_params(),
        &OptimizeOptions {
            max_iters: 60,
            initial_step: 5.0,
            min_step: 1e-9,
            grad_tol: 1e-10,
            bounds: vec![(0.5, 4.0)],
        },
    )
    .expect("optimize");

    assert!(result.stop != StopReason::MaxIters, "failed to converge");
    assert!(
        (result.theta[0] - r_star).abs() < 1e-3,
        "found r = {} vs target r* = {r_star}",
        result.theta[0]
    );
}
