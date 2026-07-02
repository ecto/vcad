//! M8 surface skin — objectives that read the tessellated surface, priced
//! through the M5 pullback and summed with the mass-property core.
//!
//! The M8 design note named this extension precisely: "the mass-property
//! factorization is the smooth core, the surface pullback the contact skin."
//! These gates hold [`rollout_gradient_with_surface`] and
//! [`surface_gradient`] to it:
//!
//! 1. **Skin in isolation, machine precision**: with a constant rollout, a
//!    volume surface term's `dJ/dr` must equal the N-gon prism closed form
//!    `2·k·r·h` — the surface channel is exact (analytic node gradient →
//!    pullback → contraction; no FD anywhere).
//! 2. **Core + skin together** (`1e-4`, FD-limited by the core): flywheel
//!    spin-up plus a radial surface penalty, against a full
//!    rebuild-and-resimulate central FD; both channels asserted live.

use vcad_kernel_diff::{evaluate_with_sensitivity, ParamSeeding, SurfaceSeed};
use vcad_kernel_geom::CylinderSurface;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient_with_surface, surface_gradient, BodyMassProps, DiffBody,
    MassPropFdSteps, SurfaceTerm,
};
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

use phyz::math::{SpatialTransform, Vec3 as PVec3};
use phyz::{aba_with_external_forces, ModelBuilder};

const DENSITY: f64 = 2700.0;
const HEIGHT_MM: f64 = 8.0;
const R0: f64 = 10.0;
const SEGS: u32 = 64;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: SEGS,
        height_segments: 2,
        ..Default::default()
    }
}

