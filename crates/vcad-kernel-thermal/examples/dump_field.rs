//! Dump the hot_chip steady temperature field as a raw f32 grid, for
//! compressed-field experiments (docs/compressed-fields.md).
//!
//! Same physics as `hot_chip` (2 W die on a 100×100×1.6 mm board,
//! anisotropic k = [15, 15, 0.4], h = 10), on a finer 128×128×16 grid.
//! Writes `OUT.f32` (little-endian, row-major (z, y, x), °C; void voxels
//! pinned to the 25 °C ambient) and `OUT.json` (dims, origin, voxel size,
//! T range, solver residual).
//!
//! Run: `cargo run --release -p vcad-kernel-thermal --example dump_field -- OUT`

use vcad_kernel_thermal::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};
use vcad_kernel_thermal::solve::{solve_steady, SolveOptions};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "hot_chip".into());
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [100.0, 100.0, 2.6], [128, 128, 16]);
    m.materials.push(MaterialRegion::anisotropic(
        Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [100.0, 100.0, 1.6],
        },
        [15.0, 15.0, 0.4],
    ));
    let die = Shape::Box {
        min_mm: [45.0, 45.0, 1.6],
        size_mm: [10.0, 10.0, 1.0],
    };
    m.materials
        .push(MaterialRegion::isotropic(die.clone(), 120.0));
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: die,
        power_w: 2.0,
    });
    let conv = Boundary::Convection {
        h_w_m2k: 10.0,
        ambient_c: 25.0,
    };
    m.domain_faces[4] = conv;
    m.domain_faces[5] = conv;
    m.exposed = conv;

    let s = solve_steady(&m, &SolveOptions::default()).expect("solve");
    // Void voxels carry no physics; pin them to ambient so the stored
    // field is a deterministic function of the model.
    let f: Vec<f32> = s
        .t_c
        .iter()
        .zip(&s.solid)
        .map(|(&t, &solid)| if solid { t as f32 } else { 25.0 })
        .collect();
    let bytes: Vec<u8> = f.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(format!("{out}.f32"), bytes).unwrap();
    let (lo, hi) = f
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let [nx, ny, nz] = s.divisions;
    let meta = format!(
        "{{\"field\":\"temperature_c\",\"source\":\"vcad-kernel-thermal hot_chip h=10 k=[15,15,0.4]\",\
         \"dims_zyx\":[{nz},{ny},{nx}],\"origin_mm\":{:?},\"voxel_mm\":{:?},\"min\":{lo},\"max\":{hi},\
         \"t_max_c\":{},\"energy_residual_rel\":{:e}}}\n",
        s.origin_mm, s.voxel_mm, s.t_max_c, s.energy.residual_rel
    );
    std::fs::write(format!("{out}.json"), &meta).unwrap();
    print!("{meta}");
}
