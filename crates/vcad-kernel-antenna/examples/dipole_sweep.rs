//! Frequency sweep of a concrete dipole: S11 and Z_in across
//! 0.7–1.3 × resonance, CSV on stdout, summary on stderr.
//!
//! The antenna: a center-fed dipole with two 0.5 m arms (1.0 m tip to
//! tip) of 1 mm-radius wire — a 2 m-band ham dipole, near enough. Run:
//!
//! ```text
//! cargo run --release -p vcad-kernel-antenna --example dipole_sweep > sweep.csv
//! ```

use vcad_kernel_antenna::constants::C0;
use vcad_kernel_antenna::farfield::{directivity_dbi, gain_dbi, radiation_efficiency};
use vcad_kernel_antenna::{find_resonance, solve_driven, sweep, Mesh, SolveOptions, WireGrid};

fn main() {
    let length_mm = 1000.0; // 2 × 0.5 m arms
    let radius_mm = 1.0;
    let nseg = 40;
    let z0 = 50.0;

    let mut grid = WireGrid::new();
    grid.add_wire(
        [0.0, 0.0, -length_mm / 2.0],
        [0.0, 0.0, length_mm / 2.0],
        radius_mm,
        nseg,
    )
    .expect("wire");
    let mesh = Mesh::build(&grid).expect("mesh");
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).expect("feed");
    let opts = SolveOptions::default();

    // Resonance: Im(Z) crosses zero slightly below the half-wave frequency.
    let f_half = C0 / (2.0 * length_mm * 1e-3);
    let f_res = find_resonance(&mesh, feed, 0.80 * f_half, 1.05 * f_half, &opts)
        .expect("resonance in bracket");

    let sol = solve_driven(&mesh, feed, f_res, &opts).expect("solve at resonance");
    let d_bdside = directivity_dbi(&mesh, &sol, std::f64::consts::FRAC_PI_2, 0.0, 32);
    let eff = radiation_efficiency(&mesh, &sol, 32);
    eprintln!("dipole: {length_mm} mm tip-to-tip, a = {radius_mm} mm, N = {nseg} segments");
    eprintln!(
        "resonance: {:.4} MHz  (l/lambda = {:.4})",
        f_res / 1e6,
        length_mm * 1e-3 * f_res / C0
    );
    eprintln!(
        "Z_in at resonance: {:.2} {:+.2}j ohm   S11 vs 50 ohm: {:.2} dB",
        sol.z_in.re,
        sol.z_in.im,
        vcad_kernel_antenna::s11_db(sol.z_in, z0)
    );
    eprintln!("broadside directivity: {d_bdside:.3} dBi   radiation efficiency: {eff:.4}");

    // Sweep 0.7–1.3 × resonance.
    let n_points = 61;
    let freqs: Vec<f64> = (0..n_points)
        .map(|i| f_res * (0.7 + 0.6 * i as f64 / (n_points - 1) as f64))
        .collect();
    let points = sweep(&mesh, feed, &freqs, z0, &opts).expect("sweep");

    println!("freq_mhz,r_ohm,x_ohm,s11_db,gain_broadside_dbi");
    for pt in &points {
        let sol = solve_driven(&mesh, feed, pt.freq_hz, &opts).expect("solve");
        let g = gain_dbi(&mesh, &sol, std::f64::consts::FRAC_PI_2, 0.0);
        println!(
            "{:.4},{:.4},{:.4},{:.4},{:.4}",
            pt.freq_hz / 1e6,
            pt.z_in.re,
            pt.z_in.im,
            pt.s11_db,
            g
        );
    }
}
