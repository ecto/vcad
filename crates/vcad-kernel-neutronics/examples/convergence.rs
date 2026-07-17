//! M5 convergence and validation study.
//!
//! Block 1: uncollided attenuation — MC vs the exact analytic curve
//! φ(r) = e^{−Σt·r}/4πr² in a pure absorber, at 1–5 mean free paths.
//! Block 2: RSE scaling — the operator-dose error bar must fall as
//! 1/√(histories); the table prints rse·√N, which should be flat.
//!
//! Run: `cargo run --release -p vcad-kernel-neutronics --example convergence`

use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::groups::SOURCE_GROUP;
use vcad_kernel_neutronics::materials::{self, Material};
use vcad_kernel_neutronics::transport::{run, RunConfig, Source};

fn main() {
    println!("== uncollided attenuation: MC vs analytic (pure absorber, Σt = 0.25/cm)");
    let sigma = 0.25;
    let m = || Material::pure_absorber(sigma);
    // Tally shells centered at 4, 8, 12, 16, 20 cm (1–5 mfp).
    let mut layers = Vec::new();
    let mut det = Vec::new();
    let mut r0 = 0.0;
    for rc in [40.0f64, 80.0, 120.0, 160.0, 200.0] {
        layers.push(Layer::new(m(), rc - 10.0 - r0));
        layers.push(Layer::new(m(), 20.0));
        det.push((rc, layers.len() - 1));
        r0 = rc + 10.0;
    }
    layers.push(Layer::new(m(), 40.0));
    let c = RunConfig::new(
        Geometry::Sphere(layers),
        Source::IsotropicPoint,
        100_000,
        20260717,
    );
    let r = run(&c).unwrap();
    println!(
        "{:>8} {:>8} {:>14} {:>14} {:>10} {:>8}",
        "r (cm)", "mfp", "MC flux", "analytic", "MC/exact", "rse %"
    );
    for (rc_mm, region) in &det {
        let rc = rc_mm * 0.1;
        let (lo, hi) = (rc - 1.0, rc + 1.0);
        let v = 4.0 / 3.0 * std::f64::consts::PI * (hi.powi(3) - lo.powi(3));
        let exact = ((-sigma * lo).exp() - (-sigma * hi).exp()) / (sigma * v);
        let mc = r.flux_per_source[*region][SOURCE_GROUP];
        println!(
            "{:>8.1} {:>8.1} {:>14.4e} {:>14.4e} {:>10.4} {:>8.2}",
            rc,
            sigma * rc,
            mc.mean,
            exact,
            mc.mean / exact,
            mc.rse * 100.0
        );
    }

    println!("\n== RSE scaling: operator dose behind 10 cm HDPE, 20 batches");
    println!(
        "{:>12} {:>14} {:>10} {:>12}",
        "hist/batch", "dose (pSv/n)", "rse %", "rse·√N"
    );
    for hpb in [1_000usize, 4_000, 16_000, 64_000] {
        let g = Geometry::Sphere(vec![
            Layer::new(materials::air(), 300.0),
            Layer::new(materials::hdpe(), 100.0),
            Layer::new(materials::air(), 580.0),
            Layer::new(materials::air(), 40.0),
            Layer::new(materials::air(), 30.0),
        ]);
        let c = RunConfig::new(g, Source::IsotropicPoint, hpb, 424242);
        let r = run(&c).unwrap();
        let d = r.dose_per_source_psv[3];
        let n = (hpb * 20) as f64;
        println!(
            "{:>12} {:>14.4e} {:>10.3} {:>12.3}",
            hpb,
            d.mean,
            d.rse * 100.0,
            d.rse * n.sqrt()
        );
    }
    println!("(a flat rse·√N column is the 1/√N law; the absolute level is the\n figure of merit of the thin-shell track-length estimator)");
}
