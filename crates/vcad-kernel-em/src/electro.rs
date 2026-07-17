//! Electrostatics on the shared grid core: capacitance extraction for
//! axisymmetric and planar geometries.
//!
//! Solves `∇·(ε ∇φ) = 0` with conductors as Dirichlet regions. The same
//! face-conductance machinery as the magnetostatic modules, with
//!
//! - axisymmetric weights `G ∝ 2π·ε·r` (the axis is a free column — the
//!   finite-volume half-cell weight `Δr²/8` reproduces the standard axis
//!   stencil automatically);
//! - planar weights `G ∝ ε` (results per meter of depth).
//!
//! Unfixed domain edges are natural Neumann (symmetry / no normal D);
//! ground planes and shields are explicit electrodes.
//!
//! Capacitance comes two independent ways and both are reported:
//! energy (`C = 2W/V²`) and induced charge (`C = Q/V`, with `Q` summed
//! from discrete D-fluxes through the conductor surface). Their gap is a
//! solve-quality diagnostic in the same spirit as the magnetostatic
//! energy-balance residual.

use crate::constants::EPS_0;
use crate::grid::{FvSystem, Grid2D, SolveError, SolveOptions};

/// A conductor or dielectric footprint in the solve plane, mm.
///
/// In axisymmetric geometry `x` is the radius and shapes revolve about
/// the axis: a `Circle` centered on `x = 0` is a sphere, a `CircleShell`
/// is a spherical shell, a `Rect` is a cylinder/annulus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Axis-aligned rectangle.
    Rect {
        /// Left edge (inner radius when axisymmetric), mm.
        x_min_mm: f64,
        /// Right edge, mm.
        x_max_mm: f64,
        /// Bottom edge, mm.
        y_min_mm: f64,
        /// Top edge, mm.
        y_max_mm: f64,
    },
    /// Filled circle (a ball when revolved).
    Circle {
        /// Center x, mm (0 for a sphere on the axis).
        cx_mm: f64,
        /// Center y, mm.
        cy_mm: f64,
        /// Radius, mm.
        radius_mm: f64,
    },
    /// Annular shell between two radii (a spherical shell when revolved).
    CircleShell {
        /// Center x, mm.
        cx_mm: f64,
        /// Center y, mm.
        cy_mm: f64,
        /// Inner radius, mm.
        r_inner_mm: f64,
        /// Outer radius, mm.
        r_outer_mm: f64,
    },
}

impl Shape {
    /// Whether `(x_m, y_m)` (SI meters) lies inside.
    pub fn contains_m(&self, x_m: f64, y_m: f64) -> bool {
        let x = x_m * 1e3;
        let y = y_m * 1e3;
        match *self {
            Shape::Rect {
                x_min_mm,
                x_max_mm,
                y_min_mm,
                y_max_mm,
            } => x >= x_min_mm && x <= x_max_mm && y >= y_min_mm && y <= y_max_mm,
            Shape::Circle {
                cx_mm,
                cy_mm,
                radius_mm,
            } => (x - cx_mm).powi(2) + (y - cy_mm).powi(2) <= radius_mm * radius_mm,
            Shape::CircleShell {
                cx_mm,
                cy_mm,
                r_inner_mm,
                r_outer_mm,
            } => {
                let d2 = (x - cx_mm).powi(2) + (y - cy_mm).powi(2);
                d2 >= r_inner_mm * r_inner_mm && d2 <= r_outer_mm * r_outer_mm
            }
        }
    }
}

/// A conductor held at a fixed potential.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Electrode {
    /// Conductor footprint (nodes inside are Dirichlet; curved shapes are
    /// staircased at grid resolution — refine and watch convergence).
    pub shape: Shape,
    /// Potential, volts.
    pub potential_v: f64,
}

/// A linear dielectric region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dielectric {
    /// Region it occupies (later entries win; background is vacuum).
    pub shape: Shape,
    /// Relative permittivity.
    pub eps_r: f64,
}

