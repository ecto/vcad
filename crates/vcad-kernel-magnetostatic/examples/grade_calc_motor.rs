//! Grade `calc_motor`'s closed form against the 3D oracle.
//!
//! `vcad_ecad_sim::magnetics::motor_torque_constant` computes
//! `Kt = kw · N · p · B_gap · A_pole` with `A_pole = π(r_out² − r_in²)/(2p)`.
//! Its own docs call it first-order. This measures *how* first-order, and in
//! which direction, on a machine both models can describe.
//!
//! To isolate the torque formula from the flux model, the closed form is fed the
//! oracle's own airgap flux density. Any disagreement that remains is the
//! formula's geometry treatment, not a difference of opinion about `B_gap`.

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{Filament, IronStack, Machine, MagnetRing, Phase, Vec3};

// --- geometry: discrete circular coils on a pitch circle, six ferrite poles ---
const POLES: usize = 6;
const POLE_PAIRS: f64 = 3.0;
const PITCH_R: f64 = 0.0225;
const COIL_R_IN: f64 = 0.002;
const COIL_R_OUT: f64 = 0.0075;
const TURNS_PER_COIL: usize = 6;
const COILS_PER_PHASE: usize = 2;
const COIL_Z: f64 = 0.0008;
const MAGNET_D: f64 = 0.015;
const MAGNET_Z0: f64 = 0.0035;
const MAGNET_T: f64 = 0.003;
const REMANENCE: f64 = 0.385;
/// Stator annulus the coils sweep — what a user would hand `calc_motor`.
const STATOR_R_IN: f64 = PITCH_R - COIL_R_OUT;
const STATOR_R_OUT: f64 = PITCH_R + COIL_R_OUT;

fn coil(cx: f64, cy: f64, cur: f64) -> Vec<Filament> {
    (0..TURNS_PER_COIL)
        .map(|t| {
            let f = t as f64 / (TURNS_PER_COIL - 1) as f64;
            let r = COIL_R_IN + (COIL_R_OUT - COIL_R_IN) * f;
            let pts = (0..96)
                .map(|j| {
                    let a = 2.0 * PI * (j as f64) / 96.0;
                    Vec3::new(cx + r * a.cos(), cy + r * a.sin(), COIL_Z)
                })
                .collect();
            Filament::closed_loop(pts, cur, 100e-6)
        })
        .collect()
}

fn machine(iron: IronStack) -> Machine {
    let phases = (0..3)
        .map(|p| {
            let turns = (0..COILS_PER_PHASE)
                .flat_map(|k| {
                    let a =
                        (2.0 * PI / 3.0) * (p as f64) / POLE_PAIRS + PI * (k as f64) / POLE_PAIRS;
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    coil(PITCH_R * a.cos(), PITCH_R * a.sin(), sign)
                })
                .collect();
            Phase::new(["A", "B", "C"][p], turns)
        })
        .collect();
    Machine {
        phases,
        rotor: MagnetRing::discs(POLES, PITCH_R, MAGNET_D, MAGNET_Z0, MAGNET_T, REMANENCE, 64),
        iron,
        magnet_slices: 6,
    }
}

/// Effective `Kt` under ideal commutation at 1 A peak, N·m/A, plus the torque
/// ripple as a fraction of the mean.
///
/// The current is shaped like the machine's own back-EMF — `i_p ∝ Ke_p(θ)`,
/// normalized so the peak phase current is 1 A. That is what a field-oriented
/// drive does, and it is maximum-torque-per-amp for whatever `Ke` waveform the
/// geometry happens to produce.
///
/// Assuming a textbook `cos(pθ − 2πp/3)` instead is how this went wrong the first
/// time: guessing the electrical phase put the current in phase with `λ` rather
/// than with `dλ/dθ`, and the mean torque over a revolution cancelled to zero.
/// Taking the phase from the machine removes the guess. Torque is still
/// evaluated by the independent Lorentz route, so the reported number is not
/// merely `Ke` fed back to itself.
fn kt_commutated(m: &Machine, samples: usize) -> (f64, f64) {
    let thetas: Vec<f64> = (0..samples)
        .map(|k| 2.0 * PI * (k as f64) / (samples as f64))
        .collect();
    let ke: Vec<Vec<f64>> = thetas
        .iter()
        .map(|&th| (0..3).map(|p| m.ke_at(p, th)).collect())
        .collect();
    let ke_max = ke.iter().flatten().fold(0.0_f64, |acc, v| acc.max(v.abs()));
    assert!(ke_max > 0.0, "machine has no back-EMF");

    let t: Vec<f64> = thetas
        .iter()
        .zip(&ke)
        .map(|(&th, row)| {
            let cur: Vec<f64> = row.iter().map(|k| k / ke_max).collect();
            m.torque_lorentz(&cur, th)
        })
        .collect();
    let mean = t.iter().sum::<f64>() / t.len() as f64;
    let mx = t.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mn = t.iter().cloned().fold(f64::INFINITY, f64::min);
    (mean, (mx - mn) / mean.abs())
}

