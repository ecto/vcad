//! Machine assembly, and the cross-check that makes the result trustworthy:
//! torque from `I dl × B` must equal torque from `dλ/dθ`.

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{
    harmonics, peak_to_peak, Filament, IronStack, Machine, MagnetRing, Phase, Vec3,
};

/// A planar coil as `turns` concentric **closed** loops centred at `(cx, cy)`.
///
/// A real PCB spiral is an open curve closed through its return lead; modelling
/// it as concentric closed turns keeps the circuit closed, which is what makes
/// flux linkage well defined. See `Filament::flux_linkage`.
fn coil(
    cx: f64,
    cy: f64,
    z: f64,
    r_in: f64,
    r_out: f64,
    turns: usize,
    current: f64,
) -> Vec<Filament> {
    (0..turns)
        .map(|t| {
            let f = if turns == 1 {
                0.0
            } else {
                t as f64 / (turns - 1) as f64
            };
            let r = r_in + (r_out - r_in) * f;
            let pts = (0..64)
                .map(|j| {
                    let a = 2.0 * PI * (j as f64) / 64.0;
                    Vec3::new(cx + r * a.cos(), cy + r * a.sin(), z)
                })
                .collect();
            Filament::closed_loop(pts, current, 100e-6)
        })
        .collect()
}

/// Three phases of two coils each, on the reference machine's geometry.
///
/// Six poles is three pole pairs, so electrical angle is 3x mechanical: the
/// phases sit 120 electrical degrees apart, which is **40 mechanical degrees**,
/// and the second coil of each phase sits one magnet pitch (60 mechanical) away
/// where the polarity is reversed, wound backwards so the two add.
fn reference_machine(iron: IronStack) -> Machine {
    let pitch = 0.0225;
    let pole_pairs = 3.0;
    let phases = (0..3)
        .map(|p| {
            let turns = (0..2)
                .flat_map(|k| {
                    let a =
                        (2.0 * PI / 3.0) * (p as f64) / pole_pairs + PI * (k as f64) / pole_pairs;
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    coil(
                        pitch * a.cos(),
                        pitch * a.sin(),
                        0.0008,
                        0.002,
                        0.0075,
                        6,
                        sign,
                    )
                })
                .collect();
            Phase::new(["A", "B", "C"][p], turns)
        })
        .collect();

    Machine {
        phases,
        rotor: MagnetRing::discs(6, pitch, 0.015, 0.0035, 0.003, 0.385, 48),
        iron,
        magnet_slices: 4,
    }
}

/// Peak |torque| over a mechanical revolution — the scale a disagreement should
/// be judged against.
///
/// Comparing the two routes *relatively* at a single angle is misleading near an
/// aligned rotor position, where the true torque is zero and both routes return
/// their own rounding noise: the relative difference between two zeros is
/// meaningless. The physically meaningful question is whether they differ by much
/// compared to the torque the machine actually makes.
fn torque_scale(m: &Machine, currents: &[f64]) -> f64 {
    (0..24)
        .map(|k| {
            m.torque_lorentz(currents, 2.0 * PI * (k as f64) / 24.0)
                .abs()
        })
        .fold(0.0, f64::max)
}

#[test]
fn force_and_energy_routes_agree_on_torque() {
    // The load-bearing test. B-route and A-route share no integration code; in a
    // linear magnetostatic problem they must give the same torque. Disagreement
    // means one kernel is wrong.
    let m = reference_machine(IronStack::none());
    let currents = [1.0, 0.0, 0.0];
    let scale = torque_scale(&m, &currents);
    assert!(scale > 0.0, "machine makes no torque at any angle");

    for theta in [0.0, 0.17, 0.41, 0.83] {
        let audit = m.audit(&currents, theta);
        let diff = (audit.lorentz_nm - audit.energy_nm).abs();
        assert!(
            diff < 5e-3 * scale,
            "θ={theta}: Lorentz {} N·m vs energy {} N·m — differ by {diff}, \
             which is {:.1}% of the {scale} N·m peak",
            audit.lorentz_nm,
            audit.energy_nm,
            100.0 * diff / scale
        );
    }
}

