//! Magnetic confinement of a species in the device's *own* coil field —
//! the Orbitron/polywell question the neutralization dead-end pointed to.
//!
//! The e-injection sweep found naive electron injection recovers nothing:
//! the −V ion well is a potential *maximum* for electrons, so they fall
//! out. But the shield rings also make a magnetic cusp, and near the wires
//! electrons are magnetized. The live question for the whole Q-lane:
//! **does that cusp trap electrons well enough to matter?**
//!
//! This module answers it the honest way — trace an electron ensemble in
//! the full field (E + coil B), trace the *identical* launch with the coil
//! currents zeroed (the ballistic reference), and report the confinement-
//! time enhancement. Enhancement ≈ 1 means the geometry leaks electrons
//! out its cusps (a two-ring cusp is loss-dominated — the textbook reason
//! polywells use six coils); enhancement ≫ 1 means magnetic trapping is
//! real and neutralization becomes an architecture worth pricing.
//!
//! Caveats carried on every result: the Boris pusher is non-relativistic,
//! so absolute confinement times for ≳50 keV electrons carry a ~10–15%
//! velocity error — but the B-on/B-off *ratio* largely cancels it, and the
//! qualitative trapping verdict is robust. Single particles, no electron
//! self-fields, no collisions (M0 scope).

use crate::device::{Device, WireRing};
use crate::field::FieldMap;
use crate::poisson::{solve, SolveError, SolveOptions};
use crate::trace::{Fate, Species, TraceOptions, Tracer};

/// Confinement statistics for one species in one device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfinementReport {
    /// Ensemble size.
    pub n: usize,
    /// Mean flight time before loss, full field (E + coil B), s.
    pub mean_time_s: f64,
    /// Mean flight time with the coil currents zeroed (ballistic), s.
    pub ballistic_time_s: f64,
    /// `mean_time_s / ballistic_time_s` — the magnetic confinement gain.
    /// ≈ 1: cusp-loss-dominated; ≫ 1: real trapping.
    pub enhancement: f64,
    /// Fraction lost to the chamber wall / end caps (full field).
    pub wall_fraction: f64,
    /// Fraction lost to a wire ring (full field).
    pub wire_fraction: f64,
    /// Fraction still confined at the time budget (full field) — the
    /// trapped population.
    pub survivor_fraction: f64,
    /// Representative gyroradius at the full well drop and the near-ring
    /// field, mm — the magnetization scale vs the device size.
    pub gyroradius_mm: f64,
}

/// A copy of `device` with every ring current set to zero (same
/// electrostatics, no magnetic field) — the ballistic reference.
fn demagnetized(device: &Device) -> Device {
    Device {
        rings: device
            .rings
            .iter()
            .map(|r| WireRing {
                ampere_turns: 0.0,
                ..*r
            })
            .collect(),
        ..device.clone()
    }
}

fn mean_time(outcomes: &[crate::trace::TraceOutcome]) -> f64 {
    if outcomes.is_empty() {
        return 0.0;
    }
    outcomes.iter().map(|o| o.time_s).sum::<f64>() / outcomes.len() as f64
}

