//! Let the optimizer design the shielded cathode.
//!
//! Two free parameters — shield ampere-turns and ring axial spacing — and
//! one objective: predicted D-D neutron yield per injected ion at fixed
//! −30 kV bias. This is the design question the M0 sweep surfaced: the
//! shield helps until the cusp starts reflecting ions off the core, so
//! somewhere in between is an optimum. Find it by gradient ascent instead
//! of squinting at sweep tables.
//!
//! Run: `cargo run --release -p vcad-kernel-particle --example optimize_shield`

use vcad_kernel_particle::device::Device;
use vcad_kernel_particle::field::FieldMap;
use vcad_kernel_particle::fom::{neutron_rate_per_s, stats};
use vcad_kernel_particle::optimize::{maximize, FdOptions};
use vcad_kernel_particle::poisson::{solve, SolveOptions};
use vcad_kernel_particle::trace::{TraceOptions, Tracer, DEUTERON};
use vcad_kernel_particle::xsection::d2_deuteron_density_m3;

const BIAS_V: f64 = -30_000.0;

fn yield_per_ion(ampere_turns: f64, ring_z_mm: f64) -> f64 {
    let device = Device::shielded_two_ring(150.0, 45.0, ring_z_mm, 3.0, BIAS_V, ampere_turns);
    let Ok(sol) = solve(&device, 101, 201, &SolveOptions::default()) else {
        return 0.0;
    };
    let fields = FieldMap::new(&device, &sol);
    let opts = TraceOptions {
        max_passes: 25,
        ..TraceOptions::default()
    };
    let tracer = Tracer::new(&device, &fields, &sol, opts);
    let outcomes = tracer.launch_ensemble(DEUTERON, 64);
    stats(&outcomes).mean_ddn_sigma_v_m3
}

fn main() {
    // The yield landscape is multimodal (a recirculation hill at low
    // current, an energy-quality hill at high current where ions reach the
    // core at full speed through a well-shielded grid), so gradient ascent
    // gets a few independent starts and the best basin wins.
    let seeds: [[f64; 2]; 3] = [[20_000.0, 25.0], [80_000.0, 25.0], [200_000.0, 30.0]];
    let mut evals = 0usize;
    let mut best_seen = 0.0f64;
    let mut objective = |x: &[f64]| {
        let v = yield_per_ion(x[0], x[1]);
        evals += 1;
        if v > best_seen {
            best_seen = v;
            eprintln!(
                "# eval {evals}: A-turns {:.0}, ring z ±{:.1} mm -> sigma_v {:.3e} m^3 (new best)",
                x[0], x[1], v
            );
        }
        v
    };

    let mut result = None;
    for seed in &seeds {
        eprintln!(
            "# start from A-turns {:.0}, ring z ±{:.1} mm",
            seed[0], seed[1]
        );
        let r = maximize(
            &mut objective,
            seed,
            &[0.0, 15.0],
            &[300_000.0, 40.0],
            &FdOptions {
                max_iters: 8,
                ..FdOptions::default()
            },
        );
        let better = result
            .as_ref()
            .map(|b: &vcad_kernel_particle::optimize::FdResult| r.value > b.value)
            .unwrap_or(true);
        if better {
            result = Some(r);
        }
    }
    let result = result.expect("at least one start");

    let n_d = d2_deuteron_density_m3(2.0, 300.0);
    let baseline = yield_per_ion(0.0, 25.0);
    println!("bias_v,{BIAS_V}");
    println!("best_ampere_turns,{:.0}", result.x[0]);
    println!("best_ring_z_mm,{:.2}", result.x[1]);
    println!("best_sigma_v_m3,{:.4e}", result.value);
    println!("unshielded_sigma_v_m3,{:.4e}", baseline);
    println!(
        "yield_gain_vs_unshielded,{:.2}",
        result.value / baseline.max(1e-300)
    );
    println!(
        "pred_n_per_s_at_10mA_2mTorr,{:.3e}",
        neutron_rate_per_s(result.value, 0.010, n_d)
    );
    println!("objective_evals,{}", result.evals);
    println!(
        "history,{}",
        result
            .history
            .iter()
            .map(|v| format!("{v:.3e}"))
            .collect::<Vec<_>>()
            .join(";")
    );
}
