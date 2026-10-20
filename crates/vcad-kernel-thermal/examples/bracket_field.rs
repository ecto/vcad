//! Multiphysics demo, thermal layer: steady temperature in an aluminum
//! motor-controller L-bracket (docs/compressed-fields.md, "multiphysics").
//!
//! Shared geometry (identical in the topopt and em `bracket_field`
//! examples): domain 96×48×36 mm on a 128×64×48 grid (0.75 mm cubes); base
//! plate x 0–96, y 0–48, z 0–12; wall flange x 0–9, z 0–36. A 6 W MOSFET
//! sits on the plate's far end (x 76–90, y 17–31, top 1.5 mm). The flange
//! back face (x = 0) is bolted to a 40 °C chassis; every other exposed face
//! convects at h = 12 W/m²K to 25 °C air (supplied, not derived).
//!
//! Writes `OUT.f32` (row-major (z, y, x), °C; void = 25) and `OUT.json`.
//! Run: `cargo run --release -p vcad-kernel-thermal --example bracket_field -- OUT`

use vcad_kernel_thermal::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bracket_thermal".into());
    let mut m = ThermalModel::new([0.0; 3], [96.0, 48.0, 36.0], [128, 64, 48]);
    let al = 167.0; // 6061-T6, W/m·K
    for (min_mm, size_mm) in [
        ([0.0, 0.0, 0.0], [96.0, 48.0, 12.0]),
        ([0.0, 0.0, 0.0], [9.0, 48.0, 36.0]),
    ] {
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box { min_mm, size_mm },
            al,
        ));
    }
    m.sources.push(PowerSource {
        name: "mosfet".into(),
        shape: Shape::Box {
            min_mm: [76.0, 17.0, 10.5],
            size_mm: [14.0, 14.0, 1.5],
        },
        power_w: 6.0,
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

    let s = solve_steady(&m, &SolveOptions::default()).expect("solve");
    let f: Vec<f32> = s
        .t_c
        .iter()
        .zip(&s.solid)
        .map(|(&t, &solid)| if solid { t as f32 } else { 25.0 })
        .collect();
    let (lo, hi) = f
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    std::fs::write(
        format!("{out}.f32"),
        f.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(),
    )
    .unwrap();
    let meta = format!(
        "{{\"field\":\"temperature\",\"units\":\"degC\",\"source\":\"vcad-kernel-thermal bracket (6 W MOSFET, 40 C wall, h=12)\",\
         \"dims_zyx\":[48,64,128],\"origin_mm\":[0,0,0],\"voxel_mm\":[0.75,0.75,0.75],\"min\":{lo},\"max\":{hi},\
         \"energy_residual_rel\":{:e}}}\n",
        s.energy.residual_rel
    );
    std::fs::write(format!("{out}.json"), &meta).unwrap();
    print!("{meta}");
}
