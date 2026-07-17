//! M5 honesty tables: grid convergence + a JEDEC-style θ_ja consistency
//! check.
//!
//! Table 1 — θ_ja and T_max of the hot_chip board across grid
//! resolutions. The coarsest grid cannot even represent the 10 mm die on
//! its 2 mm pitch (center-containment paints a 12 mm footprint) and the
//! table shows exactly what that costs, instead of hiding it. The floor
//! is set by source-footprint representation, not by the solver.
//!
//! Table 2 — a JESD51-7-shaped 2s2p board (76.2×114.3×1.6 mm, effective
//! in-plane k ≈ 20 from two buried planes, through-plane ≈ 0.4) with a
//! 9×9 mm 1 W die, swept over plausible still-air *combined*
//! convection+radiation coefficients. Datasheet θ_ja for 9–10 mm
//! exposed-pad packages on JEDEC 2s2p boards commonly lands in the
//! 20–30 K/W band; the model must land in that band at plausible h to
//! pass the smell test. This is a **consistency check, not a
//! validation** — h and the package model (absent here: the die couples
//! straight to the board, junction-to-board resistance unmodeled) are the
//! uncontrolled variables. Closing the loop is the M6 measurement pack.
//!
//! Run: `cargo run --release -p vcad-kernel-thermal --example convergence`

use vcad_kernel_thermal::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};

fn hot_chip(divisions: [usize; 3]) -> ThermalModel {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [100.0, 100.0, 2.6], divisions);
    m.materials.push(MaterialRegion::anisotropic(
        Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [100.0, 100.0, 1.6],
        },
        [15.0, 15.0, 0.5],
    ));
    m.materials.push(MaterialRegion::isotropic(
        Shape::Box {
            min_mm: [45.0, 45.0, 1.6],
            size_mm: [10.0, 10.0, 1.0],
        },
        120.0,
    ));
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: Shape::Box {
            min_mm: [45.0, 45.0, 1.6],
            size_mm: [10.0, 10.0, 1.0],
        },
        power_w: 2.0,
    });
    let conv = Boundary::Convection {
        h_w_m2k: 10.0,
        ambient_c: 25.0,
    };
    m.domain_faces[4] = conv;
    m.domain_faces[5] = conv;
    m.exposed = conv;
    m
}

fn jedec_2s2p(h: f64) -> ThermalModel {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [76.2, 114.3, 2.1], [76, 114, 8]);
    // 2s2p effective conductivity: ~75 µm net copper across 1.6 mm gives
    // in-plane ~20 W/m·K; through-plane stays dielectric-bound ~0.4.
    m.materials.push(MaterialRegion::anisotropic(
        Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [76.2, 114.3, 1.6],
        },
        [20.0, 20.0, 0.4],
    ));
    // 9×9 mm die, 0.5 mm, centered; couples straight to the board (no
    // package model — stated in the header).
    m.materials.push(MaterialRegion::isotropic(
        Shape::Box {
            min_mm: [33.6, 52.65, 1.6],
            size_mm: [9.0, 9.0, 0.5],
        },
        120.0,
    ));
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: Shape::Box {
            min_mm: [33.6, 52.65, 1.6],
            size_mm: [9.0, 9.0, 0.5],
        },
        power_w: 1.0,
    });
    let conv = Boundary::Convection {
        h_w_m2k: h,
        ambient_c: 25.0,
    };
    m.domain_faces[4] = conv;
    m.domain_faces[5] = conv;
    m.exposed = conv;
    m
}

fn main() {
    let opts = SolveOptions::default();

    println!("grid convergence: hot_chip (anisotropic board [15,15,0.5], h = 10, 2 W)\n");
    println!(
        "{:>16} {:>10} {:>10} {:>12} {:>10}",
        "grid", "pitch(mm)", "T_max(C)", "theta(K/W)", "CG iters"
    );
    let mut last_theta = f64::NAN;
    for div in [
        [25usize, 25, 13],
        [50, 50, 13],
        [100, 100, 13],
        [100, 100, 26],
        [200, 200, 26],
    ] {
        let m = hot_chip(div);
        let sol = solve_steady(&m, &opts).expect("solve");
        let theta = sol.sources[0].theta_c_per_w.expect("theta");
        let delta = if last_theta.is_finite() {
            format!("  ({:+.2}%)", 100.0 * (theta - last_theta) / last_theta)
        } else {
            String::new()
        };
        println!(
            "{:>16} {:>10.2} {:>10.2} {:>12.3} {:>10}{delta}",
            format!("{}x{}x{}", div[0], div[1], div[2]),
            100.0 / div[0] as f64,
            sol.t_max_c,
            theta,
            sol.iterations,
        );
        last_theta = theta;
    }
    println!(
        "\nfloor: the 2-4 mm pitches cannot represent the 10 mm die footprint\n\
         (center containment mis-paints it; theta jumps -5% then +13% as the\n\
         footprint snaps into place). Once the footprint is exact (<= 1 mm pitch)\n\
         theta drifts ~1.3% per further halving, still falling — quote theta_ja\n\
         from the 1 mm / 0.2 mm grid with a ~2% grid band, and say so on the\n\
         receipt.\n"
    );

    println!("JEDEC-style 2s2p consistency check: 76.2x114.3 mm, 9x9 mm 1 W die");
    println!("(published datasheet theta_ja for this class: ~20-30 K/W in still air)\n");
    println!(
        "{:>18} {:>10} {:>12} {:>10}",
        "h_eff (W/m2K)", "T_max(C)", "theta(K/W)", "in band?"
    );
    for h in [8.0, 10.0, 12.0, 15.0] {
        let m = jedec_2s2p(h);
        let sol = solve_steady(&m, &opts).expect("solve");
        let theta = sol.sources[0].theta_c_per_w.expect("theta");
        println!(
            "{h:>18.0} {:>10.2} {:>12.2} {:>10}",
            sol.t_max_c,
            theta,
            if (20.0..=30.0).contains(&theta) {
                "yes"
            } else {
                "no"
            }
        );
    }
    println!(
        "\nconsistency, not validation: h_eff bundles convection AND radiation (JEDEC\n\
         still-air chambers include both), and the package is absent (die couples\n\
         straight to the board; junction-to-board resistance ~1-3 K/W for exposed-pad\n\
         packages is unmodeled). Landing in-band at plausible h says the geometry and\n\
         copper bookkeeping are sane; only the M6 measurement pack can say more."
    );
}
