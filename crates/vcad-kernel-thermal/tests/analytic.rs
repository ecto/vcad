//! The M0 validation ladder: every rung is a closed-form solution from
//! Incropera & DeWitt (Fundamentals of Heat and Mass Transfer) territory,
//! and each rung states what it proves about the discretization.
//!
//! 1. Composite two-layer slab → the series-resistance formula
//!    q = ΔT / (L₁/k₁ + L₂/k₂). Exact at voxel centers — this is the
//!    harmonic-mean face-conductance proof: an arithmetic-mean face would
//!    miss the interface resistance and fail this test at high contrast.
//! 2. Heated slab with convection out → T_hot = T∞ + q(L/k + 1/h), with
//!    the source-layer half-cell correction stated exactly.
//! 3. Radial conduction through a cylinder shell → the ln(r₂/r₁) profile.
//!    A voxelized circle is a staircase, so this rung is a *convergence*
//!    statement with quantified error, not an exactness statement.
//! 4. Energy balance on a 3D chip-on-plate: power in = boundary heat out
//!    to well under 0.1%.

use vcad_kernel_thermal::model::{
    Axis, Boundary, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};

/// Composite slab: k₁ = 200 (aluminum-ish) for 20 mm, k₂ = 1 (insulator)
/// for 10 mm, faces pinned at 100 °C and 0 °C. The exact interface flux is
/// q = ΔT / (L₁/k₁ + L₂/k₂), and the piecewise-linear profile is exact at
/// voxel centers because the harmonic face conductance *is* the series
/// resistance of the two half-cells (Patankar's classic argument).
#[test]
fn composite_slab_matches_series_resistance_exactly() {
    let (k1, k2) = (200.0, 1.0);
    let (l1, l2) = (0.020, 0.010); // m
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
    let sol = solve_steady(&m, &SolveOptions::default()).unwrap();

    let q = 100.0 / (l1 / k1 + l2 / k2); // W/m², exact
    let dx = 0.030 / nx as f64;
    for i in 0..nx {
        let x = (i as f64 + 0.5) * dx;
        let exact = if x <= l1 {
            100.0 - q * x / k1
        } else {
            100.0 - q * l1 / k1 - q * (x - l1) / k2
        };
        let got = sol.temperature_c(i, 0, 0);
        assert!(
            (got - exact).abs() < 1e-8,
            "voxel {i}: computed {got:.9}, exact {exact:.9}"
        );
    }

    // Heat flow through the hot face equals the series-resistance value.
    let area = 0.010 * 0.010;
    let g_face = 2.0 * k1 * area / dx;
    let q_num = g_face * (100.0 - sol.temperature_c(0, 0, 0)) / area;
    assert!(
        (q_num - q).abs() / q < 1e-9,
        "face flux {q_num:.6} vs series formula {q:.6}"
    );
    assert!(sol.energy.residual_rel < 1e-9);
}

