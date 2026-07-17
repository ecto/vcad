//! The e-injection sweep: how much yield does the electron cloud buy back?
//!
//! At the ceiling configuration, space charge taxes the linear yield
//! claim (correction 0.42 at 30 mA — see the research log). This sweep
//! runs the **perfect-injection electron-cloud bound**
//! ([`vcad_kernel_particle::space_charge::neutralized`]) across ion
//! currents and reports the recovery: taxed → neutralized → vacuum, plus
//! the net beam-potential ratio before and after the cloud.
//!
//! Read the output with its own caveat attached: perfect injection is an
//! upper bound (injector efficiency, cusp electron losses, and electron
//! thermalization — the polywell's demons — are unmodeled). What this
//! prices is the *ceiling of the neutralization lane*, not a promise.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example neutralization_sweep`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::fom::neutron_rate_per_s;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::space_charge::{self, SelfConsistentOptions};
use vcad_kernel_particle::trace::TraceOptions;
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const PRESSURE_MTORR: f64 = 4.0;
const RECORD_N_PER_S: f64 = 5.0e6;

fn main() {
    let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -100_000.0, 584_237.0);
    let topts = TraceOptions {
        max_passes: 30,
        ..TraceOptions::default()
    };
    let sc = SelfConsistentOptions {
        iterations: 6,
        relax: 0.25,
        particles: 96,
        ..SelfConsistentOptions::default()
    };
    let n_d = d2_deuteron_density_m3(PRESSURE_MTORR, 300.0);

    println!("# e-injection sweep at 100 kV + 584 kA·t (4 mTorr); e-current = ion current");
    println!(
        "current_ma,ion_ratio,net_ratio,neutralization_frac,vacuum_n_per_s,taxed_n_per_s,neutralized_n_per_s,recovery_frac,vs_record,obs_converged"
    );
    for &ma in &[30.0, 60.0, 100.0] {
        let i_a = ma * 1e-3;
        let r = space_charge::neutralized(
            &device,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            i_a,
            i_a,
            0.5,
            &sc,
        )
        .expect("neutralized run");
        let vac = neutron_rate_per_s(r.ion_only.vacuum_stats.mean_ddn_sigma_v_m3, i_a, n_d);
        let taxed = neutron_rate_per_s(r.ion_only.final_stats().mean_ddn_sigma_v_m3, i_a, n_d);
        let neut = neutron_rate_per_s(r.recovered_stats.mean_ddn_sigma_v_m3, i_a, n_d);
        let ion_ratio = r.ion_only.iterations.last().map(|i| i.ratio).unwrap_or(0.0);
        let recovery = if vac > taxed {
            ((neut - taxed) / (vac - taxed)).clamp(-1.0, 2.0)
        } else {
            0.0
        };
        println!(
            "{:.0},{:.3},{:.3},{:.2},{:.3e},{:.3e},{:.3e},{:.2},{:.1},{}",
            ma,
            ion_ratio,
            r.net_ratio,
            r.neutralization_fraction,
            vac,
            taxed,
            neut,
            recovery,
            neut / RECORD_N_PER_S,
            r.ion_only.observably_converged
        );
    }
    println!(
        "\nread: recovery_frac = (neutralized − taxed)/(vacuum − taxed); 1.0 = the \
         cloud fully undoes the space-charge tax. Perfect-injection UPPER BOUND — \
         injector efficiency, cusp e-losses, and e-thermalization unmodeled. The \
         bench signs the receipt."
    );
}
