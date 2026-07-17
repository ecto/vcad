//! The M0 validation ladder: the solver against published antenna theory,
//! end-to-end through the public API.
//!
//! Sources cited per test. The discipline (from `vcad-kernel-particle`):
//! every rung is an analytic or published result, tolerances state *why*
//! they are what they are, and the convergence study names its own floor.

use vcad_kernel_antenna::constants::C0;
use vcad_kernel_antenna::farfield::{directivity_dbi, far_field, radiation_efficiency};
use vcad_kernel_antenna::linalg::lu_decompose;
use vcad_kernel_antenna::mom::fill_impedance_matrix;
use vcad_kernel_antenna::{
    find_resonance, solve_driven, sweep, AntennaError, Complex, Mesh, SolveOptions, WireGrid,
};

const OPTS: SolveOptions = SolveOptions {
    quad_outer: 6,
    quad_inner: 6,
};

fn dipole_mesh(len_mm: f64, radius_mm: f64, nseg: usize) -> Mesh {
    let mut g = WireGrid::new();
    g.add_wire(
        [0.0, 0.0, -len_mm / 2.0],
        [0.0, 0.0, len_mm / 2.0],
        radius_mm,
        nseg,
    )
    .unwrap();
    Mesh::build(&g).unwrap()
}

fn center_fed(mesh: &Mesh) -> usize {
    mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap()
}

/// Balanis, *Antenna Theory* (4th ed.), §4.6/§8.8: the ideal (sinusoidal
/// current) half-wave dipole has Z_in = 73 + j42.5 Ω. A finite-radius MoM
/// solution at *exactly* ℓ = λ/2 sits somewhat higher in R (the current is
/// not quite sinusoidal; NEC-class codes read ~80–90 Ω for thin wires) —
/// the tight checks live in the resonance test below. Bands here bracket
/// the published ideal and the known finite-radius shift.
#[test]
fn half_wave_dipole_impedance_is_in_the_published_band() {
    let len_mm = 1000.0;
    let f = C0 / (2.0 * len_mm * 1e-3); // ℓ = λ/2 exactly
    let mesh = dipole_mesh(len_mm, 1.0, 40); // ℓ/a = 1000
    let sol = solve_driven(&mesh, center_fed(&mesh), f, &OPTS).unwrap();
    let z = sol.z_in;
    assert!(
        (70.0..=92.0).contains(&z.re),
        "half-wave R = {:.2} Ω outside [70, 92] (ideal 73, finite-radius MoM higher)",
        z.re
    );
    assert!(
        (30.0..=58.0).contains(&z.im),
        "half-wave X = {:.2} Ω outside [30, 58] (ideal +42.5)",
        z.im
    );
}

/// Balanis §4.6: a real dipole resonates slightly *below* λ/2 — at
/// ℓ ≈ 0.47–0.48 λ for practical thickness ratios — with resonant
/// resistance ≈ 65–73 Ω (approaching 73 Ω as the wire thins). ℓ/a = 1000
/// here, so the thin end of the band.
#[test]
fn dipole_resonates_slightly_below_half_wave_at_the_published_resistance() {
    let len_mm = 1000.0;
    let mesh = dipole_mesh(len_mm, 1.0, 40);
    let feed = center_fed(&mesh);
    let f_half = C0 / (2.0 * len_mm * 1e-3);
    let f_res = find_resonance(&mesh, feed, 0.80 * f_half, 1.05 * f_half, &OPTS).unwrap();

    let l_over_lambda = len_mm * 1e-3 * f_res / C0;
    assert!(
        (0.46..=0.49).contains(&l_over_lambda),
        "resonant length ℓ/λ = {l_over_lambda:.4} outside the published 0.46–0.49"
    );
    let sol = solve_driven(&mesh, feed, f_res, &OPTS).unwrap();
    assert!(
        sol.z_in.im.abs() < 0.5,
        "X at found resonance should be ~0, got {:.3} Ω",
        sol.z_in.im
    );
    assert!(
        (60.0..=74.0).contains(&sol.z_in.re),
        "resonant resistance {:.2} Ω outside the published 60–74 Ω",
        sol.z_in.re
    );
}

