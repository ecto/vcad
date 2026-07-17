//! The M5 face-off: this solver against the published NEC-2 sample runs.
//!
//! Reference values are transcribed verbatim from the NEC-2 Manual,
//! Part III (User's Guide), Burke & Poggio, Lawrence Livermore Laboratory
//! (WDBN v0.92 web edition, pp. 83–93) — the line-printer outputs shipped
//! with the code in 1981. NEC-2 uses a 3-term sinusoidal basis with point
//! matching; we use triangular Galerkin. Sources differ too (their
//! Example 2 uses the current-slope-discontinuity source, we use a delta
//! gap). Few-percent deltas are the expected physics of those choices;
//! order-10% would mean a bug. Comparisons are at equal *electrical*
//! geometry (NEC's λ-referenced frequencies map to ours via our exact c).

use vcad_kernel_antenna::{solve_driven, AntennaError, Complex, Mesh, SolveOptions, WireGrid};

const OPTS: SolveOptions = SolveOptions {
    quad_outer: 6,
    quad_inner: 6,
};

fn dipole(len_mm: f64, radius_mm: f64, nseg: usize) -> (Mesh, usize) {
    let mut g = WireGrid::new();
    g.add_wire(
        [0.0, 0.0, -len_mm / 2.0],
        [0.0, 0.0, len_mm / 2.0],
        radius_mm,
        nseg,
    )
    .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
    (mesh, feed)
}

/// NEC-2 User's Guide Example 1 (p. 83–84): center-fed dipole, 0.5 m tip
/// to tip, a = 0.001 m, λ = 1.0000 m. Printed: Z = 82.6979 + j46.3060 Ω
/// (7 segments, sinusoidal basis).
///
/// Ours at N = 64: 86.18 + j46.27 — reactance within 0.04 Ω, resistance
/// 4.2% above NEC's coarse-mesh value, |ΔZ|/|Z| = 3.7%.
#[test]
fn nec2_example_1_half_wave_dipole() {
    let nec = Complex::new(82.6979, 46.3060);
    let (mesh, feed) = dipole(500.0, 1.0, 64);
    let sol = solve_driven(&mesh, feed, 299.792458e6, &OPTS).unwrap();
    let dz = (sol.z_in - nec).abs() / nec.abs();
    assert!(
        dz < 0.06,
        "Example 1: ours {:?} vs NEC-2 {:?} (|ΔZ|/|Z| = {:.3})",
        sol.z_in,
        nec,
        dz
    );
    assert!(
        (sol.z_in.im - nec.im).abs() < 4.0,
        "X delta too large: {:.2} vs {:.2}",
        sol.z_in.im,
        nec.im
    );
}

/// NEC-2 User's Guide Example 2 (p. 87–90): the same dipole with an
/// ultra-thin wire (a = 1e-5 m ≈ 1e-5 λ), swept 200/250/300 MHz with the
/// current-slope-discontinuity source. Printed impedance table:
///
/// ```text
/// 200 MHz   26.5762 − j632.060
/// 250 MHz   47.1431 − j272.372
/// 300 MHz   80.5511 + j45.7144
/// ```
///
/// Ours (N = 64, delta gap): 25.61 − j608.1 / 45.57 − j261.8 /
/// 78.17 + j45.42 — every point within 4% of |Z| across a reactance
/// swing from −632 Ω through resonance.
#[test]
fn nec2_example_2_thin_dipole_sweep() {
    let cases = [
        (200e6, Complex::new(26.5762, -632.060)),
        (250e6, Complex::new(47.1431, -272.372)),
        (300e6, Complex::new(80.5511, 45.7144)),
    ];
    for (f, nec) in cases {
        let (mesh, feed) = dipole(500.0, 0.01, 64);
        let sol = solve_driven(&mesh, feed, f, &OPTS).unwrap();
        let dz = (sol.z_in - nec).abs() / nec.abs();
        assert!(
            dz < 0.06,
            "Example 2 at {:.0} MHz: ours {:?} vs NEC-2 {:?} (|ΔZ|/|Z| = {:.3})",
            f / 1e6,
            sol.z_in,
            nec,
            dz
        );
    }
    // Structure: the sweep crosses from strongly capacitive to inductive,
    // same as the printed table.
    let (mesh, feed) = dipole(500.0, 0.01, 64);
    let x200 = solve_driven(&mesh, feed, 200e6, &OPTS).unwrap().z_in.im;
    let x300 = solve_driven(&mesh, feed, 300e6, &OPTS).unwrap().z_in.im;
    assert!(x200 < -500.0 && x300 > 0.0);
}

/// NEC-2 User's Guide Example 3 (p. 92–93): vertical half-wave antenna
/// over perfect ground, 5 m long, wire radius **0.3 m**, 9 segments at
/// 30 MHz — Δ/a = 1.85 and k·a = 0.19. NEC-2 needed its extended
/// thin-wire kernel (the EK card) for exactly this geometry.
///
/// This crate implements the standard kernel with hard validity gates, so
/// the correct behavior on Example 3 is to REFUSE — fail-closed, naming
/// the violated limit — rather than emit standard-kernel numbers for a
/// wire the kernel cannot represent. (The extended kernel is future work;
/// the printed NEC value 106.44 + j99.06 Ω becomes reproducible then.)
#[test]
fn nec2_example_3_fat_wire_fails_closed_without_the_extended_kernel() {
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    g.add_wire([0.0, 0.0, 2000.0], [0.0, 0.0, 7000.0], 300.0, 9)
        .unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 2000.0]).unwrap();
    match solve_driven(&mesh, feed, 30e6, &OPTS) {
        Err(AntennaError::ThinWireViolation {
            length_mm,
            radius_mm,
            ..
        }) => {
            // Δ = 5000/9 ≈ 555.6 mm < 4a = 1200 mm — the gate names it.
            assert!((length_mm - 5000.0 / 9.0).abs() < 1.0);
            assert_eq!(radius_mm, 300.0);
        }
        other => panic!("expected the thin-wire gate to refuse Example 3, got {other:?}"),
    }
}
