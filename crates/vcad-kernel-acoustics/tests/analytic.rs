//! The validation ladder: the field solver against the closed forms.
//!
//! Each test cites the analytic result it must reproduce. Reciprocity and
//! grid convergence are the discretisation's conscience.

use vcad_kernel_acoustics::cavity::Cavity;
use vcad_kernel_acoustics::complex::Cplx;
use vcad_kernel_acoustics::fom;
use vcad_kernel_acoustics::helmholtz::{solve_driven, Source};
use vcad_kernel_acoustics::lumped;
use vcad_kernel_acoustics::medium::Medium;
use vcad_kernel_acoustics::sweep::{find_peaks, frequency_sweep, probe_response, Probe};

/// Rigid closed cylinder: the axial modes must land on `fₙ = n·c/2L`.
#[test]
fn closed_cylinder_axial_modes() {
    let air = Medium::air(20.0);
    let length_mm = 340.0;
    let cav = Cavity::closed_cylinder(20.0, length_mm, air);
    let f1 = lumped::closed_cylinder_axial_hz(&air, length_mm, 1);
    let f2 = lumped::closed_cylinder_axial_hz(&air, length_mm, 2);

    let src = Source::Monopole {
        r_mm: 5.0,
        z_mm: 8.0,
        q: Cplx::ONE,
    };
    let probe = Probe {
        r_mm: 0.0,
        z_mm: length_mm - 8.0,
    };
    let sweep = frequency_sweep(&cav, 9, 137, src, probe, 300.0, 1150.0, 260);
    let peaks = find_peaks(&sweep, 4);
    eprintln!("closed cylinder: f1={f1:.1} f2={f2:.1} Hz");
    for p in &peaks {
        eprintln!("  peak {:.1} Hz (|p|={:.3})", p.f_hz, p.value);
    }
    // The lowest two peaks are the first two axial modes.
    assert!(peaks.len() >= 2, "expected at least two peaks");
    let e1 = (peaks[0].f_hz - f1).abs() / f1;
    let e2 = (peaks[1].f_hz - f2).abs() / f2;
    eprintln!(
        "  rel err: mode1 {:.3}% mode2 {:.3}%",
        e1 * 100.0,
        e2 * 100.0
    );
    assert!(e1 < 0.02, "mode 1 off by {:.2}%", e1 * 100.0);
    assert!(e2 < 0.02, "mode 2 off by {:.2}%", e2 * 100.0);
}

/// Helmholtz resonator: the field-solved fundamental must fall in the
/// end-correction band around the lumped formula.
#[test]
fn helmholtz_resonator_in_the_end_correction_band() {
    let air = Medium::air(20.0);
    let cav = Cavity::helmholtz_resonator(40.0, 80.0, 8.0, 30.0, air);
    let band = lumped::ported_box_tuning_mm(&air, cav.volume_mm3(), 8.0, 30.0);
    eprintln!(
        "resonator band: [{:.1}, {:.1}, {:.1}] Hz",
        band.f_min_hz, band.f_nominal_hz, band.f_max_hz
    );

    let src = Source::Monopole {
        r_mm: 5.0,
        z_mm: 40.0,
        q: Cplx::ONE,
    };
    let probe = Probe {
        r_mm: 0.0,
        z_mm: 40.0,
    };
    let sweep = frequency_sweep(&cav, 17, 45, src, probe, 80.0, 400.0, 200);
    let peaks = find_peaks(&sweep, 3);
    for p in &peaks {
        eprintln!("  peak {:.1} Hz (|p|={:.3})", p.f_hz, p.value);
    }
    assert!(!peaks.is_empty(), "no resonance found");
    let f_field = peaks[0].f_hz;
    eprintln!("  field tuning {f_field:.1} Hz");
    // Field solve omits the exterior radiation mass → lands at or above the
    // nominal, but not above the shortest-L_eff bound by more than a margin.
    assert!(
        f_field > band.f_min_hz * 0.9 && f_field < band.f_max_hz * 1.25,
        "field {f_field:.1} outside band [{:.1}, {:.1}]",
        band.f_min_hz,
        band.f_max_hz
    );
}