/// Heated slab, adiabatic back, convection front: generation P in the
/// first voxel layer (depth δ = dx). The exact steady solution has the
/// hottest point at the adiabatic face:
///
///   T_max − T∞ = q·( (L − δ/2)/k + 1/h ),  q = P/A
///
/// (the δ/2 term is the parabolic drop inside the generating layer; as
/// δ → 0 this is the textbook T = T∞ + q(L/k + 1/h)). The finite-volume
/// solution reproduces this *exactly* — including the δ/2 correction —
/// because the discrete source voxel balances its total power against the
/// face flux, which is the same bookkeeping.
#[test]
fn heated_slab_with_convection_matches_the_closed_form() {
    let k = 10.0;
    let h = 200.0;
    let t_inf = 25.0;
    let l = 0.020; // m
    let nx = 8;
    let dx = l / nx as f64;
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [20.0, 10.0, 10.0], [nx, 1, 1]);
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [20.0, 10.0, 10.0],
        },
        conductivity_w_mk: k,
    });
    m.sources.push(PowerSource {
        name: "heater".into(),
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [1000.0 * dx, 10.0, 10.0],
        },
        power_w: 5.0,
    });
    m.domain_faces[1] = Boundary::Convection {
        h_w_m2k: h,
        ambient_c: t_inf,
    };
    let sol = solve_steady(&m, &SolveOptions::default()).unwrap();

    let area = 0.010 * 0.010;
    let q = 5.0 / area; // 50 kW/m²
    let t_max_exact = t_inf + q * ((l - 0.5 * dx) / k + 1.0 / h);
    assert!(
        (sol.t_max_c - t_max_exact).abs() / t_max_exact < 1e-9,
        "T_max {:.6} vs exact {:.6}",
        sol.t_max_c,
        t_max_exact
    );
    // Interior centers follow T(x) = T∞ + q/h + q(L − x)/k exactly.
    for i in 1..nx {
        let x = (i as f64 + 0.5) * dx;
        let exact = t_inf + q / h + q * (l - x) / k;
        let got = sol.temperature_c(i, 0, 0);
        assert!(
            (got - exact).abs() < 1e-7,
            "voxel {i}: computed {got:.9}, exact {exact:.9}"
        );
    }
    // Every generated watt leaves through the film.
    assert!((sol.energy.convection_out_w - 5.0).abs() < 1e-6);
    assert!(sol.energy.residual_rel < 1e-7);
    // θ for the heater against the (unique) convection ambient.
    let theta = sol.sources[0].theta_c_per_w.expect("theta");
    let theta_exact = (t_max_exact - t_inf) / 5.0;
    assert!((theta - theta_exact).abs() / theta_exact < 1e-9);
}

