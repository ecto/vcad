//! The M0 validation ladder: solved fields against closed forms and
//! against an independent implementation in the same workspace.
//!
//! Every rung states its reference and its expected agreement. The
//! tolerances are set by what the discretization honestly delivers at the
//! (deliberately coarse, test-speed) grids used here — see
//! `docs/em-m0.md` for the convergence numbers behind them.

use vcad_kernel_em::analytic;
use vcad_kernel_em::axisym::{Annulus, AxisymMagnetostatics, Coil};
use vcad_kernel_em::grid::{Bc, SolveOptions};

/// A thin (1 mm²-section) circular loop, the closest solver analog of a
/// filament.
fn thin_loop(r_mm: f64, z_mm: f64, turns: f64, current_a: f64) -> Coil {
    Coil {
        region: Annulus {
            r_inner_mm: r_mm - 0.5,
            r_outer_mm: r_mm + 0.5,
            z_min_mm: z_mm - 0.5,
            z_max_mm: z_mm + 0.5,
        },
        turns,
        current_a,
    }
}

/// Rung 1 — the loop field against `vcad_kernel_particle::field::b_ring`:
/// **two independent implementations in one workspace**. The particle
/// crate evaluates the exact filament field through complete elliptic
/// integrals; this crate solves the boundary-value problem on a grid.
/// They must meet in the mid-field to grid + truncation accuracy.
#[test]
fn loop_field_cross_checks_against_the_particle_crate() {
    use vcad_kernel_particle::field::{b_ring, RingCoil};

    let r_loop = 30.0; // mm
    let current = 100.0; // A·turns
    let mut dev = AxisymMagnetostatics::new(150.0, -150.0, 150.0);
    dev.coils.push(thin_loop(r_loop, 0.0, 1.0, current));
    let sol = dev.solve(121, 241, &SolveOptions::default()).unwrap();

    let filament = RingCoil {
        radius_m: 0.030,
        z_m: 0.0,
        ampere_turns: current,
        wire_radius_m: 5e-4,
    };

    // Mid-field probes, chosen by three conditioning rules learned while
    // building this rung:
    // - away from the conductor (finite cross-section vs filament) and
    //   the ψ = 0 boundary (truncation);
    // - at CELL CENTERS (computed from the grid, not hard-coded): the
    //   exact-curl sampler is second-order there, while node-aligned
    //   samples of B_z = ψ_r/r carry the inherent h/(2r) staggering
    //   offset (measured: 4.6% at r = 10 mm);
    // - away from the dipole zero cone θ ≈ 54.7° where a component
    //   crosses zero and relative comparison is ill-conditioned by
    //   construction.
    let g = sol.system.grid.clone();
    let snap = |r_t: f64, z_t: f64| -> (f64, f64) {
        let r = if r_t == 0.0 {
            0.0
        } else {
            ((r_t / g.dx).floor() + 0.5) * g.dx
        };
        let z = g.y0 + (((z_t - g.y0) / g.dy).floor() + 0.5) * g.dy;
        (r, z)
    };
    // Tolerances are the measured error budget at h = 1.25 mm: 3% in the
    // near/mid field; 4.5% for the farthest probe, where truncation and
    // patch curvature both grow (the budget scales with distance — see
    // the truncation rung below).
    for (r_t, z_t, tol) in [
        (0.000, 0.010, 0.03),
        (0.010, 0.005, 0.03),
        (0.015, 0.025, 0.03),
        (0.045, 0.000, 0.03),
        (0.040, 0.010, 0.03),
        (0.025, 0.040, 0.045),
    ] {
        let (r, z) = snap(r_t, z_t);
        let (br_s, bz_s) = sol.b_at(r, z);
        let (br_a, bz_a) = b_ring(&filament, r, z);
        let mag = (br_a * br_a + bz_a * bz_a).sqrt();
        let err = ((br_s - br_a).powi(2) + (bz_s - bz_a).powi(2)).sqrt() / mag;
        assert!(
            err < tol,
            "loop field mismatch at (r={r}, z={z}): solver ({br_s:.4e}, {bz_s:.4e}) \
             vs b_ring ({br_a:.4e}, {bz_a:.4e}), rel {err:.3e}"
        );
    }
}