/// Mean axial flux density over a pole face at the coil plane — the `B_gap` the
/// closed form wants.
fn b_gap(m: &Machine) -> f64 {
    let src = m.rotor_sources(0.0);
    let (mut acc, mut n) = (0.0, 0);
    // Sample a disc of the pole's radius, centred on the pitch circle.
    for i in 0..24 {
        for j in 0..24 {
            let (u, v) = (
                -1.0 + 2.0 * (i as f64 + 0.5) / 24.0,
                -1.0 + 2.0 * (j as f64 + 0.5) / 24.0,
            );
            if u * u + v * v > 1.0 {
                continue;
            }
            let p = Vec3::new(PITCH_R + u * MAGNET_D * 0.5, v * MAGNET_D * 0.5, COIL_Z);
            acc += src.iter().map(|f| f.b_at(p)).sum::<Vec3>().z.abs();
            n += 1;
        }
    }
    acc / n as f64
}

/// `Kt = kw · N · p · B_gap · A_pole`, `A_pole = π(r_out² − r_in²)/(2p)`.
fn closed_form_kt(kw: f64, turns_per_phase: f64, b: f64) -> f64 {
    let annulus = PI * (STATOR_R_OUT * STATOR_R_OUT - STATOR_R_IN * STATOR_R_IN);
    let pole_area = annulus / (2.0 * POLE_PAIRS);
    kw * turns_per_phase * POLE_PAIRS * b * pole_area
}

fn main() {
    let turns_per_phase = (TURNS_PER_COIL * COILS_PER_PHASE) as f64;

    for (label, iron) in [
        ("no iron", IronStack::none()),
        ("rotor back-iron", IronStack::single(0.0)),
    ] {
        let m = machine(iron);
        let b = b_gap(&m);
        let (kt_oracle, ripple) = kt_commutated(&m, 72);
        let kt_closed = closed_form_kt(1.0, turns_per_phase, b);

        let annulus = PI * (STATOR_R_OUT * STATOR_R_OUT - STATOR_R_IN * STATOR_R_IN);
        let a_pole_formula = annulus / (2.0 * POLE_PAIRS);
        let a_coil = PI * COIL_R_OUT * COIL_R_OUT;

        println!("=== {label} ===");
        println!(
            "  B_gap (oracle, mean over pole face) : {:>10.2} mT",
            b * 1e3
        );
        println!(
            "  A_pole per closed form              : {:>10.3e} m²",
            a_pole_formula
        );
        println!(
            "  area a coil actually encloses       : {:>10.3e} m²",
            a_coil
        );
        println!(
            "  ratio (formula / actual)            : {:>10.2}x",
            a_pole_formula / a_coil
        );
        println!(
            "  Kt oracle (sinusoidal, 1 A peak)    : {:>10.3e} N·m/A",
            kt_oracle.abs()
        );
        println!(
            "  Kt closed form                      : {:>10.3e} N·m/A",
            kt_closed
        );
        println!(
            "  closed form / oracle                : {:>10.2}x",
            kt_closed / kt_oracle.abs()
        );
        println!(
            "  torque ripple (pk-pk / mean)        : {:>10.1}%",
            ripple * 100.0
        );
        println!();
    }
}
