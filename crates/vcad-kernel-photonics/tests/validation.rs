//! The M0 validation ladder: every rung compares the solver against an
//! analytic result, with tolerances that name their own floors.
//!
//! Rungs (formulas cited at each test):
//!
//! 1. Vacuum numerical dispersion vs the exact discrete FDTD relation.
//! 2. Fresnel reflection/transmission at a dielectric half-space, both
//!    polarizations, vs r = (n₁−n₂)/(n₁+n₂).
//! 3. CPML reflection floor < −50 dB (measured by reference subtraction).
//! 4. Exact discrete energy conservation in a lossless PEC box.
//! 5. Slab-waveguide effective index: FDTD propagation phase vs the
//!    transcendental-equation mode solver, plus flux transmission ≈ 1.
//!
//! Several rungs run in the **exact-1D configuration**: a y-uniform TM
//! wave between PMC y-walls (or a TE wave between PEC y-walls) has zero
//! transverse derivatives, so the 2D stepper reproduces 1D propagation to
//! machine precision — no diffraction contamination, tolerances set by
//! physics instead of by geometry compromises.

use vcad_kernel_photonics::{
    dft_of_series, fdtd_wavenumber, fdtd_wavenumber_in_medium, objective_and_gradient,
    run_objective, solve_slab_mode_even, BoundarySpec, CpmlSpec, DesignRegion, FluxSpec, GridSpec,
    ModeOverlap, Polarization, Shape2, Simulation, Source, Waveform,
};

/// Unwrap a phase difference to the branch nearest `expected`.
fn unwrap_phase(dphi: f64, expected: f64) -> f64 {
    let two_pi = 2.0 * std::f64::consts::PI;
    dphi + two_pi * ((expected - dphi) / two_pi).round()
}

/// Rung 1 — vacuum propagation phase at 20 cells/λ, Courant 0.5, must
/// match the on-axis discrete dispersion relation
///
/// ```text
/// sin(ω·dt/2)/dt = sin(k·Δ/2)/Δ            (c = 1; Taflove ch. 4)
/// ```
///
/// far better than it matches the continuum k = ω: the solver knows its
/// own dispersion error (≈ +0.36 % in k at this resolution) instead of
/// hiding it in a loose tolerance.
#[test]
fn vacuum_dispersion_matches_fdtd_relation_tm() {
    let lambda = 1.0;
    let f0 = 1.0 / lambda;
    let delta = lambda / 20.0;
    let (nx, ny) = (200, 4);
    let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
    sim.set_boundaries(BoundarySpec::pmc_y());
    sim.set_cpml(CpmlSpec::x_only(12));
    sim.add_source(Source::line_uniform(
        30,
        0,
        ny,
        Waveform::gaussian(f0, 0.25),
    ));
    let p1 = sim.add_probe(70, 2);
    let p1_off = sim.add_probe(70, 0); // uniformity witness
    let p2 = sim.add_probe(150, 2);
    sim.run(1200);

    // Exact-1D check: the PMC y-walls must make the field y-invariant to
    // machine precision.
    let s1 = sim.probe_series(p1);
    let s1o = sim.probe_series(p1_off);
    let max_amp = s1.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(max_amp > 1e-3, "no signal reached the probes");
    for (a, b) in s1.iter().zip(s1o.iter()) {
        assert!(
            (a - b).abs() <= 1e-13 * max_amp,
            "PMC y-walls broke y-invariance"
        );
    }

    let dt = sim.dt();
    let e1 = dft_of_series(s1, dt, sim.probe_sample_time(0), f0);
    let e2 = dft_of_series(sim.probe_series(p2), dt, sim.probe_sample_time(0), f0);

    // 1D: no geometric spreading — spectral amplitude preserved to the
    // PML-reflection floor.
    let amp_ratio = (e2.abs2() / e1.abs2()).sqrt();
    assert!(
        (amp_ratio - 1.0).abs() < 1e-3,
        "amplitude not conserved in 1D: ratio {amp_ratio}"
    );

    let l = (150 - 70) as f64 * delta;
    let k_exact = 2.0 * std::f64::consts::PI * f0;
    let k_fdtd = fdtd_wavenumber(k_exact, delta, dt).unwrap();
    let k_meas = unwrap_phase(e2.arg() - e1.arg(), k_fdtd * l) / l;

    let err_vs_fdtd = ((k_meas - k_fdtd) / k_fdtd).abs();
    let err_vs_exact = ((k_meas - k_exact) / k_exact).abs();
    println!(
        "dispersion: k_meas {k_meas:.6}  k_fdtd {k_fdtd:.6}  k_exact {k_exact:.6}  \
         rel(fdtd) {err_vs_fdtd:.2e}  rel(exact) {err_vs_exact:.2e}"
    );
    // Measurement floor: PML residual (≈ −60 dB) phase contamination,
    // ~5e-5 relative. Assert 10× headroom above it.
    assert!(
        err_vs_fdtd < 5e-4,
        "measured k {k_meas} vs discrete relation {k_fdtd}: rel err {err_vs_fdtd}"
    );
    // The discrete relation must win over the continuum by a wide margin
    // (the dispersion offset here is ≈ 3.6e-3).
    assert!(
        (k_meas - k_fdtd).abs() < 0.25 * (k_meas - k_exact).abs(),
        "solver does not match its own dispersion relation: \
         |Δfdtd| {} vs |Δexact| {}",
        (k_meas - k_fdtd).abs(),
        (k_meas - k_exact).abs()
    );
    assert!(
        err_vs_exact > 2e-3,
        "dispersion offset vanished — suspicious"
    );
}

