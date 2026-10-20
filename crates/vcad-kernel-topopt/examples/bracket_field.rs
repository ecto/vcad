//! Multiphysics demo, structural layers: von Mises stress and displacement
//! in the aluminum L-bracket (geometry in vcad-kernel-thermal's
//! `bracket_field`). Flange back face (x = 0) fully fixed; 60 N downward
//! on the plate's far-end top strip (x 90–96, z = 12) — a cable connector
//! hanging off the controller. 6061-T6: E = 69 GPa, ν = 0.33.
//!
//! Voxel FEA smears concentrations (see `StaticAnalysis::max_von_mises_mpa`);
//! 12 mm plate = 16 elements through, well clear of hex locking.
//!
//! Writes `OUT_vm.{f32,json}` (MPa) and `OUT_disp.{f32,json}` (µm), row-major
//! (z, y, x), void = 0.
//! Run: `cargo run --release -p vcad-kernel-topopt --example bracket_field -- OUT`

use vcad_kernel_topopt::{analyze_fields, AnalysisSpec, Domain, Load, RegionBox, Support};

fn write(out: &str, field: &str, units: &str, v: &[f32], extra: &str) {
    let (lo, hi) = v
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    std::fs::write(
        format!("{out}.f32"),
        v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
    )
    .unwrap();
    let meta = format!(
        "{{\"field\":\"{field}\",\"units\":\"{units}\",\"source\":\"vcad-kernel-topopt voxel FEA bracket (60 N tip, fixed wall)\",\
         \"dims_zyx\":[48,64,128],\"origin_mm\":[0,0,0],\"voxel_mm\":[0.75,0.75,0.75],\"min\":{lo},\"max\":{hi}{extra}}}\n"
    );
    std::fs::write(format!("{out}.json"), &meta).unwrap();
    print!("{meta}");
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "bracket".into());
    let mut d = Domain::from_bbox([0.0; 3], [96.0, 48.0, 36.0], 128);
    assert_eq!([d.nx, d.ny, d.nz], [128, 64, 48]);
    for iz in 0..d.nz {
        for iy in 0..d.ny {
            for ix in 0..d.nx {
                let (x, z) = ((ix as f64 + 0.5) * d.h, (iz as f64 + 0.5) * d.h);
                let e = d.eidx(ix, iy, iz);
                d.active[e] = z < 12.0 || x < 9.0;
            }
        }
    }
    let spec = AnalysisSpec {
        resolution: 128,
        youngs_modulus_mpa: 69_000.0,
        poisson: 0.33,
        loads: vec![Load {
            region: RegionBox {
                min: [90.0, 0.0, 12.0],
                max: [96.0, 48.0, 12.0],
            },
            force: [0.0, 0.0, -60.0],
        }],
        supports: vec![Support {
            region: RegionBox {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 48.0, 36.0],
            },
            fix: [true; 3],
        }],
    };
    let (a, f) = analyze_fields(&d, &spec).expect("analyze");
    assert!(a.converged, "PCG residual {}", a.relative_residual);
    let extra = format!(
        ",\"max_von_mises_mpa\":{},\"max_displacement_mm\":{},\"relative_residual\":{:e}",
        a.max_von_mises_mpa, a.max_displacement_mm, a.relative_residual
    );
    let vm: Vec<f32> = f.von_mises_mpa.iter().map(|&v| v as f32).collect();
    let um: Vec<f32> = f
        .displacement_mm
        .iter()
        .map(|&v| (v * 1e3) as f32)
        .collect();
    write(&format!("{out}_vm"), "von_mises", "MPa", &vm, &extra);
    write(&format!("{out}_disp"), "displacement", "um", &um, &extra);
}