/// Rung 1b — the far-field error is boundary truncation, provably: at
/// fixed grid spacing, moving the ψ = 0 boundary out must shrink the
/// far-axis error (measured against `b_ring`). The 4R-domain solve first
/// failed this point at 5.6% — the failure taught the scaling, and this
/// is its regression test.
#[test]
fn far_field_error_is_truncation_and_shrinks_with_the_domain() {
    use vcad_kernel_particle::field::{b_ring, RingCoil};

    let filament = RingCoil {
        radius_m: 0.030,
        z_m: 0.0,
        ampere_turns: 100.0,
        wire_radius_m: 5e-4,
    };
    let (_, bz_ref) = b_ring(&filament, 0.0, 0.040);

    // Same h = 1.5 mm; domains 120 mm and 180 mm.
    let err_at = |extent_mm: f64, n_per_120: usize| -> f64 {
        let n = ((n_per_120 - 1) as f64 * extent_mm / 120.0) as usize + 1;
        let mut dev = AxisymMagnetostatics::new(extent_mm, -extent_mm, extent_mm);
        dev.coils.push(thin_loop(30.0, 0.0, 1.0, 100.0));
        let sol = dev.solve(n, 2 * n - 1, &SolveOptions::default()).unwrap();
        let (_, bz) = sol.b_at(0.0, 0.040);
        (bz - bz_ref).abs() / bz_ref.abs()
    };
    let near = err_at(120.0, 81);
    let far = err_at(180.0, 81);
    assert!(
        far < 0.6 * near,
        "truncation error must shrink with the domain: {near:.3e} → {far:.3e}"
    );
    assert!(far < 0.03, "far-axis error at 6R domain: {far:.3e}");
}

/// Rung 2 — finite solenoid against Wheeler's 1928 formula (±1% claimed
/// for ℓ > 0.8R), and the physical ordering L_finite <
/// L_infinite-formula.
///
/// Two lessons are priced into this setup, both found by probing:
/// - Wheeler models an infinitely thin current **sheet**; a 1 mm-thick
///   winding's inner-weighted flux linkage reads a real 3% lower (the
///   solver was right, the comparison was wrong) — so the model coil is
///   a thin sheet at the Wheeler radius.
/// - a ψ = 0 box at 6R measurably squeezes the return flux (5% low);
///   at 15R the truncation is below the grid error.
#[test]
fn finite_solenoid_lands_on_wheeler() {
    let (r_mm, l_mm, turns) = (10.0, 40.0, 200.0);
    let mut dev = AxisymMagnetostatics::new(150.0, -150.0, 150.0);
    dev.coils.push(Coil {
        region: Annulus {
            r_inner_mm: r_mm - 0.1,
            r_outer_mm: r_mm + 0.1,
            z_min_mm: -l_mm / 2.0,
            z_max_mm: l_mm / 2.0,
        },
        turns,
        current_a: 1.0,
    });
    let sol = dev.solve(121, 241, &SolveOptions::default()).unwrap();
    let l_solved = sol.self_inductance(0);
    let wheeler = analytic::wheeler_solenoid_inductance(r_mm * 1e-3, l_mm * 1e-3, turns);
    let ideal = analytic::solenoid_inductance_per_m(turns / (l_mm * 1e-3), r_mm * 1e-3, 0.0, 1.0)
        * l_mm
        * 1e-3;
    let rel = (l_solved - wheeler).abs() / wheeler;
    assert!(
        rel < 0.025,
        "L = {l_solved:.5e} H vs Wheeler {wheeler:.5e} H (rel {rel:.2e})"
    );
    assert!(
        l_solved < ideal,
        "finite solenoid ({l_solved:.4e}) must undershoot the infinite formula ({ideal:.4e})"
    );
    let bal = sol.energy();
    assert!(bal.residual < 1e-6, "energy imbalance {:.2e}", bal.residual);
}

/// Rung 3 — mutual inductance of coaxial loops against Maxwell's
/// elliptic-integral formula, and reciprocity through the solver.
#[test]
fn coaxial_loop_mutual_matches_maxwell() {
    let (r_loop, half_d) = (30.0, 10.0); // mm; separation d = 20 mm
    let mut dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
    dev.coils.push(thin_loop(r_loop, -half_d, 1.0, 100.0));
    dev.coils.push(thin_loop(r_loop, half_d, 1.0, 0.0));
    let sol = dev.solve(121, 241, &SolveOptions::default()).unwrap();
    let m_solved = sol.flux_linkage(1) / 100.0;
    let m_analytic = analytic::loop_mutual_inductance(0.030, 0.030, 0.020);
    let rel = (m_solved - m_analytic).abs() / m_analytic;
    assert!(
        rel < 0.03,
        "M = {m_solved:.5e} H vs Maxwell {m_analytic:.5e} H (rel {rel:.2e})"
    );
}

