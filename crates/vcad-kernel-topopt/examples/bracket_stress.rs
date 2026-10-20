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
//! `--thermal` is given (skip it with `--coupled-only`); row-major (z, y, x),
//! void = 0.
//! Run: `cargo run --release -p vcad-kernel-topopt --example bracket_stress -- OUT [--thermal T.f32]`
//!
//! Animation: `-- --frames DIR` solves every `DIR/T_####.f32` (from the
//! thermal `bracket_transient` example) in one process, warm-starting each
//! PCG solve from the previous frame's displacements, and writes
//! `DIR/S_####_vm.{f32,json}` (coupled von Mises).
//!
//! Fast animation: `-- --frames DIR --rom` exploits linearity instead of
//! solving every frame. Stress is linear in (load, ΔT), and the transient's
//! ΔT frames span a low-dimensional subspace, so: POD the frames (method of
//! snapshots), keep the r modes that capture all but 1e-10 of the energy,
//! solve the FE problem once per mode plus once for the load alone, and
//! rebuild each frame's stress *tensor* as σ_load + Σ c_i σ_i (von Mises
//! after superposition, so its nonlinearity is exact). Writes
//! `DIR/R_####_vm.{f32,json}`; when full solves `DIR/S_####_vm.f32` exist
//! it reports max |σ_rom − σ_full| against them.

use vcad_kernel_topopt::{
    analyze_fields_thermal, analyze_fields_thermal_warm, von_mises, AnalysisSpec, Domain, Load,
    RegionBox, Support, ThermalLoad,
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
    // `--coupled-only`: skip the load-only solve (animation frames).
    let coupled_only = args.iter().any(|a| a == "--coupled-only");
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
    let read_t = |p: &std::path::Path| -> Vec<f64> {
        std::fs::read(p)
            .expect("thermal field")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c) as f64)
            .collect()
    };
    if let Some(i) = args.iter().position(|a| a == "--frames") {
        if args.iter().any(|a| a == "--rom") {
            return rom_frames(&std::path::PathBuf::from(&args[i + 1]), &d, &spec, read_t);
        }
        let dir = std::path::PathBuf::from(&args[i + 1]);
        let coupled = "vcad-kernel-topopt voxel FEA bracket frame (60 N tip + thermal strain α(T−40 °C), fixed wall)";
        let mut warm = Vec::new();
        let t0 = std::time::Instant::now();
        for k in 0.. {
            let tp = dir.join(format!("T_{k:04}.f32"));
            if !tp.exists() {
                eprintln!("{k} frames in {:.1} s", t0.elapsed().as_secs_f64());
                break;
            }
            let tf = std::time::Instant::now();
            let t_c = read_t(&tp);
            assert_eq!(
                t_c.len(),
                d.num_elements(),
                "thermal grid must match the FE grid"
            );
            let load = ThermalLoad {
                delta_t_k: t_c.iter().map(|t| t - WALL_C).collect(),
                alpha_per_k: 23e-6,
            };
            let (a, f) =
                analyze_fields_thermal_warm(&d, &spec, Some(&load), &mut warm).expect("analyze");
            assert!(
                a.converged,
                "frame {k}: PCG residual {}",
                a.relative_residual
            );
            let vm: Vec<f32> = f.von_mises_mpa.iter().map(|&v| v as f32).collect();
            let extra = format!(
                ",\"max_von_mises_mpa\":{},\"relative_residual\":{:e}",
                a.max_von_mises_mpa, a.relative_residual
            );
            write(
                dir.join(format!("S_{k:04}_vm")).to_str().unwrap(),
                "von_mises",
                "MPa",
                coupled,
                &vm,
                &extra,
            );
            eprintln!("frame {k}: {:.2} s", tf.elapsed().as_secs_f64());
        }
        return;
    }
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
            if !coupled_only {
                let (vm, _, extra) = run(None);
                write(
                    &format!("{out}_vm_mech"),
                    "von_mises",
                    "MPa",
                    mech,
                    &vm,
                    &extra,
                );
            }
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

/// Symmetric eigendecomposition by cyclic Jacobi (small matrices only).
/// Returns (eigenvalues, column eigenvectors), unsorted.
#[allow(clippy::needless_range_loop)] // textbook index form of the rotations
fn jacobi_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut v: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| f64::from(i == j)).collect())
        .collect();
    for _sweep in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        let diag: f64 = (0..n).map(|i| a[i][i] * a[i][i]).sum();
        if off <= 1e-30 * diag.max(1e-300) {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum().max(0.0).mul_add(2.0, -1.0)
                    / (theta.abs() + (theta * theta + 1.0).sqrt());
                let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
                for k in 0..n {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i][i]).collect(), v)
}