/// Trace an electron (or any species) ensemble in `device`'s full field
/// and in its demagnetized copy, and report the confinement enhancement.
///
/// `topts` should give a generous time budget (large `time_budget_factor`)
/// so trapped particles accumulate flight time rather than censoring
/// immediately; the same options are used for both traces, so the
/// enhancement ratio is fair regardless of the absolute budget.
pub fn confinement(
    device: &Device,
    species: Species,
    nr: usize,
    nz: usize,
    sopts: &SolveOptions,
    topts: &TraceOptions,
    particles: usize,
) -> Result<ConfinementReport, SolveError> {
    // Electrostatics is identical for both traces (currents don't enter
    // Poisson) — solve once and reuse.
    let sol = solve(device, nr, nz, sopts)?;

    let fields_b = FieldMap::new(device, &sol);
    let tracer_b = Tracer::new(device, &fields_b, &sol, *topts);
    let with_b = tracer_b.launch_ensemble(species, particles);

    let ballistic_device = demagnetized(device);
    let fields_0 = FieldMap::new(&ballistic_device, &sol);
    let tracer_0 = Tracer::new(&ballistic_device, &fields_0, &sol, *topts);
    let ballistic = tracer_0.launch_ensemble(species, particles);

    let mean_time_s = mean_time(&with_b);
    let ballistic_time_s = mean_time(&ballistic);
    let n = with_b.len();
    let frac = |f: &dyn Fn(&crate::trace::TraceOutcome) -> bool| {
        with_b.iter().filter(|o| f(o)).count() as f64 / n.max(1) as f64
    };

    // Representative gyroradius: an electron that has fallen through the
    // full well drop, in the field a quarter-wire-radius outside the ring
    // (a strong-field probe), r_g = m·v/(e·B).
    let drop_v = device.max_potential_drop_v().max(1.0);
    let v = (2.0 * species.charge_c.abs() * drop_v / species.mass_kg).sqrt();
    let b_probe = device
        .rings
        .iter()
        .filter(|r| r.ampere_turns != 0.0)
        .map(|r| {
            let coil = crate::field::RingCoil {
                radius_m: r.ring_radius_mm * 1e-3,
                z_m: r.z_mm * 1e-3,
                ampere_turns: r.ampere_turns,
                wire_radius_m: (r.wire_radius_mm * 1e-3).max(1e-6),
            };
            let (br, bz) = crate::field::b_ring(
                &coil,
                (r.ring_radius_mm + 1.5 * r.wire_radius_mm) * 1e-3,
                r.z_mm * 1e-3,
            );
            (br * br + bz * bz).sqrt()
        })
        .fold(0.0_f64, f64::max);
    let gyroradius_mm = if b_probe > 0.0 {
        species.mass_kg * v / (species.charge_c.abs() * b_probe) * 1e3
    } else {
        f64::INFINITY
    };

    Ok(ConfinementReport {
        n,
        mean_time_s,
        ballistic_time_s,
        enhancement: mean_time_s / ballistic_time_s.max(1e-30),
        wall_fraction: frac(&|o| o.fate == Fate::Wall),
        wire_fraction: frac(&|o| matches!(o.fate, Fate::Wire(_))),
        survivor_fraction: frac(&|o| o.fate == Fate::Survived),
        gyroradius_mm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{DEUTERON, ELECTRON};

    // A modest flight budget: trapped electrons censor at ~tens of transit
    // times (plenty to distinguish from single-transit ballistic loss),
    // and the coarse grid keeps the electron step count small. Enhancement
    // saturates at ~budget/transit, which is fine for the trapping verdict.
    fn quick_budget() -> TraceOptions {
        TraceOptions {
            max_passes: 4,
            time_budget_factor: 15.0,
            launch_shell_fraction: 0.5,
            step_fraction: 0.35,
            ..TraceOptions::default()
        }
    }

    #[test]
    fn magnetic_field_never_hurts_electron_confinement() {
        // A strong cusp can only trap or be neutral — never expel faster
        // than the bare electrostatic fall.
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 200_000.0);
        let r = confinement(
            &device,
            ELECTRON,
            41,
            81,
            &SolveOptions::default(),
            &quick_budget(),
            16,
        )
        .unwrap();
        assert!(
            r.enhancement >= 0.9,
            "B must not expel electrons faster than ballistic: {:.3}",
            r.enhancement
        );
        assert!(r.ballistic_time_s > 0.0 && r.mean_time_s > 0.0);
    }

    #[test]
    fn stronger_shield_traps_electrons_better() {
        // Enhancement must rise with ampere-turns: more field, more
        // magnetization, longer confinement (monotone in the mean).
        let run = |at: f64| {
            confinement(
                &Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, at),
                ELECTRON,
                41,
                81,
                &SolveOptions::default(),
                &quick_budget(),
                16,
            )
            .unwrap()
            .enhancement
        };
        let weak = run(40_000.0);
        let strong = run(400_000.0);
        assert!(
            strong > weak,
            "stronger shield must confine electrons better: {weak:.2} -> {strong:.2}"
        );
        // And the strong cusp must actually do something.
        assert!(strong > 1.2, "strong shield shows no trapping: {strong:.2}");
    }

    #[test]
    fn gyroradius_shrinks_with_field() {
        let weak = confinement(
            &Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 40_000.0),
            ELECTRON,
            41,
            81,
            &SolveOptions::default(),
            &quick_budget(),
            8,
        )
        .unwrap();
        let strong = confinement(
            &Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 400_000.0),
            ELECTRON,
            41,
            81,
            &SolveOptions::default(),
            &quick_budget(),
            8,
        )
        .unwrap();
        assert!(strong.gyroradius_mm < weak.gyroradius_mm);
        assert!(strong.gyroradius_mm.is_finite());
    }

    #[test]
    fn ions_are_electrostatically_confined_without_help() {
        // Control: ions confine in the well with B off already, unlike
        // electrons — the asymmetry that defines the problem. Their
        // ballistic (B-off) confinement is already substantial.
        let device = Device::shielded_two_ring(120.0, 40.0, 22.0, 4.0, -20_000.0, 40_000.0);
        let r = confinement(
            &device,
            DEUTERON,
            41,
            81,
            &SolveOptions::default(),
            &quick_budget(),
            12,
        )
        .unwrap();
        assert_eq!(r.n, 12);
        assert!(
            r.ballistic_time_s > 0.0,
            "ions must recirculate in the bare well"
        );
    }
}