/// Shared Fresnel rig at `res` cells per vacuum wavelength: returns
/// (f, R, T) triples at the given frequencies.
///
/// Two runs (vacuum reference, then with the half-space): at a monitor
/// between source and interface the DFT cross terms of incident and
/// reflected waves cancel identically in Re(Ê·conj(Ĥ)), so
/// P_net = P_inc − P_refl exactly, giving R = (P_inc − P_net)/P_inc;
/// T comes from a monitor inside the dielectric.
///
/// The interface sits on a HALF-cell line: no ε sample's averaging square
/// straddles it (TM: Ez nodes at integer x; TE: ε_y at integer x, and the
/// smeared ε_x line carries Ex ≡ 0 at normal incidence), so the discrete
/// problem has a genuinely sharp step between two nodes. Node-aligned
/// placement instead smears one sample line to (ε₁+ε₂)/2, which acts as a
/// one-cell antireflection layer and lowers R by ~8 % at 20 cells/λ —
/// sub-pixel averaging doing its job, but not the Fresnel formula.
type Spectrum = Vec<(f64, f64)>;

fn fresnel_rt(pol: Polarization, n2: f64, freqs: Vec<f64>, res: usize) -> Vec<(f64, f64, f64)> {
    let lambda = 1.0;
    let f0 = 1.0 / lambda;
    let delta = lambda / res as f64;
    let s = res / 20; // geometry scale relative to the res-20 baseline
    let (nx, ny) = (260 * s, 4);
    let run = |with_dielectric: bool| -> (Spectrum, Spectrum) {
        let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), pol);
        match pol {
            // Exact-1D walls per duality: PMC for TM, PEC for TE.
            Polarization::Tm => sim.set_boundaries(BoundarySpec::pmc_y()),
            Polarization::Te => sim.set_boundaries(BoundarySpec::pec()),
        }
        sim.set_cpml(CpmlSpec::x_only(12));
        if with_dielectric {
            sim.paint(
                &Shape2::rect((150 * s) as f64 * delta + 0.5 * delta, -1.0, 1e9, 100.0),
                n2 * n2,
            );
        }
        let j_max = match pol {
            Polarization::Tm => ny,     // Ez nodes 0..=ny
            Polarization::Te => ny - 1, // Hz/Ey rows 0..=ny−1
        };
        sim.add_source(Source::line_uniform(
            40 * s,
            0,
            j_max,
            Waveform::gaussian(f0, 0.25),
        ));
        let m_r = sim.add_flux(FluxSpec::Vertical {
            i: 100 * s,
            j0: 0,
            j1: j_max,
            freqs: freqs.clone(),
        });
        let m_t = sim.add_flux(FluxSpec::Vertical {
            i: 210 * s,
            j0: 0,
            j1: j_max,
            freqs: freqs.clone(),
        });
        sim.run(1400 * s); // dt scales with Δ, so this keeps total time fixed
        (sim.flux_power(m_r), sim.flux_power(m_t))
    };
    let (p_inc, _) = run(false);
    let (p_net, p_trans) = run(true);
    freqs
        .iter()
        .enumerate()
        .map(|(k, &f)| {
            let r = (p_inc[k].1 - p_net[k].1) / p_inc[k].1;
            let t = p_trans[k].1 / p_inc[k].1;
            (f, r, t)
        })
        .collect()
}

/// The exact reflectance of the *discrete* two-media problem: matching
/// the discrete Helmholtz recurrence `Ez[i+1] + Ez[i−1] = 2cos(kΔ)·Ez[i]`
/// across a sharp ε step between adjacent nodes gives
///
/// ```text
/// r = (e^{ik₁Δ} − e^{ik₂Δ})/(e^{ik₂Δ} − e^{−ik₁Δ})
/// ⇒ R_d = sin²((k₁−k₂)Δ/2) / sin²((k₁+k₂)Δ/2)
/// ```
///
/// with k₁, k₂ the numerical wavenumbers from the medium dispersion
/// relation. R_d → ((n₁−n₂)/(n₁+n₂))² as Δ → 0, from above, as O(Δ²).
fn discrete_fresnel_r(f: f64, eps2: f64, delta: f64, dt: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f;
    let k1 = fdtd_wavenumber_in_medium(w, 1.0, delta, dt).unwrap();
    let k2 = fdtd_wavenumber_in_medium(w, eps2, delta, dt).unwrap();
    let num = ((k1 - k2) * delta / 2.0).sin();
    let den = ((k1 + k2) * delta / 2.0).sin();
    (num / den).powi(2)
}

