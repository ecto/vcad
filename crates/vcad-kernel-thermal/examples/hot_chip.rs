//! The M0 flagship: a 2 W die on a 100×100×1.6 mm board.
//!
//! Geometry: a 10×10×1 mm silicon-ish die (k = 120) sits centered on a
//! 100×100×1.6 mm plate with copper-plane-equivalent conductivity k = 15,
//! all of it convecting from both large faces. Prints T_max, θ_ja, and the
//! energy-balance residual across a sweep of film coefficients — the
//! sensitivity that matters, because h is the number nobody actually
//! knows.
//!
//! Honesty box (M0):
//! - k = 15 W/m·K isotropic is a copper-plane *in-plane* equivalent for a
//!   multilayer board; through-plane FR4 is ~0.3, so this model overstates
//!   vertical conduction. Anisotropy is M1. For this geometry the error is
//!   modest (the plate is thin; film resistance dominates) but it is real.
//! - h is supplied, not derived. 5–10 ≈ still air, 10–30 ≈ gentle airflow.
//! - No radiation: at ~60 °C surface / 25 °C ambient an ε ≈ 0.9 board
//!   radiates with an effective h_rad ≈ 6 W/m²K — the same order as
//!   natural convection. A bare M0 prediction with small h therefore runs
//!   hot vs reality; treat h as the *combined* film + radiation
//!   coefficient until radiation lands as a milestone.
//!
//! Run: `cargo run --release -p vcad-kernel-thermal --example hot_chip`
//! Add `--json` to dump the top-surface temperature map (what a thermal
//! camera would see) for the h = 10 case.

use vcad_kernel_thermal::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
use vcad_kernel_thermal::solve::{solve_steady, Solution, SolveOptions};

fn build(h: f64) -> ThermalModel {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [100.0, 100.0, 2.6], [100, 100, 13]);
    // Board: copper-plane-equivalent, isotropic at M0 (see honesty box).
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [100.0, 100.0, 1.6],
        },
        conductivity_w_mk: 15.0,
    });
    // Die on top, centered.
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [45.0, 45.0, 1.6],
            size_mm: [10.0, 10.0, 1.0],
        },
        conductivity_w_mk: 120.0,
    });
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: Shape::Box {
            min_mm: [45.0, 45.0, 1.6],
            size_mm: [10.0, 10.0, 1.0],
        },
        power_w: 2.0,
    });
    let conv = Boundary::Convection {
        h_w_m2k: h,
        ambient_c: 25.0,
    };
    // Bottom of the board and top of the die are domain faces; the exposed
    // board top (around the die) and the die sidewalls face void voxels
    // and convect through the `exposed` rule. Board edges: adiabatic
    // (0.64 cm² of edge vs 200 cm² of face — stated, not hidden).
    m.domain_faces[4] = conv;
    m.domain_faces[5] = conv;
    m.exposed = conv;
    m
}

/// Top-surface temperature map: for each column the temperature of its
/// topmost solid voxel — what a thermal camera pointed at +z sees.
fn surface_map(sol: &Solution) -> Vec<Vec<f64>> {
    let [nx, ny, nz] = sol.divisions;
    let mut rows = Vec::with_capacity(ny);
    for j in 0..ny {
        let mut row = Vec::with_capacity(nx);
        for i in 0..nx {
            let mut t = f64::NAN;
            for k in (0..nz).rev() {
                let v = sol.temperature_c(i, j, k);
                if !v.is_nan() {
                    t = v;
                    break;
                }
            }
            row.push(t);
        }
        rows.push(row);
    }
    rows
}

fn main() {
    let json = std::env::args().any(|a| a == "--json");
    let opts = SolveOptions::default();

    if json {
        let sol = solve_steady(&build(10.0), &opts).expect("solve");
        let map = surface_map(&sol);
        let cells: Vec<String> = map
            .iter()
            .map(|row| {
                let vals: Vec<String> = row
                    .iter()
                    .map(|t| {
                        if t.is_nan() {
                            "null".to_string()
                        } else {
                            format!("{t:.3}")
                        }
                    })
                    .collect();
                format!("[{}]", vals.join(","))
            })
            .collect();
        println!(
            "{{\"h_w_m2k\":10.0,\"ambient_c\":25.0,\"nx\":{},\"ny\":{},\"pitch_mm\":1.0,\
             \"t_max_c\":{:.3},\"t_max_at_mm\":[{:.1},{:.1},{:.1}],\"theta_ja_c_per_w\":{:.3},\
             \"energy_residual_rel\":{:.3e},\"t_surface_c\":[{}]}}",
            sol.divisions[0],
            sol.divisions[1],
            sol.t_max_c,
            sol.t_max_at_mm[0],
            sol.t_max_at_mm[1],
            sol.t_max_at_mm[2],
            sol.sources[0].theta_c_per_w.expect("theta"),
            sol.energy.residual_rel,
            cells.join(",")
        );
        return;
    }

    println!("hot_chip: 2 W, 10x10x1 mm die (k=120) on 100x100x1.6 mm board (k=15, isotropic)");
    println!("convection h on both faces, ambient 25 C; board edges adiabatic; no radiation\n");
    println!(
        "{:>10} {:>10} {:>12} {:>12} {:>10} {:>8}",
        "h (W/m2K)", "T_max (C)", "theta (K/W)", "balance", "CG iters", "resid"
    );
    for h in [5.0, 10.0, 15.0, 20.0, 30.0] {
        let sol = solve_steady(&build(h), &opts).expect("solve");
        let theta = sol.sources[0].theta_c_per_w.expect("theta");
        println!(
            "{h:>10.0} {:>10.2} {:>12.2} {:>12.2e} {:>10} {:>8.1e}",
            sol.t_max_c, theta, sol.energy.residual_rel, sol.iterations, sol.residual_rel
        );
    }
    println!(
        "\ntheta_ja = (T_die,max - 25 C) / 2 W. h is supplied, not derived: quote every\n\
         prediction with the h it was priced at. Radiation (~6 W/m2K equivalent at these\n\
         temperatures) is NOT included — fold it into h or wait for the radiation milestone."
    );
}
