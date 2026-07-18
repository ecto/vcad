//! The flagship: the optimizer rediscovers the achromat.
//!
//! Multi-start finite-difference optimization of (a) a BK7 singlet
//! (2 curvatures) and (b) a cemented BK7/F2 doublet (3 curvatures),
//! both constrained to EFL = 100 mm, minimizing the polychromatic RMS
//! spot (F, d, C lines, equal weight) at f/5 on axis. The doublet must
//! beat the singlet by the textbook margin, and its thin-element power
//! split must land near the analytic achromat condition
//! φ₁/φ = V₁/(V₁−V₂) (Dollond, 1758).
//!
//! Run: `cargo run --release -p vcad-kernel-optics --example achromat_design`

use vcad_kernel_optics::glass::Glass;
use vcad_kernel_optics::lines;
use vcad_kernel_optics::optimize::{minimize_multi_start, FdOptions};
use vcad_kernel_optics::paraxial::first_order;
use vcad_kernel_optics::prescription::{Prescription, Surface};
use vcad_kernel_optics::receipt::{design_claims, predicted_claims};
use vcad_kernel_optics::spot::{airy_radius_um, analyze};
use vcad_kernel_optics::trace::{trace_to_image, Ray, RayFate, Vec3};

const F_TARGET: f64 = 100.0;
const PUPIL: f64 = 10.0; // f/5
const RINGS: usize = 8;
const SEMI_D: f64 = 12.7;
const LAMS: [f64; 3] = [lines::F, lines::D, lines::C];

fn singlet(c: &[f64]) -> Option<Prescription> {
    Prescription::new(vec![
        Surface::sphere(1.0 / c[0], SEMI_D, 4.0, Glass::n_bk7()),
        Surface::sphere(1.0 / c[1], SEMI_D, 0.0, Glass::Air),
    ])
    .ok()
}

fn doublet(c: &[f64]) -> Option<Prescription> {
    Prescription::new(vec![
        Surface::sphere(1.0 / c[0], SEMI_D, 5.0, Glass::n_bk7()),
        Surface::sphere(1.0 / c[1], SEMI_D, 2.5, Glass::f2()),
        Surface::sphere(1.0 / c[2], SEMI_D, 0.0, Glass::Air),
    ])
    .ok()
}

/// Objective: polychromatic RMS spot (µm) + EFL-constraint penalty.
/// Infeasible designs (no focus, TIR, missed surfaces, empty bundles)
/// return +∞ — never accepted, never silently averaged over.
fn objective(build: &dyn Fn(&[f64]) -> Option<Prescription>, x: &[f64]) -> f64 {
    let Some(p) = build(x) else {
        return f64::INFINITY;
    };
    let Some(fo) = first_order(&p, lines::D) else {
        return f64::INFINITY;
    };
    let a = analyze(&p, PUPIL, RINGS, &[0.0], &LAMS, fo.image_z_mm);
    if a.hard_failures() > 0 {
        return f64::INFINITY;
    }
    let Some(rms) = a.poly_rms_um[0] else {
        return f64::INFINITY;
    };
    rms + 100.0 * (fo.efl_mm - F_TARGET).powi(2)
}

fn lens_table(name: &str, p: &Prescription) {
    println!("\n  {name} — lens data (mm):");
    println!(
        "  {:>4} {:>12} {:>10} {:>10}  glass",
        "surf", "R", "thick", "semi-d"
    );
    for (i, s) in p.surfaces.iter().enumerate() {
        println!(
            "  {:>4} {:>12.3} {:>10.3} {:>10.2}  {}",
            i,
            s.radius_mm,
            s.thickness_mm,
            s.semi_diameter_mm,
            s.glass.name()
        );
    }
}

fn report(name: &str, p: &Prescription) -> f64 {
    let fo = first_order(p, lines::D).unwrap();
    let a = analyze(p, PUPIL, RINGS, &[0.0], &LAMS, fo.image_z_mm);
    let rms = a.poly_rms_um[0].unwrap();
    let n_number = fo.efl_mm / (2.0 * PUPIL);
    lens_table(name, p);
    println!(
        "  EFL {:.3} mm | BFD {:.3} mm | f/{:.1} | poly RMS spot {:.2} µm | Airy {:.2} µm | vignetted {:.1}%",
        fo.efl_mm,
        fo.bfd_mm,
        n_number,
        rms,
        airy_radius_um(lines::D, n_number),
        100.0 * a.vignetted_fraction()
    );
    for r in &a.results {
        println!(
            "    λ = {:.4} µm: rms {:>8.2} µm  centroid y {:>9.5} mm  ({} rays)",
            r.lambda_um,
            r.rms_um.unwrap(),
            r.centroid_mm.1,
            r.n_imaged
        );
    }
    rms
}

/// Dump per-wavelength image points for spot diagrams (machine-readable).
fn dump_spot_points(tag: &str, p: &Prescription) {
    let fo = first_order(p, lines::D).unwrap();
    let mut out = Vec::new();
    for &lam in &LAMS {
        let mut pts = Vec::new();
        for (px, py) in vcad_kernel_optics::spot::pupil_points(PUPIL, RINGS) {
            let ray = Ray {
                p: Vec3::new(px, py, -1.0),
                d: Vec3::new(0.0, 0.0, 1.0),
            };
            if let RayFate::Imaged(pt) = trace_to_image(p, lam, ray, fo.image_z_mm, false).fate {
                pts.push((pt.x * 1000.0, pt.y * 1000.0)); // µm
            }
        }
        out.push((lam, pts));
    }
    println!(
        "JSON_SPOTDIAG {}",
        serde_json::to_string(&(tag, out)).unwrap()
    );
}

