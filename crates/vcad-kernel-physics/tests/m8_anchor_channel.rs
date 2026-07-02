//! M8 anchor channel — `dJ/dθ` when θ moves a joint anchor / mount frame as
//! well as the body's mass properties.
//!
//! The M8 note promised: "anchor coordinates slot in as more scalars" of the
//! same factorization. These gates hold [`rollout_gradient_with_anchors`] to
//! that:
//!
//! 1. **Anchor channel in isolation** (`1e-6`): a pendulum whose *geometry*
//!    is θ-independent (empty seeding ⇒ zero mass-property channel) but whose
//!    pivot offset is `a(θ) = θ·MM_TO_M` — the gradient must equal a full
//!    rebuild-and-resimulate central FD, and must be far from zero.
//! 2. **Both channels together** (`1e-4`): the cylinder radius drives the
//!    mass properties *and* the pivot offset (`a = 2r`, a mount hole that
//!    scales with the part). The summed gradient must match the end-to-end
//!    FD, and must differ measurably from the mass-only gradient (the anchor
//!    channel is load-bearing, not decorative).

use phyz::math::{SpatialTransform, Vec3};
use phyz::{aba_with_external_forces, ModelBuilder};
use vcad_kernel_diff::{ParamSeeding, SurfaceSeed};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient, rollout_gradient_with_anchors, AnchorFdSteps,
    BodyMassProps, DiffBody, MassPropFdSteps,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::TessellationParams;

const DENSITY: f64 = 2700.0;
const HEIGHT_MM: f64 = 8.0;
const R0: f64 = 10.0;
const MM_TO_M: f64 = 1e-3;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

/// Differentiable cylinder body: radius = θ[0] (mm).
fn cylinder_body<'a>() -> DiffBody<'a> {
    DiffBody {
        build: Box::new(|theta: &[f64]| make_cylinder(theta[0], HEIGHT_MM, 64)),
        seeding_for: Box::new(|brep: &BRepSolid, _theta: &[f64], _k: usize| {
            let mut s = ParamSeeding::new();
            let n = s.seed_where(
                &brep.geometry,
                |surf| surf.as_any().downcast_ref::<CylinderSurface>().is_some(),
                SurfaceSeed::CylinderRadius { rate: 1.0 },
            );
            assert_eq!(n, 1, "expected exactly one cylinder surface");
            Ok(s)
        }),
        density_kg_m3: DENSITY,
        tess: tess(),
    }
}

/// θ-independent body (fixed 10 mm cylinder): the seeding is empty, so the
/// mass-property channel contributes exactly zero and any gradient must come
/// from the anchor channel alone.
fn fixed_body<'a>() -> DiffBody<'a> {
    DiffBody {
        build: Box::new(|_theta: &[f64]| make_cylinder(R0, HEIGHT_MM, 64)),
        seeding_for: Box::new(|_brep: &BRepSolid, _theta: &[f64], _k: usize| {
            Ok(ParamSeeding::new())
        }),
        density_kg_m3: DENSITY,
        tess: tess(),
    }
}

/// Gravity pendulum with a translated mount: the cylinder's axis is rotated
/// to +X (as in the M8 pendulum) and the whole body is shifted +X by the
/// anchor offset `anchors[0]` (m) before attaching to the revolute Z joint at
/// the origin — a pivot hole whose position is a model parameter. Returns
/// q(T).
fn offset_pendulum_rollout(
    props: &[BodyMassProps],
    anchors: &[f64],
    q0: f64,
    t_final: f64,
    dt: f64,
) -> f64 {
    let ry90 = phyz::math::Mat3::new(0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0);
    let mount = SpatialTransform::new(ry90, Vec3::new(anchors[0], 0.0, 0.0));
    let si = props[0].to_spatial_inertia().transform(&mount);

    let model = ModelBuilder::new()
        .gravity(Vec3::new(-9.81, 0.0, 0.0))
        .dt(dt)
        .add_revolute_body("bob", -1, SpatialTransform::identity(), si)
        .build();
    let mut state = model.default_state();
    state.q[0] = q0;
    let steps = (t_final / dt).round() as usize;
    for _ in 0..steps {
        let qdd = aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * dt;
        state.q[0] += state.v[0] * dt;
    }
    state.q[0]
}