/// Rung 2 (TM) — normal-incidence reflection at a n₁=1 → n₂=2 half-space
/// against **two** analytic references:
///
/// 1. The exact discrete reflectance [`discrete_fresnel_r`] — tight
///    (1e−3 relative): the solver must reproduce its own discretization's
///    closed form, not hide it in slack.
/// 2. The continuum Fresnel R = ((n₁−n₂)/(n₁+n₂))² = 1/9 — approached
///    as O(Δ²): doubling the resolution must shrink the continuum error
///    ≈ 4× (asserted at 20 → 40 cells/λ).
///
/// Power balance R + T = 1 closes the books at each resolution.
#[test]
fn fresnel_half_space_tm() {
    let r_a: f64 = 1.0 / 9.0;
    let freqs = vec![0.9, 1.0, 1.1];
    let courant_dt = |res: usize| 0.5 * (1.0 / res as f64) / 2f64.sqrt();

    let coarse = fresnel_rt(Polarization::Tm, 2.0, freqs.clone(), 20);
    for (f, r, t) in &coarse {
        let rd = discrete_fresnel_r(*f, 4.0, 1.0 / 20.0, courant_dt(20));
        println!(
            "fresnel TM res20: f {f}  R {r:.6}  R_disc {rd:.6}  T {t:.6}  R+T {:.6}",
            r + t
        );
        assert!(
            ((r - rd) / rd).abs() < 1e-3,
            "TM R({f}) = {r} vs exact discrete {rd}"
        );
        assert!(
            (r + t - 1.0).abs() < 3e-3,
            "TM power balance R+T = {}",
            r + t
        );
    }

    let fine = fresnel_rt(Polarization::Tm, 2.0, vec![1.0], 40);
    let rd_fine = discrete_fresnel_r(1.0, 4.0, 1.0 / 40.0, courant_dt(40));
    println!("fresnel TM res40: R {:.6}  R_disc {rd_fine:.6}", fine[0].1);
    assert!(
        ((fine[0].1 - rd_fine) / rd_fine).abs() < 1e-3,
        "TM R(res 40) = {} vs exact discrete {rd_fine}",
        fine[0].1
    );
    // O(Δ²) march to the continuum: 20 → 40 cells/λ shrinks the error to
    // the Fresnel value ≈ 4× (measured ratio ≈ 0.24).
    let err_coarse = (coarse[1].1 - r_a).abs();
    let err_fine = (fine[0].1 - r_a).abs();
    assert!(
        err_fine < 0.32 * err_coarse,
        "continuum convergence broken: |ΔR| {err_coarse:.5} → {err_fine:.5}"
    );
    assert!(
        err_coarse > 5e-3,
        "discretization offset vanished — suspicious"
    );
}

/// Rung 2 (TE) — the dual code path ((Hz, Ex, Ey) stepper, staggered
/// ε_x/ε_y, PEC exact-1D walls) must reproduce the same discrete
/// reflectance; in 1D the TE and TM discretizations are exact duals, so
/// they are also asserted against each other at near-roundoff level.
#[test]
fn fresnel_half_space_te() {
    let freqs = vec![0.9, 1.0, 1.1];
    let dt = 0.5 * (1.0 / 20.0) / 2f64.sqrt();
    let te = fresnel_rt(Polarization::Te, 2.0, freqs.clone(), 20);
    let tm = fresnel_rt(Polarization::Tm, 2.0, freqs, 20);
    for ((f, r, t), (_, r_tm, _)) in te.iter().zip(tm.iter()) {
        let rd = discrete_fresnel_r(*f, 4.0, 1.0 / 20.0, dt);
        assert!(
            ((r - rd) / rd).abs() < 1e-3,
            "TE R({f}) = {r} vs exact discrete {rd}"
        );
        assert!(
            (r + t - 1.0).abs() < 3e-3,
            "TE power balance R+T = {}",
            r + t
        );
        assert!(
            ((r - r_tm) / r_tm).abs() < 1e-5,
            "TE/TM 1D duality broken: {r} vs {r_tm}"
        );
    }
}

