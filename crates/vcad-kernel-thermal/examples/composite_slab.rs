//! Composite-slab validation, printable: the harmonic-mean face proof.
//!
//! Two layers in series — 20 mm of k = 200 against 10 mm of k = 1 — with
//! the outer faces pinned at 100 °C and 0 °C. The finite-volume solution
//! with harmonic-mean face conductances reproduces the series-resistance
//! closed form *exactly at voxel centers*; an arithmetic-mean face would
//! read the interface as ~100× more conductive than it is.
//!
//! Run: `cargo run -p vcad-kernel-thermal --example composite_slab`
//! (`--json` emits computed-vs-exact pairs for plotting.)

use vcad_kernel_thermal::model::{Boundary, MaterialRegion, Shape, ThermalModel};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};

fn main() {
    let json = std::env::args().any(|a| a == "--json");
    let (k1, k2) = (200.0, 1.0);
    let (l1, l2) = (0.020, 0.010);
    let nx = 12;
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [30.0, 10.0, 10.0], [nx, 1, 1]);
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [20.0, 10.0, 10.0],
        },
        conductivity_w_mk: k1,
    });
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [20.0, 0.0, 0.0],
            size_mm: [10.0, 10.0, 10.0],
        },
        conductivity_w_mk: k2,
    });
    m.domain_faces[0] = Boundary::FixedTemperature {
        temperature_c: 100.0,
    };
    m.domain_faces[1] = Boundary::FixedTemperature { temperature_c: 0.0 };
    let sol = solve_steady(&m, &SolveOptions::default()).expect("solve");

    let q = 100.0 / (l1 / k1 + l2 / k2);
    let dx = 0.030 / nx as f64;
    let exact_at = |x: f64| {
        if x <= l1 {
            100.0 - q * x / k1
        } else {
            100.0 - q * l1 / k1 - q * (x - l1) / k2
        }
    };

    if json {
        let pts: Vec<String> = (0..nx)
            .map(|i| {
                let x = (i as f64 + 0.5) * dx;
                format!(
                    "{{\"x_mm\":{:.4},\"computed_c\":{:.9},\"exact_c\":{:.9}}}",
                    x * 1e3,
                    sol.temperature_c(i, 0, 0),
                    exact_at(x)
                )
            })
            .collect();
        println!(
            "{{\"q_w_m2\":{q:.6},\"k1\":{k1},\"k2\":{k2},\"l1_mm\":20.0,\"l2_mm\":10.0,\
             \"points\":[{}]}}",
            pts.join(",")
        );
        return;
    }

    println!("composite slab: 20 mm k=200 | 10 mm k=1, faces at 100 C / 0 C");
    println!("series-resistance flux q = {q:.3} W/m2\n");
    println!(
        "{:>8} {:>16} {:>16} {:>12}",
        "x (mm)", "computed (C)", "exact (C)", "error"
    );
    for i in 0..nx {
        let x = (i as f64 + 0.5) * dx;
        let got = sol.temperature_c(i, 0, 0);
        let exact = exact_at(x);
        println!(
            "{:>8.2} {:>16.9} {:>16.9} {:>12.2e}",
            x * 1e3,
            got,
            exact,
            (got - exact).abs()
        );
    }
}
