//! M2: the conjugate seam to `vcad-kernel-thermal`.
//!
//! Segregated (loose) coupling on one shared voxel grid: the flow solver
//! computes the convective side — a film coefficient h priced from the
//! heat its scalar lattice actually exchanged with the walls, and a bulk
//! fluid temperature — and hands them to the thermal solver's existing
//! `Boundary::Convection` slot; the thermal solver conducts through the
//! solid and hands back its surface temperature field, which lands in
//! [`FlowModel::solid_temp_c`]. Iterate to a wall-temperature fixed
//! point, fail-closed on the iteration budget.
//!
//! **Honesty:** the thermal crate's `exposed` boundary is a single slot,
//! so the coupling is *film-averaged* — one h and one bulk temperature
//! describe the whole wetted surface per iteration (the wall
//! *temperature* field stays fully per-voxel in the other direction).
//! Strongly non-uniform convection (a jet hitting one corner) deserves
//! finer treatment than a film average; the cross-route residual
//! against the duct Nu correlation is the tell. Both solvers stay
//! independently testable: this module owns only the exchange loop.

use serde::{Deserialize, Serialize};

use vcad_kernel_thermal::model::{Boundary, ThermalModel};
use vcad_kernel_thermal::solve as thermal_solve;

use crate::model::{Cell, FlowModel};
use crate::solve::{solve_steady, Solution, SolveError, SolveOptions};

/// Conjugate loop options.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConjugateOptions {
    /// Outer-iteration budget.
    pub max_outer: usize,
    /// Convergence: largest wall-temperature change between outer
    /// iterations, °C.
    pub wall_tol_c: f64,
    /// Flow/scalar solve options per outer iteration.
    pub flow: SolveOptions,
    /// Thermal solve options per outer iteration.
    pub thermal_tol: f64,
    /// Thermal CG iteration budget.
    pub thermal_max_iters: usize,
}

impl Default for ConjugateOptions {
    fn default() -> Self {
        ConjugateOptions {
            max_outer: 20,
            wall_tol_c: 0.05,
            flow: SolveOptions::default(),
            thermal_tol: 1e-8,
            thermal_max_iters: 20_000,
        }
    }
}

/// Why the conjugate solve failed. Fail-closed.
#[derive(Debug)]
pub enum ConjugateError {
    /// Flow model must have thermal transport enabled.
    NoThermalTransport,
    /// The two models disagree on grid divisions or voxel size.
    GridMismatch {
        /// Flow grid.
        flow: [usize; 3],
        /// Thermal grid.
        thermal: [usize; 3],
    },
    /// No fluid↔solid interface exists to couple through.
    NoWettedSurface,
    /// Flow solve failed.
    Flow(SolveError),
    /// Thermal solve failed.
    Thermal(thermal_solve::SolveError),
    /// The wall-temperature fixed point did not converge in budget.
    NotConverged {
        /// Iterations run.
        iters: usize,
        /// Last wall-temperature change, °C.
        delta_c: f64,
        /// Tolerance it failed to meet.
        tol_c: f64,
    },
    /// The film coefficient could not be priced (zero wetted area or a
    /// degenerate wall-to-bulk temperature difference).
    DegenerateFilm,
}

impl std::fmt::Display for ConjugateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConjugateError::NoThermalTransport => {
                write!(
                    f,
                    "flow model needs thermal transport enabled for a conjugate solve"
                )
            }
            ConjugateError::GridMismatch { flow, thermal } => write!(
                f,
                "conjugate solvers must share one grid: flow {flow:?} vs thermal {thermal:?}"
            ),
            ConjugateError::NoWettedSurface => {
                write!(f, "no fluid-solid interface to couple through")
            }
            ConjugateError::Flow(e) => write!(f, "flow side: {e}"),
            ConjugateError::Thermal(e) => write!(f, "thermal side: {e}"),
            ConjugateError::NotConverged {
                iters,
                delta_c,
                tol_c,
            } => write!(
                f,
                "wall temperature not converged after {iters} outer iterations: last change \
                 {delta_c:.3} C > tol {tol_c:.3} C"
            ),
            ConjugateError::DegenerateFilm => write!(
                f,
                "film coefficient undefined: zero wetted area or wall at bulk temperature"
            ),
        }
    }
}

