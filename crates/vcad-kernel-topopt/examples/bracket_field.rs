//! Multiphysics demo, structural layers: von Mises stress and displacement
//! in the aluminum L-bracket (geometry in vcad-kernel-thermal's
//! `bracket_field`). Flange back face (x = 0) fully fixed; 60 N downward
//! on the plate's far-end top strip (x 90–96, z = 12) — a cable connector
//! hanging off the controller. 6061-T6: E = 69 GPa, ν = 0.33.
//!
//! Voxel FEA smears concentrations (see `StaticAnalysis::max_von_mises_mpa`);
//! 12 mm plate = 16 elements through, well clear of hex locking.
//!
//! Coupling (thermal → structural): with `--thermal T.f32` (the thermal
//! `bracket_field` output — same 128×64×48 grid, °C) the temperature rise
//! over the 40 °C wall is applied as thermal strain α·ΔT (6061-T6
//! α = 23e-6/K) on top of the 60 N load. Reference = wall temperature: the
//! chassis is the same alloy at 40 °C, so a bracket uniformly at 40 °C
//! grows with it stress-free and only T − 40 °C is mismatch. (A 25 °C
//! reference against a rigid wall reads ~99 MPa — it charges the bracket
//! for expansion the chassis shares.) The wall face is fully fixed, which
//! over-restrains a real bolted joint, so near-wall stress is an upper
//! bound.
//!
//! Writes `OUT_vm.{f32,json}` (MPa) and `OUT_disp.{f32,json}` (µm) for the
//! coupled case, plus `OUT_vm_mech.{f32,json}` (the 60 N load alone) when
//! `--thermal` is given; row-major (z, y, x), void = 0.
//! Run: `cargo run --release -p vcad-kernel-topopt --example bracket_field -- OUT [--thermal T.f32]`

use vcad_kernel_topopt::{
    analyze_fields_thermal, AnalysisSpec, Domain, Load, RegionBox, Support, ThermalLoad,
};

fn write(out: &str, field: &str, units: &str, source: &str, v: &[f32], extra: &str) {
    let (lo, hi) = v
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    std::fs::write(
        format!("{out}.f32"),
        v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
    )
    .unwrap();
    let meta = format!(
        "{{\"field\":\"{field}\",\"units\":\"{units}\",\"source\":\"{source}\",\
         \"dims_zyx\":[48,64,128],\"origin_mm\":[0,0,0],\"voxel_mm\":[0.75,0.75,0.75],\"min\":{lo},\"max\":{hi}{extra}}}\n"
    );
    std::fs::write(format!("{out}.json"), &meta).unwrap();
    print!("{meta}");
}

/// Wall / chassis temperature, °C: the stress-free reference (see docs).
const WALL_C: f64 = 40.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "bracket".into());
    let thermal_path = args
        .iter()
        .position(|a| a == "--thermal")
        .map(|i| args[i + 1].clone());
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
    let run = |thermal: Option<&ThermalLoad>| {
        let (a, f) = analyze_fields_thermal(&d, &spec, thermal).expect("analyze");
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
        (vm, um, extra)
    };
    let mech = "vcad-kernel-topopt voxel FEA bracket (60 N tip, fixed wall)";
    match thermal_path {
        None => {
            let (vm, um, extra) = run(None);
            write(&format!("{out}_vm"), "von_mises", "MPa", mech, &vm, &extra);
            write(
                &format!("{out}_disp"),
                "displacement",
                "um",
                mech,
                &um,
                &extra,
            );
        }
        Some(p) => {
            let t_c: Vec<f64> = std::fs::read(&p)
                .expect("thermal field")
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&c| f32::from_le_bytes(c) as f64)
                .collect();
            assert_eq!(
                t_c.len(),
                d.num_elements(),
                "thermal grid must match the FE grid"
            );
            let load = ThermalLoad {
                delta_t_k: t_c.iter().map(|t| t - WALL_C).collect(),
                alpha_per_k: 23e-6,
            };
            let (vm, _, extra) = run(None);
            write(
                &format!("{out}_vm_mech"),
                "von_mises",
                "MPa",
                mech,
                &vm,
                &extra,
            );
            let coupled = "vcad-kernel-topopt voxel FEA bracket (60 N tip + thermal strain α(T−40 °C) from the thermal layer, fixed wall)";
            let (vm, um, extra) = run(Some(&load));
            write(
                &format!("{out}_vm"),
                "von_mises",
                "MPa",
                coupled,
                &vm,
                &extra,
            );
            write(
                &format!("{out}_disp"),
                "displacement",
                "um",
                coupled,
                &um,
                &extra,
            );
        }
    }
}
