//! M11 — the phyz inertia-parameter adjoint replaces the FD `∂J/∂p` factor.
//!
//! The M8 chain `dJ/dθ = Σ ∂J/∂p·dp/dθ` had one inexact factor: `∂J/∂p` by
//! central FD on the mass-property scalars (~20 rollouts per body). With the
//! phyz trajectory adjoint both factors are analytic. Gates:
//!
//! 1. **Adjoint vs FD path** (`1e-5`): the same rollout priced by
//!    `rollout_gradient_adjoint` and by the FD-based `rollout_gradient`
//!    must agree to the FD path's own accuracy (its central differences sit
//!    near the ~1e-5 relative sweet spot the step policy targets).
//! 2. **End-to-end** (`1e-4`): the adjoint gradient against a *full*
//!    rebuild-and-resimulate central FD — rebuild the CAD at `r ± h`,
//!    convert to mass properties, re-simulate — the same bar as the M8
//!    gates in `m8_rollout_gradient.rs`.
//! 3. **Determinism**: two adjoint passes are bit-identical.
//!
//! Both rollouts are the M8 pair re-expressed as [`AdjointRolloutSpec`]s:
//! the torque-driven flywheel (inertia channel isolated; `ω(T) = τT/I_zz`
//! exactly) and a gravity pendulum (mass, COM, and inertia channels all
//! live). The pendulum mounts the cylinder via the **joint axis** (revolute
//! about body-X) rather than a transformed inertia, honouring the spec's
//! body-frame contract.

use phyz::math::{DVec, SpatialTransform, Vec3};
use phyz::{Joint, JointType, Model, ModelBuilder};
use vcad_kernel_diff::{ParamSeeding, SurfaceSeed};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_physics::diff::interop::AdjointRolloutSpec;
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient, rollout_gradient_adjoint, BodyMassProps, DiffBody,
    MassPropFdSteps,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
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
// Flywheel: revolute about Z, COM on the axis, constant torque, J = ω(T).
// ---------------------------------------------------------------------------

const TORQUE: f64 = 1e-4;
const FLY_DT: f64 = 1.0 / 480.0;
const FLY_STEPS: usize = 96; // 0.2 s

fn flywheel_model(props: &[BodyMassProps]) -> Model {
    ModelBuilder::new()
        .gravity(Vec3::new(0.0, 0.0, -9.81))
        .dt(FLY_DT)
        .add_revolute_body(
            "disc",
            -1,
            SpatialTransform::identity(),
            props[0].to_spatial_inertia(),
        )
        .build()
}

fn flywheel_spec<'a>() -> AdjointRolloutSpec<'a> {
    AdjointRolloutSpec {
        build_model: Box::new(|p: &[BodyMassProps]| flywheel_model(p)),
        q0: vec![0.0],
        v0: vec![0.0],
        steps: FLY_STEPS,
        ctrl: Box::new(|_t| DVec::from_slice(&[TORQUE])),
        objective_value: Box::new(|_q, v| v[0]),
        objective_gradient: Box::new(|q, v| {
            let mut gv = vec![0.0; v.len()];
            gv[0] = 1.0;
            (vec![0.0; q.len()], gv)
        }),
    }
}

/// The identical rollout as an opaque closure, for the FD path and the
/// end-to-end oracle: same integrator (semi-implicit Euler), same steps.
fn flywheel_rollout(props: &[BodyMassProps]) -> f64 {
    let model = flywheel_model(props);
    let mut state = model.default_state();
    for _ in 0..FLY_STEPS {
        state.ctrl[0] = TORQUE;
        let qdd = phyz::aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * FLY_DT;
        state.q[0] += state.v[0] * FLY_DT;
    }
    state.v[0]
}

// ---------------------------------------------------------------------------
// Pendulum: revolute about body-X, gravity −Y; the CAD cylinder's COM at
// (0, 0, h/2) gives a live lever arm without any inertia transform.
// ---------------------------------------------------------------------------

const PEND_DT: f64 = 1.0 / 960.0;
const PEND_STEPS: usize = 144; // 0.15 s
const Q0: f64 = 0.4;

fn pendulum_model(props: &[BodyMassProps]) -> Model {
    let joint = Joint {
        joint_type: JointType::Revolute,
        parent_to_joint: SpatialTransform::identity(),
        axis: Vec3::new(1.0, 0.0, 0.0),
        damping: 0.0,
        limits: None,
        // phyz's Joint grew limit stiffness/damping, armature, spring, and
        // friction-loss fields; this pendulum wants all of them neutral.
        ..Default::default()
    };
    ModelBuilder::new()
        .gravity(Vec3::new(0.0, -9.81, 0.0))
        .dt(PEND_DT)
        .add_body("bob", -1, joint, props[0].to_spatial_inertia())
        .build()
}