/// Solve-plane geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    /// (r, z) half-plane revolved about the axis; absolute farads.
    Axisymmetric,
    /// (x, y) cross-section; farads per meter of depth.
    Planar,
}

/// An electrostatic device.
#[derive(Debug, Clone, PartialEq)]
pub struct Electrostatics {
    /// Solve-plane geometry.
    pub geometry: Geometry,
    /// Domain left edge, mm (must be 0 for axisymmetric).
    pub x_min_mm: f64,
    /// Domain right edge, mm.
    pub x_max_mm: f64,
    /// Domain bottom edge, mm.
    pub y_min_mm: f64,
    /// Domain top edge, mm.
    pub y_max_mm: f64,
    /// Conductors (later entries win where footprints overlap).
    pub electrodes: Vec<Electrode>,
    /// Dielectric regions.
    pub dielectrics: Vec<Dielectric>,
}

impl Electrostatics {
    /// An empty device on the given domain.
    pub fn new(
        geometry: Geometry,
        x_min_mm: f64,
        x_max_mm: f64,
        y_min_mm: f64,
        y_max_mm: f64,
    ) -> Self {
        assert!(
            geometry != Geometry::Axisymmetric || x_min_mm == 0.0,
            "axisymmetric domains start at the axis (x_min_mm = 0)"
        );
        Self {
            geometry,
            x_min_mm,
            x_max_mm,
            y_min_mm,
            y_max_mm,
            electrodes: Vec::new(),
            dielectrics: Vec::new(),
        }
    }

    fn eps_at(&self, x_m: f64, y_m: f64) -> f64 {
        let mut e = EPS_0;
        for d in &self.dielectrics {
            if d.shape.contains_m(x_m, y_m) {
                e = EPS_0 * d.eps_r;
            }
        }
        e
    }

    /// Solve on an `nx × ny` node grid.
    pub fn solve(
        &self,
        nx: usize,
        ny: usize,
        opts: &SolveOptions,
    ) -> Result<ElectroSolution, SolveError> {
        if nx < 3 || ny < 3 {
            return Err(SolveError::GridTooSmall);
        }
        let x_min = self.x_min_mm * 1e-3;
        let x_max = self.x_max_mm * 1e-3;
        let y_min = self.y_min_mm * 1e-3;
        let y_max = self.y_max_mm * 1e-3;
        let dx = (x_max - x_min) / (nx - 1) as f64;
        let dy = (y_max - y_min) / (ny - 1) as f64;
        let axi = self.geometry == Geometry::Axisymmetric;
        let grid = Grid2D {
            nx,
            ny,
            dx,
            dy,
            x0: x_min,
            y0: y_min,
            periodic_x: false,
        };
        let mut sys = FvSystem::new(grid);
        let g = sys.grid.clone();
        let two_pi = 2.0 * std::f64::consts::PI;

        // Dielectrics live on CELLS, sampled at cell centers (sample
        // points can never land on a region boundary), and every face
        // conductance is the parallel sum of its two flanking half-cells
        // — for axisymmetric geometry with the exact ∫x dx measure of
        // each half (the axis column's Δx²/8 falls out automatically).
        let eps_cell = |ci: usize, cj: usize| -> f64 {
            self.eps_at(
                x_min + (ci as f64 + 0.5) * dx,
                y_min + (cj as f64 + 0.5) * dy,
            )
        };
        for i in 0..nx - 1 {
            let x_f = x_min + (i as f64 + 0.5) * dx;
            let geom = if axi { two_pi * x_f } else { 1.0 };
            for j in 0..ny {
                let mut eps_ext = 0.0;
                if j > 0 {
                    eps_ext += eps_cell(i, j - 1) * 0.5 * dy;
                }
                if j < ny - 1 {
                    eps_ext += eps_cell(i, j) * 0.5 * dy;
                }
                sys.gx[g.fx(i, j)] = geom * eps_ext / dx;
            }
        }
        for i in 0..nx {
            let x_i = g.x(i);
            let measure_lo = if axi {
                0.5 * (x_i * x_i - (x_i - 0.5 * dx).powi(2))
            } else {
                0.5 * dx
            };
            let measure_hi = if axi {
                0.5 * ((x_i + 0.5 * dx).powi(2) - x_i * x_i)
            } else {
                0.5 * dx
            };
            for j in 0..ny - 1 {
                let mut eps_measure = 0.0;
                if i > 0 {
                    eps_measure += eps_cell(i - 1, j) * measure_lo;
                }
                if i < nx - 1 {
                    eps_measure += eps_cell(i, j) * measure_hi;
                }
                sys.gy[g.fy(i, j)] = if axi {
                    two_pi * eps_measure / dy
                } else {
                    eps_measure / dy
                };
            }
        }

        // Electrode masks; later electrodes win on overlap.
        let mut owner: Vec<i64> = vec![-1; nx * ny];
        for (k, e) in self.electrodes.iter().enumerate() {
            for i in 0..nx {
                for j in 0..ny {
                    if e.shape.contains_m(g.x(i), g.y(j)) {
                        let id = g.idx(i, j);
                        owner[id] = k as i64;
                        sys.fixed[id] = true;
                        sys.u0[id] = e.potential_v;
                    }
                }
            }
        }

        let sol = sys.solve(opts)?;
        Ok(ElectroSolution {
            potentials: self.electrodes.iter().map(|e| e.potential_v).collect(),
            owner,
            phi: sol.u,
            sweeps: sol.sweeps,
            residual: sol.residual,
            system: sys,
        })
    }
}

