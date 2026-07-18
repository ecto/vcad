//! Dump the spherical-aberration U-curve: exact-trace LSA vs the
//! third-order Seidel thin-lens formula across shape factors.
//!
//! Run: `cargo run --release -p vcad-kernel-optics --example ucurve`

use vcad_kernel_optics::glass::Glass;
use vcad_kernel_optics::lines;
use vcad_kernel_optics::paraxial::first_order;
use vcad_kernel_optics::prescription::{Prescription, Surface};
use vcad_kernel_optics::thirdorder;
use vcad_kernel_optics::trace::{trace_to_image, Ray, RayFate, Vec3};

fn main() {
    let (n, f, h, t) = (1.5168, 100.0, 3.0, 0.5);
    let glass = Glass::Constant {
        name: "n=1.5168".into(),
        nd: n,
    };
    let mut rows = Vec::new();
    let mut q = -2.0;
    while q <= 2.0 + 1e-9 {
        let dc = 1.0 / ((n - 1.0) * f);
        let (c1, c2) = ((q + 1.0) * dc / 2.0, (q - 1.0) * dc / 2.0);
        let r = |c: f64| if c == 0.0 { f64::INFINITY } else { 1.0 / c };
        let p = Prescription::new(vec![
            Surface::sphere(r(c1), 8.0, t, glass.clone()),
            Surface::sphere(r(c2), 8.0, 0.0, Glass::Air),
        ])
        .unwrap();
        let fo = first_order(&p, lines::D).unwrap();
        let x_at = |z: f64| -> f64 {
            let ray = Ray {
                p: Vec3::new(h, 0.0, -10.0),
                d: Vec3::new(0.0, 0.0, 1.0),
            };
            match trace_to_image(&p, lines::D, ray, z, false).fate {
                RayFate::Imaged(pt) => pt.x,
                other => panic!("q={q}: {other:?}"),
            }
        };
        let (z1, z2) = (fo.image_z_mm - 5.0, fo.image_z_mm + 5.0);
        let (x1, x2) = (x_at(z1), x_at(z2));
        let z_cross = z1 - x1 * (z2 - z1) / (x2 - x1);
        let lsa_exact = fo.image_z_mm - z_cross;
        let lsa_seidel = thirdorder::thin_lens_lsa_infinity(n, fo.efl_mm, q, h);
        println!("q {q:>6.2}  exact {lsa_exact:>8.4} mm  seidel {lsa_seidel:>8.4} mm");
        rows.push((q, lsa_exact, lsa_seidel));
        q += 0.1;
    }
    println!(
        "best-form q (Seidel): {:.4}",
        thirdorder::best_form_q(n, -1.0)
    );
    println!("JSON_UCURVE {}", serde_json::to_string(&rows).unwrap());
}
