//! The rune stepper's stage stack — the transient solver's flagship case.
//!
//! Reconstruction of the R0 stack from the rune project's manual
//! (Z stage → crossed rails → XY stage → chuck → wafer, chamber floor as
//! the thermal anchor), the same stack whose steady solves produced the
//! "first solved physics" numbers (θ ≈ 0.25 K/W wafer-to-floor in chuck
//! contact). This example asks the two questions steady state can't:
//!
//! 1. **Overlay drift**: the chamber floor steps +2 °C mid-job. How long
//!    until the wafer has moved through most of that step? A lithography
//!    layer takes ~2 h; if the stack's thermal time constant is a large
//!    fraction of that, ambient control (or mid-job re-registration)
//!    is mandatory, not optional.
//! 2. **RTP heat/soak/cool**: the lamp dumps power into the wafer for a
//!    soak, then shuts off. How hot does the stack under it get, and how
//!    long until the wafer is back near baseline?
//!
//! Run with `--release`; the full grid is 100×100×75 = 750k voxels.
//!
//! ```sh
//! cargo run --release -p vcad-kernel-thermal --example rune_stage_stack
//! ```

use vcad_kernel_thermal::model::{
    Axis, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};
use vcad_kernel_thermal::transient::{solve_transient_schedule, ScheduleSegment};

// Handbook values, volumetric heat capacity ρc_p in J/(m³·K).
const AL: (f64, f64) = (167.0, 2.42e6); // 6061 aluminum
const STEEL: (f64, f64) = (45.0, 3.90e6); // rail steel
const SI: (f64, f64) = (148.0, 1.66e6); // silicon wafer

fn stack() -> ThermalModel {
    // Domain: 220 × 220 mm around the stack, 100 mm tall, 100×100×75
    // voxels (2.2 × 2.2 × 1.33 mm cells) — the grid of the steady runs.
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [220.0, 220.0, 100.0], [100, 100, 75]);
    let al = |shape| MaterialRegion::isotropic(shape, AL.0).with_heat_capacity(AL.1);

    // Z stage, 200 × 200 × 40, bolted to the chamber floor.
    m.materials.push(al(Shape::Box {
        min_mm: [10.0, 10.0, 0.0],
        size_mm: [200.0, 200.0, 40.0],
    }));
    // Crossed rails: two 180 × 12 × 10 steel rails.
    for y in [40.0, 168.0] {
        m.materials.push(
            MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [20.0, y, 40.0],
                    size_mm: [180.0, 12.0, 10.0],
                },
                STEEL.0,
            )
            .with_heat_capacity(STEEL.1),
        );
    }
    // XY stage, 170 × 170 × 30.
    m.materials.push(al(Shape::Box {
        min_mm: [25.0, 25.0, 50.0],
        size_mm: [170.0, 170.0, 30.0],
    }));
    // Chuck, Ø120 × 15, centered.
    m.materials.push(
        MaterialRegion::isotropic(
            Shape::Tube {
                axis: Axis::Z,
                center_mm: [110.0, 110.0],
                span_mm: [80.0, 95.0],
                outer_radius_mm: 60.0,
                inner_radius_mm: 0.0,
            },
            AL.0,
        )
        .with_heat_capacity(AL.1),
    );
    // Wafer, Ø100 × 2, in full chuck contact.
    m.materials.push(
        MaterialRegion::isotropic(
            Shape::Tube {
                axis: Axis::Z,
                center_mm: [110.0, 110.0],
                span_mm: [95.0, 97.0],
                outer_radius_mm: 50.0,
                inner_radius_mm: 0.0,
            },
            SI.0,
        )
        .with_heat_capacity(SI.1),
    );

    // The chamber floor is the anchor: the Z stage's bottom layer is
    // pinned at 20 °C (a fixed *region*, so the schedule can step it).
    m.fixed.push(FixedTemperature {
        shape: Shape::Box {
            min_mm: [10.0, 10.0, 0.0],
            size_mm: [200.0, 200.0, 1.5],
        },
        temperature_c: 20.0,
    });
    // Vacuum everywhere else: all other surfaces adiabatic (default).
    m.reference_c = Some(20.0);
    m
}

