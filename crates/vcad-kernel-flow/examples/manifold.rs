//! Flagship M0 example: a printed splitter manifold.
//!
//! One 4 mm square inlet duct tees into two parallel 4 mm outlet ducts.
//! The solve predicts the pressure cost and the split ratio, the lumped
//! oracle prices the straight-duct portion, and the claim set — with the
//! cross-route residual — prints as JSON.
//!
//! Run: `cargo run --release -p vcad-kernel-flow --example manifold`

use vcad_kernel_flow::lumped;
use vcad_kernel_flow::model::Cell;
use vcad_kernel_flow::receipt::predicted_claims;
use vcad_kernel_flow::solve::{solve_steady, SolveOptions};
use vcad_kernel_flow::spec::{FlowSpec, FluidSpec, RegionSpec, Role, ShapeSpec};

fn boxr(min_mm: [f64; 3], size_mm: [f64; 3], role: Role) -> RegionSpec {
    RegionSpec {
        shape: ShapeSpec::Box { min_mm, size_mm },
        role,
    }
}

fn main() {
    // 40 x 16 x 6 mm at 0.5 mm voxels. Inlet duct runs along +x at the
    // center, tees into a cross-duct, which feeds two outlet ducts
    // continuing along +x at the sides.
    let spec = FlowSpec {
        origin_mm: [0.0; 3],
        size_mm: [40.0, 16.0, 6.0],
        divisions: [80, 32, 12],
        background: Role::Solid,
        regions: vec![
            // Inlet duct: x in [1, 20), centered in y.
            boxr([1.0, 6.0, 1.0], [19.0, 4.0, 4.0], Role::Fluid),
            // Cross duct at x in [16, 20).
            boxr([16.0, 1.0, 1.0], [4.0, 14.0, 4.0], Role::Fluid),
            // Two outlet ducts: x in [16, 39).
            boxr([16.0, 1.0, 1.0], [23.0, 4.0, 4.0], Role::Fluid),
            boxr([16.0, 11.0, 1.0], [23.0, 4.0, 4.0], Role::Fluid),
            // Ports.
            boxr([0.0, 6.0, 1.0], [1.0, 4.0, 4.0], Role::Inlet),
            boxr([39.0, 1.0, 1.0], [1.0, 4.0, 4.0], Role::Outlet),
            boxr([39.0, 11.0, 1.0], [1.0, 4.0, 4.0], Role::Outlet),
        ],
        fluid: FluidSpec {
            density_kg_m3: 1.204,
            viscosity_pa_s: 1.825e-5,
        },
        inlet_velocity_m_s: [0.15, 0.0, 0.0],
        outlet_gauge_pa: 0.0,
        body_force_n_m3: [0.0; 3],
        periodic: [false; 3],
        re_envelope: None,
        thermal: None,
        hot_walls: vec![],
    };

    let model = spec.resolve().expect("spec resolves");
    let re = model.reynolds().unwrap();
    println!(
        "manifold: {} fluid voxels, inlet Re = {re:.0}",
        model.count(Cell::Fluid)
    );

    let opts = SolveOptions::default();
    let sol = solve_steady(&model, &opts).expect("solve");
    println!(
        "steady in {} steps (residual {:.2e}); dp = {:.4} Pa, Q = {:.3e} m3/s, mass \
         residual {:.2e}",
        sol.steps,
        sol.steady_residual,
        sol.pressure_drop_pa,
        sol.outlet_flow_m3_s,
        sol.mass_balance_residual
    );

    // Split ratio between the two outlets (symmetric geometry -> ~0.5).
    let split = outlet_split(&model, &sol);
    println!("outlet split: {:.3} / {:.3}", split, 1.0 - split);

    // Lumped route: straight-duct gradient over the total wetted length
    // (inlet duct + one branch), priced at the inlet flow.
    let w = 4.0e-3;
    let len_m = 38.0e-3;
    let dpdx = lumped::rect_duct_pressure_gradient_pa_m(sol.inlet_flow_m3_s, w, w, 1.825e-5);
    let dp_oracle = dpdx * len_m * 0.75; // branch carries half the flow
    let claims = predicted_claims(&model, &sol, &opts, Some(dp_oracle));
    println!(
        "{}",
        serde_json::to_string_pretty(&claims).expect("claims serialize")
    );
}

/// Fraction of outlet flow through the -y branch.
fn outlet_split(
    model: &vcad_kernel_flow::model::FlowModel,
    sol: &vcad_kernel_flow::solve::Solution,
) -> f64 {
    let (nx, ny, nz) = (model.divisions[0], model.divisions[1], model.divisions[2]);
    let mut low = 0.0;
    let mut high = 0.0;
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let x = model.index(i, j, k);
                if model.cells[x] != Cell::Outlet {
                    continue;
                }
                // Fluid neighbor in -x feeds this outlet.
                if i == 0 {
                    continue;
                }
                let fx = model.index(i - 1, j, k);
                if model.cells[fx] != Cell::Fluid {
                    continue;
                }
                let u = sol.velocity_m_s[fx][0];
                if (j as f64) < ny as f64 / 2.0 {
                    low += u;
                } else {
                    high += u;
                }
            }
        }
    }
    let total = low + high;
    if total.abs() < f64::MIN_POSITIVE {
        return 0.5;
    }
    low / total
}
