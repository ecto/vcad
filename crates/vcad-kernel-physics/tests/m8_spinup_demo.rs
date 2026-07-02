//! M8 demo — gradient descent through the full CAD → physics chain.
//!
//! Brief: a torque-driven flywheel (a parametric disc on a revolute joint)
//! must spin up to a **target speed** `ω*` after a fixed time under a fixed
//! torque. The single CAD parameter is the disc radius `r`. Recover the
//! radius `r*` that hits the target, by projected gradient descent whose
//! gradient flows entirely through the M8 factorization:
//!
//! ```text
//! J(r) = (ω(r) − ω*)²,   dJ/dr = ∂J/∂p · dp/dr   (mass-property factorization)
//! ```
//!
//! The optimizer never sees the physics internals — it drives
//! [`rollout_gradient`] as a black-box `(J, dJ/dr)` oracle, exactly the shape
//! [`objective_gradient`](vcad_kernel_diff::objective_gradient) presents to the
//! seam's own `minimize`. (The seam's `minimize` / `minimize_lbfgs` take
//! B-rep build functions and mesh objectives, so they do not compose with a
//! physics objective directly; this demo drives a small projected-GD loop over
//! the same oracle shape instead.)

use phyz::math::{SpatialTransform, Vec3};
use phyz::{aba_with_external_forces, ModelBuilder};
use vcad_kernel_diff::{synthesize_seeding, ParamSeeding, SurfaceSeed};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient, BodyMassProps, DiffBody, MassPropFdSteps,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::TessellationParams;

const DENSITY: f64 = 2700.0;
const HEIGHT_MM: f64 = 8.0;
const TORQUE: f64 = 1e-4;
const T_FINAL: f64 = 0.2;
const DT: f64 = 1.0 / 480.0;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

fn build_cylinder(theta: &[f64]) -> BRepSolid {
    make_cylinder(theta[0], HEIGHT_MM, 64)
}

/// Final spin speed ω(T) of the torque-driven disc.
fn spin(props: &[BodyMassProps]) -> f64 {
    let si = props[0].to_spatial_inertia();
    let model = ModelBuilder::new()
        .gravity(Vec3::new(0.0, 0.0, -9.81))
        .dt(DT)
        .add_revolute_body("disc", -1, SpatialTransform::identity(), si)
        .build();
    let mut state = model.default_state();
    let steps = (T_FINAL / DT).round() as usize;
    for _ in 0..steps {
        state.ctrl[0] = TORQUE;
        let qdd = aba_with_external_forces(&model, &state, None);
        state.v[0] += qdd[0] * DT;
        state.q[0] += state.v[0] * DT;
    }
    state.v[0]
}

fn hand_body<'a>() -> DiffBody<'a> {
    DiffBody {
        build: Box::new(build_cylinder),
        seeding_for: Box::new(|brep: &BRepSolid, _theta: &[f64], _k: usize| {
            let mut s = ParamSeeding::new();
            s.seed_where(
                &brep.geometry,
                |surf| surf.as_any().downcast_ref::<CylinderSurface>().is_some(),
                SurfaceSeed::CylinderRadius { rate: 1.0 },
            );
            Ok(s)
        }),
        density_kg_m3: DENSITY,
        tess: tess(),
    }
}

#[test]
fn recover_radius_hitting_target_spin_speed() {
    // Target: the spin speed of the *true* radius r* = 12 mm.
    let r_star = 12.0;
    let target = spin(&nominal_mass_props(&[hand_body()], &[r_star]).expect("props"));

    // The optimization objective is the squared miss, differentiated through
    // the full chain by the adapter.
    let rollout = |p: &[BodyMassProps]| {
        let w = spin(p);
        let miss = w - target;
        miss * miss
    };

    let bodies = [hand_body()];
    let fd = MassPropFdSteps::default();
    let bounds = (1.0, 30.0);

    // Projected gradient descent with backtracking. Start well away from r*.
    let mut r = 8.0_f64;
    let (mut j, mut grad) =
        rollout_gradient(&bodies, &rollout, &[r], &fd).expect("initial gradient");
    let mut step = 1.0;
    let mut iters = 0;
    for _ in 0..200 {
        if grad[0].abs() < 1e-14 {
            break;
        }
        // Backtracking line search on the projected step.
        let accepted = loop {
            let trial = (r - step * grad[0]).clamp(bounds.0, bounds.1);
            if (trial - r).abs() > 0.0 {
                match rollout_gradient(&bodies, &rollout, &[trial], &fd) {
                    Ok((jt, gt)) if jt < j => break Some((trial, jt, gt)),
                    _ => {}
                }
            }
            step *= 0.5;
            if step < 1e-10 {
                break None;
            }
        };
        match accepted {
            Some((rt, jt, gt)) => {
                r = rt;
                j = jt;
                grad = gt;
                step *= 2.0; // warm-start the next search a little longer
                iters += 1;
            }
            None => break,
        }
    }

    let achieved = spin(&nominal_mass_props(&bodies, &[r]).expect("props"));
    // Recovered the radius, and the simulated spin matches the target.
    assert!(
        (r - r_star).abs() < 1e-2,
        "recovered r = {r} mm vs r* = {r_star} mm after {iters} iters (J = {j})"
    );
    assert!(
        (achieved - target).abs() / target.abs() < 1e-5,
        "achieved spin {achieved} vs target {target}"
    );
}

/// The `seeding_for` closure supports [`synthesize_seeding`] naturally: a
/// machine-derived seeding drives the same gradient as the hand-written one,
/// to the seam's agreement tolerance.
#[test]
fn synthesized_seeding_matches_hand_written() {
    let rollout = |p: &[BodyMassProps]| spin(p);
    let fd = MassPropFdSteps::default();
    let r0 = 10.0;

    let hand = [hand_body()];
    let (_j_h, g_h) = rollout_gradient(&hand, &rollout, &[r0], &fd).expect("hand gradient");

    let synth = [DiffBody {
        build: Box::new(build_cylinder),
        seeding_for: Box::new(|_brep: &BRepSolid, theta: &[f64], k: usize| {
            synthesize_seeding(&build_cylinder, theta, k, 1e-6)
        }),
        density_kg_m3: DENSITY,
        tess: tess(),
    }];
    let (_j_s, g_s) = rollout_gradient(&synth, &rollout, &[r0], &fd).expect("synth gradient");

    let rel = (g_h[0] - g_s[0]).abs() / g_h[0].abs().max(1.0);
    assert!(
        rel < 1e-4,
        "synthesized dω/dr {} vs hand-written {} (rel {rel:.3e})",
        g_s[0],
        g_h[0]
    );
}
