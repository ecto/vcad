//! The honest Q of the virtual-cathode machine.
//!
//! The e-injection sweep showed the electron cloud boosts ion yield (×8.4
//! at 10 A). This example prices that boost: it runs the neutralized
//! machine, measures the electron confinement time, and feeds the full
//! power ledger — because a sustained virtual cathode costs electron
//! injection power, and in steady state that power is `I_e · V` (the
//! injector replaces every lost electron, each carrying ~the well out).
//! The verdict the yield claim can't dodge: does the boost survive its own
//! power bill?
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example sustained_q`

use vcad_kernel_particle::confinement::confinement;
use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::fom::neutron_rate_per_s;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::power::LedgerInputs;
use vcad_kernel_particle::space_charge::{neutralized, SelfConsistentOptions};
use vcad_kernel_particle::trace::{TraceOptions, ELECTRON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const KV: f64 = 100.0;
const ION_CURRENT_A: f64 = 0.030;
const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

fn main() {
    let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -KV * 1e3, 584_237.0);
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);
    let ion_topts = TraceOptions {
        max_passes: 30,
        ..TraceOptions::default()
    };
    let sc = SelfConsistentOptions {
        iterations: 4,
        relax: 0.25,
        particles: 64,
        ..SelfConsistentOptions::default()
    };

    // Electron confinement time in this cusp (B-on), reused across the
    // current points: in-flight charge = I_e · τ, sustain power = I_e · V.
    let conf = confinement(
        &device,
        ELECTRON,
        61,
        121,
        &SolveOptions::default(),
        &TraceOptions {
            max_passes: 60,
            time_budget_factor: 20.0,
            launch_shell_fraction: 0.55,
            ..TraceOptions::default()
        },
        24,
    )
    .expect("confinement");
    let tau_e = conf.mean_time_s;
    println!(
        "# sustained Q at {KV:.0} kV, {} mA ions; e-confinement tau = {:.2e} s (enhancement {:.0}x, survivors {:.0}%)",
        ION_CURRENT_A * 1e3,
        tau_e,
        conf.enhancement,
        conf.survivor_fraction * 100.0
    );
    println!("e_current_a,neutron_n_per_s,vs_record,P_fus_W,P_ion_W,P_e_W,input_W,Q");

    for &ie in &[0.03, 1.0, 3.0, 10.0] {
        let r = neutralized(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &ion_topts,
            ION_CURRENT_A,
            ie,
            0.55,
            &sc,
        )
        .expect("neutralized");
        let n_rate = neutron_rate_per_s(r.recovered_stats.mean_ddn_sigma_v_m3, ION_CURRENT_A, n_d);
        let led = LedgerInputs {
            neutron_rate_n_per_s: n_rate,
            ion_current_a: ION_CURRENT_A,
            voltage_v: KV * 1e3,
            trapped_electron_charge_c: ie * tau_e,
            electron_confinement_time_s: tau_e,
            electron_loss_energy_ev: 0.0, // defaults to the full well
            magnet_power_w: 0.0,          // flagged unpriced (see below)
        }
        .evaluate();
        println!(
            "{:.2},{:.3e},{:.0},{:.2e},{:.2e},{:.2e},{:.2e},{:.2e}",
            ie,
            n_rate,
            n_rate / RECORD_N_PER_S,
            led.fusion_power_w,
            led.ion_beam_power_w,
            led.electron_sustain_power_w,
            led.input_power_w,
            led.q
        );
    }

    println!(
        "\n──── the honest ledger ────\n\
         The electron cloud that boosts yield costs I_e·V of injection power, \
         and it dominates the input the moment I_e exceeds the ion current. The \
         virtual cathode raises neutron RATE (a records-lane win) but not Q — \
         Q falls as the cloud grows, because sustain power outruns fusion power. \
         Plus the shield MAGNET term is unpriced here (dominant at MA-turns — \
         needs the em+thermal crates), so even these Q values are over-estimates.\n\
         Bottom line: neutralization is a NEUTRON-RATE lever (records + physics \
         lanes), not a Q lever. The Q lane needs a different physics — direct \
         energy recovery on the losses — which this ledger is now built to price."
    );
}