/// Dump a meridional ray fan through the lens for the ray diagram.
fn dump_ray_fan(tag: &str, p: &Prescription) {
    let fo = first_order(p, lines::D).unwrap();
    let mut fans = Vec::new();
    let n_rays = 9;
    for i in 0..n_rays {
        let h = PUPIL * (2.0 * i as f64 / (n_rays - 1) as f64 - 1.0);
        let ray = Ray {
            p: Vec3::new(h, 0.0, -15.0),
            d: Vec3::new(0.0, 0.0, 1.0),
        };
        let out = trace_to_image(p, lines::D, ray, fo.image_z_mm + 8.0, true);
        if matches!(out.fate, RayFate::Imaged(_)) {
            let mut poly = vec![(ray.p.z, ray.p.x)];
            poly.extend(out.hits.iter().map(|v| (v.z, v.x)));
            fans.push(poly);
        }
    }
    let surfaces: Vec<(f64, f64, f64)> = p
        .surfaces
        .iter()
        .enumerate()
        .map(|(i, s)| (p.vertex_z(i), s.radius_mm, s.semi_diameter_mm))
        .collect();
    println!(
        "JSON_RAYFAN {}",
        serde_json::to_string(&(tag, surfaces, fans, fo.image_z_mm)).unwrap()
    );
}

fn main() {
    println!("=== achromat_design: the optimizer rediscovers 1758 ===");
    println!(
        "objective: polychromatic RMS spot (F,d,C) on axis at f/5, EFL pinned to {F_TARGET} mm\n"
    );
    let opts = FdOptions::default();

    // --- Singlet: multi-start over bendings.
    let lo_s = [-1.0 / 30.0, -1.0 / 30.0];
    let hi_s = [1.0 / 30.0, 1.0 / 30.0];
    let dc = 1.0 / ((Glass::n_bk7().index(lines::D) - 1.0) * F_TARGET);
    let starts_s: Vec<Vec<f64>> = [-1.0f64, 0.0, 0.714, 1.5]
        .iter()
        .map(|q| vec![(q + 1.0) * dc / 2.0, (q - 1.0) * dc / 2.0])
        .collect();
    let mut f_s = |x: &[f64]| objective(&singlet, x);
    let best_s = minimize_multi_start(&mut f_s, &starts_s, &lo_s, &hi_s, &opts).unwrap();
    let p_s = singlet(&best_s.x).unwrap();
    println!(
        "singlet optimized: {} evals, objective {:.3}",
        best_s.evals, best_s.value
    );
    let rms_s = report("BK7 singlet (best form found)", &p_s);

    // --- Doublet: multi-start over crown/flint curvature basins.
    let lo_d = [0.005, -0.06, -0.02];
    let hi_d = [0.045, 0.01, 0.02];
    let starts_d: Vec<Vec<f64>> = vec![
        vec![0.020, -0.025, -0.002],
        vec![0.030, -0.035, -0.005],
        vec![0.015, -0.015, 0.002],
        vec![0.025, -0.045, -0.010],
        vec![0.035, -0.020, 0.005],
    ];
    let mut f_d = |x: &[f64]| objective(&doublet, x);
    let best_d = minimize_multi_start(&mut f_d, &starts_d, &lo_d, &hi_d, &opts).unwrap();
    let p_d = doublet(&best_d.x).unwrap();
    println!(
        "\ndoublet optimized: {} evals, objective {:.3}",
        best_d.evals, best_d.value
    );
    let rms_d = report("BK7/F2 cemented doublet (optimized)", &p_d);

    // --- The analytic check: power split vs the Abbe ratio.
    let (v1, v2) = (Glass::n_bk7().abbe_number(), Glass::f2().abbe_number());
    let phi1 = p_d.thin_element_power(0, lines::D);
    let phi2 = p_d.thin_element_power(1, lines::D);
    let split = phi1 / (phi1 + phi2);
    let split_theory = v1 / (v1 - v2);
    println!("\n  achromat condition check (Dollond):");
    println!(
        "    φ1/φ found = {split:.3}  |  V1/(V1−V2) = {split_theory:.3}  |  deviation {:.1}%",
        100.0 * (split / split_theory - 1.0).abs()
    );
    let shift = |p: &Prescription| {
        first_order(p, lines::C).unwrap().bfd_mm - first_order(p, lines::F).unwrap().bfd_mm
    };
    println!(
        "    chromatic focal shift: singlet {:.3} mm (thin-lens f/V = {:.3}), doublet {:.3} mm",
        shift(&p_s),
        F_TARGET / v1,
        shift(&p_d)
    );
    println!(
        "\n  VERDICT: doublet poly RMS {:.2} µm vs singlet {:.2} µm — {:.1}× better",
        rms_d,
        rms_s,
        rms_s / rms_d
    );

    // --- Receipt.
    let fo = first_order(&p_d, lines::D).unwrap();
    let a = analyze(&p_d, PUPIL, RINGS, &[0.0], &LAMS, fo.image_z_mm);
    let set = predicted_claims(&p_d, &a, lines::D).unwrap();
    println!(
        "\n  receipt ({}, {} claims, all basis=predicted → rolls up Provisional):",
        set.schema,
        set.claims.len()
    );
    for c in &set.claims {
        println!("    {:<28} {:>12.4} {}", c.name, c.value, c.unit);
    }
    let unified = design_claims(&set);
    println!("  unified vcad.receipt claims: {}", unified.len());

    // --- Machine-readable dumps for visuals.
    dump_spot_points("singlet", &p_s);
    dump_spot_points("doublet", &p_d);
    dump_ray_fan("doublet", &p_d);
}
