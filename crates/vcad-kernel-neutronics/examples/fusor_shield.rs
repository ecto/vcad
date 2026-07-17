//! The M0 headline: dose rate at the operator positions of a D-D fusor
//! vs HDPE shield thickness.
//!
//! Isotropic 2.45 MeV point source at the center of a spherical model:
//! 30 cm air (chamber stand-in), an HDPE (or HDPE + 5% borated) shell,
//! then air out past detector shells at 1 m and 2 m. Doses are ambient
//! dose equivalent H*(10), ICRP-74-style factors, quoted at source rates
//! of 10⁶ and 10⁸ n/s with Monte Carlo error bars.
//!
//! Run: `cargo run --release -p vcad-kernel-neutronics --example fusor_shield`

use vcad_kernel_neutronics::geometry::{Geometry, Layer};
use vcad_kernel_neutronics::materials::{self, Material};
use vcad_kernel_neutronics::transport::{run, RunConfig, Source};

struct Config {
    label: &'static str,
    layers: Vec<(Material, f64)>, // shield stack (material, mm)
}

/// Sphere: air chamber, shield stack, air to 2.1 m with 4 cm detector
/// shells centered at 1 m and 2 m. Returns (geometry, det_1m, det_2m).
fn build(shield: &[(Material, f64)]) -> (Geometry, usize, usize) {
    let air = materials::air;
    let mut layers = vec![Layer::new(air(), 300.0)];
    let mut r_mm = 300.0;
    for (m, t) in shield {
        layers.push(Layer::new(m.clone(), *t));
        r_mm += t;
    }
    assert!(
        r_mm < 980.0,
        "shield stack must fit inside the 1 m detector"
    );
    layers.push(Layer::new(air(), 980.0 - r_mm));
    layers.push(Layer::new(air(), 40.0)); // 98–102 cm
    let det1 = layers.len() - 1;
    layers.push(Layer::new(air(), 880.0));
    layers.push(Layer::new(air(), 40.0)); // 198–202 cm
    let det2 = layers.len() - 1;
    layers.push(Layer::new(air(), 80.0));
    (Geometry::Sphere(layers), det1, det2)
}

fn main() {
    let configs = vec![
        Config {
            label: "bare (no shield)",
            layers: vec![],
        },
        Config {
            label: "5 cm HDPE",
            layers: vec![(materials::hdpe(), 50.0)],
        },
        Config {
            label: "10 cm HDPE",
            layers: vec![(materials::hdpe(), 100.0)],
        },
        Config {
            label: "20 cm HDPE",
            layers: vec![(materials::hdpe(), 200.0)],
        },
        Config {
            label: "15 cm HDPE + 5 cm borated-5%",
            layers: vec![
                (materials::hdpe(), 150.0),
                (materials::borated_hdpe_5(), 50.0),
            ],
        },
    ];

    println!("fusor shield sweep — isotropic 2.45 MeV point source");
    println!("1e6 histories/config (20 batches × 50k), seed 20260717\n");
    println!(
        "{:<32} {:>26} {:>26}",
        "shield", "dose @ 1 m (µSv/h)", "dose @ 2 m (µSv/h)"
    );

    // Analytic bare-source anchor at 1 m for the first row's sanity.
    let bare_1m = {
        let flux = 1.0 / (4.0 * std::f64::consts::PI * 100.0f64.powi(2));
        let h = vcad_kernel_neutronics::dose::group_dose_factors_psv_cm2()
            [vcad_kernel_neutronics::groups::SOURCE_GROUP];
        flux * h * 3600.0 * 1.0e-6 // µSv/h per (n/s)
    };

    for source_n_per_s in [1.0e6, 1.0e8] {
        println!("\n--- source rate {source_n_per_s:.0e} n/s ---");
        for cfg in &configs {
            let (geometry, det1, det2) = build(&cfg.layers);
            let rc = RunConfig::new(geometry, Source::IsotropicPoint, 50_000, 20260717);
            let result = run(&rc).unwrap();
            assert_eq!(
                result.truncated_histories, 0,
                "truncated histories would taint the claim"
            );
            let d1 = result.dose_rate_usv_per_h(det1, source_n_per_s);
            let d2 = result.dose_rate_usv_per_h(det2, source_n_per_s);
            println!(
                "{:<32} {:>17.3} ± {:>4.1}% {:>17.4} ± {:>4.1}%",
                cfg.label,
                d1.mean,
                d1.rse * 100.0,
                d2.mean,
                d2.rse * 100.0
            );
        }
        println!(
            "(analytic uncollided bare source at 1 m: {:.3} µSv/h — the bare row \
             sits slightly above it from air in-scatter)",
            bare_1m * source_n_per_s
        );
    }

    println!(
        "\nCaveats (M0, stated not silent):\n\
         - Neutron dose only. Capture gammas (H(n,γ) 2.22 MeV in the shield)\n\
         \x20 are NOT transported; a lead liner is the gamma answer and is out\n\
         \x20 of scope here. Budget gamma dose separately before trusting a\n\
         \x20 total.\n\
         - Design-estimate library (±20–30% group constants), free field (no\n\
         \x20 room return — concrete walls add back-scatter), no chamber steel.\n\
         - Isotropic scattering (M1 adds P1 anisotropy; forward-peaked H\n\
         \x20 scatter shifts deep-penetration doses up)."
    );
}
