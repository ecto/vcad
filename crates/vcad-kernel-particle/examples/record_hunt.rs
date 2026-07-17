//! The record hunt: how many neutrons can this machine make?
//!
//! Sweeps cathode voltage × shield current on the rev-b geometry and
//! prices every configuration as a sustained D-D neutron rate at a
//! record-attempt operating point, against the scoreboard:
//!
//! - **5×10⁶ n/s** — the amateur DIY record (fusor.net community)
//! - **Joule One** — 1 J of banked fusion energy (≈ 8.5×10¹¹ D-D
//!   reactions); no amateur device has ever done it
//!
//! Honesty rails: beam-on-background only, so two numbers bracket
//! reality — the CX-off ceiling (ions recirculate unmolested) and the
//! CX-on floor (single-generation charge exchange, no chain). Pressure
//! and current enter linearly in this model; real machines see breakdown
//! and space-charge limits the model does not price. Every printed rate
//! is a prediction; the bench signs the receipt.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example record_hunt`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::{neutron_rate_per_s, stats, EnsembleStats};
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{CxModel, TraceOptions, Tracer, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

// Rev-b geometry (docs/shielded-grid-experiment.md).
const CHAMBER_R_MM: f64 = 150.0;
const RING_R_MM: f64 = 45.0;
const RING_Z_MM: f64 = 25.0;
const WIRE_A_MM: f64 = 3.0;

// Record-attempt operating point. Current and pressure are at the hot
// end of documented amateur practice; both enter linearly here.
const ION_CURRENT_A: f64 = 0.030;
const PRESSURE_MTORR: f64 = 4.0;
const CX_SIGMA_M2: f64 = 1.0e-19;

const RECORD_N_PER_S: f64 = 5.0e6;
const JOULE_ONE_REACTIONS: f64 = 8.5e11; // ~1 J across both D-D branches

// Shield-coil realization from the codesign receipt: 400 turns, L ≈ 23 mH
// per coil at that winding — stored energy prices the supply/cryo ask.
const COIL_TURNS: f64 = 400.0;
const COIL_L_H: f64 = 0.023;

fn run(volts: f64, amp_turns: f64, cx: bool, particles: usize) -> EnsembleStats {
    let device = Device::shielded_two_ring(
        CHAMBER_R_MM,
        RING_R_MM,
        RING_Z_MM,
        WIRE_A_MM,
        -volts,
        amp_turns,
    );
    let sol = solve(&device, 121, 241, &SolveOptions::default()).expect("poisson");
    let fields = FieldMap::new(&device, &sol);
    let mut topts = TraceOptions {
        max_passes: 40,
        ..TraceOptions::default()
    };
    if cx {
        topts.cx = Some(CxModel {
            sigma_cx_m2: CX_SIGMA_M2,
            background_deuteron_density_m3: d2_deuteron_density_m3(PRESSURE_MTORR, 300.0),
        });
    }
    let tracer = Tracer::new(&device, &fields, &sol, topts);
    stats(&tracer.launch_ensemble(DEUTERON, particles))
}

fn ceiling_rate(s: &EnsembleStats) -> f64 {
    neutron_rate_per_s(
        s.mean_ddn_sigma_v_m3,
        ION_CURRENT_A,
        d2_deuteron_density_m3(PRESSURE_MTORR, 300.0),
    )
}

fn floor_rate(s: &EnsembleStats) -> f64 {
    (ION_CURRENT_A / vcad_kernel_particle::constants::ELEMENTARY_CHARGE)
        * (s.mean_neutrons_ion_channel + s.mean_neutrons_cx_channel)
}

fn main() {
    println!(
        "# record hunt: {} mA, {} mTorr D2, rev-b geometry; record = {:.1e} n/s",
        ION_CURRENT_A * 1e3,
        PRESSURE_MTORR,
        RECORD_N_PER_S
    );
    println!("volts,amp_turns,intercept_frac,ceiling_n_per_s,vs_record,cathode_heat_w");

    let mut best: Option<(f64, f64, EnsembleStats)> = None;
    for &kv in &[30.0, 40.0, 50.0, 60.0, 75.0] {
        // The shield optimum scales ~sqrt(V) (r_L law): anchor 160 kA·t at
        // 30 kV and probe 0 / 1x / 1.5x the scaled optimum.
        let scaled = 160_000.0 * (kv / 30.0_f64).sqrt();
        for &at in &[0.0, scaled, 1.5 * scaled] {
            let s = run(kv * 1e3, at, false, 96);
            let rate = ceiling_rate(&s);
            let heat = s.interception_fraction * ION_CURRENT_A * kv * 1e3;
            println!(
                "{:.0},{:.0},{:.3},{:.3e},{:.2},{:.0}",
                kv,
                at,
                s.interception_fraction,
                rate,
                rate / RECORD_N_PER_S,
                heat
            );
            let better = best.as_ref().map(|(_, _, b)| rate > ceiling_rate(b));
            if better.unwrap_or(true) {
                best = Some((kv, at, s));
            }
        }
    }

    let (kv, at, s) = best.expect("sweep ran");
    let ceiling = ceiling_rate(&s);
    let heat_w = s.interception_fraction * ION_CURRENT_A * kv * 1e3;

    // The floor: same winner, single-generation charge exchange on.
    let s_cx = run(kv * 1e3, at, true, 96);
    let floor = floor_rate(&s_cx);

    let coil_current_a = at / COIL_TURNS;
    let stored_j = 0.5 * COIL_L_H * coil_current_a * coil_current_a;

    println!("\n──── winner: {kv:.0} kV, {at:.0} A·t ────");
    println!(
        "ceiling (CX-off): {ceiling:.2e} n/s = {:.1}× the amateur record",
        ceiling / RECORD_N_PER_S
    );
    println!(
        "floor (CX-on, no chain): {floor:.2e} n/s = {:.2}× the record \
         (real fusors sit between: the CX chain re-accelerates every \
         product ion — unmodeled upside)",
        floor / RECORD_N_PER_S
    );
    println!(
        "cathode heat at full duty: {heat_w:.0} W (interception {:.2}) — \
         the shield is the thermal fix: unshielded at this point would be \
         {:.0} W",
        s.interception_fraction,
        ION_CURRENT_A * kv * 1e3
    );
    println!(
        "shield realization: {COIL_TURNS:.0} t × {coil_current_a:.0} A per \
         coil, {stored_j:.0} J stored each — REBCO territory for sustained \
         runs (copper only pulses this)"
    );
    let hours_to_joule = JOULE_ONE_REACTIONS / (2.0 * ceiling) / 3600.0;
    println!(
        "Joule One at the ceiling rate: ~{hours_to_joule:.0} h of sustained \
         operation (≈{:.1} weekends) — and every hour is receipted",
        hours_to_joule / 16.0
    );
    println!(
        "\nall numbers are predictions at {} mA / {} mTorr (both linear in \
         this model; breakdown and space charge are not priced). The bench \
         signs the receipt.",
        ION_CURRENT_A * 1e3,
        PRESSURE_MTORR
    );
}