/// Balanis §4.6: half-wave dipole directivity is 1.643 → 2.15 dBi. The
/// solved current is near-sinusoidal, so the pattern integral should land
/// within a tenth of a dB or so.
#[test]
fn half_wave_dipole_directivity_is_2p15_dbi() {
    let mesh = dipole_mesh(1000.0, 1.0, 40);
    let feed = center_fed(&mesh);
    let f_half = C0 / 2.0;
    let f_res = find_resonance(&mesh, feed, 0.80 * f_half, 1.05 * f_half, &OPTS).unwrap();
    let sol = solve_driven(&mesh, feed, f_res, &OPTS).unwrap();
    let d = directivity_dbi(&mesh, &sol, std::f64::consts::FRAC_PI_2, 0.0, 32);
    assert!(
        (2.0..=2.3).contains(&d),
        "broadside directivity {d:.3} dBi, published 2.15 dBi"
    );
}

/// Balanis §5.2: the electrically small loop has radiation resistance
/// R_r = 20π²(C/λ)⁴ for a circular loop of circumference C — equivalently
/// R_r = 320π⁴(A/λ²)² in terms of enclosed area A, the form that applies
/// to any small planar shape (the field is set by the magnetic dipole
/// moment I·A). Both the absolute value and the fourth-power scaling are
/// asserted, on a regular 16-gon at C ≈ 0.08λ and 0.12λ.
#[test]
fn small_loop_radiation_resistance_follows_the_fourth_power_law() {
    let f = 30e6;
    let lambda = C0 / f;

    let solve_loop = |c_over_lambda: f64| -> (f64, f64, Mesh, vcad_kernel_antenna::DrivenSolution) {
        let n = 16;
        let circumference = c_over_lambda * lambda; // m
        let r_circ = circumference / (2.0 * n as f64 * (std::f64::consts::PI / n as f64).sin());
        let pts: Vec<[f64; 3]> = (0..n)
            .map(|i| {
                let th = std::f64::consts::TAU * i as f64 / n as f64;
                [r_circ * 1e3 * th.cos(), r_circ * 1e3 * th.sin(), 0.0]
            })
            .collect();
        let mut g = WireGrid::new();
        g.add_loop(&pts, 1.0, &vec![1; n]).unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis(pts[0]).unwrap();
        let sol = solve_driven(&mesh, feed, f, &OPTS).unwrap();
        // Polygon area (planar, convex, about the origin).
        let area = 0.5 * n as f64 * r_circ * r_circ * (std::f64::consts::TAU / n as f64).sin();
        let r_analytic = 320.0 * std::f64::consts::PI.powi(4) * (area / (lambda * lambda)).powi(2);
        (sol.z_in.re, r_analytic, mesh, sol)
    };

    let (r1, a1, _, _) = solve_loop(0.08);
    let (r2, a2, mesh2, sol2) = solve_loop(0.12);

    // Absolute value: within 25% (thin-wire + small-loop higher-order
    // terms both scale as (C/λ)², percent-level here; quadrature is finer).
    assert!(
        (r1 - a1).abs() < 0.25 * a1,
        "R_r(0.08λ) = {r1:.5} Ω vs analytic {a1:.5} Ω"
    );
    assert!(
        (r2 - a2).abs() < 0.25 * a2,
        "R_r(0.12λ) = {r2:.5} Ω vs analytic {a2:.5} Ω"
    );
    // The law itself: R_r ∝ C⁴ at fixed shape → ratio (0.12/0.08)⁴ = 5.06.
    let ratio = r2 / r1;
    assert!(
        (ratio - 5.0625).abs() < 0.25 * 5.0625,
        "fourth-power scaling ratio {ratio:.3}, expected ≈ 5.06"
    );
    // Small loop is inductive, radiates the sin²θ doughnut about its axis
    // (D = 1.5 → 1.76 dBi), and must balance energy like everything else.
    assert!(sol2.z_in.im > 0.0, "small loop must be inductive");
    let d = directivity_dbi(&mesh2, &sol2, std::f64::consts::FRAC_PI_2, 0.0, 32);
    assert!(
        (1.5..=2.0).contains(&d),
        "small-loop directivity {d:.3} dBi, published 1.76 dBi"
    );
    let eff = radiation_efficiency(&mesh2, &sol2, 32);
    assert!(
        (0.95..=1.05).contains(&eff),
        "loop energy balance P_rad/P_in = {eff:.4}"
    );
}