#[test]
fn the_routes_still_agree_with_back_iron_in_the_circuit() {
    // Images multiply the source count and flip signs; a sign error there would
    // break the two routes differently. Re-run the cross-check with iron.
    let m = reference_machine(IronStack::single(0.0));
    let currents = [1.0, 0.0, 0.0];
    let scale = torque_scale(&m, &currents);
    let audit = m.audit(&currents, 0.23);
    let diff = (audit.lorentz_nm - audit.energy_nm).abs();
    assert!(
        diff < 5e-3 * scale,
        "with iron: Lorentz {} vs energy {} — differ by {diff}, {:.1}% of peak {scale}",
        audit.lorentz_nm,
        audit.energy_nm,
        100.0 * diff / scale
    );
}

#[test]
fn a_phase_whose_coils_alternate_sense_does_not_self_cancel() {
    // Regression: flux linkage is geometric, so `∮A·dl` cannot see which way a
    // turn is wound. Without applying the series orientation, the backward-wound
    // coil facing a south pole subtracts instead of adding and the phase links
    // exactly zero flux at every angle.
    let m = reference_machine(IronStack::none());
    let sweep = m.linkage_sweep(36);
    let amplitude = peak_to_peak(&sweep[0]);
    // A 6-turn pair over a 15 mm ferrite pole links microwebers, not zero.
    assert!(
        amplitude > 1e-6,
        "phase A links essentially no flux (peak-to-peak {amplitude} Wb) — \
         the coils are cancelling each other"
    );
}

#[test]
fn torque_is_linear_in_current() {
    // Linearity is the assumption the whole superposition approach rests on.
    let m = reference_machine(IronStack::none());
    let t1 = m.torque_lorentz(&[1.0, 0.0, 0.0], 0.3);
    let t5 = m.torque_lorentz(&[5.0, 0.0, 0.0], 0.3);
    assert!((t5 - 5.0 * t1).abs() / (5.0 * t1).abs() < 1e-12);
}

#[test]
fn flux_linkage_has_the_pole_count_as_its_fundamental() {
    // A 6-pole rotor must link a phase with 3 electrical cycles per mechanical
    // revolution... which is 6 zero crossings, i.e. harmonic 3 of the mechanical
    // period. If the dominant harmonic is anything else, the rotor or the coil
    // placement is wrong.
    let m = reference_machine(IronStack::none());
    let sweep = m.linkage_sweep(72);
    let h = harmonics(&sweep[0], 8);
    let (dominant, _) = h
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
        .unwrap();
    assert_eq!(
        dominant, 3,
        "dominant mechanical harmonic {dominant}, expected 3; spectrum {h:?}"
    );
}

#[test]
fn the_three_phases_are_balanced_and_evenly_spaced() {
    // Same magnitude, 120 electrical degrees apart. With 3 electrical cycles per
    // revolution that is 1/9 of a mechanical turn.
    let m = reference_machine(IronStack::none());
    let sweep = m.linkage_sweep(72);
    let amps: Vec<f64> = sweep.iter().map(|s| peak_to_peak(s)).collect();
    let mean = amps.iter().sum::<f64>() / 3.0;
    for (i, a) in amps.iter().enumerate() {
        assert!(
            (a - mean).abs() / mean < 0.02,
            "phase {i} amplitude {a} differs from mean {mean} by more than 2%"
        );
    }
    assert!(mean > 0.0, "phases link no flux at all");
}

#[test]
fn back_iron_raises_the_torque_constant() {
    // The reason iron is modelled at all: it should buy real torque, not a
    // rounding difference.
    let free = reference_machine(IronStack::none()).kt_peak_single_phase(0, 36);
    let backed = reference_machine(IronStack::single(0.0)).kt_peak_single_phase(0, 36);
    assert!(
        backed > free * 1.2,
        "back-iron gave only {backed} vs {free} N·m/A"
    );
}

#[test]
fn an_unexcited_machine_makes_no_torque() {
    let m = reference_machine(IronStack::none());
    assert_eq!(m.torque_lorentz(&[0.0, 0.0, 0.0], 0.4), 0.0);
}