fn beam(power_w: f64) -> PowerSource {
    // Ø16 beam spot into the wafer top surface.
    PowerSource {
        name: "beam".into(),
        shape: Shape::Tube {
            axis: Axis::Z,
            center_mm: [110.0, 110.0],
            span_mm: [95.5, 97.0],
            outer_radius_mm: 8.0,
            inner_radius_mm: 0.0,
        },
        power_w,
    }
}

fn main() {
    let opts = SolveOptions::default();

    // --- Steady baseline: 30 W beam, chuck contact -------------------
    let mut m = stack();
    m.sources.push(beam(30.0));
    let steady = solve_steady(&m, &opts).expect("steady solve");
    let theta = steady.sources[0].theta_c_per_w.unwrap();
    println!("steady, 30 W beam, chuck contact:");
    println!(
        "  wafer T_max = {:.2} C  (theta = {:.3} K/W; energy residual {:.1e})",
        steady.sources[0].t_max_c, theta, steady.energy.residual_rel
    );

    // --- Overlay drift: chamber floor steps +2 C ---------------------
    // Wafer at thermal equilibrium (no beam), floor steps 20 -> 22 C.
    // A 100 mm silicon wafer at 2.6 ppm/K moves ~0.26 µm across its
    // radius per K, so "most of 2 K" is the overlay budget gone.
    let mut m = stack();
    m.sources.push(beam(0.0)); // 0 W: a pure wafer temperature probe
    let mut step = ScheduleSegment::plain(60.0, 120); // 2 h at 1-min steps
    step.fixed_temperature_c.insert(0, 22.0);
    let drift = solve_transient_schedule(&m, &opts, 20.0, 0, &[step]).expect("drift solve");
    let wafer = &drift.source_t_max_c[0];
    println!("\nambient step +2 C at the chamber floor (2 h at 1-min steps):");
    for frac in [0.5, 0.9, 0.99] {
        let target = 20.0 + 2.0 * frac;
        match wafer.iter().position(|&t| t >= target) {
            Some(i) => println!(
                "  wafer through {:.0}% of the step after {:>6.0} s ({:.1} min)",
                frac * 100.0,
                drift.times_s[i],
                drift.times_s[i] / 60.0
            ),
            None => println!(
                "  wafer NOT through {:.0}% of the step in 2 h (T_end = {:.3} C)",
                frac * 100.0,
                wafer.last().unwrap()
            ),
        }
    }
    println!(
        "  energy audit residual {:.1e}",
        drift.energy_audit_residual_rel
    );

    // --- RTP-shaped soak: lamp on / soak / off -----------------------
    // 200 W into the wafer for 60 s, then off for 10 min: how hot does
    // the wafer get in chuck contact, and how fast does it recover?
    let mut m = stack();
    m.sources.push(beam(0.0));
    let mut heat = ScheduleSegment::plain(1.0, 60);
    heat.source_power_w.insert("beam".into(), 200.0);
    let mut cool = ScheduleSegment::plain(5.0, 120);
    cool.source_power_w.insert("beam".into(), 0.0);
    let rtp = solve_transient_schedule(&m, &opts, 20.0, 0, &[heat, cool]).expect("rtp solve");
    let peak = rtp
        .t_max_c
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    println!("\nRTP-shaped: 200 W x 60 s into the wafer, then off (chuck contact):");
    println!(
        "  peak wafer T = {:.1} C at t = 60 s; T after 10 min cool = {:.2} C",
        peak,
        rtp.t_max_c.last().unwrap()
    );
    println!(
        "  stored {:.1} J vs injected {:.1} J (audit {:.1e}, {} CG iters total)",
        rtp.stored_delta_j, rtp.injected_j, rtp.energy_audit_residual_rel, rtp.cg_iterations_total
    );
    println!(
        "\n(The chuck-contact theta = {theta:.3} K/W is why RTP at 1000 C needs lift \
         pins: the soak would drain kilowatts into the stack. Conduction only; \
         radiation not modeled.)"
    );
}