/// Segment-refinement convergence, and the floor the thin-wire kernel
/// names for itself: Z_in settles to ~1% by N ≈ 32–48 for a 1 mm wire,
/// and pushing segment length below 4a is a hard error, not a slow drift
/// into garbage — the mesh that would need it is out of kernel validity.
#[test]
fn segment_convergence_reaches_percent_level_and_the_floor_errors() {
    let f = 143.6e6; // near resonance, where Z is O(70 Ω) and well-conditioned
    let z_at = |nseg: usize| {
        let mesh = dipole_mesh(1000.0, 1.0, nseg);
        solve_driven(&mesh, center_fed(&mesh), f, &OPTS)
            .unwrap()
            .z_in
    };
    let z_ref = z_at(48);
    let mut last_rel = f64::INFINITY;
    for nseg in [12, 16, 24, 32] {
        let rel = (z_at(nseg) - z_ref).abs() / z_ref.abs();
        assert!(
            rel < last_rel * 1.2,
            "refinement must not diverge: N={nseg} rel {rel:.4} after {last_rel:.4}"
        );
        last_rel = rel;
    }
    assert!(
        last_rel < 0.02,
        "N=32 vs N=48 differ by {last_rel:.4}, expected < 2%"
    );

    // The floor: a 5 mm-radius wire cut into 56 segments has 17.9 mm
    // segments < 4a = 20 mm → fail-closed, by design.
    let mesh = dipole_mesh(1000.0, 5.0, 56);
    match solve_driven(&mesh, center_fed(&mesh), f, &OPTS) {
        Err(AntennaError::ThinWireViolation { .. }) => {}
        other => panic!("expected the thin-wire floor to error, got {other:?}"),
    }
}

/// Reciprocity: the Galerkin matrix is symmetric by construction, so the
/// transfer admittance between any two ports must be identical with
/// source and load swapped — checked here as a two-element transmit/
/// receive link and as two off-center ports on one wire.
#[test]
fn reciprocity_holds_to_machine_precision() {
    let f = 149.9e6;

    // (a) Two parallel 1 m dipoles, half a wavelength apart.
    let mut g = WireGrid::new();
    g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 16)
        .unwrap();
    g.add_wire([1000.0, 0.0, -500.0], [1000.0, 0.0, 500.0], 1.0, 16)
        .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let port_a = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let port_b = mesh.nearest_basis([1000.0, 0.0, 0.0]).unwrap();
    assert_ne!(port_a, port_b);

    let k = 2.0 * std::f64::consts::PI * f / C0;
    let z = fill_impedance_matrix(&mesh, k, &OPTS);
    let lu = lu_decompose(z).unwrap();
    let n = mesh.bases.len();
    let mut rhs_a = vec![Complex::ZERO; n];
    rhs_a[port_a] = Complex::ONE;
    let mut rhs_b = vec![Complex::ZERO; n];
    rhs_b[port_b] = Complex::ONE;
    let y_ba = lu.solve(&rhs_a)[port_b]; // current at B from 1 V at A
    let y_ab = lu.solve(&rhs_b)[port_a]; // current at A from 1 V at B
    assert!(
        (y_ba - y_ab).abs() < 1e-9 * y_ba.abs(),
        "link reciprocity: Y_ba = {y_ba:?}, Y_ab = {y_ab:?}"
    );

    // (b) Two off-center ports on a single wire.
    let mesh = dipole_mesh(1000.0, 1.0, 20);
    let p1 = mesh.nearest_basis([0.0, 0.0, -250.0]).unwrap();
    let p2 = mesh.nearest_basis([0.0, 0.0, 250.0]).unwrap();
    let z = fill_impedance_matrix(&mesh, k, &OPTS);
    let lu = lu_decompose(z).unwrap();
    let n = mesh.bases.len();
    let mut rhs_1 = vec![Complex::ZERO; n];
    rhs_1[p1] = Complex::ONE;
    let mut rhs_2 = vec![Complex::ZERO; n];
    rhs_2[p2] = Complex::ONE;
    let y_21 = lu.solve(&rhs_1)[p2];
    let y_12 = lu.solve(&rhs_2)[p1];
    assert!(
        (y_21 - y_12).abs() < 1e-9 * y_21.abs(),
        "same-wire reciprocity: {y_21:?} vs {y_12:?}"
    );
}

