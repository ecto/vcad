//! POPS flagship: shape the well toward harmonic by electrode design.
//!
//! POPS coherent compression needs a harmonic well (bounce frequency
//! independent of amplitude). A stock IEC well is anharmonic — it flattens
//! away from center, so big-amplitude ions bounce slower and the cloud
//! decoheres under a single drive. This example measures that, then lets
//! the optimizer reshape the electrodes to maximize harmonicity — "design
//! a quadratic well," a request only a differentiable field-solver can act
//! on.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example pops_harmonic_well`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::optimize::{maximize, FdOptions};
use vcad_kernel_particle::poisson::SolveOptions;
use vcad_kernel_particle::pops::harmonicity;
use vcad_kernel_particle::trace::TraceOptions;

// A three-electrode axisymmetric well: the cathode ring pair plus a
// guard ring pair further out whose potential is the shaping knob. Two
// free parameters — cathode ring radius and guard potential fraction —
// give the optimizer room to flatten the anharmonicity.
fn device_from(ring_r_mm: f64, guard_frac: f64) -> Device {
    let cathode_v = -30_000.0;
    let mut d = Device::shielded_two_ring(150.0, ring_r_mm, 25.0, 3.0, cathode_v, 0.0);
    // Guard rings at 1.7× the cathode radius, ±40 mm, biased to a fraction
    // of the cathode — a positive fraction convexifies the well.
    for &z in &[40.0, -40.0] {
        d.rings.push(vcad_kernel_particle::device::WireRing {
            ring_radius_mm: ring_r_mm * 1.7,
            z_mm: z,
            wire_radius_mm: 3.0,
            potential_v: cathode_v * guard_frac,
            ampere_turns: 0.0,
        });
    }
    d
}

fn measure(ring_r_mm: f64, guard_frac: f64) -> f64 {
    let device = device_from(ring_r_mm, guard_frac);
    harmonicity(
        &device,
        61,
        121,
        &SolveOptions::default(),
        &TraceOptions {
            max_passes: 16,
            time_budget_factor: 25.0,
            ..TraceOptions::default()
        },
        &[0.2, 0.35, 0.5, 0.65, 0.8],
    )
    .map(|r| r.harmonicity)
    .unwrap_or(0.0)
}

fn main() {
    let base_r = 45.0;
    let baseline = measure(base_r, 0.0);
    println!("# POPS harmonic-well inverse design");
    println!("baseline (bare cathode, {base_r:.0} mm rings): harmonicity {baseline:.3}");

    let mut evals = 0usize;
    let mut objective = |x: &[f64]| {
        evals += 1;
        measure(x[0], x[1])
    };
    let result = maximize(
        &mut objective,
        &[base_r, 0.2],
        &[30.0, 0.0],
        &[70.0, 0.9],
        &FdOptions {
            max_iters: 10,
            ..FdOptions::default()
        },
    );

    println!(
        "optimized: ring radius {:.1} mm, guard {:.2}× cathode -> harmonicity {:.3} ({} evals)",
        result.x[0], result.x[1], result.value, evals
    );
    println!(
        "improvement: {:.3} -> {:.3} ({:+.0}%)",
        baseline,
        result.value,
        100.0 * (result.value - baseline) / baseline.max(1e-9)
    );

    // Report the frequency droop at both ends, the physical POPS signal.
    for (label, r, g) in [
        ("baseline", base_r, 0.0),
        ("optimized", result.x[0], result.x[1]),
    ] {
        let device = device_from(r, g);
        if let Ok(rep) = harmonicity(
            &device,
            61,
            121,
            &SolveOptions::default(),
            &TraceOptions {
                max_passes: 16,
                time_budget_factor: 25.0,
                ..TraceOptions::default()
            },
            &[0.2, 0.35, 0.5, 0.65, 0.8],
        ) {
            println!(
                "  {label}: bounce-frequency droop small->large amplitude = {:+.1}% \
                 ({} amplitudes clean)",
                100.0 * rep.droop,
                rep.frequencies.len()
            );
        }
    }

    println!(
        "\nread: harmonicity 1.0 = every ion bounces at one frequency (POPS-ready, \
         phase-lockable); lower = the cloud decoheres under a single drive. The \
         optimizer is shaping the vacuum well; space-charge flattening (unmodeled \
         here) is the next anharmonicity to fight. Perfect-well upper bound."
    );
}
