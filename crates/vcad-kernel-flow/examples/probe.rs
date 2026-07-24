use vcad_kernel_flow::model::{Cell, FlowModel, Fluid, ThermalTransport};
use vcad_kernel_flow::solve::{solve_steady, SolveOptions};

fn main() {
    let (nx, w) = (30usize, 7usize);
    let mut m = FlowModel::new([0.0; 3], [30.0, 7.0, 7.0], [nx, w, w]);
    for k in 0..w {
        for j in 0..w {
            for i in 0..nx {
                let x = m.index(i, j, k);
                m.cells[x] = if i == 0 {
                    Cell::Inlet
                } else if i == nx - 1 {
                    Cell::Outlet
                } else {
                    Cell::Fluid
                };
            }
        }
    }
    m.fluid = Fluid::AIR_20C;
    m.inlet_velocity_m_s = [0.08, 0.0, 0.0];
    let mut t = ThermalTransport::AIR_20C;
    t.inlet_temp_c = 47.0;
    m.thermal = Some(t);
    for tol in [1e-6f64, 1e-8] {
        let opts = SolveOptions {
            steady_tol: tol,
            max_steps: 2_000_000,
            ..Default::default()
        };
        let sol = solve_steady(&m, &opts).expect("solve");
        let temp = sol.temperature_c.as_ref().unwrap();
        let center = temp[m.index(nx - 2, w / 2, w / 2)];
        let corner = temp[m.index(nx - 2, 1, 1)];
        println!(
            "tol={tol:.0e}: steps={} t_out={:.3} center={center:.3} corner={corner:.3}",
            sol.steps,
            sol.outlet_temp_c.unwrap()
        );
    }
}