/// Energy balance: for a lossless wire the power through the far sphere
/// equals the power accepted at the feed. The two sides come from
/// different integrals (radiation-zone phase integral vs the Galerkin
/// quadratic form), so ±3% is a genuine cross-check of the whole chain,
/// asserted off-resonance too.
#[test]
fn radiated_power_matches_feed_power_across_lengths() {
    for l_over_lambda in [0.30, 0.479, 0.70] {
        let f = 143.6e6;
        let lambda_mm = C0 / f * 1e3;
        let mesh = dipole_mesh(l_over_lambda * lambda_mm, 1.0, 32);
        let sol = solve_driven(&mesh, center_fed(&mesh), f, &OPTS).unwrap();
        let eff = radiation_efficiency(&mesh, &sol, 32);
        assert!(
            (0.97..=1.03).contains(&eff),
            "ℓ/λ = {l_over_lambda}: P_rad/P_in = {eff:.4}"
        );
    }
}

/// The S11 story a NanoVNA will eventually retell: sweeping through
/// resonance, |S11| against 50 Ω dips where Im(Z) crosses zero, and the
/// dip depth matches Γ = (R − 50)/(R + 50) for the resonant R ≈ 72 Ω.
#[test]
fn s11_sweep_dips_at_resonance() {
    let mesh = dipole_mesh(1000.0, 1.0, 40);
    let feed = center_fed(&mesh);
    let f_half = C0 / 2.0;
    let f_res = find_resonance(&mesh, feed, 0.80 * f_half, 1.05 * f_half, &OPTS).unwrap();

    let n = 41;
    let freqs: Vec<f64> = (0..n)
        .map(|i| f_res * (0.85 + 0.30 * i as f64 / (n - 1) as f64))
        .collect();
    let pts = sweep(&mesh, feed, &freqs, 50.0, &OPTS).unwrap();

    let (i_min, best) = pts
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.s11_db.partial_cmp(&b.1.s11_db).unwrap())
        .unwrap();
    assert!(
        (best.freq_hz - f_res).abs() < 0.015 * f_res,
        "S11 minimum at {:.2} MHz vs resonance {:.2} MHz",
        best.freq_hz / 1e6,
        f_res / 1e6
    );
    assert!(
        best.s11_db < -14.0,
        "dip should reach ≈ −15 dB for R ≈ 72 Ω vs 50 Ω, got {:.2} dB",
        best.s11_db
    );
    assert!(
        pts.first().unwrap().s11_db > -5.0 && pts.last().unwrap().s11_db > -5.0,
        "band edges should be badly matched (it is a dip, not a plateau)"
    );
    assert!(
        i_min > 0 && i_min < n - 1,
        "dip must be interior to the sweep"
    );
}

/// The far field of the resonant dipole is θ-polarized, azimuthally
/// uniform, and null on axis — asserted through the public API (the
/// module tests cover the same at unit level).
#[test]
fn dipole_pattern_shape_is_physical() {
    let mesh = dipole_mesh(1000.0, 1.0, 40);
    let feed = center_fed(&mesh);
    let sol = solve_driven(&mesh, feed, 143.6e6, &OPTS).unwrap();
    let broadside = far_field(&mesh, &sol, std::f64::consts::FRAC_PI_2, 1.0);
    let axial = far_field(&mesh, &sol, 0.01, 0.0);
    assert!(broadside.e_phi.abs() < 1e-9 * broadside.e_theta.abs());
    assert!(axial.intensity() < 1e-3 * broadside.intensity());
}
