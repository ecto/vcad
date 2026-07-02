//! M8 — physics-rollout gradients: `dJ/dθ` of a simulation objective with
//! respect to a CAD parameter, via the mass-property factorization.
//!
//! Three gates:
//!
//! 1. **Mass-property chain in isolation** (`1e-6`): the seam's exact
//!    `d(I_zz)/dr` against a central difference of a CAD rebuild. This is the
//!    `dp/dθ` factor, and it reproduces the M3 result inside the physics
//!    crate.
//! 2. **End-to-end rollout gradient** (`1e-4`): the adapter's `dJ/dr` against a
//!    *full* central FD — rebuild the CAD at `r ± h`, convert to mass
//!    properties, re-simulate. Two dynamically meaningful objectives:
//!    a torque-driven flywheel spin-up (isolates the inertia channel) and a
//!    gravity pendulum (exercises the mass, COM, and inertia channels
//!    together).
//! 3. **Determinism**: two identical rollouts return a bit-identical objective,
//!    so the FD estimate of `∂J/∂p` is meaningful.

use phyz::math::{SpatialTransform, Vec3};
use phyz::{aba_with_external_forces, ModelBuilder};
use vcad_kernel_diff::{
    evaluate_with_sensitivity, mass_properties, mass_properties_with_derivative, ParamSeeding,
    SurfaceSeed,
};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient, BodyMassProps, DiffBody, MassPropFdSteps,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

/// Aluminium-ish density (kg/m³).
const DENSITY: f64 = 2700.0;
/// Cylinder height (mm).
const HEIGHT_MM: f64 = 8.0;
/// Nominal radius (mm).
const R0: f64 = 10.0;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

/// The single differentiable body: a cylinder of radius `θ[0]` (mm), axis Z.
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

// ---------------------------------------------------------------------------
// Rollouts (contact-free, deterministic, pure in their mass-property input).
// ---------------------------------------------------------------------------

/// Torque-driven flywheel: a body on a revolute joint about Z (its COM on the
/// axis, so gravity exerts no moment), spun from rest by a constant torque.
/// Returns the final angular speed ω(T) in rad/s.
///
/// With constant torque and constant inertia, semi-implicit Euler gives
/// `ω(T) = τ·T / I_zz` exactly (independent of dt), so this isolates the
/// inertia channel of the factorization.
fn flywheel_rollout(props: &[BodyMassProps], torque: f64, t_final: f64, dt: f64) -> f64 {
    let si = props[0].to_spatial_inertia();
    let model = ModelBuilder::new()
        .gravity(Vec3::new(0.0, 0.0, -9.81))
        .dt(dt)
        .add_revolute_body("bob", -1, SpatialTransform::identity(), si)
        .build();
    let mut state = model.default_state();
    let steps = (t_final / dt).round() as usize;
    for _ in 0..steps {
        state.ctrl[0] = torque;
        let qdd = aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * dt;
        state.q[0] += state.v[0] * dt;
    }
    state.v[0]
}

/// Gravity pendulum: the cylinder is mounted with its axis rotated to lie
/// along +X (via a 90° rotation about Y), so its COM sits off the revolute
/// axis (Z, at the origin). Released from `q0`, it swings under gravity.
/// Returns the joint angle q(T) in radians.
///
/// Gravity is set along −X so the pendulum is planar in the XY plane about the
/// Z joint axis. The gravity torque depends on **mass** and **COM lever arm**;
/// the swing rate depends on the **inertia about the pivot** — so all three
/// mass-property channels enter.
fn pendulum_rollout(props: &[BodyMassProps], q0: f64, t_final: f64, dt: f64) -> f64 {
    // Mount rotation: body-Z (cylinder axis, COM at (0,0,h/2)) → joint +X.
    let ry90 = phyz::math::Mat3::new(0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0);
    let mount = SpatialTransform::new(ry90, Vec3::zeros());
    let si_body = props[0].to_spatial_inertia();
    let si = si_body.transform(&mount);

    let model = ModelBuilder::new()
        .gravity(Vec3::new(-9.81, 0.0, 0.0))
        .dt(dt)
        .add_revolute_body("bob", -1, SpatialTransform::identity(), si)
        .build();
    let mut state = model.default_state();
    state.q[0] = q0;
    let steps = (t_final / dt).round() as usize;
    for _ in 0..steps {
        // No control torque — free swing under gravity.
        let qdd = aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * dt;
        state.q[0] += state.v[0] * dt;
    }
    state.q[0]
}

// ---------------------------------------------------------------------------
// Gate 1 — the dp/dθ factor in isolation (1e-6), M3 pattern.
// ---------------------------------------------------------------------------