/// Radial conduction: annulus r ∈ [12, 24] mm, k = 10, one voxel thick,
/// inner ring pinned at 80 °C, outer at 20 °C. Exact: T(ρ) follows
/// A + B·ln ρ and the shell resistance is R = ln(r₂/r₁)/(2π k H).
///
/// A voxel grid renders the circles as staircases, so the pinned
/// boundaries are only defined to ±half a voxel — this is a first-order
/// geometric error, and the test *quantifies* it instead of hiding it:
/// the resistance error must shrink with refinement and the fine-grid
/// profile must actually be logarithmic (least-squares fit residual).
#[test]
fn cylinder_shell_converges_to_the_log_profile() {
    let k = 10.0;
    let (r1, r2) = (12.0_f64, 24.0_f64); // mm
    let h_mm = 2.0;
    let r_exact = (r2 / r1).ln() / (2.0 * std::f64::consts::PI * k * h_mm * 1e-3);

    let run = |nxy: usize| {
        let mut m = ThermalModel::new([-30.0, -30.0, 0.0], [60.0, 60.0, h_mm], [nxy, nxy, 1]);
        m.materials.push(MaterialRegion {
            shape: Shape::Tube {
                axis: Axis::Z,
                center_mm: [0.0, 0.0],
                span_mm: [0.0, h_mm],
                outer_radius_mm: 30.0,
                inner_radius_mm: 0.0,
            },
            conductivity_w_mk: k,
        });
        m.fixed.push(FixedTemperature {
            shape: Shape::Tube {
                axis: Axis::Z,
                center_mm: [0.0, 0.0],
                span_mm: [0.0, h_mm],
                outer_radius_mm: r1,
                inner_radius_mm: 0.0,
            },
            temperature_c: 80.0,
        });
        m.fixed.push(FixedTemperature {
            shape: Shape::Tube {
                axis: Axis::Z,
                center_mm: [0.0, 0.0],
                span_mm: [0.0, h_mm],
                outer_radius_mm: 100.0,
                inner_radius_mm: r2,
            },
            temperature_c: 20.0,
        });
        solve_steady(&m, &SolveOptions::default()).unwrap()
    };

    let mut errors = Vec::new();
    for nxy in [60usize, 120] {
        let sol = run(nxy);
        // All heat leaving the hot ring lands in the cold ring.
        let q_in = -sol.reservoirs[0].heat_absorbed_w;
        let q_out = sol.reservoirs[1].heat_absorbed_w;
        assert!(
            (q_in - q_out).abs() / q_in < 1e-7,
            "conservation between rings: in {q_in}, out {q_out}"
        );
        let r_num = 60.0 / q_in;
        errors.push((r_num - r_exact).abs() / r_exact);
    }
    // Stair-step honesty: the error is real at coarse resolution, shrinks
    // under refinement, and is a few percent at 0.5 mm voxels.
    assert!(
        errors[1] < errors[0],
        "resistance error must shrink with refinement: {errors:?}"
    );
    assert!(
        errors[1] < 0.05,
        "fine-grid resistance error too large: {errors:?}"
    );

    // Fine grid: the free-region profile is logarithmic. Fit T = a + b·lnρ
    // over voxels well inside the annulus and check the slope against
    // B = −ΔT/ln(r₂/r₁) plus a small fit residual.
    let sol = run(120);
    let nxy = 120usize;
    let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut pts = Vec::new();
    for j in 0..nxy {
        for i in 0..nxy {
            let c = sol.voxel_center_mm(i, j, 0);
            let rho = (c[0] * c[0] + c[1] * c[1]).sqrt();
            if rho > r1 + 1.5 && rho < r2 - 1.5 {
                let t = sol.temperature_c(i, j, 0);
                if t.is_nan() {
                    continue;
                }
                let x = rho.ln();
                sx += x;
                sy += t;
                sxx += x * x;
                sxy += x * t;
                n += 1.0;
                pts.push((x, t));
            }
        }
    }
    let b = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    let a = (sy - b * sx) / n;
    let b_exact = -60.0 / (r2 / r1).ln();
    assert!(
        (b - b_exact).abs() / b_exact.abs() < 0.05,
        "log-profile slope {b:.3} vs exact {b_exact:.3}"
    );
    let rms = (pts
        .iter()
        .map(|&(x, t)| (t - (a + b * x)).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();
    assert!(
        rms < 0.01 * 60.0,
        "fit residual {rms:.3} K exceeds 1% of the 60 K drop"
    );
}

/// Chip on a plate in 3D: a 2 W source on a 60×60×2 mm k = 15 plate,
/// convection on both large faces. Power in must equal boundary heat out
/// to well under the 0.1% honesty bar, the field must respect the
/// maximum principle (nothing cooler than ambient, hottest point inside
/// the source), and θ must land between the isothermal-plate floor
/// 1/(h·A) and any physically credible spreading penalty.
#[test]
fn chip_on_plate_energy_balance_closes() {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [60.0, 60.0, 2.0], [30, 30, 2]);
    m.materials.push(MaterialRegion {
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [60.0, 60.0, 2.0],
        },
        conductivity_w_mk: 15.0,
    });
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: Shape::Box {
            min_mm: [25.0, 25.0, 0.0],
            size_mm: [10.0, 10.0, 2.0],
        },
        power_w: 2.0,
    });
    let conv = Boundary::Convection {
        h_w_m2k: 15.0,
        ambient_c: 25.0,
    };
    m.domain_faces[4] = conv;
    m.domain_faces[5] = conv;
    let sol = solve_steady(&m, &SolveOptions::default()).unwrap();

    assert!(
        sol.energy.residual_rel < 1e-3,
        "energy balance residual {} exceeds 0.1%",
        sol.energy.residual_rel
    );
    assert!(
        (sol.energy.convection_out_w - 2.0).abs() < 2e-3,
        "2 W in, {} W out",
        sol.energy.convection_out_w
    );
    // Maximum principle: with a single ambient and positive sources,
    // nothing sits below ambient.
    for &t in sol.t_c.iter().filter(|t| !t.is_nan()) {
        assert!(t >= 25.0 - 1e-9, "voxel below ambient: {t}");
    }
    // The hottest voxel is inside the die footprint.
    assert!(sol.t_max_at_mm[0] >= 25.0 && sol.t_max_at_mm[0] <= 35.0);
    assert!(sol.t_max_at_mm[1] >= 25.0 && sol.t_max_at_mm[1] <= 35.0);
    // θ_ja floor: a perfectly isothermal plate would give
    // 1/(h·2·A_plate) = 1/(15 · 2 · 0.0036) = 9.26 K/W. Spreading through
    // k = 15 over a 60 mm plate adds a finite penalty on top.
    let theta = sol.sources[0].theta_c_per_w.expect("theta");
    assert!(
        theta > 9.26 && theta < 40.0,
        "theta_ja {theta:.2} outside the physical bracket"
    );
}
