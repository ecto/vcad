//! The M1 validation ladder: ground plane (image theory) and multi-wire
//! junctions against published antenna theory.

use vcad_kernel_antenna::constants::C0;
use vcad_kernel_antenna::farfield::{directivity_dbi, gain_dbi, radiation_efficiency};
use vcad_kernel_antenna::linalg::lu_decompose;
use vcad_kernel_antenna::mom::fill_impedance_matrix;
use vcad_kernel_antenna::{find_resonance, solve_driven, Complex, Mesh, SolveOptions, WireGrid};

const OPTS: SolveOptions = SolveOptions {
    quad_outer: 6,
    quad_inner: 6,
};

/// Balanis §4.7: a quarter-wave monopole over a perfect ground plane has
/// exactly half the dipole impedance (36.5 + j21.25 Ω ideal at ℓ = λ/4,
/// resonating slightly shorter with R ≈ 30–36 Ω) and doubled directivity:
/// 5.16 dBi at the horizon. Image theory must reproduce all three.
#[test]
fn quarter_wave_monopole_is_half_a_dipole() {
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 500.0], 1.0, 20)
        .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();

    // Resonance where ℓ = 0.5 m ≈ 0.24 λ → f ≈ 0.48 c / (2ℓ).
    let f_quarter = C0 / (4.0 * 0.5); // ℓ = λ/4 exactly
    let f_res = find_resonance(&mesh, feed, 0.80 * f_quarter, 1.05 * f_quarter, &OPTS).unwrap();
    let l_over_lambda = 0.5 * f_res / C0;
    assert!(
        (0.23..=0.245).contains(&l_over_lambda),
        "monopole resonant length ℓ/λ = {l_over_lambda:.4}, published ≈ 0.235–0.24"
    );
    let sol = solve_driven(&mesh, feed, f_res, &OPTS).unwrap();
    assert!(
        (30.0..=37.0).contains(&sol.z_in.re),
        "monopole resonant resistance {:.2} Ω, published ≈ 30–36.5 Ω (half a dipole)",
        sol.z_in.re
    );

    // Cross-check against the equivalent dipole solved without a ground
    // plane: Z_monopole = Z_dipole / 2 to discretization accuracy.
    let mut gd = WireGrid::new();
    gd.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 40)
        .unwrap();
    let dip = Mesh::build(&gd).unwrap();
    let dip_feed = dip.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let dsol = solve_driven(&dip, dip_feed, f_res, &OPTS).unwrap();
    let ratio = sol.z_in.abs() / dsol.z_in.abs();
    assert!(
        (ratio - 0.5).abs() < 0.02,
        "|Z_mono|/|Z_dip| = {ratio:.4}, image theory says exactly 0.5"
    );

    // Directivity at the horizon: 2 × dipole → 5.16 dBi (Balanis §4.7).
    let d = directivity_dbi(&mesh, &sol, std::f64::consts::FRAC_PI_2 - 1e-3, 0.0, 32);
    assert!(
        (4.9..=5.4).contains(&d),
        "monopole horizon directivity {d:.3} dBi, published 5.16 dBi"
    );

    // Energy balance over the hemisphere.
    let eff = radiation_efficiency(&mesh, &sol, 32);
    assert!(
        (0.97..=1.03).contains(&eff),
        "monopole energy balance P_rad/P_in = {eff:.4}"
    );

    // Nothing radiates below the PEC horizon.
    let below = vcad_kernel_antenna::farfield::far_field(&mesh, &sol, 2.5, 0.4);
    assert_eq!(below.intensity(), 0.0);
}

