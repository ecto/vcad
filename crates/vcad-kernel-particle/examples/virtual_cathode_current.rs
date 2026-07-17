//! Virtual-cathode current sweep: how much electron current deepens the
//! core well enough to matter?
//!
//! The launch-radius probe showed electrons trap (75–87%) but at matched
//! 30 mA the core well deepens only ~0.3 kV — the trapped cloud is real
//! but its *charge* is too small to neutralize the ion beam centrally.
//! Trapping is what makes higher electron current survivable (electrons no
//! longer leave in one transit), so the lever is electron current. This
//! sweeps it at a fixed near-ring launch and reports the on-axis well
//! deepening and the recovered ion yield.
//!
//! Perfect-injection UPPER BOUND (injector efficiency, cusp losses,
//! electron thermalization unmodeled; non-relativistic Boris). The virtual
//! cathode this prices is the ceiling of the mechanism, not a promise.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example virtual_cathode_current`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::fom::neutron_rate_per_s;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::space_charge::{self, SelfConsistentOptions};
use vcad_kernel_particle::trace::TraceOptions;
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;
const ION_MA: f64 = 30.0;
const E_SHELL: f64 = 0.55;

fn main() {
    let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -100_000.0, 584_237.0);
    let topts = TraceOptions {
        max_passes: 30,
        ..TraceOptions::default()
    };
    let sc = SelfConsistentOptions {
        iterations: 5,
        relax: 0.25,
        particles: 96,
        ..SelfConsistentOptions::default()
    };
    let i_a = ION_MA * 1e-3;
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);
    let taxed = {
        // One reference ion loop (electron current 0 gives the taxed base).
        let r = space_charge::neutralized(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            i_a,
            0.0,
            E_SHELL,
            &sc,
        )
        .expect("ref");
        neutron_rate_per_s(r.ion_only.final_stats().mean_ddn_sigma_v_m3, i_a, n_d)
    };

    println!(
        "# virtual-cathode current sweep: {ION_MA:.0} mA ions at 100 kV, electrons at shell {E_SHELL}"
    );
    println!("e_current_ma,e_over_i,core_dV_kv,net_ratio,neutralized_n_per_s,gain,vs_record");
    for &e_ma in &[30.0, 100.0, 300.0, 1000.0, 3000.0] {
        let e_a = e_ma * 1e-3;
        let r = space_charge::neutralized(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            i_a,
            e_a,
            E_SHELL,
            &sc,
        )
        .expect("neutralized");
        let neut = neutron_rate_per_s(r.recovered_stats.mean_ddn_sigma_v_m3, i_a, n_d);
        println!(
            "{:.0},{:.1},{:.1},{:.3},{:.3e},{:.2},{:.1}",
            e_ma,
            e_ma / ION_MA,
            r.core_potential_change_v / 1e3,
            r.net_ratio,
            neut,
            neut / taxed.max(1e-300),
            neut / RECORD_N_PER_S
        );
    }
    println!(
        "\nread: core_dV_kv < 0 deepens the on-axis ion well (virtual cathode). \
         `gain` = neutralized/taxed ion yield at {ION_MA:.0} mA. Electron current \
         is the neutralization lever that trapping unlocks — but each mA is real \
         injected current with a real power cost the receipt must carry. \
         Perfect-injection upper bound."
    );
}
