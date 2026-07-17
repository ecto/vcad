//! Adjoint-vs-finite-difference gradient check, printable.
//!
//! Runs the adjoint on a two-material board (FR4-ish plate, aluminum
//! spreader, one 3 W die, two convection films, one cold reservoir) and
//! prints every gradient against a central finite difference on the same
//! frozen grid. The conduction operator is self-adjoint, so the whole
//! gradient costs one extra CG solve; the FD column costs two full
//! solves *per parameter* — the table is also the price argument.
//!
//! Run: `cargo run --release -p vcad-kernel-thermal --example gradient_check`

use vcad_kernel_thermal::adjoint::{smooth_max_gradient, ObjectiveOptions};
use vcad_kernel_thermal::model::{
    Boundary, FixedTemperature, MaterialRegion, PowerSource, Shape, ThermalModel,
};
use vcad_kernel_thermal::solve::SolveOptions;

fn model() -> ThermalModel {
    let mut m = ThermalModel::new([0.0, 0.0, 0.0], [40.0, 40.0, 4.0], [20, 20, 2]);
    m.materials.push(MaterialRegion::anisotropic(
        Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [40.0, 40.0, 4.0],
        },
        [2.0, 2.0, 0.8],
    ));
    m.materials.push(MaterialRegion::isotropic(
        Shape::Box {
            min_mm: [12.0, 12.0, 0.0],
            size_mm: [16.0, 16.0, 4.0],
        },
        160.0,
    ));
    m.sources.push(PowerSource {
        name: "die".into(),
        shape: Shape::Box {
            min_mm: [16.0, 16.0, 0.0],
            size_mm: [8.0, 8.0, 4.0],
        },
        power_w: 3.0,
    });
    m.fixed.push(FixedTemperature {
        shape: Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [4.0, 40.0, 4.0],
        },
        temperature_c: 20.0,
    });
    m.domain_faces[4] = Boundary::Convection {
        h_w_m2k: 12.0,
        ambient_c: 25.0,
    };
    m.domain_faces[5] = Boundary::Convection {
        h_w_m2k: 8.0,
        ambient_c: 25.0,
    };
    m.reference_c = Some(25.0);
    m
}

fn objective(m: &ThermalModel, opts: &SolveOptions) -> f64 {
    smooth_max_gradient(m, opts, &ObjectiveOptions::default())
        .expect("solve")
        .1
        .value_c
}

fn main() {
    let opts = SolveOptions {
        tol: 1e-12,
        max_iters: 200_000,
    };
    let m = model();
    let (sol, grad) = smooth_max_gradient(&m, &opts, &ObjectiveOptions::default()).expect("solve");

    println!(
        "objective: smoothed T_max = {:.4} C (hard max {:.4} C, p = {}, {} active voxels)",
        grad.value_c, grad.hard_max_c, grad.p, grad.n_active
    );
    println!(
        "forward {} CG iters, adjoint {} — the entire gradient costs one extra solve\n",
        grad.forward_iterations, grad.adjoint_iterations
    );
    println!(
        "{:<28} {:>14} {:>14} {:>10}",
        "parameter", "adjoint", "central FD", "rel err"
    );

    let row = |name: &str, adj: f64, fd: f64| {
        println!(
            "{name:<28} {adj:>14.6e} {fd:>14.6e} {:>10.1e}",
            (adj - fd).abs() / fd.abs().max(1e-300)
        );
    };

    // Source power.
    {
        let h = 0.03;
        let mut up = m.clone();
        up.sources[0].power_w += h;
        let mut dn = m.clone();
        dn.sources[0].power_w -= h;
        let fd = (objective(&up, &opts) - objective(&dn, &opts)) / (2.0 * h);
        row("dJ/dP  (die, K/W)", grad.d_source_power[0], fd);
    }
    // Conductivities: plate x and z, spreader y.
    for (label, region, axis, h) in [
        ("dJ/dk  (plate, x)", 0usize, 0usize, 0.002),
        ("dJ/dk  (plate, z)", 0, 2, 0.001),
        ("dJ/dk  (spreader, y)", 1, 1, 0.2),
    ] {
        let mut up = m.clone();
        up.materials[region].k_w_mk[axis] += h;
        let mut dn = m.clone();
        dn.materials[region].k_w_mk[axis] -= h;
        let fd = (objective(&up, &opts) - objective(&dn, &opts)) / (2.0 * h);
        row(label, grad.d_conductivity[region][axis], fd);
    }
    // Films.
    for (label, slot) in [("dJ/dh  (bottom face)", 4usize), ("dJ/dh  (top face)", 5)] {
        let h = 0.01;
        let mut up = m.clone();
        let mut dn = m.clone();
        if let Boundary::Convection { h_w_m2k, .. } = &mut up.domain_faces[slot] {
            *h_w_m2k += h;
        }
        if let Boundary::Convection { h_w_m2k, .. } = &mut dn.domain_faces[slot] {
            *h_w_m2k -= h;
        }
        let fd = (objective(&up, &opts) - objective(&dn, &opts)) / (2.0 * h);
        row(label, grad.d_film[slot].expect("convection"), fd);
    }

    println!(
        "\nsteady T_max {:.4} C at ({:.0}, {:.0}, {:.0}) mm; energy residual {:.1e}",
        sol.t_max_c,
        sol.t_max_at_mm[0],
        sol.t_max_at_mm[1],
        sol.t_max_at_mm[2],
        sol.energy.residual_rel
    );
    println!(
        "geometry gradients are deliberately absent: shapes move the discrete material\n\
         mask, which no smooth adjoint covers — geometry stays finite-difference until\n\
         a shape-adjoint milestone (frozen-mask differentiation)."
    );
}
