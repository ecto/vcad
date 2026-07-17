//! The M0 validation ladder: every rung is an analytic or structural
//! truth the transport must reproduce, with statistical acceptance bands
//! wide enough to be deterministic-seed stable and tight enough to catch
//! physics bugs.
//!
//! 1. Uncollided point-source flux in a pure absorber — exact:
//!    φ(r) = S·e^{−Σt·r}/(4πr²)  (volume-averaged over the tally shell).
//! 2. Uncollided slab transmission — exact: T = e^{−Σt·x}.
//! 3. Batch-statistics honesty: RSE scales as 1/√N.
//! 4. Scattering buildup: total flux in a moderator exceeds the
//!    uncollided prediction (quantified band).
//! 5. Dose falls monotonically with HDPE shield thickness (and by a
//!    quantified factor at 20 cm).
//! 6. Slowing-down observables in water are physically sized (the
//!    quantitative Fermi-age comparison is the M5 benchmark).
//! 7. Boron does its job: borated poly absorbs more and leaks less
//!    thermal flux than plain poly.

use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::groups::{N_GROUPS, SOURCE_GROUP, THERMAL_GROUP};
use vcad_kernel_neutronics::materials::{self, Material};
use vcad_kernel_neutronics::transport::{run, RunConfig, Source};

/// Exact volume-averaged uncollided flux of a unit point source over a
/// spherical shell [r0, r1] in a uniform total cross section Σt:
/// (1/V)∫ e^{−Σt r}/(4πr²) dV = (e^{−Σt r0} − e^{−Σt r1})/(Σt V).
fn uncollided_shell_avg(sigma_t: f64, r0: f64, r1: f64) -> f64 {
    let v = 4.0 / 3.0 * std::f64::consts::PI * (r1.powi(3) - r0.powi(3));
    ((-sigma_t * r0).exp() - (-sigma_t * r1).exp()) / (sigma_t * v)
}

#[test]
fn rung1_uncollided_point_source_flux_is_exact() {
    // Pure absorber, Σt = 0.2/cm; tally shell 4.9–5.1 cm (1 mfp deep).
    let m = Material::pure_absorber(0.2);
    let g = Geometry::Sphere(vec![
        Layer::new(m.clone(), 49.0),
        Layer::new(m.clone(), 2.0),
        Layer::new(m, 29.0),
    ]);
    let c = RunConfig::new(g, Source::IsotropicPoint, 2_500, 20260717);
    let r = run(&c).unwrap();
    assert_eq!(r.truncated_histories, 0);
    let exact = uncollided_shell_avg(0.2, 4.9, 5.1);
    let mc = r.flux_per_source[1][SOURCE_GROUP];
    assert!(
        mc.rse < 0.02,
        "need meaningful statistics for the comparison (rse {})",
        mc.rse
    );
    assert!(
        mc.consistent_with(exact, 4.0),
        "MC {mc} vs exact {exact:.4e}"
    );
    assert!(
        (mc.mean / exact - 1.0).abs() < 0.05,
        "MC/exact = {}",
        mc.mean / exact
    );
    // A pure absorber has no collided flux: groups below the source
    // group scored nothing (mean 0, RSE ∞ — the fail-closed zero).
    for g2 in SOURCE_GROUP + 1..N_GROUPS {
        assert_eq!(r.flux_per_source[1][g2].mean, 0.0);
        assert!(r.flux_per_source[1][g2].rse.is_infinite());
    }
}

#[test]
fn rung2_uncollided_slab_transmission_is_exact() {
    // Σt = 0.15/cm, 20 cm slab → 3 mfp: T = e^{−3} = 0.049787.
    let m = Material::pure_absorber(0.15);
    let g = Geometry::Slab(vec![Layer::new(m, 200.0)]);
    let c = RunConfig::new(g, Source::BeamPlusX, 2_500, 31415);
    let r = run(&c).unwrap();
    let exact = (-3.0f64).exp();
    let t = r.leaked_out;
    assert!(t.rse < 0.05, "rse {}", t.rse);
    assert!(t.consistent_with(exact, 4.0), "T {t} vs exact {exact:.4e}");
    // No back-leak for a normal beam into a pure absorber, and the
    // group-wise leakage spectrum equals the total (all source group).
    assert_eq!(r.leaked_back.mean, 0.0);
    let last = r.net_outward_current.last().unwrap();
    assert!((last[SOURCE_GROUP].mean - t.mean).abs() < 1.0e-12);
}

#[test]
fn rung3_rse_scales_as_inverse_sqrt_n() {
    let build = |hpb: usize| {
        let m = Material::pure_absorber(0.15);
        let g = Geometry::Slab(vec![Layer::new(m, 200.0)]);
        RunConfig {
            batches: 16,
            ..RunConfig::new(g, Source::BeamPlusX, hpb, 271828)
        }
    };
    let r1 = run(&build(1_000)).unwrap();
    let r4 = run(&build(4_000)).unwrap();
    let ratio = r1.leaked_out.rse / r4.leaked_out.rse;
    // 4× the histories ⇒ 2× smaller RSE; the band is generous because
    // the RSE of an RSE is itself noisy at 16 batches — but a broken
    // batch reduction (e.g. correlated streams) lands far outside it.
    assert!(
        (1.4..=2.9).contains(&ratio),
        "rse ratio {ratio} (expect ≈ 2)"
    );
}