/// Balanis §4.8 (Fig. 4.31): a horizontal half-wave dipole's input
/// resistance vs height over PEC ground collapses at low height (the image
/// cancels the radiation: R → 0 as h → 0) and swings above the free-space
/// value near h ≈ 0.35 λ. Two points off that published curve.
#[test]
fn horizontal_dipole_resistance_tracks_height_over_ground() {
    let f = 149.896e6; // λ ≈ 2.0 m, dipole ℓ = 1 m = λ/2
    let lambda_mm = C0 / f * 1e3;
    let r_at = |h_over_lambda: f64| {
        let h = h_over_lambda * lambda_mm;
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([-500.0, 0.0, h], [500.0, 0.0, h], 1.0, 40)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, h]).unwrap();
        solve_driven(&mesh, feed, f, &OPTS).unwrap().z_in.re
    };

    let r_low = r_at(0.1);
    assert!(
        (10.0..=30.0).contains(&r_low),
        "R at h = 0.1 λ is {r_low:.2} Ω; Balanis Fig. 4.31 reads ≈ 20 Ω"
    );
    // First peak of the curve. Balanis draws it near 95–100 Ω for the
    // ideal 73 Ω dipole; our finite-radius dipole reads 82 Ω at exactly
    // λ/2 (see the M0 tests), and R(h) = R_self − R_mutual(2h) with
    // R_mutual(0.7 λ) ≈ −25 Ω puts the peak near 107 — the band brackets
    // that with the same ±10% the other rungs get.
    let r_third = r_at(0.35);
    assert!(
        (95.0..=122.0).contains(&r_third),
        "R at h = 0.35 λ is {r_third:.2} Ω; R_self − R_mutual(2h) predicts ≈ 107 Ω"
    );
    // And the limit behavior: lower is much smaller.
    assert!(r_low < 0.35 * r_third);
}

/// Image theory as an exact algebraic identity, not a band: a horizontal
/// dipole at height h over PEC must have *identical* Z_in to the same
/// dipole in free space with an explicit mirror twin at −h driven
/// antisymmetrically (V, −V). Same integrals, two different code paths —
/// they must agree to solver rounding, not physics tolerance.
#[test]
fn ground_solve_equals_antisymmetric_twin_solve_exactly() {
    let f = 149.9e6;
    let h = 350.0; // mm
    let n = 20;

    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    g.add_wire([-500.0, 0.0, h], [500.0, 0.0, h], 1.0, n)
        .unwrap();
    let mesh_g = Mesh::build(&g).unwrap();
    let feed_g = mesh_g.nearest_basis([0.0, 0.0, h]).unwrap();
    let z_ground = solve_driven(&mesh_g, feed_g, f, &OPTS).unwrap().z_in;

    let mut g2 = WireGrid::new();
    g2.add_wire([-500.0, 0.0, h], [500.0, 0.0, h], 1.0, n)
        .unwrap();
    g2.add_wire([-500.0, 0.0, -h], [500.0, 0.0, -h], 1.0, n)
        .unwrap();
    let mesh_2 = Mesh::build(&g2).unwrap();
    let feed_top = mesh_2.nearest_basis([0.0, 0.0, h]).unwrap();
    let feed_bot = mesh_2.nearest_basis([0.0, 0.0, -h]).unwrap();
    let k = 2.0 * std::f64::consts::PI * f / C0;
    let z2 = fill_impedance_matrix(&mesh_2, k, &OPTS);
    let lu = lu_decompose(z2).unwrap();
    let nb = mesh_2.bases.len();
    let mut rhs = vec![Complex::ZERO; nb];
    rhs[feed_top] = Complex::ONE;
    rhs[feed_bot] = -Complex::ONE;
    let currents = lu.solve(&rhs);
    let z_twin = Complex::ONE / currents[feed_top];

    // The two matrices differ only by the ground path's symmetrization of
    // the self-image blocks (a quadrature-level 1e-7 effect the twin path
    // has no reason to apply), so the agreement floor is quadrature
    // symmetry, not LU rounding — still four orders below any physics
    // tolerance in this suite.
    let rel = (z_ground - z_twin).abs() / z_ground.abs();
    assert!(
        rel < 1e-6,
        "image solve {z_ground:?} vs antisymmetric twin {z_twin:?} (rel {rel:.2e}) — \
         these are the same linear system through two code paths"
    );
}

