//! Multiphysics demo, transient: a power cycle on the bracket of
//! `bracket_field` (same geometry, grid, sources and boundaries; see there).
//!
//! t = 0: the whole bracket sits at the 40 °C wall temperature (stress-free
//! for the thermal-strain layer). 0–240 s: the 6 W MOSFET and the 0.266 W
//! busbar Joule source are on. 240–480 s: both off; the bracket relaxes back
//! toward the wall. 6061-T6 ρc = 2.43e6 J/m³K. Backward Euler, Δt = 2 s
//! (the bracket rises with a ~40 s time constant), a snapshot every
//! 8 s.
//!
//! Writes `OUT/T_0000.f32 … T_0060.f32` (row-major (z, y, x), °C; void = 25)
//! and `OUT/frames.json` (time, power and T_max per frame).
//! Run: `cargo run --release -p vcad-kernel-thermal --example bracket_transient -- OUT`

use std::collections::BTreeMap;
use vcad_kernel_thermal::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
use vcad_kernel_thermal::solve::SolveOptions;
use vcad_kernel_thermal::transient::{solve_transient_schedule, ScheduleSegment};

fn main() {
    let out = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "bracket_transient".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let mut m = ThermalModel::new([0.0; 3], [96.0, 48.0, 36.0], [128, 64, 48]);
    for (min_mm, size_mm) in [
        ([0.0, 0.0, 0.0], [96.0, 48.0, 12.0]),
        ([0.0, 0.0, 0.0], [9.0, 48.0, 36.0]),
    ] {
        m.materials.push(
            MaterialRegion::isotropic(Shape::Box { min_mm, size_mm }, 167.0)
                .with_heat_capacity(2.43e6),
        );
    }
    let mosfet_w = 6.0;
    let joule_w = 200.0f64.powi(2) * 3.99e-8 * 0.096 / (0.048 * 0.012);
    m.sources.push(PowerSource {
        name: "mosfet".into(),
        shape: Shape::Box {
            min_mm: [76.0, 17.0, 10.5],
            size_mm: [14.0, 14.0, 1.5],
        },
        power_w: mosfet_w,
    });
    m.sources.push(PowerSource {
        name: "busbar_joule".into(),
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [96.0, 48.0, 12.0],
        },
        power_w: joule_w,
    });
    let conv = Boundary::Convection {
        h_w_m2k: 12.0,
        ambient_c: 25.0,
    };
    m.domain_faces = [conv; 6];
    m.domain_faces[0] = Boundary::FixedTemperature {
        temperature_c: 40.0,
    };
    m.exposed = conv;

    let (dt, on_steps, off_steps, every) = (2.0, 120, 120, 4);
    let mut off = ScheduleSegment::plain(dt, off_steps);
    off.source_power_w = BTreeMap::from([("mosfet".into(), 0.0), ("busbar_joule".into(), 0.0)]);
    let s = solve_transient_schedule(
        &m,
        &SolveOptions::default(),
        40.0,
        every,
        &[ScheduleSegment::plain(dt, on_steps), off],
    )
    .expect("transient");

    let solid = &s.final_state.solid;
    let write = |k: usize, t_c: &[f64]| {
        let f: Vec<u8> = t_c
            .iter()
            .zip(solid)
            .flat_map(|(&t, &sd)| (if sd { t as f32 } else { 25.0 }).to_le_bytes())
            .collect();
        std::fs::write(out.join(format!("T_{k:04}.f32")), f).unwrap();
    };
    let mut frames = vec![(0.0, mosfet_w + joule_w, 40.0)];
    write(0, &vec![40.0; solid.len()]);
    for (k, snap) in s.snapshots.iter().enumerate() {
        write(k + 1, &snap.t_c);
        let tmax = snap
            .t_c
            .iter()
            .zip(solid)
            .filter(|(_, &sd)| sd)
            .map(|(&t, _)| t)
            .fold(f64::MIN, f64::max);
        let p = if snap.time_s <= dt * on_steps as f64 + 1e-9 {
            mosfet_w + joule_w
        } else {
            0.0
        };
        frames.push((snap.time_s, p, tmax));
    }
    let json: Vec<String> = frames
        .iter()
        .enumerate()
        .map(|(k, (t, p, tm))| {
            format!("{{\"frame\":{k},\"time_s\":{t},\"power_w\":{p},\"t_max_c\":{tm}}}")
        })
        .collect();
    std::fs::write(
        out.join("frames.json"),
        format!("[\n{}\n]\n", json.join(",\n")),
    )
    .unwrap();
    println!(
        "{} frames; T_max peak {:.3} °C; energy audit {:.1e}",
        frames.len(),
        frames.iter().map(|f| f.2).fold(f64::MIN, f64::max),
        s.energy_audit_residual_rel
    );
}
