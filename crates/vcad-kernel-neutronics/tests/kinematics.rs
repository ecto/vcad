//! M1 integration: exact elastic kinematics vs the M0 multigroup model.
//!
//! The physical claim under test: the angle–energy correlation of real
//! elastic scattering (small-angle ⇔ small energy loss, and for hydrogen
//! no lab backscatter at all) pushes *more* dose through a thick
//! hydrogenous shield than isotropic-in-lab multigroup scattering does.
//! M1 quantifies the shift instead of hand-waving it; the band below was
//! measured, then pinned with margin.

use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::materials;
use vcad_kernel_neutronics::transport::{run, EnergyModel, RunConfig, Source};

/// Point source, air gap, shield, air out to a 1 m detector shell.
fn shield_config(model: EnergyModel, seed: u64) -> (RunConfig, usize) {
    let air = materials::air;
    let layers = vec![
        Layer::new(air(), 300.0),
        Layer::new(materials::hdpe(), 150.0),
        Layer::new(air(), 530.0),
        Layer::new(air(), 40.0), // detector 98–102 cm
        Layer::new(air(), 30.0),
    ];
    let mut c = RunConfig::new(
        Geometry::Sphere(layers),
        Source::IsotropicPoint,
        3_000,
        seed,
    );
    c.energy_model = model;
    (c, 3)
}

#[test]
fn exact_kinematics_raises_deep_penetration_dose() {
    let (c_mg, det) = shield_config(EnergyModel::Multigroup, 777);
    let (c_ek, _) = shield_config(EnergyModel::ExactKinematics, 777);
    let r_mg = run(&c_mg).unwrap();
    let r_ek = run(&c_ek).unwrap();
    let d_mg = r_mg.dose_per_source_psv[det];
    let d_ek = r_ek.dose_per_source_psv[det];
    assert!(d_mg.rse < 0.1 && d_ek.rse < 0.1);
    let ratio = d_ek.mean / d_mg.mean;
    assert!(
        (1.02..=3.0).contains(&ratio),
        "exact kinematics vs multigroup dose ratio through 15 cm HDPE: {ratio} \
         (forward-peaked H scatter must raise deep-penetration dose)"
    );
    // Provenance must say which physics produced each number.
    assert_eq!(r_mg.provenance.energy_model, "multigroup");
    assert_eq!(r_ek.provenance.energy_model, "exact-kinematics");
}

#[test]
fn exact_kinematics_balances_and_reproduces() {
    let (c, _) = shield_config(EnergyModel::ExactKinematics, 4242);
    let r1 = run(&c).unwrap();
    let r2 = run(&c).unwrap();
    assert_eq!(r1.truncated_histories, 0);
    assert!(r1.balance_max_dev < 1.0e-12);
    assert_eq!(
        r1.dose_per_source_psv[3].mean,
        r2.dose_per_source_psv[3].mean
    );
}

#[test]
fn collisions_to_thermal_approach_the_continuous_ladder() {
    // Continuous-energy slowing down in water: the hydrogen ladder says
    // ~ln(2.45 MeV / 0.5 eV) ≈ 15.4 collisions to *reach the thermal
    // boundary* in pure H (ξ = 1); oxygen's small ξ pushes the mixture
    // a little higher. The multigroup model can only descend 5 rungs —
    // exact kinematics must land near the physical count, and above the
    // group-hop floor.
    let g = Geometry::Sphere(vec![Layer::new(materials::water(), 400.0)]);
    let mut c = RunConfig::new(g, Source::IsotropicPoint, 2_000, 1618);
    c.energy_model = EnergyModel::ExactKinematics;
    let r = run(&c).unwrap();
    let th = r.thermalization.unwrap();
    assert!(
        th.mean_collisions.mean > 12.0 && th.mean_collisions.mean < 30.0,
        "collisions to thermal (exact kinematics, water) = {} — expect ≈ 16–20",
        th.mean_collisions
    );
}
