//! The flagship: a ported (bass-reflex) loudspeaker enclosure.
//!
//! A driver piston on the bottom face, a sealed box, and a vent (port) out
//! the top — all axisymmetric. We
//!
//! 1. price the port tuning with the lumped Thiele–Small-style formula
//!    (`f_b = (c/2π)√(S/(V·L_eff))`), with its end-correction band;
//! 2. confirm it against the **field solver** — sweep the driven box, find
//!    the Helmholtz (port) resonance;
//! 3. read the bass-reflex signature — port volume velocity peaking at `f_b`;
//! 4. let the optimizer **size the port length** for a target tuning, closing
//!    the design loop against the field solve itself;
//! 5. emit a `vcad.acoustics-claims/1` receipt (Provisional — predicted).
//!
//! Run: `cargo run --release -p vcad-kernel-acoustics --example ported_box`

use vcad_kernel_acoustics::cavity::Cavity;
use vcad_kernel_acoustics::complex::Cplx;
use vcad_kernel_acoustics::fom::{self, port_volume_velocity};
use vcad_kernel_acoustics::helmholtz::{solve_driven, Source};
use vcad_kernel_acoustics::lumped::{self, TuningBand};
use vcad_kernel_acoustics::medium::Medium;
use vcad_kernel_acoustics::optimize::{maximize, FdOptions};
use vcad_kernel_acoustics::receipt::{self, Provenance, ResponsePoint};

const NR: usize = 21;
const BOX_R_MM: f64 = 100.0;
const BOX_H_MM: f64 = 300.0;
const PORT_R_MM: f64 = 25.0;
const DRIVER_R_MM: f64 = 80.0;

fn air() -> Medium {
    Medium::air(20.0)
}

/// Build the ported box with a given port length (mm).
fn box_with_port(port_len_mm: f64) -> Cavity {
    Cavity::ported_box(
        BOX_R_MM,
        BOX_H_MM,
        PORT_R_MM,
        port_len_mm,
        DRIVER_R_MM,
        air(),
    )
}

/// A z-resolution that keeps ~5 mm cells regardless of port length.
fn nz_for(cav: &Cavity) -> usize {
    let (zmin, zmax) = cav.z_span_mm();
    (((zmax - zmin) / 5.0).round() as usize).max(60) + 1
}

/// Lumped tuning band for a port length.
fn lumped_band(port_len_mm: f64) -> TuningBand {
    let cav = box_with_port(port_len_mm);
    // Box volume only (the compliance) — exclude the port's own air column.
    let box_vol = cav.segments[0].volume_mm3();
    lumped::ported_box_tuning_mm(&air(), box_vol, PORT_R_MM, port_len_mm)
}

/// Field-solved tuning for a port length (Hz), searched around the lumped band.
fn field_tuning(port_len_mm: f64) -> Option<f64> {
    let cav = box_with_port(port_len_mm);
    let nz = nz_for(&cav);
    let band = lumped_band(port_len_mm);
    let lo = (band.f_min_hz * 0.7).max(10.0);
    let hi = band.f_max_hz * 1.6;
    fom::driven_tuning_hz(&cav, NR, nz, lo, hi, 34)
}