impl std::error::Error for ConjugateError {}

/// A converged conjugate result.
#[derive(Debug, Clone)]
pub struct ConjugateResult {
    /// Final flow/scalar solution.
    pub flow: Solution,
    /// Final thermal solution.
    pub thermal: thermal_solve::Solution,
    /// Film coefficient the loop converged at, W/(m²·K).
    pub film_h_w_m2k: f64,
    /// Bulk fluid temperature the film is priced against, °C.
    pub t_bulk_c: f64,
    /// Hottest solid voxel, °C — the number an enclosure design asks
    /// for.
    pub hotspot_temp_c: f64,
    /// Outer iterations to convergence.
    pub outer_iters: usize,
    /// Final wall-temperature change, °C.
    pub wall_delta_c: f64,
    /// Wetted (fluid↔solid) interface area, m².
    pub wetted_area_m2: f64,
}

/// Iterate flow ⇄ thermal to a wall-temperature fixed point.
///
/// Contract: both models describe the same box on the same grid; the
/// flow model's solid voxels are (a superset of) the thermal model's
/// material; the flow model has thermal transport enabled. The thermal
/// model's `exposed` boundary is overwritten each iteration with the
/// film state — configure everything else (sources, domain faces,
/// materials) as usual.
pub fn solve_conjugate(
    flow_model: &FlowModel,
    thermal_model: &ThermalModel,
    opts: &ConjugateOptions,
) -> Result<ConjugateResult, ConjugateError> {
    let transport = flow_model
        .thermal
        .as_ref()
        .ok_or(ConjugateError::NoThermalTransport)?;
    if flow_model.divisions != thermal_model.divisions {
        return Err(ConjugateError::GridMismatch {
            flow: flow_model.divisions,
            thermal: thermal_model.divisions,
        });
    }

    let (nx, ny, nz) = (
        flow_model.divisions[0],
        flow_model.divisions[1],
        flow_model.divisions[2],
    );
    let n = nx * ny * nz;
    let dx_m = flow_model.voxel_mm() / 1000.0;

    // Wetted interface: (fluid cell, solid neighbor) faces.
    let dirs: [(isize, isize, isize); 6] = [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ];
    let mut wetted_faces: Vec<usize> = Vec::new(); // solid voxel index per face
    let mut wetted_solids: Vec<usize> = Vec::new(); // unique solid voxels
    {
        let mut seen = vec![false; n];
        for k in 0..nz as isize {
            for j in 0..ny as isize {
                for i in 0..nx as isize {
                    let x = ((k * ny as isize + j) * nx as isize + i) as usize;
                    if flow_model.cells[x] != Cell::Fluid {
                        continue;
                    }
                    for (di, dj, dk) in dirs {
                        let (si, sj, sk) = (i + di, j + dj, k + dk);
                        if si < 0
                            || sj < 0
                            || sk < 0
                            || si >= nx as isize
                            || sj >= ny as isize
                            || sk >= nz as isize
                        {
                            continue;
                        }
                        let sx = ((sk * ny as isize + sj) * nx as isize + si) as usize;
                        if flow_model.cells[sx] == Cell::Solid {
                            wetted_faces.push(sx);
                            if !seen[sx] {
                                seen[sx] = true;
                                wetted_solids.push(sx);
                            }
                        }
                    }
                }
            }
        }
    }
    if wetted_faces.is_empty() {
        return Err(ConjugateError::NoWettedSurface);
    }
    let wetted_area = wetted_faces.len() as f64 * dx_m * dx_m;

    // Initial wall temperature: the transport's initial fluid temp.
    let mut wall_t = vec![transport.initial_temp_c; n];
    let mut delta = f64::INFINITY;

    let t_opts = thermal_solve::SolveOptions {
        tol: opts.thermal_tol,
        max_iters: opts.thermal_max_iters,
    };

    for outer in 1..=opts.max_outer {
        // Flow side: solve with the current wall temperatures painted
        // onto the wetted solids.
        let mut fm = flow_model.clone();
        let mut st = vec![f64::NAN; n];
        for &sx in &wetted_solids {
            st[sx] = wall_t[sx];
        }
        fm.solid_temp_c = Some(st);
        let flow_sol = solve_steady(&fm, &opts.flow).map_err(ConjugateError::Flow)?;

        // Price the film from what the scalar lattice actually moved.
        let q_wall = flow_sol
            .wall_heat_walls_only_w
            .ok_or(ConjugateError::DegenerateFilm)?;
        let temp_field = flow_sol
            .temperature_c
            .as_ref()
            .ok_or(ConjugateError::DegenerateFilm)?;
        let mut t_bulk = 0.0;
        let mut fluid_count = 0usize;
        for (cell, tf) in flow_model.cells.iter().zip(temp_field.iter()) {
            if *cell == Cell::Fluid {
                t_bulk += tf;
                fluid_count += 1;
            }
        }
        t_bulk /= fluid_count.max(1) as f64;
        let mut t_wall_mean = 0.0;
        for &sx in &wetted_faces {
            t_wall_mean += wall_t[sx];
        }
        t_wall_mean /= wetted_faces.len() as f64;
        let dt_film = t_wall_mean - t_bulk;
        // Bootstrap: on the first pass the walls sit at the bulk
        // temperature (nothing has heated yet), so the film cannot be
        // priced from the exchange. Seed it with the laminar
        // developed-duct correlation h = Nu·k/D_h (Nu = 3.66) — the
        // same closed form the cross-route check compares against.
        // From iteration 2 on, a degenerate film is a real error.
        let h = if dt_film.abs() < 1e-6 {
            if outer > 1 {
                return Err(ConjugateError::DegenerateFilm);
            }
            let k_fluid = transport.diffusivity_m2_s
                * flow_model.fluid.density_kg_m3
                * transport.heat_capacity_j_kg_k;
            let dh_m = flow_model
                .inlet_hydraulic_diameter_mm()
                .unwrap_or(flow_model.voxel_mm())
                / 1000.0;
            3.66 * k_fluid / dh_m
        } else {
            // Heat flows wall->fluid when the wall is hotter; h is
            // positive by construction or the film is degenerate.
            let h = q_wall / (wetted_area * dt_film);
            if !h.is_finite() || h <= 0.0 {
                return Err(ConjugateError::DegenerateFilm);
            }
            h
        };

        // Thermal side: conduct with the film as the exposed BC.
        let mut tm = thermal_model.clone();
        tm.exposed = Boundary::Convection {
            h_w_m2k: h,
            ambient_c: t_bulk,
        };
        let thermal_sol =
            thermal_solve::solve_steady(&tm, &t_opts).map_err(ConjugateError::Thermal)?;

        // Hand wall temperatures back and measure the fixed-point step.
        delta = 0.0f64;
        for &sx in &wetted_solids {
            let t_new = thermal_sol.t_c[sx];
            if t_new.is_finite() {
                delta = delta.max((t_new - wall_t[sx]).abs());
                wall_t[sx] = t_new;
            }
        }

        let hotspot = thermal_sol.t_max_c;
        if delta < opts.wall_tol_c && outer >= 2 {
            return Ok(ConjugateResult {
                flow: flow_sol,
                thermal: thermal_sol,
                film_h_w_m2k: h,
                t_bulk_c: t_bulk,
                hotspot_temp_c: hotspot,
                outer_iters: outer,
                wall_delta_c: delta,
                wetted_area_m2: wetted_area,
            });
        }
    }

    Err(ConjugateError::NotConverged {
        iters: opts.max_outer,
        delta_c: delta,
        tol_c: opts.wall_tol_c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fluid, ThermalTransport};
    use vcad_kernel_thermal::model::{MaterialRegion, PowerSource, Shape};

    /// A heated block cooled by duct flow: 40x9x12 mm at 1 mm voxels.
    /// Fluid duct (z in [5,12)) over a powered aluminum block
    /// (z in [0,5)). The conjugate answer must (a) converge, (b) close
    /// the energy loop — the fluid carries away the source power —
    /// and (c) price a film in the physically plausible laminar range.
    fn setup() -> (FlowModel, ThermalModel, f64) {
        let (nx, ny, nz) = (40usize, 9usize, 12usize);
        let mut fm = FlowModel::new([0.0; 3], [40.0, 9.0, 12.0], [nx, ny, nz]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = fm.index(i, j, k);
                    fm.cells[x] = if k < 5 {
                        Cell::Solid
                    } else if i == 0 {
                        Cell::Inlet
                    } else if i == nx - 1 {
                        Cell::Outlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        fm.fluid = Fluid::AIR_20C;
        fm.inlet_velocity_m_s = [0.09, 0.0, 0.0];
        fm.thermal = Some(ThermalTransport::AIR_20C);

        let power_w = 0.05;
        let mut tm = ThermalModel::new([0.0; 3], [40.0, 9.0, 12.0], [nx, ny, nz]);
        tm.materials.push(MaterialRegion {
            shape: Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [40.0, 9.0, 5.0],
            },
            k_w_mk: [180.0; 3],
            heat_capacity_j_m3k: None,
        });
        tm.sources.push(PowerSource {
            name: "chip".into(),
            shape: Shape::Box {
                min_mm: [16.0, 3.0, 0.0],
                size_mm: [8.0, 3.0, 2.0],
            },
            power_w,
        });
        tm.reference_c = Some(20.0);
        (fm, tm, power_w)
    }

    #[test]
    fn heated_block_conjugate_converges_and_closes_energy() {
        let (fm, tm, power_w) = setup();
        let opts = ConjugateOptions::default();
        let r = solve_conjugate(&fm, &tm, &opts).expect("conjugate solve");

        assert!(r.outer_iters >= 2 && r.outer_iters <= opts.max_outer);
        assert!(r.hotspot_temp_c > 20.5, "hotspot {:.2} C", r.hotspot_temp_c);

        // Energy loop: at the fixed point, the heat the fluid picks up
        // equals the source power (everything else is adiabatic).
        let pickup = r.flow.heat_pickup_w.expect("ported thermal run");
        let resid = (pickup - power_w).abs() / power_w;
        assert!(
            resid < 0.1,
            "energy loop: fluid picked up {pickup:.4} W of {power_w} W ({:.1}% off)",
            resid * 100.0
        );

        // Cross-route: laminar developed-duct correlation h = Nu*k/D_h,
        // Nu in the 3-8 range for this geometry -> h of order 10-40
        // W/m2K at these dimensions. The film-averaged h must land in
        // that physical decade.
        assert!(
            r.film_h_w_m2k > 3.0 && r.film_h_w_m2k < 100.0,
            "film h {:.1} W/m2K outside the plausible laminar range",
            r.film_h_w_m2k
        );
    }

    #[test]
    fn grid_mismatch_is_refused() {
        let (fm, mut tm, _) = setup();
        tm.divisions = [10, 9, 12];
        assert!(matches!(
            solve_conjugate(&fm, &tm, &ConjugateOptions::default()),
            Err(ConjugateError::GridMismatch { .. })
        ));
    }

    #[test]
    fn missing_transport_is_refused() {
        let (mut fm, tm, _) = setup();
        fm.thermal = None;
        assert!(matches!(
            solve_conjugate(&fm, &tm, &ConjugateOptions::default()),
            Err(ConjugateError::NoThermalTransport)
        ));
    }
}