/// Rung 4 — axial force between coaxial coils, three ways: the `J×B`
/// volume integral and the Maxwell-stress surface integral (internal
/// consistency), against the analytic `F = I₁·I₂·dM/dz` (external truth).
/// Same-sense currents must attract.
#[test]
fn coil_force_agrees_with_dm_dz_and_stress() {
    let (r_loop, half_d, i_a) = (30.0, 10.0, 100.0);
    let mut dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
    dev.coils.push(thin_loop(r_loop, -half_d, 1.0, i_a));
    dev.coils.push(thin_loop(r_loop, half_d, 1.0, i_a));
    let sol = dev.solve(121, 241, &SolveOptions::default()).unwrap();

    let f_analytic = analytic::loop_axial_force(0.030, 0.030, 0.020, i_a, i_a);
    assert!(f_analytic < 0.0, "same-sense loops attract");

    // J×B on the upper coil.
    let f_jxb = sol.axial_force_on_coil(1);
    let rel = (f_jxb - f_analytic).abs() / f_analytic.abs();
    assert!(
        rel < 0.04,
        "J×B force {f_jxb:.5e} N vs analytic {f_analytic:.5e} N (rel {rel:.2e})"
    );

    // Maxwell stress on a closed cylinder enclosing only the upper coil.
    let f_stress = sol.axial_force_stress(45.0, 2.0, 30.0, 600);
    let rel_s = (f_stress - f_jxb).abs() / f_jxb.abs();
    assert!(
        rel_s < 0.03,
        "stress force {f_stress:.5e} N vs J×B {f_jxb:.5e} N (rel {rel_s:.2e})"
    );

    // Newton's third law through the solver.
    let f_lower = sol.axial_force_on_coil(0);
    assert!(
        (f_lower + f_jxb).abs() < 1e-3 * f_jxb.abs(),
        "action–reaction: {f_lower:.5e} vs {f_jxb:.5e}"
    );
}

/// Rung 5 — the quantities the ladder graded stay put under refinement:
/// mutual inductance at two grids must agree within the coarse-grid error
/// budget (grid dependence is measured, not assumed).
#[test]
fn mutual_inductance_is_refinement_consistent() {
    let build = |nr: usize, nz: usize| {
        let mut dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
        dev.coils.push(thin_loop(30.0, -10.0, 1.0, 100.0));
        dev.coils.push(thin_loop(30.0, 10.0, 1.0, 0.0));
        let sol = dev.solve(nr, nz, &SolveOptions::default()).unwrap();
        sol.flux_linkage(1) / 100.0
    };
    let coarse = build(61, 121);
    let fine = build(121, 241);
    let rel = (coarse - fine).abs() / fine.abs();
    assert!(
        rel < 0.05,
        "grid dependence too large: coarse {coarse:.5e} vs fine {fine:.5e} ({rel:.2e})"
    );
}

/// Rung 6 — Neumann symmetry halving: a z-symmetric problem solved on the
/// half domain with a Neumann midplane must reproduce the full-domain
/// answer (the boundary machinery is load-bearing for every "exact
/// anchor" test).
#[test]
fn neumann_midplane_reproduces_the_full_domain() {
    let full = {
        let mut dev = AxisymMagnetostatics::new(80.0, -80.0, 80.0);
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 25.0,
                r_outer_mm: 30.0,
                z_min_mm: -10.0,
                z_max_mm: 10.0,
            },
            turns: 100.0,
            current_a: 2.0,
        });
        dev.solve(81, 161, &SolveOptions::default()).unwrap()
    };
    let half = {
        let mut dev = AxisymMagnetostatics::new(80.0, 0.0, 80.0);
        dev.bc_z_low = Bc::Neumann; // midplane symmetry
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 25.0,
                r_outer_mm: 30.0,
                z_min_mm: 0.0,
                z_max_mm: 10.0,
            },
            turns: 50.0,
            current_a: 2.0,
        });
        dev.solve(81, 81, &SolveOptions::default()).unwrap()
    };
    // Same field in the shared half-space.
    for (r, z) in [(0.01, 0.02), (0.02, 0.04), (0.04, 0.01), (0.015, 0.06)] {
        let (br_f, bz_f) = full.b_at(r, z);
        let (br_h, bz_h) = half.b_at(r, z);
        let mag = (br_f * br_f + bz_f * bz_f).sqrt();
        let err = ((br_f - br_h).powi(2) + (bz_f - bz_h).powi(2)).sqrt() / mag;
        assert!(
            err < 1e-2,
            "half-domain mismatch at ({r},{z}): rel {err:.3e}"
        );
    }
    // Half-domain stores half the energy.
    let ratio = full.energy().source / half.energy().source;
    assert!(
        (ratio - 2.0).abs() < 0.02,
        "energy ratio full/half = {ratio}"
    );
}

