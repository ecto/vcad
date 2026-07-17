//! The M5 convergence study: how each error class shrinks with the
//! grid, against exact references.
//!
//! Two problems, two error regimes:
//!
//! 1. **Smooth, grid-aligned** — the Neumann-bounded infinite solenoid
//!    (exact: Ampère's law incl. the winding term). Expected O(h²).
//! 2. **Curved, staircased** — a permeable cylindrical shell in a
//!    uniform transverse field, against the published closed form
//!    (Jackson, *Classical Electrodynamics*, 3rd ed., problem 5.14):
//!    `H_in/H₀ = 4μ·b² / ((μ+1)²·b² − (μ−1)²·a²)`.
//!    Curved μ interfaces staircase at cell resolution — expected O(h),
//!    the honest floor every curved-geometry claim must carry.
//!
//! Run: `cargo run --release -p vcad-kernel-em --example convergence`

use vcad_kernel_em::analytic;
use vcad_kernel_em::axisym::{Annulus, AxisymMagnetostatics, Coil};
use vcad_kernel_em::constants::MU_0;
use vcad_kernel_em::grid::{Bc, SolveOptions};
use vcad_kernel_em::planar::{Conductor, PlanarMagnetostatics, Rect, RingMaterial};

fn solenoid_exact_l_per_m() -> f64 {
    let bore = analytic::solenoid_inductance_per_m(10_000.0, 0.020, 0.0, 1.0);
    let t: f64 = 0.002;
    let m = 4000;
    let mut wind = 0.0;
    for p in 0..m {
        let r = 0.020 + (p as f64 + 0.5) * t / m as f64;
        let h = (0.022 - r) / t;
        wind += h * h * 2.0 * std::f64::consts::PI * r * (t / m as f64);
    }
    bore + MU_0 * 1e8 * wind
}

fn main() {
    let opts = SolveOptions::default();

    println!("== 1. infinite solenoid (exact anchor, smooth): expect O(h²) ==");
    println!("   h [mm]   rel err L      order");
    let expect = solenoid_exact_l_per_m();
    let mut last: Option<(f64, f64)> = None;
    for nr in [21usize, 41, 81, 161, 321] {
        let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 100.0);
        dev.bc_r_outer = Bc::Neumann;
        dev.bc_z_low = Bc::Neumann;
        dev.bc_z_high = Bc::Neumann;
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 22.0,
                z_min_mm: 0.0,
                z_max_mm: 100.0,
            },
            turns: 1000.0,
            current_a: 1.0,
        });
        let sol = dev.solve(nr, 11, &opts).unwrap();
        let l = 2.0 * sol.energy().source / 0.1;
        let h = 40.0 / (nr - 1) as f64;
        let err = (l - expect).abs() / expect;
        let order = last
            .map(|(h0, e0)| (e0 / err).ln() / (h0 / h).ln())
            .map(|o| format!("{o:.2}"))
            .unwrap_or_else(|| "—".into());
        println!("   {h:6.3}   {err:.4e}     {order}");
        last = Some((h, err));
    }

    println!();
    println!("== 2. cylindrical shell shielding (Jackson 5.14, staircased): expect ~O(h) ==");
    // Shell a = 8 mm, b = 10 mm, μ_r = 100 in a uniform transverse field:
    // H_in/H0 = 4μb²/((μ+1)²b² − (μ−1)²a²).
    let (a, b, mu) = (8.0_f64, 10.0_f64, 100.0_f64);
    let shield_exact = 4.0 * mu * b * b / ((mu + 1.0).powi(2) * b * b - (mu - 1.0).powi(2) * a * a);
    println!("   exact interior/applied field ratio: {shield_exact:.5}");
    println!("   h [mm]   ratio      rel err    order");
    let mut last: Option<(f64, f64)> = None;
    for n in [41usize, 81, 161, 321] {
        // Uniform B_x between a wide sheet pair (the M0 machinery), the
        // shell centered between them.
        let mut dev = PlanarMagnetostatics::new(0.0, 80.0, 0.0, 80.0);
        dev.bc_x_low = Bc::Neumann;
        dev.bc_x_high = Bc::Neumann;
        dev.bc_y_low = Bc::Zero;
        dev.bc_y_high = Bc::Neumann;
        let sheet_i = 100.0;
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 80.0,
                y_min_mm: 6.0,
                y_max_mm: 8.0,
            },
            total_current_a: sheet_i,
        });
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 80.0,
                y_min_mm: 72.0,
                y_max_mm: 74.0,
            },
            total_current_a: -sheet_i,
        });
        dev.rings.push(RingMaterial {
            cx_mm: 40.0,
            cy_mm: 40.0,
            r_inner_mm: a,
            r_outer_mm: b,
            mu_r: mu,
        });
        let sol = dev.solve(n, n, &opts).unwrap();
        // Applied field: sampled far from the shell (near the wall,
        // mid-height); interior field at the shell center.
        let (bx_app, _) = sol.b_at(0.002, 0.040);
        let (bx_in, _) = sol.b_at(0.040, 0.040);
        let ratio = bx_in / bx_app;
        let err = (ratio - shield_exact).abs() / shield_exact;
        let h = 80.0 / (n - 1) as f64;
        let order = last
            .map(|(h0, e0)| (e0 / err).ln() / (h0 / h).ln())
            .map(|o| format!("{o:.2}"))
            .unwrap_or_else(|| "—".into());
        println!("   {h:6.3}   {ratio:.5}    {err:.3e}   {order}");
        last = Some((h, err));
    }
    println!();
    println!("Reading: the smooth anchor rides h² all the way down; the");
    println!("staircased shell converges at first order — any claim on curved");
    println!("geometry must be bracketed by exactly this kind of study.");
}