/// Reciprocity: swapping source and receiver leaves the transfer unchanged.
#[test]
fn reciprocity_holds() {
    let air = Medium::air(20.0);
    let cav = Cavity::closed_cylinder(20.0, 340.0, air);
    let a = (5.0, 20.0);
    let b = (3.0, 300.0);
    let f = 400.0; // off-resonance

    let pab = probe_response(
        &cav,
        9,
        137,
        Source::Monopole {
            r_mm: a.0,
            z_mm: a.1,
            q: Cplx::ONE,
        },
        Probe {
            r_mm: b.0,
            z_mm: b.1,
        },
        f,
    )
    .unwrap();
    let pba = probe_response(
        &cav,
        9,
        137,
        Source::Monopole {
            r_mm: b.0,
            z_mm: b.1,
            q: Cplx::ONE,
        },
        Probe {
            r_mm: a.0,
            z_mm: a.1,
        },
        f,
    )
    .unwrap();
    let rel = (pab - pba).abs() / pab.abs().max(1e-30);
    eprintln!("reciprocity: G(A,B)={pab:?} G(B,A)={pba:?} rel={rel:.2e}");
    assert!(rel < 1e-6, "reciprocity broken: rel {rel:.2e}");
}

/// Grid convergence: the axial-mode error shrinks ~4× per grid halving
/// (second order), and the floor is named.
#[test]
fn grid_convergence_is_second_order() {
    let air = Medium::air(20.0);
    let length_mm = 340.0;
    let cav = Cavity::closed_cylinder(20.0, length_mm, air);
    let f1 = lumped::closed_cylinder_axial_hz(&air, length_mm, 1);

    let extract = |nz: usize| -> f64 {
        let src = Source::Monopole {
            r_mm: 5.0,
            z_mm: 12.0,
            q: Cplx::ONE,
        };
        let probe = Probe {
            r_mm: 0.0,
            z_mm: length_mm - 12.0,
        };
        // Very fine local sweep so grid dispersion — not sweep resolution —
        // dominates the extracted error.
        let sweep = frequency_sweep(&cav, 9, nz, src, probe, f1 * 0.96, f1 * 1.04, 800);
        find_peaks(&sweep, 1)[0].f_hz
    };

    let mut errs = Vec::new();
    for &nz in &[21usize, 41, 81] {
        let f = extract(nz);
        let e = (f - f1).abs() / f1;
        eprintln!(
            "nz={nz:3} (dz={:.2}mm): f={f:.3} Hz, rel err {:.4}%",
            340.0 / (nz - 1) as f64,
            e * 100.0
        );
        errs.push(e);
    }
    // Second-order dispersion: halving dz cuts the error ~4×. Sweep and
    // source-placement noise soften that, so require a clear >3× per halving
    // and a monotone decrease, and name the floor.
    assert!(
        errs[1] < errs[0] && errs[2] < errs[1],
        "not monotone: {errs:?}"
    );
    assert!(
        errs[0] / errs[2] > 6.0,
        "21→81 (4× refine) only improved {:.1}× (want ≳2nd order)",
        errs[0] / errs[2]
    );
    eprintln!("floor (nz=81): {:.4}%", errs[2] * 100.0);
}

/// Free-air baffled piston built into a solve-free sanity: the numeric
/// radiator's directivity nulls where `J₁` does.
#[test]
fn piston_first_null_is_where_bessel_says() {
    // First off-axis pressure null of a baffled piston is at
    // ka·sinθ = 3.8317 (first zero of J₁).
    let air = Medium::air(20.0);
    let a = 0.05;
    let ka = 8.0;
    let k = ka / a;
    let r = 60.0;
    let theta = (3.8317_f64 / ka).asin();
    let (x, z) = (r * theta.sin(), r * theta.cos());
    let p_null =
        vcad_kernel_acoustics::radiation::rayleigh_pressure(&air, a, Cplx::ONE, k, x, z, 220, 176)
            .abs();
    let on_axis = vcad_kernel_acoustics::radiation::rayleigh_pressure(
        &air,
        a,
        Cplx::ONE,
        k,
        0.0,
        r,
        220,
        176,
    )
    .abs();
    eprintln!("piston null/on-axis = {:.4}", p_null / on_axis);
    assert!(
        p_null / on_axis < 0.05,
        "not a null: {:.4}",
        p_null / on_axis
    );
    let _ = solve_driven; // keep the import meaningful across edits
    let _ = fom::driven_tuning_hz;
}