#[test]
fn mass_property_chain_isolation() {
    const H: f64 = 1e-6;
    const GATE: f64 = 1e-6;
    let body = cylinder_body();
    let seam_density = body.density_kg_m3 * 1e-9;

    let brep = (body.build)(&[R0]);
    let plan = capture_plan(&brep, &body.tess).expect("capture");
    let seeding = (body.seeding_for)(&brep, &[R0], 0).expect("seeding");
    let seam = evaluate_with_sensitivity(&brep, &plan, &seeding).expect("seam");
    let (_props, dprops) = mass_properties_with_derivative(&seam, seam_density);

    // CAD-rebuild central difference of the mass properties (same frozen plan
    // is not required here — the cylinder topology is invariant in r — so a
    // fresh evaluation at r ± h and the exact integral compare directly).
    let props_at = |r: f64| {
        let b = (body.build)(&[r]);
        let p = capture_plan(&b, &body.tess).expect("capture");
        let s = evaluate_with_sensitivity(&b, &p, &ParamSeeding::new()).expect("seam");
        mass_properties(&s.positions, &s.triangles, seam_density)
    };
    let plus = props_at(R0 + H);
    let minus = props_at(R0 - H);

    let check = |dual: f64, fd: f64, what: &str| {
        let rel = (dual - fd).abs() / fd.abs().max(1.0);
        assert!(
            rel <= GATE,
            "{what}: seam {dual} vs fd {fd} (rel {rel:.3e})"
        );
    };
    check(dprops.mass, (plus.mass - minus.mass) / (2.0 * H), "dm/dr");
    for i in 0..3 {
        check(
            dprops.inertia_centroid[i][i],
            (plus.inertia_centroid[i][i] - minus.inertia_centroid[i][i]) / (2.0 * H),
            "dI/dr",
        );
    }
}

// ---------------------------------------------------------------------------
// Gate 2a — end-to-end flywheel spin-up gradient (1e-4).
// ---------------------------------------------------------------------------

#[test]
fn flywheel_spinup_gradient_matches_end_to_end_fd() {
    const TORQUE: f64 = 1e-4;
    const T_FINAL: f64 = 0.2;
    const DT: f64 = 1.0 / 480.0;
    const GATE: f64 = 1e-4;
    // Outer FD step in θ = mm; the mass properties are smooth in r (no
    // topology change), so a relative step of 1e-4 sits well inside the
    // O(h²)-truncation / roundoff sweet spot.
    const H_THETA: f64 = 1e-3;

    let bodies = [cylinder_body()];
    let rollout = |p: &[BodyMassProps]| flywheel_rollout(p, TORQUE, T_FINAL, DT);

    let (j, grad) =
        rollout_gradient(&bodies, &rollout, &[R0], &MassPropFdSteps::default()).expect("gradient");
    assert_eq!(grad.len(), 1);

    // Full end-to-end central FD: rebuild CAD at r ± h, convert, re-simulate.
    let j_at = |r: f64| {
        let props = nominal_mass_props(&bodies, &[r]).expect("props");
        rollout(&props)
    };
    let fd = (j_at(R0 + H_THETA) - j_at(R0 - H_THETA)) / (2.0 * H_THETA);
    let rel = (grad[0] - fd).abs() / fd.abs().max(1.0);
    assert!(
        rel <= GATE,
        "dω/dr: adapter {} vs end-to-end fd {fd} (rel {rel:.3e}); J = {j}",
        grad[0]
    );

    // Sign / magnitude sanity: I_zz ∝ r⁴, so ω = τT/I_zz falls with r.
    assert!(
        grad[0] < 0.0,
        "spin-up speed must decrease as the disc grows"
    );
}

// ---------------------------------------------------------------------------
// Gate 2b — end-to-end gravity pendulum gradient (1e-4), all channels.
// ---------------------------------------------------------------------------

#[test]
fn gravity_pendulum_gradient_matches_end_to_end_fd() {
    const Q0: f64 = 0.4; // rad off the gravity-aligned rest
    const T_FINAL: f64 = 0.15;
    const DT: f64 = 1.0 / 960.0;
    const GATE: f64 = 1e-4;
    const H_THETA: f64 = 1e-3;

    let bodies = [cylinder_body()];
    let rollout = |p: &[BodyMassProps]| pendulum_rollout(p, Q0, T_FINAL, DT);

    let (_j, grad) =
        rollout_gradient(&bodies, &rollout, &[R0], &MassPropFdSteps::default()).expect("gradient");

    let j_at = |r: f64| {
        let props = nominal_mass_props(&bodies, &[r]).expect("props");
        rollout(&props)
    };
    let fd = (j_at(R0 + H_THETA) - j_at(R0 - H_THETA)) / (2.0 * H_THETA);
    let rel = (grad[0] - fd).abs() / fd.abs().max(1.0);
    assert!(
        rel <= GATE,
        "dq(T)/dr: adapter {} vs end-to-end fd {fd} (rel {rel:.3e})",
        grad[0]
    );
}

// ---------------------------------------------------------------------------
// Gate 3 — determinism.
// ---------------------------------------------------------------------------

#[test]
fn rollout_is_deterministic() {
    let bodies = [cylinder_body()];
    let props = nominal_mass_props(&bodies, &[R0]).expect("props");
    let a = flywheel_rollout(&props, 1e-4, 0.2, 1.0 / 480.0);
    let b = flywheel_rollout(&props, 1e-4, 0.2, 1.0 / 480.0);
    assert_eq!(a, b, "flywheel rollout must be bit-identical across runs");

    let c = pendulum_rollout(&props, 0.4, 0.15, 1.0 / 960.0);
    let d = pendulum_rollout(&props, 0.4, 0.15, 1.0 / 960.0);
    assert_eq!(c, d, "pendulum rollout must be bit-identical across runs");
}