#[test]
fn rung4_moderator_buildup_exceeds_uncollided() {
    // Water sphere, tally shell at 15 cm (3.3 mfp at 2.45 MeV): the
    // total flux must exceed the uncollided line — scattered neutrons
    // arrive on top of the survivors.
    let w = materials::water();
    let sigma_fast = w.sigma_t[SOURCE_GROUP];
    let g = Geometry::Sphere(vec![
        Layer::new(w.clone(), 149.0),
        Layer::new(w.clone(), 2.0),
        Layer::new(w, 99.0),
    ]);
    let c = RunConfig::new(g, Source::IsotropicPoint, 2_500, 424242);
    let r = run(&c).unwrap();
    assert_eq!(r.truncated_histories, 0);
    let uncollided = uncollided_shell_avg(sigma_fast, 14.9, 15.1);
    let total = r.total_flux_mean_per_source(1);
    let buildup = total / uncollided;
    assert!(
        buildup > 1.5,
        "buildup {buildup} — scattering must add flux over the uncollided line"
    );
    assert!(buildup < 100.0, "buildup {buildup} implausibly large");
    // And the uncollided component itself is still present in the
    // source group: source-group flux ≥ the uncollided line.
    assert!(r.flux_per_source[1][SOURCE_GROUP].mean >= uncollided * 0.8);
}

/// Point source, air gap, HDPE shell of thickness `t_mm`, air out to a
/// detector shell at 1 m.
fn shield_geometry(t_mm: f64, shield: Material) -> (Geometry, usize) {
    let air = materials::air;
    let mut layers = vec![Layer::new(air(), 300.0)];
    if t_mm > 0.0 {
        layers.push(Layer::new(shield, t_mm));
    }
    layers.push(Layer::new(air(), 980.0 - 300.0 - t_mm));
    layers.push(Layer::new(air(), 40.0)); // detector shell 98–102 cm
    let det = layers.len() - 1;
    layers.push(Layer::new(air(), 30.0));
    (Geometry::Sphere(layers), det)
}

#[test]
fn rung5_dose_monotone_in_hdpe_thickness() {
    let mut doses = Vec::new();
    for t_mm in [0.0, 50.0, 100.0, 200.0] {
        let (g, det) = shield_geometry(t_mm, materials::hdpe());
        let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 8675309);
        let r = run(&c).unwrap();
        assert_eq!(r.truncated_histories, 0);
        let d = r.dose_per_source_psv[det];
        assert!(
            d.rse < 0.1,
            "detector dose needs real statistics (rse {})",
            d.rse
        );
        doses.push(d);
    }
    for i in 1..doses.len() {
        assert!(
            doses[i].mean < doses[i - 1].mean * 0.9,
            "dose must fall with thickness: {} → {}",
            doses[i - 1],
            doses[i]
        );
    }
    let reduction = doses[3].mean / doses[0].mean;
    assert!(
        reduction < 0.15,
        "20 cm of HDPE must buy at least ~7× dose reduction (got 1/{:.1})",
        1.0 / reduction
    );
}

#[test]
fn rung6_slowing_down_observables_physically_sized() {
    // Big water sphere: most histories thermalize; ⟨r²⟩ at first
    // thermal entry is the Fermi-age observable (6τ ≈ 160 cm² for
    // fission-like sources in water; group coarseness shifts it — the
    // honest quantitative comparison is the M5 benchmark).
    let g = Geometry::Sphere(vec![Layer::new(materials::water(), 400.0)]);
    let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 1618);
    let r = run(&c).unwrap();
    let th = r.thermalization.expect("sphere runs report slowing-down");
    assert!(
        th.fraction.mean > 0.6,
        "most fast neutrons thermalize in 40 cm of water (got {})",
        th.fraction
    );
    assert!(
        th.mean_collisions.mean > 5.0 && th.mean_collisions.mean < 40.0,
        "collisions to thermal {} (pure-H continuous-energy value ≈ 18)",
        th.mean_collisions
    );
    assert!(
        th.mean_r2_cm2.mean > 60.0 && th.mean_r2_cm2.mean < 350.0,
        "⟨r²⟩ at thermalization {} cm²",
        th.mean_r2_cm2
    );
}

#[test]
fn rung7_boron_absorbs_the_thermal_column() {
    let run_shield = |shield: Material| {
        let (g, det) = shield_geometry(150.0, shield);
        let c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 5551212);
        (run(&c).unwrap(), det)
    };
    let (plain, det) = run_shield(materials::hdpe());
    let (borated, _) = run_shield(materials::borated_hdpe_5());
    // Boron raises total absorption…
    assert!(
        borated.absorbed.mean > plain.absorbed.mean,
        "borated {} vs plain {}",
        borated.absorbed,
        plain.absorbed
    );
    // …and specifically kills the thermal leak at the detector.
    let th_plain = plain.flux_per_source[det][THERMAL_GROUP];
    let th_bor = borated.flux_per_source[det][THERMAL_GROUP];
    assert!(
        th_bor.mean < th_plain.mean * 0.5,
        "thermal flux: borated {th_bor} vs plain {th_plain}"
    );
}

#[test]
fn isotropic_halfspace_slab_source_balances() {
    let g = Geometry::Slab(vec![Layer::new(materials::water(), 100.0)]);
    let c = RunConfig::new(g, Source::IsotropicHalfSpace, 2_000, 99);
    let r = run(&c).unwrap();
    assert_eq!(r.truncated_histories, 0);
    assert!(r.balance_max_dev < 1.0e-12);
    // A 10 cm water slab reflects a real fraction of an isotropic
    // fast-neutron load (albedo): back-leak must be substantial.
    assert!(
        r.leaked_back.mean > 0.1,
        "water albedo {} implausibly small",
        r.leaked_back
    );
}
