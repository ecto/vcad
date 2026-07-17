//! The ceiling hunt: how hard can this design actually be pushed?
//!
//! Three questions, three instruments:
//!
//! 1. **Where does the shield turn over?** The record hunt's optimum was
//!    still rising at its sweep edge — escalate ampere-turns until the
//!    magnetic aperture closes the core and yield falls.
//! 2. **Does geometry want to move?** Probe ring radius around the
//!    baseline at the shield optimum.
//! 3. **Where does the current claim break?** The space-charge gauge
//!    (`space_charge::estimate`): beam potential vs applied well from the
//!    traced dwell map, and the current at which the linearity assumption
//!    stops being honest.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example ceiling_hunt`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::{neutron_rate_per_s, stats, EnsembleStats};
use vcad_kernel_particle::poisson::{solve, Solution, SolveOptions};
use vcad_kernel_particle::space_charge;
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const CHAMBER_R_MM: f64 = 150.0;
const RING_Z_MM: f64 = 25.0;
const WIRE_A_MM: f64 = 3.0;
const ION_CURRENT_A: f64 = 0.030;
const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

struct Run {
    stats: EnsembleStats,
    solution: Solution,
    dwell: Vec<f64>,
    particles: usize,
}

fn run(volts: f64, amp_turns: f64, ring_r_mm: f64, particles: usize) -> Run {
    let device = Device::shielded_two_ring(
        CHAMBER_R_MM,
        ring_r_mm,
        RING_Z_MM,
        WIRE_A_MM,
        -volts,
        amp_turns,
    );
    let solution = solve(&device, 121, 241, &SolveOptions::default()).expect("poisson");
    let fields = FieldMap::new(&device, &solution);
    let topts = TraceOptions {
        max_passes: 40,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &solution, topts);
    let (outcomes, dwell) = tracer.launch_ensemble_dwell(DEUTERON, particles);
    Run {
        stats: stats(&outcomes),
        solution,
        dwell,
        particles,
    }
}

fn rate(s: &EnsembleStats) -> f64 {
    neutron_rate_per_s(
        s.mean_ddn_sigma_v_m3,
        ION_CURRENT_A,
        d2_deuteron_density_m3(PRESSURE_MTORR, 300.0),
    )
}

fn main() {
    // ── 1. shield escalation: find the turnover ───────────────────────
    println!("# escalation at 45 mm rings; record = {RECORD_N_PER_S:.1e} n/s");
    println!("volts,amp_turns,intercept_frac,survive_frac,n_per_s,vs_record");
    let mut best: Option<(f64, f64, f64)> = None;
    for &kv in &[75.0, 100.0] {
        let scaled = 160_000.0 * (kv / 30.0_f64).sqrt();
        for &mult in &[1.5, 2.0, 3.0, 4.0] {
            let at = scaled * mult;
            let r = run(kv * 1e3, at, 45.0, 64);
            let n = rate(&r.stats);
            println!(
                "{:.0},{:.0},{:.3},{:.3},{:.3e},{:.1}",
                kv,
                at,
                r.stats.interception_fraction,
                r.stats.survivor_fraction,
                n,
                n / RECORD_N_PER_S
            );
            let better = best.map(|(_, _, b)| n > b).unwrap_or(true);
            if better {
                best = Some((kv, at, n));
            }
        }
    }
    let (kv, at, _) = best.expect("escalation ran");

    // ── 2. geometry probe at the winner ───────────────────────────────
    println!("\n# ring-radius probe at {kv:.0} kV, {at:.0} A·t");
    println!("ring_r_mm,n_per_s,vs_record");
    let mut geo_best: Option<(f64, Run, f64)> = None;
    for &rr in &[35.0, 45.0, 55.0] {
        let r = run(kv * 1e3, at, rr, 64);
        let n = rate(&r.stats);
        println!("{:.0},{:.3e},{:.1}", rr, n, n / RECORD_N_PER_S);
        let better = geo_best.as_ref().map(|(_, _, b)| n > *b).unwrap_or(true);
        if better {
            geo_best = Some((rr, r, n));
        }
    }
    let (rr, winner, n_best) = geo_best.expect("probe ran");

    // ── 3. the space-charge gauge at the winner ───────────────────────
    let sc = space_charge::estimate(
        &winner.solution,
        &winner.dwell,
        winner.particles,
        ION_CURRENT_A,
        &SolveOptions::default(),
    )
    .expect("space charge");
    println!("\n──── the ceiling card ────");
    println!(
        "best config: {kv:.0} kV, {at:.0} A·t, {rr:.0} mm rings → {n_best:.2e} n/s \
         = {:.0}× the amateur record (beam-on-background ceiling)",
        n_best / RECORD_N_PER_S
    );
    println!(
        "space charge: beam potential {:.0} V vs {:.0} V well → ratio {:.3} at \
         {:.0} mA",
        sc.phi_beam_peak_v,
        sc.well_depth_v,
        sc.ratio,
        ION_CURRENT_A * 1e3
    );
    println!(
        "linearity holds (ratio ≤ 10%) up to ≈{:.0} mA; space-charge-limited \
         (ratio ~ 1) at ≈{:.1} A — beyond that the model must go \
         self-consistent (PIC), and so must the claims",
        sc.current_at_ratio_a(0.10) * 1e3,
        sc.current_at_ratio_a(1.0)
    );
    println!(
        "cathode heat at the winner: {:.0} W of {:.0} W beam \
         (interception {:.2})",
        winner.stats.interception_fraction * ION_CURRENT_A * kv * 1e3,
        ION_CURRENT_A * kv * 1e3,
        winner.stats.interception_fraction
    );
    println!(
        "\nhonesty: pressure/current linear in-model; CX chain unmodeled; \
         100 kV is the practical amateur HV ceiling; every number is a \
         prediction — the bench signs the receipt."
    );
}