/// Rung 7 (M1) — AC rod in a solenoid against the complex Bessel closed
/// form. A conducting cylinder (σ, radius R) in a uniform AC applied
/// field admits `Φ(ω)/Φ_dc = 2·J₁(kR)/(kR·J₀(kR))` with `k² = −jωμ₀σ`
/// (Stoll, *The Analysis of Eddy Currents*, 1974, §2; Lammeraner &
/// Štafl). Exercises the phasor solver's flux amplitude AND phase at
/// R/δ = 2, where both are far from their DC values.
#[test]
fn ac_rod_flux_matches_the_bessel_solution() {
    use vcad_kernel_em::ac::{solve_axisym_ac, AxisymSigma};
    use vcad_kernel_em::constants::MU_0;

    // Complex arithmetic on (re, im) pairs, enough for the series.
    type C = (f64, f64);
    fn cmul(a: C, b: C) -> C {
        (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
    }
    fn cdiv(a: C, b: C) -> C {
        let m = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / m, (a.1 * b.0 - a.0 * b.1) / m)
    }
    /// J_n(z) by the power series — fine for |z| ≲ 10.
    fn bessel_j(n: u32, z: C) -> C {
        let half = (z.0 / 2.0, z.1 / 2.0);
        let mut term = (1.0, 0.0);
        for k in 1..=n {
            term = cmul(term, cdiv(half, (k as f64, 0.0)));
        }
        let z2 = cmul(half, half);
        let mut sum = term;
        let mut tk = term;
        for m in 1..40 {
            tk = cmul(tk, z2);
            tk = cdiv(tk, (-(m as f64) * (m as f64 + n as f64), 0.0));
            sum = (sum.0 + tk.0, sum.1 + tk.1);
        }
        sum
    }

    let sigma = 3.5e7; // aluminum
    let r_rod = 0.010;
    let delta = 0.005; // skin depth → R/δ = 2
    let omega = 2.0 / (MU_0 * sigma * delta * delta);

    // Infinite solenoid via Neumann boundaries, rod filling r < 10 mm.
    let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 30.0);
    dev.bc_r_outer = Bc::Neumann;
    dev.bc_z_low = Bc::Neumann;
    dev.bc_z_high = Bc::Neumann;
    dev.coils.push(Coil {
        region: Annulus {
            r_inner_mm: 20.0,
            r_outer_mm: 22.0,
            z_min_mm: 0.0,
            z_max_mm: 30.0,
        },
        turns: 300.0,
        current_a: 1.0,
    });
    let rod = AxisymSigma {
        region: Annulus {
            r_inner_mm: 0.0,
            r_outer_mm: 10.0,
            z_min_mm: 0.0,
            z_max_mm: 30.0,
        },
        sigma_s_m: sigma,
    };

    let opts = SolveOptions::default();
    let dc = dev.solve(81, 7, &opts).unwrap();
    let phi_dc = dc.system.grid.value_at(&dc.psi, r_rod, 0.015);
    let sol = solve_axisym_ac(&dev, &[rod], omega, 81, 7, &opts).unwrap();
    let (pr, pi) = sol.value_at(r_rod, 0.015);
    let got = (pr / phi_dc, pi / phi_dc);

    // 2·J₁(kR)/(kR·J₀(kR)), k = √(−jωμ₀σ) = (1−j)/δ.
    let kr = (r_rod / delta, -r_rod / delta);
    let j0 = bessel_j(0, kr);
    let j1 = bessel_j(1, kr);
    let want = cdiv(cmul((2.0, 0.0), j1), cmul(kr, j0));
    let mag_w = (want.0 * want.0 + want.1 * want.1).sqrt();

    let err = ((got.0 - want.0).powi(2) + (got.1 - want.1).powi(2)).sqrt() / mag_w;
    assert!(
        err < 0.02,
        "rod flux ratio ({:.4}, {:.4}) vs Bessel ({:.4}, {:.4}), rel {err:.3e}",
        got.0,
        got.1,
        want.0,
        want.1
    );
    // The flux must lag the drive and be attenuated at R/δ = 2.
    assert!(mag_w < 0.75 && got.1 < 0.0, "attenuation/lag sanity");
}