/// Balanis §9.5: a folded dipole (two close parallel half-wave wires
/// joined at both ends) transforms the dipole impedance by the square of
/// the current division — 4 × 73 ≈ 292 Ω at resonance. This is the bent-
/// geometry rung: the loop closes through four corner nodes.
#[test]
fn folded_dipole_transforms_impedance_by_four() {
    let len = 1000.0; // mm, ≈ λ/2 at 143–150 MHz
    let sep = 25.0; // mm spacing between the two conductors
    let mut g = WireGrid::new();
    // Closed rectangular loop: bottom wire (fed), two shorting ends, top.
    let pts = [
        [-len / 2.0, 0.0, 0.0],
        [len / 2.0, 0.0, 0.0],
        [len / 2.0, 0.0, sep],
        [-len / 2.0, 0.0, sep],
    ];
    g.add_loop(&pts, 1.0, &[20, 1, 20, 1]).unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();

    let f_half = C0 / (2.0 * len * 1e-3);
    let f_res = find_resonance(&mesh, feed, 0.80 * f_half, 1.02 * f_half, &OPTS).unwrap();
    let sol = solve_driven(&mesh, feed, f_res, &OPTS).unwrap();
    assert!(
        (220.0..=360.0).contains(&sol.z_in.re),
        "folded dipole resonant R = {:.1} Ω, published ≈ 4 × 73 = 292 Ω",
        sol.z_in.re
    );
    // The 4:1 step-up against the plain dipole solved at the same f.
    let mut gd = WireGrid::new();
    gd.add_wire([-len / 2.0, 0.0, 0.0], [len / 2.0, 0.0, 0.0], 1.0, 40)
        .unwrap();
    let dip = Mesh::build(&gd).unwrap();
    let dip_feed = dip.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let dip_res = find_resonance(&dip, dip_feed, 0.80 * f_half, 1.05 * f_half, &OPTS).unwrap();
    let r_dip = solve_driven(&dip, dip_feed, dip_res, &OPTS)
        .unwrap()
        .z_in
        .re;
    let step_up = sol.z_in.re / r_dip;
    assert!(
        (3.2..=5.0).contains(&step_up),
        "impedance step-up = {step_up:.2}, transmission-line theory says 4"
    );
    // Energy balance holds through the bent geometry.
    let eff = radiation_efficiency(&mesh, &sol, 32);
    assert!(
        (0.97..=1.03).contains(&eff),
        "folded dipole balance {eff:.4}"
    );
}

/// Capacitive top loading (a T-hat on a monopole — a real degree-3
/// junction) lowers the resonant frequency and the resonant resistance:
/// the classic reason electrically-short verticals wear hats. Kraus,
/// *Antennas*, ch. 5; any AM broadcast tower.
#[test]
fn top_hat_junction_loads_the_monopole_down_in_frequency() {
    let h = 500.0; // mm mast
    let plain = {
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, h], 1.0, 16).unwrap();
        Mesh::build(&g).unwrap()
    };
    let hatted = {
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, h], 1.0, 16).unwrap();
        // Two horizontal arms at the top: degree-3 junction up there.
        g.add_wire([0.0, 0.0, h], [250.0, 0.0, h], 1.0, 8).unwrap();
        g.add_wire([0.0, 0.0, h], [-250.0, 0.0, h], 1.0, 8).unwrap();
        Mesh::build(&g).unwrap()
    };

    let f_quarter = C0 / (4.0 * h * 1e-3);
    let feed_p = plain.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let feed_h = hatted.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let f_plain =
        find_resonance(&plain, feed_p, 0.60 * f_quarter, 1.05 * f_quarter, &OPTS).unwrap();
    let f_hat = find_resonance(&hatted, feed_h, 0.40 * f_quarter, 1.05 * f_quarter, &OPTS).unwrap();
    let shift = f_hat / f_plain;
    assert!(
        (0.55..=0.90).contains(&shift),
        "top hat should pull resonance well down: f_hat/f_plain = {shift:.3}"
    );
    let r_hat = solve_driven(&hatted, feed_h, f_hat, &OPTS).unwrap().z_in.re;
    let r_plain = solve_driven(&plain, feed_p, f_plain, &OPTS)
        .unwrap()
        .z_in
        .re;
    assert!(
        r_hat < r_plain,
        "the shortened effective radiator must show lower R: {r_hat:.1} vs {r_plain:.1} Ω"
    );
    // Reciprocity survives junction bases: the matrix is still symmetric.
    let k = 2.0 * std::f64::consts::PI * f_hat / C0;
    let z = fill_impedance_matrix(&hatted, k, &OPTS);
    let mut worst: f64 = 0.0;
    for i in 0..z.n() {
        for j in 0..z.n() {
            worst = worst.max((z.at(i, j) - z.at(j, i)).abs());
        }
    }
    assert!(
        worst < 1e-12 * z.max_abs(),
        "junction fill asymmetry {worst:.3e}"
    );
    // And energy still balances through the junction.
    let sol = solve_driven(&hatted, feed_h, f_hat, &OPTS).unwrap();
    let eff = radiation_efficiency(&hatted, &sol, 32);
    assert!((0.97..=1.03).contains(&eff), "top-hat balance {eff:.4}");
}

