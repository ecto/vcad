//! Cross-scale gradient: `d(rollout objective)/d(lattice constant)` — the
//! atoms → continuum → part → trajectory chain, end to end.
//!
//! θ = `[a]`, the FCC lattice constant of the part's material, in Ångström.
//! The chain runs from the ångström-scale lattice to a centimetre-scale
//! part's trajectory — eight orders of magnitude of length scale:
//!
//! ```text
//! a (Å)  →  ρ(a) (kg/m³)   vcad-kernel-atoms::homogenize::density
//!        →  p(ρ) (kg, m)   fixed cylinder B-rep through the seam
//!        →  J    (rad/s)   phyz flywheel rollout
//! ```
//!
//! and `dJ/da = Σ ∂J/∂p · dp/dρ · dρ/da` via
//! [`rollout_gradient_via_density`]. Two independent referees check the
//! chain: a brute-force central difference that rebuilds *everything* at
//! `a ± h`, and the closed form — under semi-implicit Euler with a constant
//! torque and no gravity moment, `ω(T) = τT/I_zz` exactly, and with
//! `I_zz ∝ ρ ∝ a⁻³` that gives `dJ/da = 3J/a`.

use phyz::math::{SpatialTransform, Vec3};
use phyz::{aba_with_external_forces, ModelBuilder};
use vcad_kernel_atoms::builder::fcc;
use vcad_kernel_atoms::homogenize::{density, fd_gradient};
use vcad_kernel_atoms::AtomSystem;
use vcad_kernel_diff::ParamSeeding;
use vcad_kernel_physics::{
    nominal_mass_props, rollout_gradient_via_density, BodyMassProps, DiffBody, MassPropFdSteps,
};
use vcad_kernel_primitives::make_cylinder;
use vcad_kernel_tessellate::TessellationParams;

/// Argon FCC lattice constant near equilibrium (Å).
const A0: f64 = 5.26;
/// Supercell repeat (density needs no force evaluation, so a small cell is
/// exact and cheap).
const NCELL: usize = 2;
/// Flywheel geometry (mm) — fixed; only the material varies.
const R0: f64 = 10.0;
const HEIGHT_MM: f64 = 8.0;
/// Rollout constants (match the M8 gates).
const TORQUE: f64 = 1e-4;
const T_FINAL: f64 = 0.2;
const DT: f64 = 1.0 / 480.0;

/// ρ(a): homogenized density of the argon FCC crystal at lattice constant
/// `a` — the atomic half of the chain.
fn rho_of_a(a: f64) -> f64 {
    let sys = AtomSystem::from_ir(&fcc("Ar", a, NCELL, NCELL, NCELL)).expect("build FCC");
    density(&sys).expect("periodic density")
}

/// The fixed-geometry flywheel body at material density `rho`.
fn flywheel_body<'a>(rho: f64) -> DiffBody<'a> {
    DiffBody {
        build: Box::new(|_theta: &[f64]| make_cylinder(R0, HEIGHT_MM, 64)),
        // Geometry carries no θ-dependence in the density channel.
        seeding_for: Box::new(|_brep, _theta, _k| Ok(ParamSeeding::new())),
        density_kg_m3: rho,
        tess: TessellationParams {
            circle_segments: 64,
            height_segments: 2,
            ..Default::default()
        },
    }
}

/// Torque-driven flywheel: revolute about Z, COM on the axis (gravity is
/// moment-free), spun from rest. Returns ω(T) in rad/s.
fn flywheel_rollout(props: &[BodyMassProps]) -> f64 {
    let si = props[0].to_spatial_inertia();
    let model = ModelBuilder::new()
        .gravity(Vec3::new(0.0, 0.0, -9.81))
        .dt(DT)
        .add_revolute_body("flywheel", -1, SpatialTransform::identity(), si)
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

/// The full chain evaluated primally at one lattice constant: build the
/// crystal, homogenize the density, integrate the part, simulate.
fn j_at(a: f64) -> f64 {
    let rho = rho_of_a(a);
    let props = nominal_mass_props(&[flywheel_body(rho)], &[]).expect("mass props");
    flywheel_rollout(&props)
}

#[test]
fn lattice_constant_to_rollout_gradient_matches_fd_and_closed_form() {
    // End-to-end gates, matching the M8 rollout-gradient tests.
    const GATE: f64 = 1e-4;
    /// Outer FD step in a (Å).
    const H_A: f64 = 1e-3;

    // Atomic half: ρ(a₀) and dρ/da by the atoms-crate FD oracle.
    let rho0 = rho_of_a(A0);
    let drho_da = fd_gradient(&|t: &[f64]| rho_of_a(t[0]), &[A0], 1e-5)[0];

    // Part half: nominal mass properties at ρ(a₀) through the real seam.
    let nominal = nominal_mass_props(&[flywheel_body(rho0)], &[]).expect("mass props");

    // The chained gradient.
    let (j, grad) = rollout_gradient_via_density(
        &nominal,
        &[rho0],
        &[vec![drho_da]],
        &flywheel_rollout,
        &MassPropFdSteps::default(),
    );
    assert_eq!(grad.len(), 1);
    let dj_da = grad[0];

    // Referee 1: brute-force central difference across the whole chain
    // (crystal rebuilt, density re-homogenized, part re-integrated,
    // trajectory re-simulated at a ± h).
    let fd = (j_at(A0 + H_A) - j_at(A0 - H_A)) / (2.0 * H_A);
    let rel_fd = (dj_da - fd).abs() / fd.abs().max(1.0);
    assert!(
        rel_fd <= GATE,
        "dJ/da: chained {dj_da} vs end-to-end fd {fd} (rel {rel_fd:.3e}); J = {j}"
    );

    // Referee 2: the closed form. With no state-dependent generalized
    // forces on the single revolute DOF (gravity is moment-free, no
    // Coriolis term for rotation about a fixed axis), qdd = τ/I_zz is a
    // constant, so semi-implicit Euler accumulates ω(T) = τT/I_zz exactly —
    // no O(dt) truncation enters J. With I_zz ∝ ρ ∝ a⁻³ that gives
    // dJ/da = 3J/a, independent of every FD step in the chain.
    let closed = 3.0 * j / A0;
    let rel_closed = (dj_da - closed).abs() / closed.abs();
    assert!(
        rel_closed <= GATE,
        "dJ/da: chained {dj_da} vs closed form {closed} (rel {rel_closed:.3e})"
    );

    // Sign sanity: a larger lattice constant means a less dense crystal, a
    // lighter flywheel, and a faster spin-up.
    assert!(
        dj_da > 0.0,
        "spin-up must accelerate as the lattice dilates"
    );

    // Determinism gate: the chain is a pure function of a.
    assert_eq!(j_at(A0).to_bits(), j_at(A0).to_bits());
}