const Q0: f64 = 0.4;
const T_FINAL: f64 = 0.15;
const DT: f64 = 1.0 / 960.0;

#[test]
fn anchor_channel_in_isolation_matches_end_to_end_fd() {
    // Geometry fixed; θ moves only the pivot offset a(θ) = θ·MM_TO_M.
    let bodies = [fixed_body()];
    let anchor_map = |theta: &[f64]| vec![theta[0] * MM_TO_M];
    let rollout = |p: &[BodyMassProps], a: &[f64]| offset_pendulum_rollout(p, a, Q0, T_FINAL, DT);

    let (j, grad) = rollout_gradient_with_anchors(
        &bodies,
        &anchor_map,
        &rollout,
        &[R0],
        &MassPropFdSteps::default(),
        &AnchorFdSteps::default(),
    )
    .expect("gradient");

    // End-to-end FD: same fixed geometry, anchors rebuilt at θ ± h.
    let props = nominal_mass_props(&bodies, &[R0]).expect("props");
    let j_at = |t: f64| rollout(&props, &anchor_map(&[t]));
    let h = 1e-3;
    let fd = (j_at(R0 + h) - j_at(R0 - h)) / (2.0 * h);
    let rel = (grad[0] - fd).abs() / fd.abs().max(1e-12);
    assert!(
        rel <= 1e-6,
        "anchor-only dq(T)/dθ: adapter {} vs end-to-end fd {fd} (rel {rel:.3e}); J = {j}",
        grad[0]
    );
    // The channel must be live: moving the pivot 1 mm visibly changes q(T).
    assert!(
        grad[0].abs() > 1e-4,
        "anchor sensitivity should be far from zero, got {}",
        grad[0]
    );
}

#[test]
fn combined_mass_and_anchor_channels_match_end_to_end_fd() {
    // θ = radius drives BOTH the mass properties and the pivot offset
    // a(θ) = 2θ·MM_TO_M (a mount hole at twice the radius).
    let bodies = [cylinder_body()];
    let anchor_map = |theta: &[f64]| vec![2.0 * theta[0] * MM_TO_M];
    let rollout = |p: &[BodyMassProps], a: &[f64]| offset_pendulum_rollout(p, a, Q0, T_FINAL, DT);

    let (_j, grad) = rollout_gradient_with_anchors(
        &bodies,
        &anchor_map,
        &rollout,
        &[R0],
        &MassPropFdSteps::default(),
        &AnchorFdSteps::default(),
    )
    .expect("gradient");

    // Full end-to-end FD: rebuild CAD, rebuild anchors, re-simulate.
    let j_at = |t: f64| {
        let props = nominal_mass_props(&bodies, &[t]).expect("props");
        rollout(&props, &anchor_map(&[t]))
    };
    let h = 1e-3;
    let fd = (j_at(R0 + h) - j_at(R0 - h)) / (2.0 * h);
    let rel = (grad[0] - fd).abs() / fd.abs().max(1e-12);
    assert!(
        rel <= 1e-4,
        "combined dq(T)/dr: adapter {} vs end-to-end fd {fd} (rel {rel:.3e})",
        grad[0]
    );

    // The anchor channel must contribute: dropping it (mass-only adapter with
    // anchors frozen at their nominal value) must give a measurably different
    // gradient.
    let anchors0 = anchor_map(&[R0]);
    let (_, mass_only) = rollout_gradient(
        &bodies,
        &|p: &[BodyMassProps]| rollout(p, &anchors0),
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("mass-only gradient");
    let gap = (grad[0] - mass_only[0]).abs();
    assert!(
        gap > 1e-3 * grad[0].abs().max(1e-12),
        "anchor channel should be load-bearing: combined {} vs mass-only {}",
        grad[0],
        mass_only[0]
    );
}
