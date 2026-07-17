//! Emit the `vcad.em-claims/1` receipt for the 70 mm PCB motor slice,
//! and demonstrate the fail-closed measurement binding.
//!
//! The claim set is real (solved here, same model as `motor_torque`).
//! The bound "measurements" at the bottom are **demonstration values**,
//! clearly labeled — the honest bench procedure that replaces them is
//! `docs/em-measurement-pack.md`.
//!
//! Run: `cargo run --release -p vcad-kernel-em --example motor_receipt`

use vcad_kernel_em::grid::SolveOptions;
use vcad_kernel_em::planar::{Conductor, MagnetBlock, PlanarMagnetostatics, PlanarMaterial, Rect};
use vcad_kernel_em::receipt::{compare, planar_torque_claims, Measurement};

fn main() {
    // The fabricated design's parameters (see examples/motor_torque.rs
    // for provenance) at the best commutation angle, 1.5 A peak.
    let pitch_radius = 22.5_f64;
    let circumference = 2.0 * std::f64::consts::PI * pitch_radius;
    let (i_peak, phi_e) = (1.5_f64, 120.0_f64.to_radians());
    let mut dev = PlanarMagnetostatics::new(0.0, circumference, -3.0, 14.0);
    dev.periodic_x = true;
    for (lo, hi) in [(0.0, 2.7), (8.3, 11.0)] {
        dev.materials.push(PlanarMaterial::linear(
            Rect {
                x_min_mm: -1.0,
                x_max_mm: circumference + 1.0,
                y_min_mm: lo,
                y_max_mm: hi,
            },
            500.0,
        ));
    }
    let pole_pitch = circumference / 6.0;
    let mag_w = std::f64::consts::PI * 7.5 * 7.5 / 15.0;
    for p in 0..6 {
        let xc = (p as f64 + 0.5) * pole_pitch;
        dev.magnets.push(MagnetBlock {
            region: Rect {
                x_min_mm: xc - mag_w / 2.0,
                x_max_mm: xc + mag_w / 2.0,
                y_min_mm: 5.3,
                y_max_mm: 8.3,
            },
            br_x_t: 0.0,
            br_y_t: if p % 2 == 0 { 0.39 } else { -0.39 },
            mu_r: 1.05,
        });
    }
    let tooth_pitch = circumference / 9.0;
    for t in 0..9 {
        let xc = (t as f64 + 0.5) * tooth_pitch;
        let amps =
            10.0 * i_peak * (phi_e - t as f64 % 3.0 * 2.0 * std::f64::consts::PI / 3.0).cos();
        for (off, sign) in [(-4.9, 1.0), (4.9, -1.0)] {
            dev.conductors.push(Conductor {
                region: Rect {
                    x_min_mm: xc + off - 2.3,
                    x_max_mm: xc + off + 2.3,
                    y_min_mm: 2.7,
                    y_max_mm: 4.3,
                },
                total_current_a: sign * amps,
            });
        }
    }

    let opts = SolveOptions::default();
    let sol = dev.solve(560, 69, &opts).expect("solve");
    let set = planar_torque_claims(
        &sol,
        0.0,
        0.0,
        pitch_radius * 1e-3,
        0.015,
        opts.tol,
        Some(4.8), // mid-airgap stress line
    );
    println!("== vcad.em-claims/1 for the 70 mm PCB motor @ 1.5 A ==");
    println!("{}", serde_json::to_string_pretty(&set).unwrap());
    println!();

    // Fail-closed demonstration. These are NOT bench data.
    println!("== binding DEMO measurements (not bench data) ==");
    let empty = compare(&set, &[]).unwrap();
    println!("no measurements → all_hold = {}", empty.all_hold);
    let demo = Measurement {
        name: "torque_nm".into(),
        value: set.claims[0].value * 0.9,
        uncertainty: 0.3e-3,
        instrument: "DEMO torque stand (fictional)".into(),
        band_factor: 1.35,
    };
    let report = compare(&set, &[demo]).unwrap();
    println!(
        "demo torque within band → verdict {:?}, all_hold = {}",
        report.entries[0].verdict, report.all_hold
    );
    let liar = Measurement {
        name: "torque_nm".into(),
        value: set.claims[0].value * 5.0,
        uncertainty: 0.3e-3,
        instrument: "DEMO optimist (fictional)".into(),
        band_factor: 1.35,
    };
    let report = compare(&set, &[liar]).unwrap();
    println!(
        "5× demo torque → verdict {:?}, all_hold = {}",
        report.entries[0].verdict, report.all_hold
    );
}
