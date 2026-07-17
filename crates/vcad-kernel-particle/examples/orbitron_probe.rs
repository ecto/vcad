//! Orbitron probe: does the shield's own cusp trap electrons?
//!
//! The e-injection dead end (electrons fall out of the −V well in one
//! transit) has a caveat: the shield rings make a magnetic cusp, and near
//! the wires electrons are magnetized. This sweep measures the electron
//! confinement-time enhancement (full field vs the same launch with the
//! coil currents zeroed) across shield ampere-turns — the load-bearing
//! number for whether the neutralization lane has any architecture at all.
//!
//! Read with the caveats on [`vcad_kernel_particle::confinement`]:
//! non-relativistic Boris (the B-on/B-off ratio cancels most of it),
//! single particles, no electron self-fields, and the enhancement
//! saturates at the flight budget (it is a floor, not the true trapping
//! time).
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example orbitron_probe`

use vcad_kernel_particle::confinement::confinement;
use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::trace::{TraceOptions, ELECTRON};

fn main() {
    // 100 kV device, 45 mm rings — the ceiling-hunt family.
    let topts = TraceOptions {
        max_passes: 8,
        time_budget_factor: 30.0,
        launch_shell_fraction: 0.5,
        step_fraction: 0.3,
        ..TraceOptions::default()
    };

    println!("# electron confinement vs shield current (100 kV, 45 mm rings)");
    println!("amp_turns,enhancement,wall_frac,wire_frac,survivor_frac,gyroradius_mm,mean_time_ns");
    for &at in &[0.0, 80_000.0, 200_000.0, 400_000.0, 800_000.0, 1_200_000.0] {
        let device = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -100_000.0, at);
        let r = confinement(
            &device,
            ELECTRON,
            121,
            241,
            &SolveOptions::default(),
            &topts,
            48,
        )
        .expect("confinement");
        println!(
            "{:.0},{:.2},{:.3},{:.3},{:.3},{:.2},{:.3}",
            at,
            r.enhancement,
            r.wall_fraction,
            r.wire_fraction,
            r.survivor_fraction,
            r.gyroradius_mm,
            r.mean_time_s * 1e9
        );
    }
    println!(
        "\nread: enhancement = mean electron flight time (full field) / \
         (currents zeroed). ~1 = the cusp leaks electrons as fast as they \
         fall (a two-ring cusp is loss-dominated — the reason polywells use \
         six coils); >>1 = magnetic trapping is real. survivor_frac is the \
         population still confined at the flight budget. Non-relativistic \
         Boris; enhancement saturates at the budget (a floor)."
    );
}