/// Rung 3 — CPML reflection floor, measured the honest way: identical
/// runs in a short domain (PML 12 cells) and a 4× domain whose far wall
/// cannot answer within the window; the field difference at a probe near
/// the PML **is** the reflected wave. Requirement: < −50 dB relative to
/// the incident peak (12-cell CPML should deliver ≈ −60 dB or better).
#[test]
fn cpml_reflection_below_minus_50_db() {
    let lambda = 1.0;
    let f0 = 1.0 / lambda;
    let delta = lambda / 20.0;
    let steps = 1150; // ≈ 20.3 time units: incident + PML echo + tail
    let run = |nx: usize| -> Vec<f64> {
        let mut sim = Simulation::new(GridSpec::new(nx, 4, delta), Polarization::Tm);
        sim.set_boundaries(BoundarySpec::pmc_y());
        sim.set_cpml(CpmlSpec::x_only(12));
        sim.add_source(Source::line_uniform(100, 0, 4, Waveform::gaussian(f0, 0.3)));
        let p = sim.add_probe(176, 2);
        sim.run(steps);
        sim.probe_series(p).to_vec()
    };
    let small = run(200); // probe 12 cells from the inner PML edge
    let reference = run(800); // reference: right wall 624 cells past the probe
    let peak = reference.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(peak > 1e-3);
    let refl = small
        .iter()
        .zip(reference.iter())
        .fold(0.0f64, |m, (a, b)| m.max((a - b).abs()));
    let db = 20.0 * (refl / peak).log10();
    println!(
        "cpml: reflection {db:.1} dB (|r| = {:.3e}) with 12-cell CPML",
        refl / peak
    );
    assert!(
        db < -50.0,
        "CPML reflection {db:.1} dB (|r| = {:.3e}) exceeds −50 dB",
        refl / peak
    );
}

/// Rung 4 (TM) — with PEC walls, no PML, and the source gated off, the
/// staggered-time energy
///
/// ```text
/// U^(n+½) = ½·(⟨ε·E^(n+1), E^n⟩ + |H^(n+½)|²)
/// ```
///
/// is an exact invariant of the leapfrog (the discrete curls are mutually
/// adjoint under PEC; see `Simulation::step_measuring_energy`). Rounding
/// noise only — any indexing or sign bug shows up at first order.
#[test]
fn energy_conserved_in_lossless_pec_box_tm() {
    let mut sim = Simulation::new(GridSpec::new(50, 50, 0.05), Polarization::Tm);
    sim.paint(&Shape2::circle(1.6, 1.2, 0.5), 6.0); // inhomogeneity on purpose
    sim.add_source(Source::point(25, 25, Waveform::gaussian(2.0, 0.5)));
    let end = Waveform::gaussian(2.0, 0.5).end_time();
    while sim.time() <= end + 2.0 * sim.dt() {
        sim.step();
    }
    let u0 = sim.step_measuring_energy();
    assert!(u0 > 0.0);
    let mut worst: f64 = 0.0;
    for _ in 0..400 {
        let u = sim.step_measuring_energy();
        worst = worst.max(((u - u0) / u0).abs());
    }
    assert!(
        worst < 1e-11,
        "energy drift {worst:.3e} (exact invariant expected)"
    );
}

/// Rung 4 (TE) — same invariant, dual field set (ε_x, ε_y staggering).
#[test]
fn energy_conserved_in_lossless_pec_box_te() {
    let mut sim = Simulation::new(GridSpec::new(50, 50, 0.05), Polarization::Te);
    sim.paint(&Shape2::circle(1.0, 1.5, 0.45), 4.0);
    sim.add_source(Source::point(25, 25, Waveform::gaussian(2.0, 0.5)));
    let end = Waveform::gaussian(2.0, 0.5).end_time();
    while sim.time() <= end + 2.0 * sim.dt() {
        sim.step();
    }
    let u0 = sim.step_measuring_energy();
    assert!(u0 > 0.0);
    let mut worst: f64 = 0.0;
    for _ in 0..400 {
        let u = sim.step_measuring_energy();
        worst = worst.max(((u - u0) / u0).abs());
    }
    assert!(
        worst < 1e-11,
        "energy drift {worst:.3e} (exact invariant expected)"
    );
}

