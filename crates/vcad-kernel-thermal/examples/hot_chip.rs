//! The M0 flagship: a 2 W die on a 100×100×1.6 mm board.
//!
//! Geometry: a 10×10×1 mm silicon-ish die (k = 120) sits centered on a
//! 100×100×1.6 mm plate with copper-plane-equivalent conductivity k = 15,
//! all of it convecting from both large faces. Prints T_max, θ_ja, and the
//! energy-balance residual across a sweep of film coefficients — the
//! sensitivity that matters, because h is the number nobody actually
//! knows.
//!
//! Honesty box:
//! - k = 15 W/m·K isotropic is a copper-plane *in-plane* equivalent for a
//!   multilayer board; through-plane FR4 is ~0.3–0.5. M0 guessed the
//!   isotropic error was "modest"; the M1 anisotropy table below *measures*
//!   it: 56.5 °C isotropic vs 69.9 °C with the real split — a 6.7 K/W θ
//!   gap. Lesson kept: don't adjective an error you can compute.
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
use vcad_kernel_thermal::transient::{solve_transient, TransientOptions};

fn build(h: f64, board_k: [f64; 3]) -> ThermalModel {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [100.0, 100.0, 2.6], [100, 100, 13]);
    // Board: copper-plane-equivalent (isotropic or in-plane/through-plane
    // split — see honesty box). FR4+copper volumetric heat capacity
    // ~2.0e6 J/m3K for the transient run.
    m.materials.push(
        MaterialRegion::anisotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [100.0, 100.0, 1.6],
            },
            board_k,
        )
        .with_heat_capacity(2.0e6),
    );
    // Die on top, centered. Silicon: rho*c ~ 1.66e6 J/m3K.
    m.materials.push(
        MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [45.0, 45.0, 1.6],
                size_mm: [10.0, 10.0, 1.0],
            },
            120.0,
        )
        .with_heat_capacity(1.66e6),
    );
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
        let sol = solve_steady(&build(10.0, [15.0; 3]), &opts).expect("solve");
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
        let sol = solve_steady(&build(h, [15.0; 3]), &opts).expect("solve");
        let theta = sol.sources[0].theta_c_per_w.expect("theta");
        println!(
            "{h:>10.0} {:>10.2} {:>12.2} {:>12.2e} {:>10} {:>8.1e}",
            sol.t_max_c, theta, sol.energy.residual_rel, sol.iterations, sol.residual_rel
        );
    }

    // The anisotropy question at h = 10: same board with the in-plane /
    // through-plane split a real 4-layer stackup has, vs the isotropic
    // idealization. Through-plane k = 0.5 puts the 1.6 mm of dielectric
    // in series with the bottom-face path — the M0 isotropic model hides
    // that resistance.
    println!("\nanisotropy at h = 10 (M1): board k [in-plane, in-plane, through-plane]");
    for (label, bk) in [
        ("isotropic [15,15,15]", [15.0, 15.0, 15.0]),
        ("real-ish  [15,15,0.5]", [15.0, 15.0, 0.5]),
        ("bare FR4  [0.3,0.3,0.3]", [0.3, 0.3, 0.3]),
    ] {
        let sol = solve_steady(&build(10.0, bk), &opts).expect("solve");
        let theta = sol.sources[0].theta_c_per_w.expect("theta");
        println!(
            "  {label:<24} T_max {:>7.2} C   theta {:>6.2} K/W",
            sol.t_max_c, theta
        );
    }

    // Step response (M1): power on at t = 0 from a 25 C soak, h = 10.
    // How long until the die is within 1 K of steady?
    let steady = solve_steady(&build(10.0, [15.0, 15.0, 0.5]), &opts).expect("steady");
    let trans = solve_transient(
        &build(10.0, [15.0, 15.0, 0.5]),
        &opts,
        &TransientOptions {
            dt_s: 5.0,
            steps: 240,
            initial_c: 25.0,
            snapshot_every: 0,
        },
    )
    .expect("transient");
    let settle = trans
        .times_s
        .iter()
        .zip(&trans.t_max_c)
        .find(|(_, &t)| t > steady.t_max_c - 1.0)
        .map(|(&s, _)| s);
    println!(
        "\nstep response (anisotropic board, h = 10): T_max {:.2} -> {:.2} C over {:.0} s",
        trans.t_max_c[0],
        trans.t_max_c.last().unwrap(),
        trans.times_s.last().unwrap()
    );
    match settle {
        Some(s) => println!(
            "  within 1 K of steady after ~{s:.0} s; energy audit residual {:.1e}",
            trans.energy_audit_residual_rel
        ),
        None => println!(
            "  not settled within the run ({:.1e} audit residual)",
            trans.energy_audit_residual_rel
        ),
    }

    println!(
        "\ntheta_ja = (T_die,max - 25 C) / 2 W. h is supplied, not derived: quote every\n\
         prediction with the h it was priced at. Radiation (~6 W/m2K equivalent at these\n\
         temperatures) is NOT included — fold it into h or wait for the radiation milestone."
    );
}
