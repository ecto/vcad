//! 90° waveguide bend loss vs radius — the M1 demo.
//!
//! A Si/SiO₂-like slab guide (n = 3.48 / 1.44, w = 0.22) turned through a
//! quarter circle: the fundamental mode is injected unidirectionally via
//! the TF/SF plane, and transmission is the ratio of the flux on the
//! vertical output arm to the flux on the horizontal input arm. Sharper
//! bends radiate more; the table quantifies it.
//!
//! Run: `cargo run --release -p vcad-kernel-photonics --example bend_loss`

use vcad_kernel_photonics::{
    solve_slab_mode_even, CpmlSpec, FluxSpec, GridSpec, Polarization, Shape2, Simulation, Source,
    Waveform,
};

fn main() {
    println!("vcad-kernel-photonics M1 — 90° bend loss vs radius");
    println!("units: c = ε₀ = μ₀ = 1; lengths in µm by convention; f = 1/λ\n");

    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 40.0;
    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm)
        .expect("guiding geometry");
    println!(
        "mode: n_eff = {:.5}, guide w = {:.2}, resolution λ₀/40\n",
        mode.n_eff,
        2.0 * half_w
    );

    println!("  R (units)   R/w     T = P_out/P_in   loss (dB)");
    for &radius in &[0.5, 1.0, 2.0, 3.0] {
        let t = bend_transmission(radius, mode.n_eff, n_core, n_clad, half_w, delta, f0);
        let db = -10.0 * t.log10();
        println!(
            "  {radius:.2}        {:>4.1}    {t:.4}           {db:.3}",
            radius / (2.0 * half_w)
        );
    }
    println!("\nLoss → 0 as R grows; the residual floor at large R is the");
    println!("monitor/injection discretization, not physics.");
}

fn bend_transmission(
    radius: f64,
    n_eff: f64,
    n_core: f64,
    n_clad: f64,
    half_w: f64,
    delta: f64,
    f0: f64,
) -> f64 {
    let jc = 34usize;
    let yc = jc as f64 * delta;
    let xb = 60.0 * delta;
    let r_cells = (radius / delta).round() as usize;
    let nx = 60 + r_cells + 40;
    let ny = jc + r_cells + 52;

    let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform(12));
    sim.fill_epsilon(n_clad * n_clad);
    sim.paint(
        &Shape2::rect(-1.0, yc - half_w, xb, yc + half_w),
        n_core * n_core,
    );
    sim.paint(
        &Shape2::ring(
            xb,
            yc + radius,
            radius - half_w,
            radius + half_w,
            -std::f64::consts::FRAC_PI_2,
            0.0,
            96,
        ),
        n_core * n_core,
    );
    sim.paint(
        &Shape2::rect(xb + radius - half_w, yc + radius, xb + radius + half_w, 1e9),
        n_core * n_core,
    );

    let (j0, j1) = (jc - 20, jc + 20);
    let profile: Vec<f64> = (j0..=j1)
        .map(|j| {
            let m =
                solve_slab_mode_even(n_core, n_clad, half_w, 1.0 / f0, Polarization::Tm).unwrap();
            m.profile((j as f64 - jc as f64) * delta)
        })
        .collect();
    sim.add_source(Source::mode_tfsf(
        24,
        j0,
        profile,
        n_eff,
        Waveform::gaussian(f0, f0 / 4.0),
    ));
    let f_in = sim.add_flux(FluxSpec::Vertical {
        i: 42,
        j0,
        j1,
        freqs: vec![f0],
    });
    let ic = ((xb + radius) / delta).round() as usize;
    let j_out = jc + r_cells + 26;
    let f_out = sim.add_flux(FluxSpec::Horizontal {
        j: j_out,
        i0: ic - 20,
        i1: ic + 20,
        freqs: vec![f0],
    });

    // Enough time for the pulse to clear source → bend → output monitor.
    let path = (nx + ny) as f64 * delta;
    let steps = ((path * 4.5 + 14.0) / sim.dt()).ceil() as usize;
    sim.run(steps);
    sim.flux_power(f_out)[0].1 / sim.flux_power(f_in)[0].1
}