/// A 3-element yagi (reflector, driven, director — parasitic elements
/// excited only by mutual coupling) must beam: forward gain well above a
/// lone dipole and a clear front-to-back ratio. Classic NBS/amateur yagi
/// data put a 3-element design at ≈ 7–9 dBi forward with F/B ≈ 8–20 dB.
#[test]
fn three_element_yagi_beams_forward() {
    // Standard proportions for a 3-element yagi near 146 MHz (λ ≈ 2.05 m):
    // reflector 0.495 λ at −0.2 λ, driven 0.473 λ, director 0.44 λ at +0.15 λ.
    let f = 146e6;
    let lambda = C0 / f * 1e3; // mm
    let mut g = WireGrid::new();
    let half = |frac: f64| frac * lambda / 2.0;
    // Elements along z, boom along x, beam toward +x.
    g.add_wire(
        [-0.2 * lambda, 0.0, -half(0.495)],
        [-0.2 * lambda, 0.0, half(0.495)],
        1.0,
        21,
    )
    .unwrap();
    g.add_wire([0.0, 0.0, -half(0.473)], [0.0, 0.0, half(0.473)], 1.0, 21)
        .unwrap();
    g.add_wire(
        [0.15 * lambda, 0.0, -half(0.44)],
        [0.15 * lambda, 0.0, half(0.44)],
        1.0,
        21,
    )
    .unwrap();
    // 21 segments per element: odd, but the center node exists because the
    // wire runs −h..+h through zero with an even node count... use nearest.
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let sol = solve_driven(&mesh, feed, f, &OPTS).unwrap();

    let fwd = gain_dbi(&mesh, &sol, std::f64::consts::FRAC_PI_2, 0.0);
    let back = gain_dbi(
        &mesh,
        &sol,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    assert!(
        (6.0..=10.5).contains(&fwd),
        "3-element yagi forward gain {fwd:.2} dBi, published ≈ 7–9 dBi"
    );
    let f_b = fwd - back;
    assert!(
        f_b > 6.0,
        "front-to-back ratio {f_b:.2} dB should be clearly positive"
    );
    // Parasitic elements really carry current (mutual coupling works).
    let refl_basis = mesh.nearest_basis([-0.2 * lambda, 0.0, 0.0]).unwrap();
    assert!(
        sol.currents[refl_basis].abs() > 0.1 * sol.currents[feed].abs(),
        "reflector should carry a substantial induced current"
    );
    let eff = radiation_efficiency(&mesh, &sol, 48);
    assert!((0.97..=1.03).contains(&eff), "yagi energy balance {eff:.4}");
}

/// Reciprocity across a ground plane: transmit/receive symmetry between a
/// monopole and an elevated horizontal dipole, port currents swapped —
/// the image fill must preserve the symmetric structure exactly.
#[test]
fn reciprocity_survives_images_and_junctions() {
    let f = 149.9e6;
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 480.0], 1.0, 10)
        .unwrap();
    // Elevated bent wire with a junction nearby.
    g.add_wire([800.0, 0.0, 300.0], [800.0, 0.0, 800.0], 1.0, 8)
        .unwrap();
    g.add_wire([800.0, 0.0, 800.0], [1100.0, 0.0, 800.0], 1.0, 5)
        .unwrap();
    g.add_wire([800.0, 0.0, 800.0], [500.0, 0.0, 800.0], 1.0, 5)
        .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let k = 2.0 * std::f64::consts::PI * f / C0;
    let z = fill_impedance_matrix(&mesh, k, &OPTS);
    let scale = z.max_abs();
    let mut worst: f64 = 0.0;
    for i in 0..z.n() {
        for j in 0..z.n() {
            worst = worst.max((z.at(i, j) - z.at(j, i)).abs());
        }
    }
    assert!(
        worst < 1e-12 * scale,
        "image+junction fill asymmetry {worst:.3e} vs scale {scale:.3e}"
    );

    let port_a = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    let port_b = mesh.nearest_basis([800.0, 0.0, 550.0]).unwrap();
    let lu = lu_decompose(z).unwrap();
    let n = mesh.bases.len();
    let mut rhs_a = vec![Complex::ZERO; n];
    rhs_a[port_a] = Complex::ONE;
    let mut rhs_b = vec![Complex::ZERO; n];
    rhs_b[port_b] = Complex::ONE;
    let y_ba = lu.solve(&rhs_a)[port_b];
    let y_ab = lu.solve(&rhs_b)[port_a];
    assert!(
        (y_ba - y_ab).abs() < 1e-9 * y_ba.abs(),
        "reciprocity over ground: {y_ba:?} vs {y_ab:?}"
    );
}