/// A converged electrostatic field.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectroSolution {
    /// The assembled system.
    pub system: FvSystem,
    /// Potential per node, volts.
    pub phi: Vec<f64>,
    /// SOR sweeps used.
    pub sweeps: usize,
    /// Final relative residual.
    pub residual: f64,
    /// Electrode index owning each node (−1 = free space).
    pub owner: Vec<i64>,
    /// Electrode potentials, volts.
    pub potentials: Vec<f64>,
}

impl ElectroSolution {
    /// Potential at `(x, y)` in **meters**, volts (bilinear).
    pub fn phi_at(&self, x_m: f64, y_m: f64) -> f64 {
        self.system.grid.value_at(&self.phi, x_m, y_m)
    }

    /// Electric field `(E_x, E_y)` at `(x, y)` in **meters**, V/m — the
    /// exact negative gradient of the bilinear patch (conservative).
    pub fn e_at(&self, x_m: f64, y_m: f64) -> (f64, f64) {
        let (gx, gy) = self.system.grid.grad_at(&self.phi, x_m, y_m);
        (-gx, -gy)
    }

    /// Field energy `½·Σ_f G·Δφ²` — joules (axisymmetric) or J/m (planar).
    pub fn energy(&self) -> f64 {
        self.system.field_energy(&self.phi)
    }

    /// Induced charge on electrode `k`, from the discrete D-flux out of
    /// its surface: `Q_k = Σ_{boundary faces} G·(V_k − φ_neighbor)` —
    /// coulombs (axisymmetric) or C/m (planar).
    pub fn charge(&self, k: usize) -> f64 {
        let g = &self.system.grid;
        let kk = k as i64;
        let mut q = 0.0;
        // Every face with exactly one endpoint owned by electrode k.
        for i in 0..g.nx - 1 {
            for j in 0..g.ny {
                let a = g.idx(i, j);
                let b = g.idx(i + 1, j);
                let f = self.system.gx[g.fx(i, j)];
                if self.owner[a] == kk && self.owner[b] != kk {
                    q += f * (self.phi[a] - self.phi[b]);
                } else if self.owner[b] == kk && self.owner[a] != kk {
                    q += f * (self.phi[b] - self.phi[a]);
                }
            }
        }
        for i in 0..g.nx {
            for j in 0..g.ny - 1 {
                let a = g.idx(i, j);
                let b = g.idx(i, j + 1);
                let f = self.system.gy[g.fy(i, j)];
                if self.owner[a] == kk && self.owner[b] != kk {
                    q += f * (self.phi[a] - self.phi[b]);
                } else if self.owner[b] == kk && self.owner[a] != kk {
                    q += f * (self.phi[b] - self.phi[a]);
                }
            }
        }
        q
    }

