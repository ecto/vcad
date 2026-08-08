//! Prints the adjoint-vs-finite-difference error for every QoI and
//! velocity field, at two mesh resolutions.
//!
//! The test suite asserts these stay under tolerance; this reports the
//! actual numbers so a change in accuracy is visible rather than merely
//! still-passing.
//!
//! Usage: `cargo run --release -p vcad-kernel-fea --example gradient_report`

use vcad_kernel_fea::adjoint::{shape_gradient, Qoi};
use vcad_kernel_fea::mesh::{box_mesh, tet_fill, TetMesh};
use vcad_kernel_fea::solve::SolveOptions;
use vcad_kernel_fea::spec::{FeaSpec, Load, RegionBox, Support};

const L: f64 = 80.0;
const B: f64 = 10.0;
const T: f64 = 8.0;
const P: f64 = 100.0;

fn tight() -> SolveOptions {
    SolveOptions {
        tol: 1e-12,
        max_iters: 200_000,
    }
}

fn spec(res: usize) -> FeaSpec {
    FeaSpec {
        resolution: res,
        youngs_modulus_mpa: 69_000.0,
        poisson: 0.33,
        yield_strength_mpa: None,
        loads: vec![Load {
            region: RegionBox {
                min: [L, -1.0, -1.0],
                max: [L, B + 1.0, T + 1.0],
            },
            force: [0.0, 0.0, -P],
        }],
        supports: vec![Support {
            region: RegionBox {
                min: [0.0, -1.0, -1.0],
                max: [0.0, B + 1.0, T + 1.0],
            },
            fix: [true, true, true],
        }],
    }
}

/// A nodal velocity field `dx/dθ` for one design parameter.
type VelField = fn(&TetMesh) -> Vec<[f64; 3]>;

fn perturbed(m: &TetMesh, vel: &[[f64; 3]], s: f64) -> TetMesh {
    let mut out = m.clone();
    for (p, v) in out.nodes.iter_mut().zip(vel) {
        for a in 0..3 {
            p[a] += s * v[a];
        }
    }
    out
}

fn main() {
    for res in [40usize, 80] {
        let m = tet_fill(&box_mesh([0.0; 3], [L, B, T]), res).unwrap();
        let sp = spec(res);
        println!(
            "\n=== resolution {res}: {} nodes, {} tets, h = {} mm ===",
            m.nodes.len(),
            m.tets.len(),
            m.h
        );

        let tip = RegionBox {
            min: [L, -1.0, -1.0],
            max: [L, B + 1.0, T + 1.0],
        };
        let qois: Vec<(&str, Qoi)> = vec![
            ("compliance (N·mm)", Qoi::Compliance),
            (
                "tip deflection (mm)",
                Qoi::MeanDisplacement {
                    region: tip,
                    direction: [0.0, 0.0, 1.0],
                },
            ),
            (
                "smooth-max stress p=8, no threshold (MPa)",
                Qoi::SmoothMaxVonMises {
                    p: 8.0,
                    threshold_mpa: None,
                },
            ),
            (
                "smooth-max stress p=8, threshold 55 MPa",
                Qoi::SmoothMaxVonMises {
                    p: 8.0,
                    threshold_mpa: Some(55.0),
                },
            ),
        ];
        let vels: [(&str, VelField); 2] = [
            ("thickness", |m: &TetMesh| {
                m.nodes.iter().map(|p| [0.0, 0.0, p[2] / T]).collect()
            }),
            ("taper", |m: &TetMesh| {
                m.nodes
                    .iter()
                    .map(|p| [0.0, 0.0, p[2] / T * (p[0] / L)])
                    .collect()
            }),
        ];

        for (qname, qoi) in &qois {
            let (_, g) = shape_gradient(&m, &sp, qoi, &tight()).unwrap();
            if let (Some(hard), Some(n)) = (g.hard_max_mpa, g.n_active) {
                println!(
                    "  {qname}: J = {:.4}  [hard max {:.4}, over-read {:+.1}%, {n} active]",
                    g.value,
                    hard,
                    (g.value / hard - 1.0) * 100.0
                );
            } else {
                println!("  {qname}: J = {:.9}", g.value);
            }
            println!(
                "    solves: {} forward + {} adjoint PCG iterations",
                g.forward_iterations, g.adjoint_iterations
            );
            for (vname, vf) in &vels {
                let vel = vf(&m);
                let adj = g.contract(&vel);
                let eps = 1e-4;
                let jp = shape_gradient(&perturbed(&m, &vel, eps), &sp, qoi, &tight())
                    .unwrap()
                    .1
                    .value;
                let jm = shape_gradient(&perturbed(&m, &vel, -eps), &sp, qoi, &tight())
                    .unwrap()
                    .1
                    .value;
                let num = (jp - jm) / (2.0 * eps);
                let rel = (adj - num).abs() / num.abs();
                println!("    d/d[{vname}]: adjoint {adj:.9e}  FD {num:.9e}  rel err {rel:.2e}");
            }
        }
    }
}
