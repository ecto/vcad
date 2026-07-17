//! Elevation-cut gain pattern of the resonant 1 m dipole: CSV of
//! `theta_deg,gain_dbi` in the φ = 0 plane (azimuthally symmetric, so this
//! is the whole story).
//!
//! ```text
//! cargo run --release -p vcad-kernel-antenna --example pattern_cut > pattern.csv
//! ```

use vcad_kernel_antenna::constants::C0;
use vcad_kernel_antenna::farfield::{gain_dbi, radiation_efficiency};
use vcad_kernel_antenna::{find_resonance, solve_driven, Mesh, SolveOptions, WireGrid};

fn main() {
    let mut grid = WireGrid::new();
    grid.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 40)
        .expect("wire");
    let mesh = Mesh::build(&grid).expect("mesh");
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).expect("feed");
    let opts = SolveOptions::default();

    let f_half = C0 / 2.0;
    let f_res =
        find_resonance(&mesh, feed, 0.80 * f_half, 1.05 * f_half, &opts).expect("resonance");
    let sol = solve_driven(&mesh, feed, f_res, &opts).expect("solve");

    eprintln!(
        "resonance {:.4} MHz, efficiency {:.4}",
        f_res / 1e6,
        radiation_efficiency(&mesh, &sol, 32)
    );
    println!("theta_deg,gain_dbi");
    for i in 0..=180 {
        let theta = (i as f64).to_radians();
        // Clamp poles slightly off-axis: the exact null has -inf dBi.
        let th = theta.clamp(0.002, std::f64::consts::PI - 0.002);
        println!("{},{:.4}", i, gain_dbi(&mesh, &sol, th, 0.0));
    }
}