fn rom_frames(
    dir: &std::path::Path,
    d: &Domain,
    spec: &AnalysisSpec,
    read_t: impl Fn(&std::path::Path) -> Vec<f64>,
) {
    let t0 = std::time::Instant::now();
    let solid: Vec<usize> = (0..d.num_elements()).filter(|&e| d.active[e]).collect();
    // Snapshot matrix X: one ΔT column per frame, over active elements.
    let mut x: Vec<Vec<f64>> = Vec::new();
    while dir.join(format!("T_{:04}.f32", x.len())).exists() {
        let t = read_t(&dir.join(format!("T_{:04}.f32", x.len())));
        x.push(solid.iter().map(|&e| t[e] - WALL_C).collect());
    }
    let n = x.len();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(p, q)| p * q).sum::<f64>();
    let gram: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| dot(&x[i], &x[j])).collect())
        .collect();
    let (lam, vecs) = jacobi_eigen(gram);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| lam[b].total_cmp(&lam[a]));
    let total: f64 = lam.iter().map(|l| l.max(0.0)).sum();
    let (mut r, mut kept) = (0, 0.0);
    while r < order.len() && r < 16 && (r == 0 || 1.0 - kept / total > 1e-10) {
        kept += lam[order[r]].max(0.0);
        r += 1;
    }
    // Modes φ_i = X v_i / √λ_i (orthonormal over active elements).
    let modes: Vec<Vec<f64>> = order[..r]
        .iter()
        .map(|&m| {
            let sl = lam[m].sqrt();
            (0..solid.len())
                .map(|e| (0..n).map(|k| x[k][e] * vecs[k][m]).sum::<f64>() / sl)
                .collect()
        })
        .collect();
    let coeff: Vec<Vec<f64>> = x
        .iter()
        .map(|xk| modes.iter().map(|phi| dot(xk, phi)).collect())
        .collect();
    let proj_err = x
        .iter()
        .zip(&coeff)
        .map(|(xk, c)| {
            let nx = dot(xk, xk).sqrt();
            let res: f64 = (0..solid.len())
                .map(|e| {
                    (xk[e] - c.iter().zip(&modes).map(|(ci, p)| ci * p[e]).sum::<f64>()).powi(2)
                })
                .sum::<f64>()
                .sqrt();
            if nx > 0.0 {
                res / nx
            } else {
                0.0
            }
        })
        .fold(0.0, f64::max);
    eprintln!(
        "POD: {n} frames → {r} modes, max projection error {proj_err:.1e} ({:.1} s)",
        t0.elapsed().as_secs_f64()
    );

    // One solve for the load alone, one per mode (load + mode); σ_i = σ_(load+mode) − σ_load.
    let mut warm = Vec::new();
    let (_, base) = analyze_fields_thermal_warm(d, spec, None, &mut warm).expect("analyze");
    let mut sig_modes: Vec<Vec<[f64; 6]>> = Vec::new();
    for phi in &modes {
        let mut dt = vec![0.0; d.num_elements()];
        for (j, &e) in solid.iter().enumerate() {
            dt[e] = phi[j];
        }
        let load = ThermalLoad {
            delta_t_k: dt,
            alpha_per_k: 23e-6,
        };
        let (a, f) = analyze_fields_thermal_warm(d, spec, Some(&load), &mut warm).expect("analyze");
        assert!(a.converged);
        sig_modes.push(
            f.stress_mpa
                .iter()
                .zip(&base.stress_mpa)
                .map(|(m, b)| std::array::from_fn(|c| m[c] - b[c]))
                .collect(),
        );
    }
    let solves_s = t0.elapsed().as_secs_f64();
    eprintln!("{} FE solves done ({:.1} s)", r + 1, solves_s);

    let coupled = "vcad-kernel-topopt voxel FEA bracket frame, reduced-order (POD) superposition of (60 N tip + thermal strain α(T−40 °C)), fixed wall";
    let mut worst: f64 = 0.0;
    let mut compared = 0;
    for (k, c) in coeff.iter().enumerate() {
        let vm: Vec<f32> = (0..d.num_elements())
            .map(|e| {
                if !d.active[e] {
                    return 0.0;
                }
                let s: [f64; 6] = std::array::from_fn(|q| {
                    base.stress_mpa[e][q]
                        + c.iter()
                            .zip(&sig_modes)
                            .map(|(ci, sm)| ci * sm[e][q])
                            .sum::<f64>()
                });
                von_mises(&s) as f32
            })
            .collect();
        let full = dir.join(format!("S_{k:04}_vm.f32"));
        if full.exists() {
            let fv: Vec<f32> = std::fs::read(&full)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| f32::from_le_bytes(b))
                .collect();
            if fv.len() == vm.len() {
                worst = worst.max(
                    vm.iter()
                        .zip(&fv)
                        .map(|(a, b)| (a - b).abs() as f64)
                        .fold(0.0, f64::max),
                );
                compared += 1;
            }
        }
        let extra = format!(",\"rom_modes\":{r},\"rom_projection_error\":{proj_err:e}");
        write(
            dir.join(format!("R_{k:04}_vm")).to_str().unwrap(),
            "von_mises",
            "MPa",
            coupled,
            &vm,
            &extra,
        );
    }
    eprintln!(
        "{n} frames in {:.1} s ({} solves instead of {n}); vs {compared} full solves: max |Δσ| = {worst:.2e} MPa",
        t0.elapsed().as_secs_f64(),
        r + 1
    );
}