fn cylinder_body<'a>() -> DiffBody<'a> {
    DiffBody {
        build: Box::new(|theta: &[f64]| make_cylinder(theta[0], HEIGHT_MM, SEGS)),
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

/// Torque-driven flywheel (as in the M8 gates): ω(T) = τT/I_zz exactly.
fn flywheel_rollout(props: &[BodyMassProps]) -> f64 {
    const TORQUE: f64 = 1e-4;
    const T_FINAL: f64 = 0.2;
    const DT: f64 = 1.0 / 480.0;
    let si = props[0].to_spatial_inertia();
    let model = ModelBuilder::new()
        .gravity(PVec3::new(0.0, 0.0, -9.81))
        .dt(DT)
        .add_revolute_body("bob", -1, SpatialTransform::identity(), si)
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

/// Mesh volume via the divergence theorem (the same signed sum the seam's
/// mass-property integrals use), plus its exact node gradient.
fn volume_term() -> SurfaceTerm<'static> {
    Box::new(|positions: &[Point3], triangles: &[[u32; 3]]| {
        let v = vcad_kernel_diff::mass_properties(positions, triangles, 1.0).volume;
        let g = vcad_kernel_diff::volume_gradient(positions, triangles);
        (v, g)
    })
}

/// Radial second-moment surface penalty `P = λ·Σ_i (x_i² + y_i²)` with its
/// exact node gradient `∂P/∂x_i = λ·(2x_i, 2y_i, 0)`.
fn radial_penalty_term(lambda: f64) -> SurfaceTerm<'static> {
    Box::new(move |positions: &[Point3], _triangles: &[[u32; 3]]| {
        let mut p = 0.0;
        let mut g = Vec::with_capacity(positions.len());
        for x in positions {
            p += lambda * (x.x * x.x + x.y * x.y);
            g.push(Vec3::new(2.0 * lambda * x.x, 2.0 * lambda * x.y, 0.0));
        }
        (p, g)
    })
}

#[test]
fn surface_skin_in_isolation_is_exact() {
    // Constant rollout ⇒ zero mass-property channel; J = V(mesh).
    // N-gon prism: V = k r² h, k = ½N sin(2π/N) ⇒ dV/dr = 2 k r h.
    let bodies = [cylinder_body()];
    let terms = vec![Some(volume_term())];
    let (j, grad) = rollout_gradient_with_surface(
        &bodies,
        &terms,
        &|_p: &[BodyMassProps]| 0.0,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("gradient");

    let n = SEGS as f64;
    let k = 0.5 * n * (2.0 * std::f64::consts::PI / n).sin();
    let v_exact = k * R0 * R0 * HEIGHT_MM;
    let dv_exact = 2.0 * k * R0 * HEIGHT_MM;
    assert!(
        (j - v_exact).abs() / v_exact < 1e-12,
        "J {j} vs V {v_exact}"
    );
    let rel = (grad[0] - dv_exact).abs() / dv_exact;
    assert!(
        rel <= 1e-9,
        "skin dV/dr {} vs closed form {dv_exact} (rel {rel:.3e})",
        grad[0]
    );

    // The raw skin entry point agrees: one pullback, same number.
    let body = cylinder_body();
    let brep = (body.build)(&[R0]);
    let plan = capture_plan(&brep, &body.tess).expect("capture");
    let seam = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new()).expect("seam");
    let djdx = vcad_kernel_diff::volume_gradient(&seam.positions, &seam.triangles);
    let skin = surface_gradient(&body, &[R0], &djdx).expect("skin");
    let rel_raw = (skin[0] - dv_exact).abs() / dv_exact;
    assert!(
        rel_raw <= 1e-9,
        "surface_gradient {} vs closed form {dv_exact} (rel {rel_raw:.3e})",
        skin[0]
    );
}

#[test]
fn core_plus_skin_matches_end_to_end_fd() {
    const LAMBDA: f64 = 1e-6; // scales the penalty near the spin-up magnitude
    let bodies = [cylinder_body()];
    let terms = vec![Some(radial_penalty_term(LAMBDA))];
    let rollout = |p: &[BodyMassProps]| flywheel_rollout(p);

    let (_j, grad) = rollout_gradient_with_surface(
        &bodies,
        &terms,
        &rollout,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("gradient");

    // End-to-end FD: rebuild CAD, fresh capture, dynamic term + surface term.
    let j_at = |r: f64| {
        let props = nominal_mass_props(&bodies, &[r]).expect("props");
        let body = cylinder_body();
        let brep = (body.build)(&[r]);
        let plan = capture_plan(&brep, &body.tess).expect("capture");
        let seam = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new()).expect("seam");
        let (p_surf, _) = radial_penalty_term(LAMBDA)(&seam.positions, &seam.triangles);
        rollout(&props) + p_surf
    };
    let h = 1e-3;
    let fd = (j_at(R0 + h) - j_at(R0 - h)) / (2.0 * h);
    let rel = (grad[0] - fd).abs() / fd.abs().max(1e-12);
    assert!(
        rel <= 1e-4,
        "core+skin dJ/dr: adapter {} vs end-to-end fd {fd} (rel {rel:.3e})",
        grad[0]
    );

    // Both channels must be load-bearing.
    let (_, core_only) = rollout_gradient_with_surface(
        &bodies,
        &[None],
        &rollout,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("core-only");
    let (_, skin_only) = rollout_gradient_with_surface(
        &bodies,
        &[Some(radial_penalty_term(LAMBDA))],
        &|_p: &[BodyMassProps]| 0.0,
        &[R0],
        &MassPropFdSteps::default(),
    )
    .expect("skin-only");
    assert!(
        core_only[0].abs() > 1e-12 && skin_only[0].abs() > 1e-12,
        "both channels should contribute: core {} skin {}",
        core_only[0],
        skin_only[0]
    );
    let sum_gap = (grad[0] - (core_only[0] + skin_only[0])).abs();
    assert!(
        sum_gap <= 1e-12 * grad[0].abs().max(1.0),
        "channels must sum linearly: {} vs {} + {}",
        grad[0],
        core_only[0],
        skin_only[0]
    );
}
