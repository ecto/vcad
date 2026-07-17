//! The records dial: shield current tunes between two fusion regimes.
//!
//! A TiD-coated cathode makes every intercepted ion a beam-target
//! reaction. The shield current then becomes a *dial*: low current →
//! high interception → beam-target-dominated (solid density, huge yield);
//! high current → low interception → gas-recirculation-dominated (what the
//! ceiling hunt optimized). This example prices both channels vs shield
//! current at the record-attempt operating point and reports the total —
//! the instrument no other machine has.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example records_dial`

use vcad_kernel_particle::beam_target;
use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::{neutron_rate_per_s, stats};
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const KV: f64 = 100.0;
const ION_CURRENT_A: f64 = 0.030;
const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

fn main() {
    println!(
        "# records dial: {KV:.0} kV, {} mA onto TiD-coated rings",
        ION_CURRENT_A * 1e3
    );
    println!("amp_turns,intercept,beam_target_n_per_s,gas_n_per_s,total_n_per_s,vs_record");
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);

    // Beam-target yield of the ions that hit the (TiD) wires, at full
    // cathode energy. The intercepted current is interception × I.
    let mut best = (0.0_f64, 0.0_f64);
    for &at in &[0.0, 160_000.0, 400_000.0, 760_000.0, 1_170_000.0] {
        let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -KV * 1e3, at);
        let sol = solve(&device, 121, 241, &SolveOptions::default()).expect("poisson");
        let fields = FieldMap::new(&device, &sol);
        let topts = TraceOptions {
            max_passes: 40,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, topts);
        let s = stats(&tracer.launch_ensemble(DEUTERON, 96));

        let intercepted_a = s.interception_fraction * ION_CURRENT_A;
        let beam_target = beam_target::neutron_rate_n_per_s(KV, intercepted_a, 1.0);
        let gas = neutron_rate_per_s(s.mean_ddn_sigma_v_m3, ION_CURRENT_A, n_d);
        let total = beam_target + gas;
        println!(
            "{:.0},{:.3},{:.3e},{:.3e},{:.3e},{:.0}",
            at,
            s.interception_fraction,
            beam_target,
            gas,
            total,
            total / RECORD_N_PER_S
        );
        if total > best.1 {
            best = (at, total);
        }
    }

    // The records play: minimize shield, maximize interception onto TiD.
    let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -KV * 1e3, 0.0);
    let sol = solve(&device, 121, 241, &SolveOptions::default()).unwrap();
    let fields = FieldMap::new(&device, &sol);
    let tracer = Tracer::new(
        &device,
        &fields,
        &sol,
        TraceOptions {
            max_passes: 40,
            ..TraceOptions::default()
        },
    );
    let s = stats(&tracer.launch_ensemble(DEUTERON, 96));
    let full_beam = beam_target::neutron_rate_n_per_s(KV, ION_CURRENT_A, 1.0);

    println!("\n──── the records card ────");
    println!(
        "unshielded TiD (100% to target): {:.2e} n/s = {:.0}× the amateur record",
        full_beam,
        full_beam / RECORD_N_PER_S
    );
    println!(
        "beam-target Q at {KV:.0} keV: {:.2e} (stopping-power-capped — records lane, not gain)",
        beam_target::q_beam_target(KV)
    );
    println!(
        "anchor: the published Ti drive-in generator, {:.1e} n/s at {:.0} mA / {:.0} keV — \
         this predicts the same physics at our current by cross-section ratio",
        beam_target::CALIBRATION.neutron_rate_n_per_s,
        beam_target::CALIBRATION.current_a * 1e3,
        beam_target::CALIBRATION.energy_kev
    );
    println!(
        "interception at zero shield: {:.2} — the dial's beam-target end",
        s.interception_fraction
    );
    println!(
        "\nhonesty: TiD loses D above ~250 C (pair with the thermal crate); \
         monatomic-beam fraction and target loading are real derates; every \
         number is a prediction — the bench signs the receipt."
    );
}