    /// Two-terminal capacitance, both routes, for a solve where electrode
    /// `hot` is the only one at nonzero potential (everything else 0 V).
    pub fn capacitance_two_terminal(&self, hot: usize) -> Capacitance {
        let v = self.potentials[hot];
        assert!(v != 0.0, "hot electrode must be at nonzero potential");
        Capacitance {
            from_energy: 2.0 * self.energy() / (v * v),
            from_charge: self.charge(hot) / v,
        }
    }
}

/// Capacitance extracted two independent ways. Their relative gap is a
/// discretization/solve-quality diagnostic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capacitance {
    /// `C = 2W/V²` from the field energy.
    pub from_energy: f64,
    /// `C = Q/V` from the induced-charge flux sum.
    pub from_charge: f64,
}

impl Capacitance {
    /// `|energy − charge| / max(...)` — the cross-route mismatch.
    pub fn mismatch(&self) -> f64 {
        let d = (self.from_energy - self.from_charge).abs();
        let m = self.from_energy.abs().max(self.from_charge.abs());
        if m > 0.0 {
            d / m
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytic;

    #[test]
    fn coax_capacitance_matches_the_log_formula() {
        // Full-height solid rod (r ≤ 10 mm) at 1 V inside a grounded
        // shell (r ≥ 30 mm); Neumann z-ends make it a pure radial problem
        // — the textbook coax per unit length.
        let mut dev = Electrostatics::new(Geometry::Axisymmetric, 0.0, 40.0, 0.0, 20.0);
        dev.electrodes.push(Electrode {
            shape: Shape::Rect {
                x_min_mm: 0.0,
                x_max_mm: 10.0,
                y_min_mm: -1.0,
                y_max_mm: 21.0,
            },
            potential_v: 1.0,
        });
        dev.electrodes.push(Electrode {
            shape: Shape::Rect {
                x_min_mm: 30.0,
                x_max_mm: 41.0,
                y_min_mm: -1.0,
                y_max_mm: 21.0,
            },
            potential_v: 0.0,
        });
        let sol = dev.solve(81, 9, &SolveOptions::default()).unwrap();
        let cap = sol.capacitance_two_terminal(0);
        let expect = analytic::coax_capacitance_per_m(0.010, 0.030, 1.0) * 0.020;
        for (label, c) in [("energy", cap.from_energy), ("charge", cap.from_charge)] {
            let rel = (c - expect).abs() / expect;
            assert!(
                rel < 2e-3,
                "coax C ({label}) = {c:.6e} vs {expect:.6e} (rel {rel:.2e})"
            );
        }
        assert!(
            cap.mismatch() < 1e-6,
            "route mismatch {:.2e}",
            cap.mismatch()
        );
        // Gauss: the grounded shell carries the opposite charge.
        let q_ratio = sol.charge(1) / sol.charge(0);
        assert!(
            (q_ratio + 1.0).abs() < 1e-6,
            "charge balance broken: {q_ratio}"
        );
    }

    #[test]
    fn concentric_spheres_capacitance_converges() {
        // Ball a = 10.2 mm at 1 V inside a grounded shell b = 25 mm:
        // C = 4πε₀·ab/(b−a). Curved conductors staircase (an O(h)
        // surface-radius bias, measured in the probe study), so this is a
        // convergence assertion, not an exact one; the radii land mid-cell
        // at the fine grid to symmetrize the staircase.
        let build = |n: usize| {
            let mut dev = Electrostatics::new(Geometry::Axisymmetric, 0.0, 40.0, -40.0, 40.0);
            dev.electrodes.push(Electrode {
                shape: Shape::Circle {
                    cx_mm: 0.0,
                    cy_mm: 0.0,
                    radius_mm: 10.2,
                },
                potential_v: 1.0,
            });
            dev.electrodes.push(Electrode {
                shape: Shape::CircleShell {
                    cx_mm: 0.0,
                    cy_mm: 0.0,
                    r_inner_mm: 25.0,
                    r_outer_mm: 29.0,
                },
                potential_v: 0.0,
            });
            let sol = dev.solve(n, 2 * n - 1, &SolveOptions::default()).unwrap();
            sol.capacitance_two_terminal(0)
        };
        let expect = analytic::concentric_spheres_capacitance(0.0102, 0.025);
        let coarse = build(41);
        let fine = build(101);
        let e_coarse = (coarse.from_charge - expect).abs() / expect;
        let e_fine = (fine.from_charge - expect).abs() / expect;
        assert!(
            e_fine < 0.03,
            "spheres C = {:.4e} vs {expect:.4e} (rel {e_fine:.2e})",
            fine.from_charge
        );
        assert!(
            e_fine < e_coarse + 1e-4,
            "refinement must not worsen: {e_coarse:.3e} → {e_fine:.3e}"
        );
        assert!(fine.mismatch() < 1e-5);
    }

    #[test]
    fn parallel_plates_with_dielectric_layers_are_exact() {
        // Full-width plates, two dielectric layers in series with the
        // interface on a node row: the discrete answer is the exact
        // series formula C′ = ε₀·w / (d₁/ε₁ + d₂/ε₂).
        let mut dev = Electrostatics::new(Geometry::Planar, 0.0, 20.0, 0.0, 12.0);
        dev.electrodes.push(Electrode {
            shape: Shape::Rect {
                x_min_mm: -1.0,
                x_max_mm: 21.0,
                y_min_mm: -1.0,
                y_max_mm: 1.0,
            },
            potential_v: 2.0,
        });
        dev.electrodes.push(Electrode {
            shape: Shape::Rect {
                x_min_mm: -1.0,
                x_max_mm: 21.0,
                y_min_mm: 11.0,
                y_max_mm: 13.0,
            },
            potential_v: 0.0,
        });
        // Layers: ε_r = 4 for y ∈ [1, 6] (5 mm), ε_r = 2 for y ∈ [6, 11].
        dev.dielectrics.push(Dielectric {
            shape: Shape::Rect {
                x_min_mm: -1.0,
                x_max_mm: 21.0,
                y_min_mm: 1.0,
                y_max_mm: 6.0,
            },
            eps_r: 4.0,
        });
        dev.dielectrics.push(Dielectric {
            shape: Shape::Rect {
                x_min_mm: -1.0,
                x_max_mm: 21.0,
                y_min_mm: 6.0,
                y_max_mm: 11.0,
            },
            eps_r: 2.0,
        });
        let sol = dev.solve(21, 13, &SolveOptions::default()).unwrap();
        let cap = sol.capacitance_two_terminal(0);
        let expect = crate::constants::EPS_0 * 0.020 / (0.005 / 4.0 + 0.005 / 2.0);
        let rel = (cap.from_charge - expect).abs() / expect;
        assert!(
            rel < 1e-6,
            "series C′ = {:.6e} vs exact {expect:.6e} (rel {rel:.2e})",
            cap.from_charge
        );
        assert!(cap.mismatch() < 1e-6);
    }

    #[test]
    fn respects_the_maximum_principle() {
        let mut dev = Electrostatics::new(Geometry::Planar, 0.0, 30.0, 0.0, 30.0);
        dev.electrodes.push(Electrode {
            shape: Shape::Circle {
                cx_mm: 10.0,
                cy_mm: 15.0,
                radius_mm: 4.0,
            },
            potential_v: 5.0,
        });
        dev.electrodes.push(Electrode {
            shape: Shape::Circle {
                cx_mm: 22.0,
                cy_mm: 15.0,
                radius_mm: 4.0,
            },
            potential_v: -3.0,
        });
        let sol = dev.solve(61, 61, &SolveOptions::default()).unwrap();
        for &p in &sol.phi {
            assert!((-3.0..=5.0).contains(&p), "potential out of bounds: {p}");
        }
    }
}