/// Rung 5 — the crown: a Si/SiO₂-like slab waveguide (n = 3.48/1.44,
/// w = 0.22, λ₀ = 1.55). The eigenmode line source injects the solver's
/// own slab solution; two probes measure the propagation phase, giving
/// n_eff(FDTD) to compare against the transcendental n_eff; two flux
/// monitors bracket 1.5λ₀ of guide and must agree (lossless straight
/// guide ⇒ T ≈ 1).
///
/// Tolerance floor (named): 11.5 cells per **material** wavelength in the
/// core at this resolution ⇒ on-axis dispersion ≈ 0.9 % slow, plus the
/// discrete guide's own O(Δ²) mode-shape shift — 2 % headroom total.
/// Waveguide dispersion must also come out with the right sign
/// (n_eff rises with frequency).
#[test]
fn slab_waveguide_neff_and_transmission_tm() {
    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 40.0;
    let (nx, ny) = (145, 64);
    let jc = 32usize; // guide axis on an Ez node row
    let yc = jc as f64 * delta;

    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm).unwrap();
    assert!(mode.n_eff > n_clad && mode.n_eff < n_core);

    // Cladding is the n_clad background; the core is painted over it
    // (paint order matters — later wins).
    let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform(12));
    sim.fill_epsilon(n_clad * n_clad);
    sim.paint(
        &Shape2::rect(-1.0, yc - half_w, 1e9, yc + half_w),
        n_core * n_core,
    );

    let (j0, j1) = (12usize, 52usize);
    let profile: Vec<f64> = (j0..=j1)
        .map(|j| mode.profile((j as f64 - jc as f64) * delta))
        .collect();
    sim.add_source(Source::line_profile(
        25,
        j0,
        profile,
        Waveform::gaussian(f0, f0 / 4.0),
    ));

    let freqs = vec![1.0 / 1.61, f0, 1.0 / 1.49];
    let f1 = sim.add_flux(FluxSpec::Vertical {
        i: 55,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let f2 = sim.add_flux(FluxSpec::Vertical {
        i: 115,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let p1 = sim.add_probe(55, jc);
    let p2 = sim.add_probe(115, jc);
    sim.run(2500);

    let dt = sim.dt();
    let l = (115 - 55) as f64 * delta;
    let neff_meas = |f: f64| -> f64 {
        let e1 = dft_of_series(sim.probe_series(p1), dt, sim.probe_sample_time(0), f);
        let e2 = dft_of_series(sim.probe_series(p2), dt, sim.probe_sample_time(0), f);
        let k0 = 2.0 * std::f64::consts::PI * f;
        let expected = mode.n_eff * k0 * l;
        unwrap_phase(e2.arg() - e1.arg(), expected) / (k0 * l)
    };

    let neff_f0 = neff_meas(f0);
    let rel = ((neff_f0 - mode.n_eff) / mode.n_eff).abs();
    println!(
        "slab: n_eff(FDTD) {neff_f0:.5}  n_eff(theory) {:.5}  rel {rel:.4}",
        mode.n_eff
    );
    assert!(
        rel < 0.02,
        "n_eff(FDTD) = {neff_f0:.4} vs transcendental {:.4} (rel {rel:.4}); \
         floor: ~0.9 % dispersion at 11.5 cells/λ_core + O(Δ²) mode shift",
        mode.n_eff
    );
    assert!(
        neff_f0 > n_clad && neff_f0 < n_core,
        "measured n_eff not guided"
    );

    // Waveguide dispersion sign: higher f ⇒ better confinement ⇒ larger n_eff.
    let neff_lo = neff_meas(1.0 / 1.61);
    let neff_hi = neff_meas(1.0 / 1.49);
    assert!(
        neff_hi > neff_lo,
        "waveguide dispersion has the wrong sign: {neff_lo} !< {neff_hi}"
    );

    // Straight lossless guide: flux at the two planes agrees.
    let pw1 = sim.flux_power(f1);
    let pw2 = sim.flux_power(f2);
    for k in 0..freqs.len() {
        assert!(pw1[k].1 > 0.0 && pw2[k].1 > 0.0, "flux direction wrong");
        let t = pw2[k].1 / pw1[k].1;
        println!("slab: T({:.4}) = {t:.5}", freqs[k]);
        let tol = if k == 1 { 0.03 } else { 0.05 };
        assert!(
            (t - 1.0).abs() < tol,
            "straight-guide transmission T({}) = {t}",
            freqs[k]
        );
    }
}

/// Rung 6 (M1) — TF/SF mode injection is directional: with the incident
/// slab mode added only inside the total-field region, the scattered-field
/// side of the plane must carry only the residual backward leakage
/// (continuum-vs-discrete mode profile + the single narrowband delay
/// n_eff(f₀) across a finite pulse band — see the honesty note on
/// `SourcePlacement::TfsfVerticalLine`). Requirement: backward power at
/// least 22 dB below forward; the measured number is printed.
#[test]
fn tfsf_mode_injection_is_directional() {
    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 40.0;
    let (nx, ny) = (145, 64);
    let jc = 32usize;
    let yc = jc as f64 * delta;

    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm).unwrap();

    let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform(12));
    sim.fill_epsilon(n_clad * n_clad);
    sim.paint(
        &Shape2::rect(-1.0, yc - half_w, 1e9, yc + half_w),
        n_core * n_core,
    );
    let (j0, j1) = (12usize, 52usize);
    let profile: Vec<f64> = (j0..=j1)
        .map(|j| mode.profile((j as f64 - jc as f64) * delta))
        .collect();
    sim.add_source(Source::mode_tfsf(
        35,
        j0,
        profile,
        mode.n_eff,
        Waveform::gaussian(f0, f0 / 4.0),
    ));
    let freqs = vec![f0];
    let f_back = sim.add_flux(FluxSpec::Vertical {
        i: 24,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let f_fwd = sim.add_flux(FluxSpec::Vertical {
        i: 70,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    let f_fwd2 = sim.add_flux(FluxSpec::Vertical {
        i: 120,
        j0,
        j1,
        freqs: freqs.clone(),
    });
    sim.run(2500);

    let p_back = sim.flux_power(f_back)[0].1;
    let p_fwd = sim.flux_power(f_fwd)[0].1;
    let p_fwd2 = sim.flux_power(f_fwd2)[0].1;
    assert!(p_fwd > 0.0, "no forward power");
    let leak = (p_back.abs() / p_fwd).max(1e-30);
    let db = 10.0 * leak.log10();
    println!(
        "tfsf: forward {p_fwd:.4e}, backward {p_back:.4e} ({db:.1} dB), fwd T {:.5}",
        p_fwd2 / p_fwd
    );
    assert!(
        db < -22.0,
        "TF/SF backward leakage {db:.1} dB exceeds −22 dB"
    );
    // The injected wave is still the guided mode: transmission between
    // the two forward monitors stays ≈ 1.
    let t = p_fwd2 / p_fwd;
    assert!((t - 1.0).abs() < 0.03, "TF/SF forward transmission {t}");
}

/// Rung 7 (M1) — 90° waveguide bends: sharper bends lose more. Two bends
/// (R ≈ 1.8·w and R ≈ 5.5·w) of the same guide, mode injected via TF/SF,
/// transmission measured on the vertical output arm with a *horizontal*
/// flux line (which also exercises the Sy monitor path). Qualitative
/// physics asserted (monotone in R, bounded by unity); the example
/// `bend_loss` prints the quantitative dB table at higher resolution.
#[test]
fn bend_loss_decreases_with_radius() {
    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 24.0;
    let (nx, ny) = (90, 92);
    let jc = 30usize;
    let yc = jc as f64 * delta;
    let xb = 45.0 * delta; // bend start

    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm).unwrap();

    let run = |radius: f64| -> f64 {
        let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(12));
        sim.fill_epsilon(n_clad * n_clad);
        // Horizontal arm, quarter-ring bend, vertical arm.
        sim.paint(
            &Shape2::rect(-1.0, yc - half_w, xb, yc + half_w),
            n_core * n_core,
        );
        sim.paint(
            &Shape2::ring(
                xb,
                yc + radius,
                radius - half_w,
                radius + half_w,
                -std::f64::consts::FRAC_PI_2,
                0.0,
                64,
            ),
            n_core * n_core,
        );
        sim.paint(
            &Shape2::rect(xb + radius - half_w, yc + radius, xb + radius + half_w, 1e9),
            n_core * n_core,
        );
        let (j0, j1) = (18usize, 42usize);
        let profile: Vec<f64> = (j0..=j1)
            .map(|j| mode.profile((j as f64 - jc as f64) * delta))
            .collect();
        sim.add_source(Source::mode_tfsf(
            20,
            j0,
            profile,
            mode.n_eff,
            Waveform::gaussian(f0, f0 / 4.0),
        ));
        let f_in = sim.add_flux(FluxSpec::Vertical {
            i: 32,
            j0,
            j1,
            freqs: vec![f0],
        });
        let ic = (xb + radius) / delta;
        let ic = ic.round() as usize;
        let f_out = sim.add_flux(FluxSpec::Horizontal {
            j: 70,
            i0: ic - 12,
            i1: ic + 12,
            freqs: vec![f0],
        });
        sim.run(1600);
        let p_in = sim.flux_power(f_in)[0].1;
        let p_out = sim.flux_power(f_out)[0].1;
        assert!(p_in > 0.0);
        p_out / p_in
    };

    let t_sharp = run(0.40);
    let t_gentle = run(1.20);
    println!("bend: T(R=0.40) = {t_sharp:.4}, T(R=1.20) = {t_gentle:.4}");
    assert!(
        t_gentle > t_sharp + 0.02,
        "bend loss not monotone in radius: {t_sharp} vs {t_gentle}"
    );
    assert!(
        t_sharp > 0.1 && t_sharp < 1.02,
        "sharp-bend T out of range: {t_sharp}"
    );
    assert!(
        t_gentle > 0.8 && t_gentle < 1.02,
        "gentle-bend T out of range: {t_gentle}"
    );
}

/// Rung 8 (M2) — the discrete adjoint against central differences, cell
/// by cell. One forward + one adjoint run produce dJ/dε for all ~200
/// design cells; three probe cells (core center, core edge, cladding)
/// are then checked against (J(ε+h) − J(ε−h))/2h with h = 1e−3 and the
/// **same frozen step count** for every run (comparing runs with
/// different windows contaminates gradients — the particle-optics
/// lesson).
///
/// Tolerance floor (named): FD truncation O(h²), FD roundoff on J
/// differences (~1e−10 relative), and the untransposed-CPML
/// approximation at the −95 dB reflection floor. 0.5 % relative on
/// probe cells clears all three with margin; measured agreement is
/// printed.
#[test]
fn adjoint_gradient_matches_finite_differences() {
    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 30.0;
    let (nx, ny) = (100, 40);
    let jc = 20usize;
    let yc = jc as f64 * delta;
    let steps = 1400;

    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm).unwrap();
    let (j0s, j1s) = (8usize, 32usize);
    let src_profile: Vec<f64> = (j0s..=j1s)
        .map(|j| mode.profile((j as f64 - jc as f64) * delta))
        .collect();

    let mut build = |with_sources: bool| -> Simulation {
        let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(10));
        sim.fill_epsilon(n_clad * n_clad);
        sim.paint(
            &Shape2::rect(-1.0, yc - half_w, 1e9, yc + half_w),
            n_core * n_core,
        );
        if with_sources {
            sim.add_source(Source::mode_tfsf(
                14,
                j0s,
                src_profile.clone(),
                mode.n_eff,
                Waveform::gaussian(f0, f0 / 4.0),
            ));
        }
        sim
    };

    let region = DesignRegion {
        i0: 40,
        i1: 54,
        j0: 14,
        j1: 26,
    };
    // Monitor rows must clear the 10-cell y-CPML (asserted by the
    // adjoint): overlap against the mode profile on rows 12..=28 only.
    let (j0m, j1m) = (12usize, 28usize);
    let mon_weights: Vec<f64> = (j0m..=j1m)
        .map(|j| mode.profile((j as f64 - jc as f64) * delta))
        .collect();
    let obj = ModeOverlap {
        i: 85,
        j0: j0m,
        weights: mon_weights,
        freq: f0,
    };

    let result = objective_and_gradient(&mut build, &region, &obj, steps);
    assert!(result.objective > 0.0, "objective vanished");

    // The gradient must live where the mode lives: the strongest cell
    // sits in the core rows.
    let mut best = (0usize, 0usize, 0.0f64);
    for di in 0..region.ns_x() {
        for dj in 0..region.ns_y() {
            let g = result.grad.at(di, dj).abs();
            if g > best.2 {
                best = (di, dj, g);
            }
        }
    }
    let core_lo = jc - 3 - region.j0;
    let core_hi = jc + 3 - region.j0;
    assert!(
        best.1 >= core_lo && best.1 <= core_hi,
        "strongest gradient at region row {} — outside the core band",
        best.1
    );

    // Central differences at three physically distinct cells.
    let h = 1e-3;
    let probes = [
        (48usize, 20usize, "core center"),
        (48usize, 22usize, "core edge"),
        (48usize, 25usize, "cladding"),
    ];
    let gmax = best.2;
    for (pi, pj, name) in probes {
        let mut plus = build(true);
        plus.perturb_epsilon_at(pi, pj, h);
        let (jp, _) = run_objective(plus, &obj, steps);
        let mut minus = build(true);
        minus.perturb_epsilon_at(pi, pj, -h);
        let (jm, _) = run_objective(minus, &obj, steps);
        let fd = (jp - jm) / (2.0 * h);
        let adj = result.grad.at(pi - region.i0, pj - region.j0);
        let rel = ((adj - fd) / fd).abs();
        println!("adjoint: {name} ({pi},{pj})  adj {adj:+.6e}  fd {fd:+.6e}  rel {rel:.2e}");
        assert!(
            (adj - fd).abs() <= 1e-4 * fd.abs().max(1e-3 * gmax),
            "{name}: adjoint {adj:e} vs FD {fd:e}"
        );
    }
}

/// Rung 9 (M3) — the full topology chain, end to end: dJ/dρ through
/// density → cone filter → projection → ε interpolation → FDTD → mode
/// overlap, adjoint + analytic chain rule vs central differences on raw
/// densities. The chain-rule stages are individually exact (filter
/// transpose and projection derivative have their own unit tests), so
/// the tolerance floor here is the M2 adjoint floor itself.
#[test]
fn topology_chain_gradient_matches_fd() {
    use vcad_kernel_photonics::TopologyParam;

    let lambda0 = 1.55;
    let f0 = 1.0 / lambda0;
    let (n_core, n_clad, half_w) = (3.48, 1.44, 0.11);
    let delta = lambda0 / 30.0;
    let (nx, ny) = (90, 40);
    let jc = 20usize;
    let yc = jc as f64 * delta;
    let steps = 1300;

    let mode = solve_slab_mode_even(n_core, n_clad, half_w, lambda0, Polarization::Tm).unwrap();
    let (j0s, j1s) = (12usize, 28usize);
    let src_profile: Vec<f64> = (j0s..=j1s)
        .map(|j| mode.profile((j as f64 - jc as f64) * delta))
        .collect();

    let region = DesignRegion {
        i0: 38,
        i1: 50,
        j0: 14,
        j1: 26,
    };
    // A structured (non-uniform) density so every chain stage is active.
    let mut topo = TopologyParam::uniform(region, 0.5, n_clad * n_clad, n_core * n_core);
    topo.beta = 4.0;
    topo.filter_radius_cells = 2.2;
    for (c, v) in topo.rho.iter_mut().enumerate() {
        *v = 0.5 + 0.35 * ((c as f64) * 0.7).sin();
    }
    let topo_master = topo.clone();

    let build_with = |t: &TopologyParam, with_sources: bool| -> Simulation {
        let mut sim = Simulation::new(GridSpec::new(nx, ny, delta), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(10));
        sim.fill_epsilon(n_clad * n_clad);
        sim.paint(
            &Shape2::rect(-1.0, yc - half_w, 1e9, yc + half_w),
            n_core * n_core,
        );
        t.apply(&mut sim);
        if with_sources {
            sim.add_source(Source::mode_tfsf(
                14,
                j0s,
                src_profile.clone(),
                mode.n_eff,
                Waveform::gaussian(f0, f0 / 4.0),
            ));
        }
        sim
    };

    let obj = ModeOverlap {
        i: 76,
        j0: j0s,
        weights: src_profile.clone(),
        freq: f0,
    };
    let _ = j1s;

    let mut build = |with_sources: bool| build_with(&topo_master, with_sources);
    let result = objective_and_gradient(&mut build, &region, &obj, steps);
    let d_j_d_rho = topo_master.chain_gradient(&result.grad);

    // FD through the whole chain at three density components.
    let h = 1e-3;
    let ry = region.ns_y();
    let cases = [
        (5 * ry + 6, "core-adjacent"),
        (7 * ry + 2, "cladding-side"),
        (10 * ry + 9, "far corner"),
    ];
    let gmax = d_j_d_rho.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(gmax > 0.0);
    for (c, name) in cases {
        let mut tp = topo_master.clone();
        tp.rho[c] += h;
        let (jp, _) = run_objective(build_with(&tp, true), &obj, steps);
        let mut tm = topo_master.clone();
        tm.rho[c] -= h;
        let (jm, _) = run_objective(build_with(&tm, true), &obj, steps);
        let fd = (jp - jm) / (2.0 * h);
        let adj = d_j_d_rho[c];
        let rel = ((adj - fd) / fd).abs();
        println!("topology: {name} c={c}  adj {adj:+.6e}  fd {fd:+.6e}  rel {rel:.2e}");
        assert!(
            (adj - fd).abs() <= 2e-4 * fd.abs().max(1e-3 * gmax),
            "{name}: chained adjoint {adj:e} vs FD {fd:e}"
        );
    }
}

/// Rung 10 (M3) — the spec seam round-trips through JSON and fails
/// closed: named parameters resolve only when bound; densities are
/// validated against the region.
#[test]
fn spec_seam_round_trips_and_fails_closed() {
    use std::collections::BTreeMap;
    use vcad_kernel_photonics::{ParamValue, SpecError, TopologyProblemSpec};

    let region = DesignRegion {
        i0: 30,
        i1: 49,
        j0: 10,
        j1: 29,
    };
    let mut spec = TopologyProblemSpec::new(1.55, 3.48, 1.44, 30, region);
    spec.wavelength = ParamValue::Named("lambda_design".into());
    spec.rho = vec![0.25; region.len()];

    let json = serde_json::to_string_pretty(&spec).unwrap();
    let back: TopologyProblemSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, spec);

    // Named params serialize as bare strings, literals as numbers.
    assert!(json.contains("\"lambda_design\""));

    // Fail-closed: no binding, no resolve.
    assert_eq!(
        back.resolve(&BTreeMap::new(), 8.0).unwrap_err(),
        SpecError::UnknownParameter("lambda_design".into())
    );
    let mut params = BTreeMap::new();
    params.insert("lambda_design".to_string(), 1.55);
    let resolved = back.resolve(&params, 8.0).unwrap();
    assert_eq!(resolved.param.rho, vec![0.25; region.len()]);
    assert!((resolved.param.eps_max - 3.48 * 3.48).abs() < 1e-12);
}
