//! Grade `calc_motor`'s closed form against the 3D oracle, on the geometry
//! `examples/pcb-motor` actually ships.
//!
//! `vcad_ecad_sim::magnetics::motor_torque_constant` computes
//! `Kt = kw · N · p · B_gap · A_pole` with `A_pole = π(r_out² − r_in²)/(2p)`.
//! Its own docs call it first-order. This measures *how* first-order, and in
//! which direction, for the board in `examples/pcb-motor`.
//!
//! To isolate the torque formula from the flux model, the closed form is fed the
//! oracle's own airgap flux density. Any disagreement that remains is the
//! formula's geometry treatment, not a difference of opinion about `B_gap`.
//!
//! # Geometry source
//!
//! Straight from `examples/pcb-motor/scripts/pipeline.py`'s `add_motor_winding`
//! call: 9 slots, 6 poles, 3 phases, pitch radius 22.5 mm, coil radii 2.6–7.2 mm,
//! 10 turns per coil, wye. Magnets are the BOM's Y30 ferrite D15×3 discs on the
//! same pitch circle, at the verified 1.000 mm air gap.
//!
//! # Winding layout
//!
//! Nine slots over six poles is 40° mechanical per slot, and with three pole
//! pairs that is **120° electrical** — exactly the three-phase spacing. So the
//! phases assign sequentially (slot `k` → phase `k mod 3`) with uniform winding
//! sense, and each phase's three coils sit 360° electrical apart, i.e. in phase.
//! That makes the distribution factor unity, which is why `kw = 1` is the fair
//! input to the closed form here.

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{Filament, IronStack, Machine, MagnetRing, Phase, Vec3};

// --- shipped pcb-motor geometry, metres ---
const SLOTS: usize = 9;
const POLES: usize = 6;
const POLE_PAIRS: f64 = 3.0;
const PITCH_R: f64 = 0.0225;
const COIL_R_IN: f64 = 0.0026;
const COIL_R_OUT: f64 = 0.0072;
const TURNS_PER_COIL: usize = 10;
/// Winding sits on FCu; take that plane as z = 0.
const COIL_Z: f64 = 0.0;
const AIR_GAP: f64 = 0.001;
const MAGNET_D: f64 = 0.015;
const MAGNET_T: f64 = 0.003;
/// Y30 ferrite remanence.
const REMANENCE: f64 = 0.385;
const BOARD_T: f64 = 0.0016;
/// Trace width, for the filament regularization radius.
const TRACE_W: f64 = 0.00025;

// --- discretization ---
//
// Cost is (coil segments) × (source segments), and the two-plane image series
// multiplies the source count by `2·reflections + 1`. At full resolution the
// as-built case is ~8.6k × 76k segment pairs per field evaluation, times ~500
// evaluations — minutes per configuration. These are characterization-grade
// settings: enough to pin the closed form's error factor to two digits, not
// enough to quote `Kt` to three. `REFLECTIONS = 6` is well inside the converged
// region for a balanced rotor (see the `IronStack` convergence table).
const COIL_FACETS: usize = 32;
const MAGNET_FACETS: usize = 24;
const MAGNET_SLICES: usize = 2;
const SWEEP_SAMPLES: usize = 12;
const REFLECTIONS: usize = 6;

const MAGNET_Z0: f64 = COIL_Z + AIR_GAP;
/// Rotor back-iron sits directly behind the magnets.
const ROTOR_IRON_Z: f64 = MAGNET_Z0 + MAGNET_T;
/// Stator back-iron behind the board.
const STATOR_IRON_Z: f64 = COIL_Z - BOARD_T;

/// Stator annulus the coils sweep — what a user would hand `calc_motor`.
const STATOR_R_IN: f64 = PITCH_R - COIL_R_OUT;
const STATOR_R_OUT: f64 = PITCH_R + COIL_R_OUT;

fn coil(cx: f64, cy: f64, cur: f64) -> Vec<Filament> {
    (0..TURNS_PER_COIL)
        .map(|t| {
            let f = t as f64 / (TURNS_PER_COIL - 1) as f64;
            let r = COIL_R_IN + (COIL_R_OUT - COIL_R_IN) * f;
            let pts = (0..COIL_FACETS)
                .map(|j| {
                    let a = 2.0 * PI * (j as f64) / COIL_FACETS as f64;
                    Vec3::new(cx + r * a.cos(), cy + r * a.sin(), COIL_Z)
                })
                .collect();
            Filament::closed_loop(pts, cur, TRACE_W * 0.5)
        })
        .collect()
}

