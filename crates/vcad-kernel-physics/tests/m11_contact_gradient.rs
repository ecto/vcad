//! M11 — contact forces inside the rollout, priced end to end.
//!
//! This closes the last honest boundary of the M8 contract ("contact-free
//! only"): a CAD parameter that changes the **collision surface** now
//! differentiates through contact dynamics. The chain is
//!
//! ```text
//! dJ/dθ = Σ ∂J/∂p·dp/dθ            (mass-property channel, both exact)
//!       + Σ pullback(∂J/∂x)·seeding (surface channel: phyz contact adjoint
//!                                    → M5 pullback, both exact)
//! ```
//!
//! with `∂J/∂x` from the phyz trajectory adjoint under the differentiable
//! per-vertex penalty contact model, on the body's own frozen-plan seam
//! mesh.
//!
//! **The model:** the CAD cylinder (radius θ) lies on its side (a rotated
//! mount frame on a vertical prismatic joint) resting on the ground plane;
//! `J = q(T)`, the settled height. Growing the radius moves the contact
//! line down the body (surface channel, `≈ +1e−3 m per mm` — the vertex at
//! `y_body = r` drops by exactly `dθ`) *and* adds mass which sinks the body
//! deeper into the penalty spring (mass channel, negative, ~2% of the
//! total) — both channels are live and pull in opposite directions.
//!
//! Gates:
//! 1. **End-to-end FD** (`1e-4`): full chain vs rebuild-and-resimulate
//!    central FD at `θ ± h` — CAD rebuild, re-tessellation, re-simulation
//!    with contact.
//! 2. **Channel liveness**: the gradient sits near the surface channel's
//!    `+1e−3` (so the FD agreement is not vacuous), and doubling the
//!    density measurably shifts it (mass channel load-bearing).
//! 3. **Determinism**: bit-identical across runs.
//!
//! The settled steady state keeps every active contact well inside its
//! smooth branch (activation margins ~3e−5 m vs FD probe movements ~1e−6 m),
//! so the central-difference oracle is clean; see the phyz-side gates for
//! the transient-free argument.

use phyz::math::{DVec, Mat3, SpatialTransform, Vec3};
use phyz::{Joint, JointType, Model, ModelBuilder};
use vcad_kernel_diff::{ParamSeeding, SurfaceSeed};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_physics::{
    contact_rollout_gradient, AdjointRolloutSpec, BodyMassProps, ContactConfig, DiffBody,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::TessellationParams;

/// Aluminium-ish density (kg/m³).
const DENSITY: f64 = 2700.0;
/// Cylinder height (mm).
const HEIGHT_MM: f64 = 8.0;
/// Nominal radius (mm).
const R0: f64 = 10.0;
/// mm → m.
const MM_TO_M: f64 = 1e-3;

const DT: f64 = 1e-3;
const STEPS: usize = 400; // 0.4 s — settled to ~e−44 of the transient.

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

fn cylinder_body<'a>(density: f64) -> DiffBody<'a> {
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
        density_kg_m3: density,
        tess: tess(),
    }
}

/// The cylinder on its side: mount rotation `R_x(90°)` in `parent_to_joint`
/// (body ẑ — the cylinder axis — maps to world ŷ), vertical prismatic
/// motion via the axis expressed in the mounted joint frame. The body's
/// spatial inertia is installed verbatim (CAD body frame), honouring the
/// spec contract — the mount lives entirely in the joint.
fn lying_cylinder_model(props: &[BodyMassProps]) -> Model {
    // world→joint rotation R_x(90°); world +Z expressed in the joint frame
    // is (0, −1, 0), which makes q a straight world-height coordinate.
    let e_wj = Mat3::new(1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0);
    let joint = Joint {
        joint_type: JointType::Prismatic,
        parent_to_joint: SpatialTransform::new(e_wj, Vec3::zeros()),
        axis: Vec3::new(0.0, -1.0, 0.0),
        damping: 0.0,
        limits: None,
    };
    ModelBuilder::new()
        .gravity(Vec3::new(0.0, 0.0, -9.81))
        .dt(DT)
        .add_body("roller", -1, joint, props[0].to_spatial_inertia())
        .build()
}