fn main() {
    let a = air();
    println!("# Ported-box (bass-reflex) enclosure — field-solved tuning + optimization\n");
    println!(
        "box: r={BOX_R_MM} mm × h={BOX_H_MM} mm ({:.2} L), port r={PORT_R_MM} mm, driver r={DRIVER_R_MM} mm",
        box_with_port(120.0).segments[0].volume_mm3() * 1e-6
    );
    println!("air: c={:.1} m/s, ρ={:.3} kg/m³\n", a.c, a.rho);

    // ── 1–2. lumped vs field tuning at the initial port length ──────────────
    let init_len = 120.0;
    let band0 = lumped_band(init_len);
    let field0 = field_tuning(init_len).expect("tuning found");
    println!("## Initial port length {init_len:.0} mm");
    println!(
        "  lumped f_b band : [{:.1}, {:.1}, {:.1}] Hz (min/nominal/max)",
        band0.f_min_hz, band0.f_nominal_hz, band0.f_max_hz
    );
    println!("  field  f_b      : {field0:.1} Hz");
    println!(
        "  field/nominal   : {:.3} (pressure-release mouth omits exterior mass → reads high)\n",
        field0 / band0.f_nominal_hz
    );

    // ── 3. bass-reflex signature: port volume velocity vs frequency ─────────
    println!("## Port output |U_port| vs frequency (the tuning peak) — initial box");
    let curve0 = port_velocity_curve(init_len, 20.0, 120.0, 51);
    print_curve(&curve0);

    // ── 4. optimize the port length for a target tuning ─────────────────────
    let target = 45.0;
    println!("\n## Optimize port length for field tuning = {target:.0} Hz");
    let mut objective = |x: &[f64]| -> f64 {
        match field_tuning(x[0]) {
            // Maximize the negative squared error (Hz²).
            Some(f) => -(f - target).powi(2),
            None => -1e6,
        }
    };
    let res = maximize(
        &mut objective,
        &[init_len],
        &[60.0],
        &[420.0],
        &FdOptions {
            rel_step: 2e-2,
            max_iters: 24,
            initial_step: 0.25,
            min_step: 5e-3,
        },
    );
    let opt_len = res.x[0];
    let opt_field = field_tuning(opt_len).unwrap();
    let opt_band = lumped_band(opt_len);
    println!(
        "  optimizer: port {init_len:.0} → {opt_len:.1} mm in {} evals; field f_b {field0:.1} → {opt_field:.1} Hz (target {target:.0})",
        res.evals
    );
    println!(
        "  lumped nominal at optimum: {:.1} Hz; residual |f−target| = {:.2} Hz",
        opt_band.f_nominal_hz,
        (opt_field - target).abs()
    );

    println!("\n## Port output |U_port| vs frequency — optimized box (peak shifted down)");
    let curve1 = port_velocity_curve(opt_len, 20.0, 120.0, 51);
    print_curve(&curve1);

    // ── 5. the receipt ──────────────────────────────────────────────────────
    let cav = box_with_port(opt_len);
    let nz = nz_for(&cav);
    // On-axis external pressure a little above the port mouth, at tuning.
    let (_, zmax) = cav.z_span_mm();
    let mouth_probe_z = zmax - 5.0;
    let field = solve_driven(
        &cav,
        NR,
        nz,
        opt_field,
        Source::Piston {
            velocity: Cplx::ONE,
        },
    )
    .unwrap();
    let mouth_p = field.magnitude_at(0.0, mouth_probe_z);
    let uport = port_volume_velocity(&field, &cav).abs();
    println!("\n## Receipt claims (predicted → Provisional)");
    println!(
        "  |U_port| at f_b = {:.3e} m³/s, |p| just inside mouth = {:.2} Pa",
        uport, mouth_p
    );

    let claims = receipt::predicted_claims(
        Provenance {
            grid: [NR, nz],
            sweep_hz: [20.0, 120.0],
            sweep_points: 51,
            sound_speed_m_s: a.c,
            density_kg_m3: a.rho,
            model: vec![
                "linear".into(),
                "lossless".into(),
                "pressure_release_mouth".into(),
                "axisymmetric".into(),
            ],
        },
        Some(opt_field),
        opt_band,
        &[opt_field],
        &[ResponsePoint {
            label: "port_mouth".into(),
            f_hz: opt_field,
            pressure_pa: mouth_p,
        }],
    );
    let receipt_claims = receipt::design_claims(&claims);
    let receipt = vcad_receipt::DesignReceipt::with_claims(receipt_claims);
    println!(
        "  unified receipt verdict: {:?} (predicted claims never Pass)",
        receipt.verdict()
    );
    println!("\n{}", serde_json::to_string_pretty(&claims).unwrap());
}

/// (frequency, |port volume velocity|) over a linear sweep.
fn port_velocity_curve(port_len_mm: f64, f_lo: f64, f_hi: f64, n: usize) -> Vec<(f64, f64)> {
    let cav = box_with_port(port_len_mm);
    let nz = nz_for(&cav);
    (0..n)
        .map(|i| {
            let f = f_lo + (f_hi - f_lo) * i as f64 / (n - 1) as f64;
            let u = solve_driven(
                &cav,
                NR,
                nz,
                f,
                Source::Piston {
                    velocity: Cplx::ONE,
                },
            )
            .map(|field| port_volume_velocity(&field, &cav).abs())
            .unwrap_or(f64::NAN);
            (f, u)
        })
        .collect()
}

/// Compact ASCII bar plot of a response curve, peak marked.
fn print_curve(curve: &[(f64, f64)]) {
    let peak = curve.iter().map(|c| c.1).fold(0.0_f64, f64::max);
    let (fpk, _) = curve
        .iter()
        .cloned()
        .fold((0.0, 0.0), |acc, c| if c.1 > acc.1 { c } else { acc });
    for &(f, u) in curve.iter().step_by(2) {
        let bars = if peak > 0.0 {
            (u / peak * 40.0).round() as usize
        } else {
            0
        };
        let mark = if (f - fpk).abs() < 1e-6 {
            " ← f_b"
        } else {
            ""
        };
        println!("  {f:5.1} Hz | {}{}", "█".repeat(bars), mark);
    }
}