fn machine(iron: IronStack) -> Machine {
    // Slot k belongs to phase k mod 3, all wound the same way.
    let phases = (0..3)
        .map(|p| {
            let turns = (0..SLOTS)
                .filter(|k| k % 3 == p)
                .flat_map(|k| {
                    let a = 2.0 * PI * (k as f64) / (SLOTS as f64);
                    coil(PITCH_R * a.cos(), PITCH_R * a.sin(), 1.0)
                })
                .collect();
            Phase::new(["A", "B", "C"][p], turns)
        })
        .collect();
    Machine {
        phases,
        rotor: MagnetRing::discs(
            POLES,
            PITCH_R,
            MAGNET_D,
            MAGNET_Z0,
            MAGNET_T,
            REMANENCE,
            MAGNET_FACETS,
        ),
        iron,
        magnet_slices: MAGNET_SLICES,
    }
}

/// Effective `Kt` under ideal commutation at 1 A peak, N·m/A, plus the torque
/// ripple as a fraction of the mean.
///
/// The current is shaped like the machine's own back-EMF — `i_p ∝ Ke_p(θ)`,
/// normalized so the peak phase current is 1 A. That is what a field-oriented
/// drive does, and it is maximum-torque-per-amp for whatever `Ke` waveform the
/// geometry produces. Taking the phase from the machine rather than assuming a
/// textbook `cos(pθ − 2πp/3)` avoids putting current in phase with `λ` instead of
/// `dλ/dθ`, which averages the torque to zero. Torque is still evaluated by the
/// independent Lorentz route, so this is not `Ke` fed back to itself.
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

/// Mean axial flux density over a pole face at the winding plane — the `B_gap`
/// the closed form wants.
fn b_gap(m: &Machine) -> f64 {
    let src = m.rotor_sources(0.0);
    let (mut acc, mut n) = (0.0, 0);
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
    // Three coils per phase, ten turns each.
    let turns_per_phase = ((SLOTS / 3) * TURNS_PER_COIL) as f64;
    let annulus = PI * (STATOR_R_OUT * STATOR_R_OUT - STATOR_R_IN * STATOR_R_IN);
    let a_pole_formula = annulus / (2.0 * POLE_PAIRS);
    let a_coil = PI * COIL_R_OUT * COIL_R_OUT;

    println!(
        "pcb-motor stator-v3: {SLOTS} slots / {POLES} poles, {turns_per_phase} turns per phase"
    );
    println!(
        "stator annulus {:.1}–{:.1} mm, air gap {:.1} mm\n",
        STATOR_R_IN * 1e3,
        STATOR_R_OUT * 1e3,
        AIR_GAP * 1e3
    );

    for (label, iron) in [
        ("no iron (hypothetical)", IronStack::none()),
        ("rotor back-iron only", IronStack::single(ROTOR_IRON_Z)),
        (
            "full circuit (as built)",
            IronStack::pair(STATOR_IRON_Z, ROTOR_IRON_Z, REFLECTIONS),
        ),
    ] {
        let m = machine(iron);
        let b = b_gap(&m);
        let (kt_oracle, ripple) = kt_commutated(&m, SWEEP_SAMPLES);
        let kt_closed = closed_form_kt(1.0, turns_per_phase, b);

        println!("=== {label} ===");
        println!(
            "  B_gap (oracle, mean over pole face) : {:>9.1} mT",
            b * 1e3
        );
        println!(
            "  Kt oracle (1 A peak, commutated)    : {:>9.3} mN·m/A",
            kt_oracle.abs() * 1e3
        );
        println!(
            "  Kt closed form                      : {:>9.3} mN·m/A",
            kt_closed * 1e3
        );
        println!(
            "  closed form / oracle                : {:>9.2}x",
            kt_closed / kt_oracle.abs()
        );
        println!(
            "  torque ripple (pk-pk / mean)        : {:>9.1}%",
            ripple * 100.0
        );
        println!();
    }

    println!("A_pole per closed form : {:.3e} m²", a_pole_formula);
    println!("area one coil encloses : {:.3e} m²", a_coil);
    println!(
        "                 ratio : {:.2}x  (9 coils cover {:.0}% of the annulus)",
        a_pole_formula / a_coil,
        100.0 * (SLOTS as f64) * a_coil / annulus
    );
}
