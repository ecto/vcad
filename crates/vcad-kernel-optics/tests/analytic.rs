//! The validation ladder: every rung is a closed-form result from the
//! optics literature, recovered by the exact tracer / paraxial trace.
//!
//! Citations per rung:
//! - Thick-lens EFL/BFD closed forms: Hecht, *Optics* (5th ed.), §6.1.
//! - Thin-lens Seidel spherical aberration and the best-form U-curve:
//!   Jenkins & White, *Fundamentals of Optics* (4th ed.), §9.5.
//! - Chromatic focal shift f/V from the Abbe number: standard first-order
//!   dispersion result (e.g. Smith, *Modern Optical Engineering*, ch. 3).
//! - Achromat condition φ₁/φ = V₁/(V₁−V₂): Dollond (1758) via any text.
//! - Published prescription: Thorlabs AC254-075-A (N-BK7/SF5 cemented
//!   doublet, R 46.5 / −33.9 / −95.5 mm, tc 7.0 / 2.5 mm, EFL 74.9 mm),
//!   catalog data via the 3DOptix mirror of Thorlabs specs, fetched
//!   2026-07-17: <https://www.3doptix.com/catalog/optics/lens/thorlabs/AC254-075-A>.

use vcad_kernel_optics::glass::Glass;
use vcad_kernel_optics::lines;
use vcad_kernel_optics::paraxial::{first_order, first_order_matrix};
use vcad_kernel_optics::prescription::{Prescription, Surface};
use vcad_kernel_optics::spot::analyze;
use vcad_kernel_optics::thirdorder;
use vcad_kernel_optics::trace::{trace_to_image, Ray, RayFate, Vec3};

/// Bending-parameterized singlet: shape factor q at fixed thin-lens focal
/// length `f` for a dispersionless glass of index `n`.
fn singlet_from_q(f: f64, q: f64, n: f64, thickness: f64, semi_d: f64) -> Prescription {
    let dc = 1.0 / ((n - 1.0) * f); // c1 − c2
    let c1 = (q + 1.0) * dc / 2.0;
    let c2 = (q - 1.0) * dc / 2.0;
    let r = |c: f64| if c == 0.0 { f64::INFINITY } else { 1.0 / c };
    Prescription::new(vec![
        Surface::sphere(
            r(c1),
            semi_d,
            thickness,
            Glass::Constant {
                name: "n-const".into(),
                nd: n,
            },
        ),
        Surface::sphere(r(c2), semi_d, 0.0, Glass::Air),
    ])
    .unwrap()
}

/// Axis-crossing z of the marginal ray launched parallel at height `h`
/// (meridional in x), found from two image-plane samples.
fn marginal_axis_crossing(p: &Prescription, lambda: f64, h: f64, near_z: f64) -> f64 {
    let ray = Ray {
        p: Vec3::new(h, 0.0, -10.0),
        d: Vec3::new(0.0, 0.0, 1.0),
    };
    let x_at = |z: f64| -> f64 {
        match trace_to_image(p, lambda, ray, z, false).fate {
            RayFate::Imaged(pt) => pt.x,
            other => panic!("marginal ray failed: {other:?}"),
        }
    };
    let (z1, z2) = (near_z - 5.0, near_z + 5.0);
    let (x1, x2) = (x_at(z1), x_at(z2));
    z1 - x1 * (z2 - z1) / (x2 - x1)
}

/// Rung 1: the paraxial trace reproduces the thick-lens closed forms
/// 1/f = (n−1)[c₁ − c₂ + (n−1)·t·c₁c₂/n] and BFD = f·(1 − (n−1)·t·c₁/n)
/// (Hecht §6.1) to first-order machine precision.
#[test]
fn thick_lens_closed_forms() {
    let (r1, r2, t, n) = (60.0, -40.0, 8.0, 1.5168);
    let p = Prescription::new(vec![
        Surface::sphere(
            r1,
            10.0,
            t,
            Glass::Constant {
                name: "n".into(),
                nd: n,
            },
        ),
        Surface::sphere(r2, 10.0, 0.0, Glass::Air),
    ])
    .unwrap();
    let (c1, c2) = (1.0 / r1, 1.0 / r2);
    let inv_f = (n - 1.0) * (c1 - c2 + (n - 1.0) * t * c1 * c2 / n);
    let f_expected = 1.0 / inv_f;
    let bfd_expected = f_expected * (1.0 - (n - 1.0) * t * c1 / n);
    let fo = first_order(&p, lines::D).unwrap();
    assert!(
        (fo.efl_mm - f_expected).abs() < 1e-9,
        "EFL {} vs closed form {f_expected}",
        fo.efl_mm
    );
    assert!(
        (fo.bfd_mm - bfd_expected).abs() < 1e-9,
        "BFD {} vs closed form {bfd_expected}",
        fo.bfd_mm
    );
    // Thin-lens limit: t → 0 recovers the lensmaker's equation.
    let thin = singlet_from_q(100.0, 0.3, n, 1e-9, 5.0);
    let fo_thin = first_order(&thin, lines::D).unwrap();
    assert!((fo_thin.efl_mm - 100.0).abs() < 1e-5, "{}", fo_thin.efl_mm);
}