fn spec<'a>() -> AdjointRolloutSpec<'a> {
    AdjointRolloutSpec {
        build_model: Box::new(|p: &[BodyMassProps]| lying_cylinder_model(p)),
        q0: vec![0.0], // lowest surface line exactly touching the ground
        v0: vec![0.0],
        steps: STEPS,
        ctrl: Box::new(|_t| DVec::from_slice(&[0.0])),
        objective_value: Box::new(|q, _v| q[0]),
        objective_gradient: Box::new(|q, v| {
            let mut gq = vec![0.0; q.len()];
            gq[0] = 1.0;
            (gq, vec![0.0; v.len()])
        }),
    }
}

fn contact() -> ContactConfig {
    ContactConfig {
        // The lowest body point (y_body = r₀) sits at world z = −r₀ at q = 0.
        ground_height_m: -R0 * MM_TO_M,
        stiffness: 100.0, // N/m per vertex — ω·dt ≈ 0.2, ζ ≈ 0.5 with c below
        damping: 0.5,
    }
}

// ---------------------------------------------------------------------------
// Gate 1 — full chain vs rebuild-and-resimulate FD (1e-4).
// ---------------------------------------------------------------------------

#[test]
fn contact_chain_matches_end_to_end_fd() {
    const GATE: f64 = 1e-4;
    const H_THETA: f64 = 1e-3; // mm; moves contact vertices by 1e−6 m,
                               // well inside the ~3e−5 m activation margins.

    let bodies = [cylinder_body(DENSITY)];
    let (j, grad) =
        contact_rollout_gradient(&bodies, &spec(), &contact(), &[0], &[R0]).expect("gradient");

    // The body must actually have settled into the spring (J < 0, order of
    // the equilibrium penetration) — otherwise the gate isn't testing
    // contact dynamics at all.
    assert!(
        j < -1e-5 && j > -1e-2,
        "settled height J = {j} not in the expected penalty-equilibrium range"
    );

    // Rebuild-and-resimulate central difference of the same rollout.
    let j_at = |r: f64| -> f64 {
        let b = [cylinder_body(DENSITY)];
        let (j, _) = contact_rollout_gradient(&b, &spec(), &contact(), &[0], &[r]).expect("probe");
        j
    };
    let fd = (j_at(R0 + H_THETA) - j_at(R0 - H_THETA)) / (2.0 * H_THETA);
    let rel = (grad[0] - fd).abs() / fd.abs();
    assert!(
        rel <= GATE,
        "dJ/dr: chain {} vs end-to-end fd {fd} (rel {rel:.3e})",
        grad[0]
    );
}

// ---------------------------------------------------------------------------
// Gate 2 — both channels load-bearing.
// ---------------------------------------------------------------------------

#[test]
fn both_channels_are_live() {
    let bodies = [cylinder_body(DENSITY)];
    let (_, grad) =
        contact_rollout_gradient(&bodies, &spec(), &contact(), &[0], &[R0]).expect("gradient");

    // Surface channel dominates: the contact line drops by exactly dθ as the
    // radius grows, so dJ/dθ ≈ +1e−3 m/mm minus the (negative) mass term.
    assert!(
        grad[0] > 0.5e-3 && grad[0] < 1.05e-3,
        "dJ/dr = {} not in the surface-dominated range (+1e−3 m/mm scale)",
        grad[0]
    );

    // Mass channel: doubling the density must measurably lower the gradient
    // (heavier body sinks further per unit of added mass — the ∂J/∂p·dp/dθ
    // term is negative and scales with density).
    let heavy = [cylinder_body(2.0 * DENSITY)];
    let (_, grad_heavy) =
        contact_rollout_gradient(&heavy, &spec(), &contact(), &[0], &[R0]).expect("gradient");
    assert!(
        grad_heavy[0] < grad[0] - 1e-5,
        "mass channel not load-bearing: {} (2ρ) vs {} (ρ)",
        grad_heavy[0],
        grad[0]
    );
}

// ---------------------------------------------------------------------------
// Gate 3 — determinism.
// ---------------------------------------------------------------------------

#[test]
fn contact_gradient_is_deterministic() {
    let bodies = [cylinder_body(DENSITY)];
    let (ja, ga) =
        contact_rollout_gradient(&bodies, &spec(), &contact(), &[0], &[R0]).expect("gradient");
    let (jb, gb) =
        contact_rollout_gradient(&bodies, &spec(), &contact(), &[0], &[R0]).expect("gradient");
    assert_eq!(ja, jb, "objective must be bit-identical");
    assert_eq!(ga, gb, "gradient must be bit-identical");
}
