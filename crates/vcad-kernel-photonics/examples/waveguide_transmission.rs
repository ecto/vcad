//! Straight slab-waveguide transmission spectrum — the M0 demo.
//!
//! A Si/SiO₂-like slab guide (n = 3.48 / 1.44, width 0.22) at telecom
//! wavelengths: the eigenmode line source injects the fundamental TM
//! (out-of-plane E) mode, two flux monitors bracket 3.1 length units of
//! guide, and the mode solver supplies the analytic effective index at
//! each wavelength. A lossless straight guide must transmit ≈ 1.
//!
//! Run: `cargo run --release -p vcad-kernel-photonics --example waveguide_transmission`

use vcad_kernel_photonics::{
    dft_of_series, solve_slab_mode_even, CpmlSpec, FluxSpec, GridSpec, Polarization, Shape2,
    Simulation, Source, Waveform,
};

fn main() {
    println!("vcad-kernel-photonics M0 — straight waveguide transmission");
    println!("units: c = ε₀ = μ₀ = 1; lengths in µm by convention; f = 1/λ\n");

    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 40.0;
    let (nx, ny) = (170, 70);
    let jc = 35usize;
    let yc = jc as f64 * delta;

    let lambdas = [1.45, 1.50, 1.55, 1.60, 1.65];
    let freqs: Vec<f64> = lambdas.iter().map(|l| 1.0 / l).collect();

    let mode0 = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm)
        .expect("guiding geometry");
    println!(
        "slab mode @ λ = {lambda0}: n_eff = {:.6} (V = {:.4}, residual {:.1e})",
        mode0.n_eff,
        mode0.v_number(),
        mode0.residual
    );

    let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform(12));
    sim.fill_epsilon(n_clad * n_clad);
    sim.paint(
        &Shape2::rect(-1.0, yc - half_w, 1e9, yc + half_w),
        n_core * n_core,
    );

    let (j0, j1) = (14usize, 56usize);
    let profile: Vec<f64> = (j0..=j1)
        .map(|j| mode0.profile((j as f64 - jc as f64) * delta))
        .collect();
    sim.add_source(Source::line_profile(
        25,
        j0,
        profile,
        Waveform::gaussian(f0, f0 / 4.0),
    ));

    let f_in = sim.add_flux(FluxSpec::Vertical {
        i: 60,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let f_out = sim.add_flux(FluxSpec::Vertical {
        i: 140,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let p1 = sim.add_probe(60, jc);
    let p2 = sim.add_probe(140, jc);

    let steps = 3200;
    sim.run(steps);
    println!(
        "grid {nx}×{ny} @ Δ = λ₀/40, Courant {}, {} steps, t = {:.1}\n",
        sim.courant(),
        steps,
        sim.time()
    );

    let p_in = sim.flux_power(f_in);
    let p_out = sim.flux_power(f_out);
    let l = (140 - 60) as f64 * delta;
    let dt = sim.dt();

    println!("  λ        f        n_eff(theory)  n_eff(FDTD)  T = P_out/P_in");
    for (k, &lambda) in lambdas.iter().enumerate() {
        let f = freqs[k];
        let m = solve_slab_mode_even(n_core, n_clad, half_w, lambda, Polarization::Tm).unwrap();
        let e1 = dft_of_series(sim.probe_series(p1), dt, sim.probe_sample_time(0), f);
        let e2 = dft_of_series(sim.probe_series(p2), dt, sim.probe_sample_time(0), f);
        let k0 = 2.0 * std::f64::consts::PI * f;
        let two_pi = 2.0 * std::f64::consts::PI;
        let dphi_raw = e2.arg() - e1.arg();
        let expected = m.n_eff * k0 * l;
        let dphi = dphi_raw + two_pi * ((expected - dphi_raw) / two_pi).round();
        let neff_fdtd = dphi / (k0 * l);
        let t = p_out[k].1 / p_in[k].1;
        println!(
            "  {lambda:.2}     {f:.4}   {:.4}         {neff_fdtd:.4}       {t:.4}",
            m.n_eff
        );
    }
    println!("\nA lossless straight guide transmits ≈ 1; deviations are the");
    println!("monitor/injection discretization floor at this resolution.");
}
