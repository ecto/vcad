//! The verdict probe: what survives self-consistency?
//!
//! The ceiling hunt's 46×-the-record headline carried an asterisk: the
//! space-charge gauge read 0.363 at 30 mA, far past the 10% linearity
//! band. This example runs the PIC-lite loop
//! ([`vcad_kernel_particle::space_charge::self_consistent`]) at the
//! winning configuration and reports what the beam's own charge does to
//! confinement and yield — the number the receipt should carry.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example self_consistent_probe`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::fom::neutron_rate_per_s;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::space_charge::{self, SelfConsistentOptions};
use vcad_kernel_particle::trace::{TraceOptions, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

fn main() {
    // The ceiling-hunt winner family: 100 kV, strong shield, 45 mm rings.
    let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -100_000.0, 584_237.0);
    let topts = TraceOptions {
        max_passes: 30,
        ..TraceOptions::default()
    };
    let sc = SelfConsistentOptions {
        iterations: 5,
        particles: 48,
        ..SelfConsistentOptions::default()
    };
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);

    println!("# self-consistent verdict at 100 kV + 584 kA·t (4 mTorr)");
    println!(
        "current_ma,ratio_first,converged,vacuum_n_per_s,selfcon_n_per_s,correction,vs_record"
    );
    for &ma in &[8.0, 30.0] {
        let report = space_charge::self_consistent(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            DEUTERON,
            ma * 1e-3,
            &sc,
        )
        .expect("self-consistent loop");
        let vac = neutron_rate_per_s(report.vacuum_stats.mean_ddn_sigma_v_m3, ma * 1e-3, n_d);
        let fin = neutron_rate_per_s(report.final_stats().mean_ddn_sigma_v_m3, ma * 1e-3, n_d);
        println!(
            "{:.0},{:.3},{},{:.3e},{:.3e},{:.2},{:.1}",
            ma,
            report.iterations.first().map(|i| i.ratio).unwrap_or(0.0),
            report.converged,
            vac,
            fin,
            fin / vac.max(1e-300),
            fin / RECORD_N_PER_S
        );
        for (k, it) in report.iterations.iter().enumerate() {
            eprintln!(
                "#   iter {}: ratio {:.3}, dRho {:.3}, passes {:.1}, intercept {:.2}",
                k + 1,
                it.ratio,
                it.rho_delta_rel,
                it.stats.mean_passes,
                it.stats.interception_fraction
            );
        }
    }
    println!(
        "\nread: `correction` is what self-consistency does to the linear \
         claim (1.00 = the gauge was conservative; <1 = space charge taxes \
         the yield). Converged=false means the density was still moving at \
         the iteration budget — treat as unsettled, not as an answer."
    );
}
