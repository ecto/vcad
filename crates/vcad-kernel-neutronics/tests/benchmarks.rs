//! M5: benchmarks against *published* reactor-physics values.
//!
//! These are the numbers a health-physics text would hold us to:
//!
//! 1. **Thermal diffusion length of water**: L ≈ 2.85 cm, D ≈ 0.16 cm
//!    (Lamarsh, Introduction to Nuclear Engineering, Table 5-2). A
//!    thermal point source in a big water sphere must produce a thermal
//!    flux falling as e^{−r/L}/r; we fit L from the MC slope.
//! 2. **Fermi age of water**: τ(fission → thermal) ≈ 27 cm² (Lamarsh
//!    Table 5-3; 2.45 MeV is fission-adjacent). The MC observable is
//!    ⟨r²⟩ at first thermal entry = 6τ for a point source.
//! 3. **Slowing-down ladder**: mean collisions to thermalize =
//!    ln(E₀/E_th)/ξ̄ with ξ̄ the scatter-weighted mean lethargy gain —
//!    computed here from the same library constants the transport uses,
//!    so the test cross-checks the MC against moderation theory rather
//!    than against a hand-picked constant.
//!
//! Acceptance bands are wide (±25–40%) and stated: a 5-group
//! design-estimate library with free-gas thermal motion neglected is
//! *supposed* to sit near these values, not on them. What the bands
//! exclude is category error — a broken estimator, a wrong unit, a
//! transfer matrix bug — which is what benchmarks are for.

use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::groups::{GROUP_BOUNDS_EV, THERMAL_GROUP};
use vcad_kernel_neutronics::materials;
use vcad_kernel_neutronics::transport::{run, RunConfig, Source};

#[test]
fn water_thermal_diffusion_length_vs_lamarsh() {
    // Thermal point source; tally shells centered at 6, 9, 12 cm.
    let w = materials::water;
    let g = Geometry::Sphere(vec![
        Layer::new(w(), 55.0),
        Layer::new(w(), 10.0), // 5.5–6.5 cm
        Layer::new(w(), 20.0),
        Layer::new(w(), 10.0), // 8.5–9.5 cm
        Layer::new(w(), 20.0),
        Layer::new(w(), 10.0), // 11.5–12.5 cm
        Layer::new(w(), 175.0),
    ]);
    let mut c = RunConfig::new(g, Source::IsotropicPoint, 1_500, 285285);
    c.source_group = THERMAL_GROUP;
    let r = run(&c).unwrap();
    assert_eq!(r.truncated_histories, 0);
    let phi = |region: usize| r.flux_per_source[region][THERMAL_GROUP];
    let (p6, p12) = (phi(1), phi(5));
    assert!(
        p6.rse < 0.05 && p12.rse < 0.10,
        "rse {} {}",
        p6.rse,
        p12.rse
    );
    // φ ∝ e^{−r/L}/r ⇒ L = Δr / ln[(r₁φ₁)/(r₂φ₂)].
    let l_measured = (12.0 - 6.0) / ((6.0 * p6.mean) / (12.0 * p12.mean)).ln();
    let published = 2.85;
    println!("water thermal L: MC {l_measured:.2} cm, published {published} cm");
    assert!(
        (l_measured / published - 1.0).abs() < 0.25,
        "water thermal L = {l_measured:.2} cm vs published {published} cm \
         (Lamarsh Table 5-2)"
    );
}

#[test]
fn water_fermi_age_vs_lamarsh() {
    let g = Geometry::Sphere(vec![Layer::new(materials::water(), 400.0)]);
    let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 272727);
    let r = run(&c).unwrap();
    let th = r.thermalization.unwrap();
    assert!(th.fraction.mean > 0.6);
    let tau_measured = th.mean_r2_cm2.mean / 6.0;
    let published = 27.0;
    println!("water Fermi age: MC {tau_measured:.1} cm², published ≈ {published} cm²");
    assert!(
        (tau_measured / published - 1.0).abs() < 0.40,
        "Fermi age τ(water) = {tau_measured:.1} cm² vs published ≈ {published} cm² \
         (Lamarsh Table 5-3; ±40% band for a 5-group design-estimate library)"
    );
}

#[test]
fn collisions_to_thermal_vs_moderation_theory() {
    // Predict from the library itself: ξ per element, weighted by its
    // share of Σ_s (using the fast-region shares where most collisions
    // happen; the weighting varies a few % across groups).
    let w = materials::water();
    let xi = |a: f64| {
        let alpha = ((a - 1.0) / (a + 1.0)).powi(2);
        if alpha == 0.0 {
            1.0
        } else {
            1.0 + alpha * alpha.ln() / (1.0 - alpha)
        }
    };
    // Share-weighted ξ̄ in the top group (representative).
    let xbar: f64 = w
        .scatterers
        .iter()
        .map(|s| s.share[0] * xi(s.mass_amu))
        .sum();
    let e0 = 2.45e6;
    let e_th = GROUP_BOUNDS_EV[THERMAL_GROUP]; // thermal boundary, 0.5 eV
    let predicted = (e0 / e_th).ln() / xbar;

    let g = Geometry::Sphere(vec![Layer::new(materials::water(), 400.0)]);
    let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 161616);
    let r = run(&c).unwrap();
    let measured = r.thermalization.unwrap().mean_collisions.mean;
    println!("collisions to thermal: MC {measured:.1}, theory {predicted:.1} (ξ̄ = {xbar:.3})");
    assert!(
        (measured / predicted - 1.0).abs() < 0.30,
        "collisions to thermal: MC {measured:.1} vs moderation theory \
         ln(E₀/E_th)/ξ̄ = {predicted:.1} (ξ̄ = {xbar:.3})"
    );
}