fn pendulum_spec<'a>() -> AdjointRolloutSpec<'a> {
    AdjointRolloutSpec {
        build_model: Box::new(|p: &[BodyMassProps]| pendulum_model(p)),
        q0: vec![Q0],
        v0: vec![0.0],
        steps: PEND_STEPS,
        ctrl: Box::new(|_t| DVec::from_slice(&[0.0])),
        objective_value: Box::new(|q, _v| q[0]),
        objective_gradient: Box::new(|q, v| {
            let mut gq = vec![0.0; q.len()];
            gq[0] = 1.0;
            (gq, vec![0.0; v.len()])
        }),
    }
}

fn pendulum_rollout(props: &[BodyMassProps]) -> f64 {
    let model = pendulum_model(props);
    let mut state = model.default_state();
    state.q[0] = Q0;
    for _ in 0..PEND_STEPS {
        let qdd = phyz::aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * PEND_DT;
        state.q[0] += state.v[0] * PEND_DT;
    }
    state.q[0]
}

// ---------------------------------------------------------------------------
// Gate 1 — adjoint vs the FD path (1e-5).
// ---------------------------------------------------------------------------

#[test]
fn adjoint_matches_fd_path() {
    let bodies = [cylinder_body()];

    let (j_adj, g_adj) =
        rollout_gradient_adjoint(&bodies, &flywheel_spec(), &[R0]).expect("adjoint");
    let (j_fd, g_fd) = rollout_gradient(
        &bodies,
        &flywheel_rollout,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("fd path");
    // Same integrator, same trajectory: the primal objectives agree to
    // roundoff; the gradients to the FD path's own accuracy.
    assert!(
        (j_adj - j_fd).abs() <= 1e-12 * j_fd.abs(),
        "flywheel J: adjoint {j_adj} vs fd-path {j_fd}"
    );
    let rel = (g_adj[0] - g_fd[0]).abs() / g_fd[0].abs();
    assert!(
        rel <= 1e-5,
        "flywheel dJ/dr: adjoint {} vs fd-path {} (rel {rel:.3e})",
        g_adj[0],
        g_fd[0]
    );

    let (j_adj, g_adj) =
        rollout_gradient_adjoint(&bodies, &pendulum_spec(), &[R0]).expect("adjoint");
    let (j_fd, g_fd) = rollout_gradient(
        &bodies,
        &pendulum_rollout,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("fd path");
    assert!(
        (j_adj - j_fd).abs() <= 1e-12 * j_fd.abs().max(1.0),
        "pendulum J: adjoint {j_adj} vs fd-path {j_fd}"
    );
    let rel = (g_adj[0] - g_fd[0]).abs() / g_fd[0].abs();
    assert!(
        rel <= 1e-5,
        "pendulum dJ/dr: adjoint {} vs fd-path {} (rel {rel:.3e})",
        g_adj[0],
        g_fd[0]
    );
}

// ---------------------------------------------------------------------------
// Gate 2 — end-to-end rebuild-and-resimulate FD (1e-4), the M8 bar.
// ---------------------------------------------------------------------------

#[test]
fn adjoint_matches_end_to_end_fd() {
    const GATE: f64 = 1e-4;
    const H_THETA: f64 = 1e-3; // mm — same reasoning as the M8 gates.
    let bodies = [cylinder_body()];

    let e2e = |rollout: &dyn Fn(&[BodyMassProps]) -> f64| -> f64 {
        let j_at = |r: f64| {
            let props = nominal_mass_props(&bodies, &[r]).expect("props");
            rollout(&props)
        };
        (j_at(R0 + H_THETA) - j_at(R0 - H_THETA)) / (2.0 * H_THETA)
    };

    let (_, g) = rollout_gradient_adjoint(&bodies, &flywheel_spec(), &[R0]).expect("adjoint");
    let fd = e2e(&flywheel_rollout);
    let rel = (g[0] - fd).abs() / fd.abs();
    assert!(
        rel <= GATE,
        "flywheel dω/dr: adjoint {} vs end-to-end fd {fd} (rel {rel:.3e})",
        g[0]
    );
    assert!(g[0] < 0.0, "spin-up speed must fall as the disc grows");

    let (_, g) = rollout_gradient_adjoint(&bodies, &pendulum_spec(), &[R0]).expect("adjoint");
    let fd = e2e(&pendulum_rollout);
    let rel = (g[0] - fd).abs() / fd.abs();
    assert!(
        rel <= GATE,
        "pendulum dq(T)/dr: adjoint {} vs end-to-end fd {fd} (rel {rel:.3e})",
        g[0]
    );
}

// ---------------------------------------------------------------------------
// Gate 3 — determinism.
// ---------------------------------------------------------------------------

#[test]
fn adjoint_is_deterministic() {
    let bodies = [cylinder_body()];
    let (ja, ga) = rollout_gradient_adjoint(&bodies, &pendulum_spec(), &[R0]).expect("adjoint");
    let (jb, gb) = rollout_gradient_adjoint(&bodies, &pendulum_spec(), &[R0]).expect("adjoint");
    assert_eq!(ja, jb, "objective must be bit-identical");
    assert_eq!(ga, gb, "gradient must be bit-identical");
}
