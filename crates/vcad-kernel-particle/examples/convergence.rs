//! Grid-convergence study for the fusor-baseline figures of merit.
//!
//! Runs the classic 5-ring fusor at four grid resolutions with a fixed
//! ensemble and prints the key FoMs — the table quoted in
//! `docs/particle-optics-m0.md`.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example convergence`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::stats;
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};

fn main() {
    println!("nr,nz,intercept_frac,mean_passes,ddn_sigma_v_m3,mean_drift,max_drift");
    let device = Device::classic_fusor(150.0, 50.0, 5, 1.0, -30_000.0);
    for (nr, nz) in [(61, 121), (81, 161), (121, 241), (161, 321)] {
        let sol = solve(&device, nr, nz, &SolveOptions::default()).expect("solve");
        let fields = FieldMap::new(&device, &sol);
        let opts = TraceOptions {
            max_passes: 20,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, opts);
        let s = stats(&tracer.launch_ensemble(DEUTERON, 48));
        println!(
            "{nr},{nz},{:.3},{:.2},{:.3e},{:.4},{:.4}",
            s.interception_fraction,
            s.mean_passes,
            s.mean_ddn_sigma_v_m3,
            s.mean_energy_drift_rel,
            s.max_energy_drift_rel
        );
    }
}
