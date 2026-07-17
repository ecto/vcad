//! Fusor-baseline benchmark: the simulated ammeter and neutron counter.
//!
//! Traces ion ensembles through (a) a classic 5-ring fusor and (b) the
//! two-ring magnetically shielded cathode across a shield-current sweep at
//! two bias voltages, printing CSV. Interception fraction is what a
//! cathode ammeter sees; mean passes is the recirculation the shield buys;
//! `pred_n_per_s` is the beam-on-background D-D neutron rate at the
//! reference operating point (10 mA ion current, 2 mTorr D₂, 300 K).
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example fusor_baseline`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::{geometric_transparency, neutron_rate_per_s, stats};
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const REF_ION_CURRENT_A: f64 = 0.010;
const REF_PRESSURE_MTORR: f64 = 2.0;

fn bench(device: &Device, label: &str, volts: f64, amp_turns: f64) {
    let sol = solve(device, 121, 241, &SolveOptions::default()).expect("poisson solve");
    let fields = FieldMap::new(device, &sol);
    let opts = TraceOptions {
        max_passes: 40,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(device, &fields, &sol, opts);
    let outcomes = tracer.launch_ensemble(DEUTERON, 96);
    let s = stats(&outcomes);
    let n_d = d2_deuteron_density_m3(REF_PRESSURE_MTORR, 300.0);
    let rate = neutron_rate_per_s(s.mean_ddn_sigma_v_m3, REF_ION_CURRENT_A, n_d);
    println!(
        "{label},{volts},{amp_turns},{:.2},{:.3},{:.3},{:.3},{:.3},{:.4},{:.3e},{:.3e}",
        s.mean_passes,
        s.interception_fraction,
        s.wall_fraction,
        s.survivor_fraction,
        s.effective_transparency,
        s.max_energy_drift_rel,
        s.mean_ddn_sigma_v_m3,
        rate
    );
}

fn main() {
    println!(
        "config,bias_v,ampere_turns,mean_passes,intercept_frac,wall_frac,survive_frac,eff_transparency,max_energy_drift,ddn_sigma_v_m3,pred_n_per_s"
    );

    // Classic fusor control: 5 rings on a 50 mm cathode sphere, 1 mm wire.
    let fusor = Device::classic_fusor(150.0, 50.0, 5, 1.0, -30_000.0);
    eprintln!(
        "# classic fusor geometric transparency: {:.3}",
        geometric_transparency(&fusor)
    );
    eprintln!(
        "# reference operating point: {} mA, {} mTorr D2, 300 K",
        REF_ION_CURRENT_A * 1e3,
        REF_PRESSURE_MTORR
    );
    bench(&fusor, "classic_fusor_5ring", -30_000.0, 0.0);

    // Shielded two-ring cathode: sweep ampere-turns at two biases.
    for &volts in &[-3_000.0_f64, -30_000.0] {
        for &at in &[
            0.0_f64, 5_000.0, 10_000.0, 20_000.0, 40_000.0, 80_000.0, 160_000.0,
        ] {
            let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, volts, at);
            bench(&device, "shielded_two_ring", volts, at);
        }
    }
}