/// Rung 2: the exact tracer's h → 0 limit is the paraxial trace — the
/// marginal axis crossing converges to the paraxial focus as h².
#[test]
fn exact_trace_paraxial_limit() {
    let p = Prescription::new(vec![
        Surface::sphere(62.8, 12.7, 4.0, Glass::n_bk7()),
        Surface::sphere(-45.7, 12.7, 2.5, Glass::sf5()),
        Surface::sphere(-128.2, 12.7, 0.0, Glass::Air),
    ])
    .unwrap();
    let fo = first_order(&p, lines::D).unwrap();
    let z_cross = marginal_axis_crossing(&p, lines::D, 1e-3, fo.image_z_mm);
    assert!(
        (z_cross - fo.image_z_mm).abs() < 1e-6,
        "exact h→0 crossing {z_cross} vs paraxial {}",
        fo.image_z_mm
    );
    // h² convergence: quadrupling h quadruples the aberration.
    let d1 = fo.image_z_mm - marginal_axis_crossing(&p, lines::D, 2.0, fo.image_z_mm);
    let d2 = fo.image_z_mm - marginal_axis_crossing(&p, lines::D, 4.0, fo.image_z_mm);
    assert!((d2 / d1 - 4.0).abs() < 0.4, "LSA(4)/LSA(2) = {}", d2 / d1);
}

/// Rung 3: a published prescription — Thorlabs AC254-075-A — traces to
/// its catalog EFL (74.9 mm) within catalog rounding.
#[test]
fn published_doublet_efl() {
    let p = Prescription::new(vec![
        Surface::sphere(46.5, 12.7, 7.0, Glass::n_bk7()),
        Surface::sphere(-33.9, 12.7, 2.5, Glass::sf5()),
        Surface::sphere(-95.5, 12.7, 0.0, Glass::Air),
    ])
    .unwrap();
    let fo = first_order(&p, lines::D).unwrap();
    assert!(
        (fo.efl_mm - 74.9).abs() < 0.4,
        "EFL {} vs catalog 74.9",
        fo.efl_mm
    );
    // The matrix path agrees (independent implementation).
    let (efl_m, _) = first_order_matrix(&p, lines::D).unwrap();
    assert!((fo.efl_mm - efl_m).abs() < 1e-9);
    // And it is a real achromat: F-to-C focal shift far below a singlet's
    // f/V ≈ 1.2 mm.
    let shift =
        first_order(&p, lines::C).unwrap().bfd_mm - first_order(&p, lines::F).unwrap().bfd_mm;
    assert!(shift.abs() < 0.2, "chromatic shift {shift} mm");
}

/// Rung 4: the U-curve. Exact-trace longitudinal spherical aberration of
/// a bent singlet matches the third-order Seidel thin-lens formula
/// (Jenkins & White §9.5) across shape factors, and the measured minimum
/// lands at the textbook best-form q = 2(n²−1)/(n+2).
#[test]
fn seidel_ucurve() {
    let (n, f, h, t) = (1.5168, 100.0, 3.0, 0.5);
    for q in [-2.0, -1.0, 0.0, 0.714, 1.0, 2.0] {
        let p = singlet_from_q(f, q, n, t, 8.0);
        let fo = first_order(&p, lines::D).unwrap();
        let z_cross = marginal_axis_crossing(&p, lines::D, h, fo.image_z_mm);
        let lsa_exact = fo.image_z_mm - z_cross;
        let lsa_formula = thirdorder::thin_lens_lsa_infinity(n, fo.efl_mm, q, h);
        assert!(
            (lsa_exact / lsa_formula - 1.0).abs() < 0.08,
            "q = {q}: exact {lsa_exact} vs third-order {lsa_formula}"
        );
    }
    // Locate the exact minimum on a fine grid.
    let mut best = (f64::INFINITY, 0.0);
    let mut qq = 0.3;
    while qq <= 1.2 {
        let p = singlet_from_q(f, qq, n, t, 8.0);
        let fo = first_order(&p, lines::D).unwrap();
        let lsa = fo.image_z_mm - marginal_axis_crossing(&p, lines::D, h, fo.image_z_mm);
        if lsa < best.0 {
            best = (lsa, qq);
        }
        qq += 0.02;
    }
    let q_expected = thirdorder::best_form_q(n, -1.0);
    assert!(
        (best.1 - q_expected).abs() < 0.08,
        "best-form q measured {} vs Seidel {q_expected}",
        best.1
    );
}

