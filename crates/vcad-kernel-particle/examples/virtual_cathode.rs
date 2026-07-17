//! Virtual-cathode probe: do the trapped electrons neutralize the *core*?
//!
//! `orbitron_probe` showed the two-ring cusp traps ~87% of electrons —
//! overturning the e-injection "dead end", which had launched electrons on
//! the 0.2 inner shell (they escape the axial point cusp before
//! magnetizing). The decisive follow-up: a trapped electron cloud only
//! helps fusion if it sits *in the core*, deepening the ion well where
//! reactions happen — a virtual cathode (the polywell mechanism) — rather
//! than circulating peripherally at the rings.
//!
//! This sweeps the electron launch radius and reports the electron
//! contribution to the on-axis core potential (negative = the well
//! deepens, the ion beam is neutralized where it matters) alongside the
//! recovered ion yield.
//!
//! Perfect-injection UPPER BOUND — injector efficiency, cusp losses, and
//! electron thermalization are unmodeled; non-relativistic Boris. The
//! bench signs the receipt.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example virtual_cathode`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::fom::neutron_rate_per_s;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::space_charge::{self, SelfConsistentOptions};
use vcad_kernel_particle::trace::TraceOptions;
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

fn main() {
    // 30 mA operating point (the sweet spot) at the ceiling shield config.
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
    let i_a = 0.030;
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);

    println!("# virtual-cathode probe: 30 mA ions + 30 mA electrons at 100 kV");
    println!(
        "e_shell,e_survivor,core_dV_kv,net_ratio,taxed_n_per_s,neutralized_n_per_s,gain,vs_record"
    );
    for &shell in &[0.2, 0.4, 0.55, 0.7, 0.85] {
        let r = space_charge::neutralized(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            i_a,
            i_a,
            shell,
            &sc,
        )
        .expect("neutralized run");
        let taxed = neutron_rate_per_s(r.ion_only.final_stats().mean_ddn_sigma_v_m3, i_a, n_d);
        let neut = neutron_rate_per_s(r.recovered_stats.mean_ddn_sigma_v_m3, i_a, n_d);
        println!(
            "{:.2},{:.3},{:.1},{:.3},{:.3e},{:.3e},{:.2},{:.1}",
            shell,
            r.electron_survivor_fraction,
            r.core_potential_change_v / 1e3,
            r.net_ratio,
            taxed,
            neut,
            neut / taxed.max(1e-300),
            neut / RECORD_N_PER_S
        );
    }
    println!(
        "\nread: core_dV_kv < 0 means the trapped electron cloud deepens the \
         ion well ON AXIS (a virtual cathode — the neutralization that helps \
         fusion). `gain` = neutralized/taxed ion yield. e_survivor is the \
         trapped fraction. Launch radius is decisive: too central and \
         electrons escape the axial cusp; near the rings they magnetize and \
         trap. Perfect-injection upper bound; the bench signs the receipt."
    );
}
