//! M2 integration: the compass (adjoint diffusion) against the oracle
//! (Monte Carlo).
//!
//! Three-way triangle on the exactly solvable one-group problem
//! (MC ↔ diffusion ↔ closed form), then the design-compass contract on a
//! real shield: the *log-gradient* of dose w.r.t. shield thickness from
//! the adjoint must agree with an MC finite difference, while the
//! absolute diffusion dose is allowed its documented void bias — which
//! is also measured here so the docs can quote it instead of guessing.

use vcad_kernel_neutronics::diffusion::{companion_report, DiffusionOptions};
use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::materials::{self, Material};
use vcad_kernel_neutronics::transport::{run, EnergyModel, RunConfig, Source};

#[test]
fn one_group_triangle_mc_diffusion_analytic() {
    // Σs = 0.8, Σa = 0.05: D = 0.392 cm, L = 2.80 cm. Deep in a big
    // sphere, all three must agree: MC (truth), diffusion (compass),
    // e^{−r/L}/(4πDr) (textbook).
    let m = Material::one_group(0.8, 0.05);
    let geometry = Geometry::Sphere(vec![
        Layer::new(m.clone(), 99.0),
        Layer::new(m.clone(), 2.0), // tally shell at 9.9–10.1 cm
        Layer::new(m, 149.0),
    ]);
    let mut c = RunConfig::new(geometry.clone(), Source::IsotropicPoint, 4_000, 314159);
    c.energy_model = EnergyModel::Multigroup; // isotropic, matches both partners
    let mc = run(&c).unwrap();
    let mc_flux = mc.flux_per_source[1][0];
    assert!(mc_flux.rse < 0.03);

    let d = 1.0 / (3.0 * 0.85);
    let l = (d / 0.05f64).sqrt();
    let r = 10.0;
    let analytic = (-r / l).exp() / (4.0 * std::f64::consts::PI * d * r);

    let rep = companion_report(&geometry, 0, 100.0, &DiffusionOptions::default()).unwrap();
    // companion_report prices dose; for the flux triangle use the model
    // directly.
    let model = vcad_kernel_neutronics::diffusion::DiffusionModel::new(
        &geometry,
        &DiffusionOptions::default(),
    )
    .unwrap();
    let fwd = model.forward(0).unwrap();
    let diff_flux = fwd.values[model.cell_at_mm(100.0).unwrap()][0];

    let mc_vs_analytic = mc_flux.mean / analytic;
    let diff_vs_analytic = diff_flux / analytic;
    assert!(
        (mc_vs_analytic - 1.0).abs() < 0.10,
        "MC/analytic = {mc_vs_analytic} (transport vs diffusion theory at 3.6 L: \
         a few % is physics, 10% is a bug)"
    );
    assert!(
        (diff_vs_analytic - 1.0).abs() < 0.03,
        "diffusion/analytic = {diff_vs_analytic}"
    );
    assert!(rep.duality_gap < 1.0e-10);
}

/// The Phase-B-shaped geometry: air chamber, HDPE shield, air out to a
/// detector shell at 1 m.
fn shield_geometry(t_mm: f64) -> Geometry {
    Geometry::Sphere(vec![
        Layer::new(materials::air(), 300.0),
        Layer::new(materials::hdpe(), t_mm),
        Layer::new(materials::air(), 680.0 - t_mm),
        Layer::new(materials::air(), 40.0), // detector 98–102 cm
        Layer::new(materials::air(), 30.0),
    ])
}

#[test]
fn compass_log_gradient_agrees_with_mc_finite_difference() {
    // Adjoint log-gradient at t = 120 mm.
    let opts = DiffusionOptions::default();
    let rep = companion_report(&shield_geometry(120.0), 0, 1000.0, &opts).unwrap();
    let grad = rep.d_dose_d_thickness_mm[1].expect("shield layer has a neighbor");
    let dlog_diffusion = grad / rep.dose_psv_per_source; // 1/mm

    // MC finite difference across ±20 mm (big enough that the dose
    // change dwarfs the MC error bars).
    let mc_dose = |t_mm: f64, seed: u64| {
        let c = RunConfig::new(shield_geometry(t_mm), Source::IsotropicPoint, 20_000, seed);
        let r = run(&c).unwrap();
        r.dose_per_source_psv[3]
    };
    let up = mc_dose(140.0, 60717);
    let dn = mc_dose(100.0, 60718);
    assert!(up.rse < 0.03 && dn.rse < 0.03);
    let dlog_mc = (up.mean / dn.mean).ln() / 40.0; // 1/mm

    // The compass must point the right way with a usable magnitude:
    // agreement to ~25% is success for a diffusion gradient priced
    // against exact-kinematics MC through a hydrogenous shield.
    assert!(dlog_diffusion < 0.0 && dlog_mc < 0.0);
    let ratio = dlog_diffusion / dlog_mc;
    assert!(
        (0.75..=1.35).contains(&ratio),
        "compass dlog {dlog_diffusion}/mm vs oracle dlog {dlog_mc}/mm — ratio {ratio}"
    );
}

#[test]
fn diffusion_absolute_void_bias_is_measured_not_hidden() {
    // Absolute diffusion dose at an in-air detector is expected high
    // by roughly (R_out/r_det)² (flux flattening across the void) —
    // measure the bias so the docs can quote it. With R_out = 1.05 m and
    // r_det = 1.0 m the geometric part is small; the residual is
    // diffusion-vs-transport physics in the shield. Band kept wide and
    // honest: the point is that the oracle owns absolute doses.
    let g = shield_geometry(120.0);
    let rep = companion_report(&g, 0, 1000.0, &DiffusionOptions::default()).unwrap();
    let c = RunConfig::new(g, Source::IsotropicPoint, 20_000, 91);
    let mc = run(&c).unwrap();
    let ratio = rep.dose_psv_per_source / mc.dose_per_source_psv[3].mean;
    assert!(
        (0.5..=30.0).contains(&ratio),
        "diffusion/MC absolute dose ratio {ratio} — outside even the honest band"
    );
    println!("diffusion/MC absolute dose ratio at 1 m through 12 cm HDPE: {ratio:.2}");
}