/// Rung 5: chromatic focal shift of a BK7 singlet matches the Abbe
/// prediction f_C − f_F ≈ f_d / V_d.
#[test]
fn abbe_chromatic_shift() {
    let g = Glass::n_bk7();
    // ~f = 100 mm BK7 equiconvex.
    let p = Prescription::new(vec![
        Surface::sphere(103.4, 8.0, 2.0, g.clone()),
        Surface::sphere(-103.4, 8.0, 0.0, Glass::Air),
    ])
    .unwrap();
    let fd = first_order(&p, lines::D).unwrap().efl_mm;
    let fc = first_order(&p, lines::C).unwrap().efl_mm;
    let ff = first_order(&p, lines::F).unwrap().efl_mm;
    let predicted = fd / g.abbe_number();
    assert!(
        ((fc - ff) / predicted - 1.0).abs() < 0.02,
        "f_C − f_F = {} vs f/V = {predicted}",
        fc - ff
    );
}

/// Rung 6: the achromat condition. A cemented BK7/F2 doublet whose
/// thin-element powers split as φ₁/φ = V₁/(V₁−V₂) (Dollond) collapses
/// the chromatic focal shift by an order of magnitude relative to the
/// equivalent singlet.
#[test]
fn achromat_condition_kills_chromatic_shift() {
    let (bk7, f2) = (Glass::n_bk7(), Glass::f2());
    let (v1, v2) = (bk7.abbe_number(), f2.abbe_number());
    let phi = 0.01; // f = 100 mm
    let phi1 = phi * v1 / (v1 - v2);
    let phi2 = -phi * v2 / (v1 - v2);
    let (n1, n2) = (bk7.index(lines::D), f2.index(lines::D));
    let c1 = 0.022;
    let c2 = c1 - phi1 / (n1 - 1.0);
    let c3 = c2 - phi2 / (n2 - 1.0);
    let doublet = Prescription::new(vec![
        Surface::sphere(1.0 / c1, 10.0, 4.0, bk7.clone()),
        Surface::sphere(1.0 / c2, 10.0, 2.5, f2),
        Surface::sphere(1.0 / c3, 10.0, 0.0, Glass::Air),
    ])
    .unwrap();
    let singlet = Prescription::new(vec![
        Surface::sphere(103.4, 10.0, 2.0, bk7),
        Surface::sphere(-103.4, 10.0, 0.0, Glass::Air),
    ])
    .unwrap();
    let shift = |p: &Prescription| {
        first_order(p, lines::C).unwrap().bfd_mm - first_order(p, lines::F).unwrap().bfd_mm
    };
    let (sd, ss) = (shift(&doublet), shift(&singlet));
    assert!(
        sd.abs() * 10.0 < ss.abs(),
        "doublet shift {sd} mm vs singlet {ss} mm"
    );
    // Sanity: the doublet still has roughly the intended power.
    let fo = first_order(&doublet, lines::D).unwrap();
    assert!((fo.efl_mm - 100.0).abs() < 4.0, "EFL {}", fo.efl_mm);
}

/// Rung 7: spot machinery end-to-end — the polychromatic spot of the
/// published achromat at its focus is far below the singlet's, and every
/// launched ray is accounted for.
#[test]
fn published_achromat_beats_singlet_spot() {
    let doublet = Prescription::new(vec![
        Surface::sphere(46.5, 12.7, 7.0, Glass::n_bk7()),
        Surface::sphere(-33.9, 12.7, 2.5, Glass::sf5()),
        Surface::sphere(-95.5, 12.7, 0.0, Glass::Air),
    ])
    .unwrap();
    let singlet = {
        // Best-form BK7 singlet (real dispersion) at the same EFL, f/7.5.
        let n = Glass::n_bk7().index(lines::D);
        let q = thirdorder::best_form_q(n, -1.0);
        let dc = 1.0 / ((n - 1.0) * 74.9);
        let (c1, c2) = ((q + 1.0) * dc / 2.0, (q - 1.0) * dc / 2.0);
        Prescription::new(vec![
            Surface::sphere(1.0 / c1, 12.7, 3.0, Glass::n_bk7()),
            Surface::sphere(1.0 / c2, 12.7, 0.0, Glass::Air),
        ])
        .unwrap()
    };
    let lams = [lines::F, lines::D, lines::C];
    let spot = |p: &Prescription| {
        let fo = first_order(p, lines::D).unwrap();
        let a = analyze(p, 5.0, 8, &[0.0], &lams, fo.image_z_mm);
        assert_eq!(a.hard_failures(), 0);
        a.poly_rms_um[0].unwrap()
    };
    let (sd, ss) = (spot(&doublet), spot(&singlet));
    assert!(sd * 4.0 < ss, "doublet poly RMS {sd} µm vs singlet {ss} µm");
}
