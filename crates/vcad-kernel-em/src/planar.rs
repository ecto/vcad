//! Planar (2D translational) magnetostatics: the motor cross-section
//! formulation.
//!
//! Solves for `A_z(x, y)` (per meter of depth):
//!
//! ```text
//!   ∇·( ν ∇A_z ) = −J_z,       ν = 1/(μ₀·μ_r)
//!   B = ∇×(A_z ẑ) = ( ∂A/∂y, −∂A/∂x )
//! ```
//!
//! Sources are rectangular conductors (uniform `J_z`) and **permanent
//! magnets** as equivalent bound-current sheets: a block with remanence
//! `B_r` and recoil permeability `μ_r` deposits `K = B_r/(μ₀·μ_r)` on the
//! two edges parallel to its magnetization (`K_b = M × n̂`), and carries
//! its recoil `μ_r` as a material region. Rotating machines unroll into a
//! **periodic-in-x** strip (set [`PlanarMagnetostatics::periodic_x`]); a
//! radial-flux cross-section can instead integrate Maxwell stress on a
//! circle in the air gap.
//!
//! Force and torque come two independent ways:
//! - volume `J×B` on an element's own deposited currents
//!   (`F = Σ s_n·∇A|_n` — the force density of a `J_z` filament is
//!   `J·∇A`), magnets included via their bound sheets (rigid, linear
//!   recoil);
//! - the Maxwell stress tensor on a closed contour: a full-period line
//!   for unrolled machines, a circle for rotating cross-sections.
//!
//! All extensive outputs (energy, force, torque) are **per meter of
//! depth**; multiply by the stack length. Not modeled at M0: curvature of
//! the unrolled annulus, radial end effects, saturation, eddy currents.

use crate::axisym::{PicardOptions, PicardReport};
use crate::constants::MU_0;
use crate::grid::{Bc, EnergyBalance, FvSystem, Grid2D, SolveError, SolveOptions};
use crate::material::{b_of_h, Saturation};

/// Axis-aligned rectangle in the (x, y) plane, mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge, mm.
    pub x_min_mm: f64,
    /// Right edge, mm.
    pub x_max_mm: f64,
    /// Bottom edge, mm.
    pub y_min_mm: f64,
    /// Top edge, mm.
    pub y_max_mm: f64,
}

impl Rect {
    /// Area, m².
    pub fn area_m2(&self) -> f64 {
        (self.x_max_mm - self.x_min_mm) * (self.y_max_mm - self.y_min_mm) * 1e-6
    }

    /// Whether `(x_m, y_m)` (SI meters) lies inside.
    pub fn contains_m(&self, x_m: f64, y_m: f64) -> bool {
        let x = x_m * 1e3;
        let y = y_m * 1e3;
        x >= self.x_min_mm && x <= self.x_max_mm && y >= self.y_min_mm && y <= self.y_max_mm
    }
}

/// A straight conductor region carrying uniform `J_z`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conductor {
    /// Cross-section.
    pub region: Rect,
    /// Total current through the region (turns × amps), A. Positive = +z
    /// (out of the plane).
    pub total_current_a: f64,
}

/// A rectangular permanent-magnet block, uniformly magnetized in-plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagnetBlock {
    /// The block.
    pub region: Rect,
    /// Remanence x-component, tesla.
    pub br_x_t: f64,
    /// Remanence y-component, tesla.
    pub br_y_t: f64,
    /// Recoil relative permeability (NdFeB/ferrite ≈ 1.05).
    pub mu_r: f64,
}

/// A magnetic material region: linear μ_r, optionally saturating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarMaterial {
    /// Region it occupies.
    pub region: Rect,
    /// Relative permeability — constant when `sat` is `None`, initial
    /// (small-signal) permeability when saturating.
    pub mu_r: f64,
    /// Saturation law (M1). `None` = linear; magnets stay linear recoil.
    pub sat: Option<Saturation>,
}

impl PlanarMaterial {
    /// A linear material.
    pub fn linear(region: Rect, mu_r: f64) -> Self {
        Self {
            region,
            mu_r,
            sat: None,
        }
    }

    /// A saturating material with initial permeability `mu_ri` and
    /// saturation polarization `js_t` (tesla).
    pub fn saturable(region: Rect, mu_ri: f64, js_t: f64) -> Self {
        Self {
            region,
            mu_r: mu_ri,
            sat: Some(Saturation { js_t }),
        }
    }
}

/// An annular (ring) linear material region — curved interfaces
/// staircase at cell resolution, so results must be bracketed by a
/// refinement study (see `examples/convergence.rs`). Linear only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingMaterial {
    /// Center x, mm.
    pub cx_mm: f64,
    /// Center y, mm.
    pub cy_mm: f64,
    /// Inner radius, mm.
    pub r_inner_mm: f64,
    /// Outer radius, mm.
    pub r_outer_mm: f64,
    /// Relative permeability.
    pub mu_r: f64,
}

impl RingMaterial {
    fn contains_m(&self, x_m: f64, y_m: f64) -> bool {
        let dx = x_m * 1e3 - self.cx_mm;
        let dy = y_m * 1e3 - self.cy_mm;
        let d2 = dx * dx + dy * dy;
        d2 >= self.r_inner_mm * self.r_inner_mm && d2 <= self.r_outer_mm * self.r_outer_mm
    }
}

/// A planar magnetostatic device.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarMagnetostatics {
    /// Domain left edge, mm.
    pub x_min_mm: f64,
    /// Domain right edge, mm.
    pub x_max_mm: f64,
    /// Domain bottom edge, mm.
    pub y_min_mm: f64,
    /// Domain top edge, mm.
    pub y_max_mm: f64,
    /// Conductors.
    pub conductors: Vec<Conductor>,
    /// Permanent magnets.
    pub magnets: Vec<MagnetBlock>,
    /// Material regions (later entries win; magnets' recoil μ is applied
    /// after these; background is vacuum).
    pub materials: Vec<PlanarMaterial>,
    /// Annular material regions (applied after `materials`, before
    /// magnet recoil; linear only, staircased).
    pub rings: Vec<RingMaterial>,
    /// Wrap x: column `nx−1` neighbors column `0`, the unrolled-machine
    /// topology. The wrap pitch is the full `[x_min, x_max]` span; sources
    /// must not straddle the seam (split them at the seam instead), and
    /// `x`-side boundary conditions are ignored.
    pub periodic_x: bool,
    /// Boundary condition on the left edge (non-periodic only).
    pub bc_x_low: Bc,
    /// Boundary condition on the right edge (non-periodic only).
    pub bc_x_high: Bc,
    /// Boundary condition on the bottom edge.
    pub bc_y_low: Bc,
    /// Boundary condition on the top edge.
    pub bc_y_high: Bc,
}

impl PlanarMagnetostatics {
    /// An empty vacuum device on the given domain, `A = 0` everywhere on
    /// the boundary, non-periodic.
    pub fn new(x_min_mm: f64, x_max_mm: f64, y_min_mm: f64, y_max_mm: f64) -> Self {
        Self {
            x_min_mm,
            x_max_mm,
            y_min_mm,
            y_max_mm,
            conductors: Vec::new(),
            magnets: Vec::new(),
            materials: Vec::new(),
            rings: Vec::new(),
            periodic_x: false,
            bc_x_low: Bc::Zero,
            bc_x_high: Bc::Zero,
            bc_y_low: Bc::Zero,
            bc_y_high: Bc::Zero,
        }
    }

    fn mu_r_at(&self, x_m: f64, y_m: f64) -> f64 {
        let mut mu = 1.0;
        for m in &self.materials {
            if m.region.contains_m(x_m, y_m) {
                mu = m.mu_r;
            }
        }
        for r in &self.rings {
            if r.contains_m(x_m, y_m) {
                mu = r.mu_r;
            }
        }
        for m in &self.magnets {
            if m.region.contains_m(x_m, y_m) {
                mu = m.mu_r;
            }
        }
        mu
    }

    fn cell_geometry(&self, nx: usize, ny: usize) -> (usize, usize, f64, f64) {
        let dx = if self.periodic_x {
            (self.x_max_mm - self.x_min_mm) * 1e-3 / nx as f64
        } else {
            (self.x_max_mm - self.x_min_mm) * 1e-3 / (nx - 1) as f64
        };
        let dy = (self.y_max_mm - self.y_min_mm) * 1e-3 / (ny - 1) as f64;
        let x_cells = if self.periodic_x { nx } else { nx - 1 };
        (x_cells, ny - 1, dx, dy)
    }

    /// Initial (linear / small-signal) reluctivity per cell, row major
    /// `ci·(ny−1) + cj`.
    pub(crate) fn initial_nu_cells(&self, nx: usize, ny: usize) -> Vec<f64> {
        let (x_cells, y_cells, dx, dy) = self.cell_geometry(nx, ny);
        let (x_min, y_min) = (self.x_min_mm * 1e-3, self.y_min_mm * 1e-3);
        let mut nu = vec![0.0; x_cells * y_cells];
        for ci in 0..x_cells {
            for cj in 0..y_cells {
                let xc = x_min + (ci as f64 + 0.5) * dx;
                let yc = y_min + (cj as f64 + 0.5) * dy;
                nu[ci * y_cells + cj] = 1.0 / (MU_0 * self.mu_r_at(xc, yc));
            }
        }
        nu
    }

    /// Which material region (index into `materials`, last wins) owns
    /// each cell; `None` = vacuum background or a magnet's recoil region
    /// (recoil μ is the magnet's, not a material parameter).
    pub(crate) fn material_cell_map(&self, nx: usize, ny: usize) -> Vec<Option<usize>> {
        let (x_cells, y_cells, dx, dy) = self.cell_geometry(nx, ny);
        let (x_min, y_min) = (self.x_min_mm * 1e-3, self.y_min_mm * 1e-3);
        let mut cells = vec![None; x_cells * y_cells];
        for ci in 0..x_cells {
            for cj in 0..y_cells {
                let xc = x_min + (ci as f64 + 0.5) * dx;
                let yc = y_min + (cj as f64 + 0.5) * dy;
                let mut hit = None;
                for (mi, m) in self.materials.iter().enumerate() {
                    if m.region.contains_m(xc, yc) {
                        hit = Some(mi);
                    }
                }
                for m in &self.magnets {
                    if m.region.contains_m(xc, yc) {
                        hit = None;
                    }
                }
                cells[ci * y_cells + cj] = hit;
            }
        }
        cells
    }

    /// Saturation law per cell (`None` = linear; magnets never saturate
    /// here — recoil is linear).
    fn sat_cells(&self, nx: usize, ny: usize) -> Vec<Option<(f64, Saturation)>> {
        let (x_cells, y_cells, dx, dy) = self.cell_geometry(nx, ny);
        let (x_min, y_min) = (self.x_min_mm * 1e-3, self.y_min_mm * 1e-3);
        let mut cells = vec![None; x_cells * y_cells];
        for ci in 0..x_cells {
            for cj in 0..y_cells {
                let xc = x_min + (ci as f64 + 0.5) * dx;
                let yc = y_min + (cj as f64 + 0.5) * dy;
                let mut hit = None;
                for m in &self.materials {
                    if m.region.contains_m(xc, yc) {
                        hit = m.sat.map(|s| (m.mu_r, s));
                    }
                }
                for m in &self.magnets {
                    if m.region.contains_m(xc, yc) {
                        hit = None;
                    }
                }
                cells[ci * y_cells + cj] = hit;
            }
        }
        cells
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn build_system(
        &self,
        nx: usize,
        ny: usize,
        nu_cells: &[f64],
    ) -> Result<(FvSystem, Vec<Vec<f64>>, Vec<Vec<f64>>), SolveError> {
        if nx < 3 || ny < 3 {
            return Err(SolveError::GridTooSmall);
        }
        let x_min = self.x_min_mm * 1e-3;
        let x_max = self.x_max_mm * 1e-3;
        let y_min = self.y_min_mm * 1e-3;
        let y_max = self.y_max_mm * 1e-3;
        // Periodic: node nx−1 sits one pitch short of x_max (which is the
        // same physical line as x_min).
        let dx = if self.periodic_x {
            (x_max - x_min) / nx as f64
        } else {
            (x_max - x_min) / (nx - 1) as f64
        };
        let dy = (y_max - y_min) / (ny - 1) as f64;
        let grid = Grid2D {
            nx,
            ny,
            dx,
            dy,
            x0: x_min,
            y0: y_min,
            periodic_x: self.periodic_x,
        };
        let mut sys = FvSystem::new(grid);
        let g = sys.grid.clone();

        // Materials live on CELLS, sampled at cell centers — sample
        // points can never land on a region boundary, so material edges
        // on node lines resolve deterministically (a point-on-boundary
        // float tie once broke mirror symmetry here). Each face
        // conductance is the parallel sum of its two flanking half-cells.
        let x_cells = if self.periodic_x { nx } else { nx - 1 };
        let y_cells = ny - 1;
        let nu_cell = |ci: usize, cj: usize| -> f64 { nu_cells[ci * y_cells + cj] };
        // Per-cell incidence weights recorded alongside G, so the
        // adjoint differentiates the same assembly the solver uses.
        let mut wx = vec![[(crate::grid::NO_CELL, 0.0); 2]; sys.gx.len()];
        let mut wy = vec![[(crate::grid::NO_CELL, 0.0); 2]; sys.gy.len()];
        for i in 0..x_cells {
            for j in 0..ny {
                // Flux tube of the x-face at row j: lower half of cell
                // (i, j−1) ∥ upper half of cell (i, j).
                let coeff = 0.5 * dy / dx;
                let mut acc = 0.0;
                let w = &mut wx[g.fx(i, j)];
                if j > 0 {
                    acc += nu_cell(i, j - 1) * coeff;
                    w[0] = (i * y_cells + (j - 1), coeff);
                }
                if j < y_cells {
                    acc += nu_cell(i, j) * coeff;
                    w[1] = (i * y_cells + j, coeff);
                }
                sys.gx[g.fx(i, j)] = acc;
            }
        }
        for i in 0..nx {
            for j in 0..y_cells {
                // Flux tube of the y-face at column i: right half of cell
                // (i−1, j) ∥ left half of cell (i, j), wrapping when
                // periodic.
                let coeff = 0.5 * dx / dy;
                let mut acc = 0.0;
                let w = &mut wy[g.fy(i, j)];
                if self.periodic_x {
                    let left = (i + x_cells - 1) % x_cells;
                    let right = i % x_cells;
                    acc += (nu_cell(left, j) + nu_cell(right, j)) * coeff;
                    w[0] = (left * y_cells + j, coeff);
                    w[1] = (right * y_cells + j, coeff);
                } else {
                    if i > 0 {
                        acc += nu_cell(i - 1, j) * coeff;
                        w[0] = ((i - 1) * y_cells + j, coeff);
                    }
                    if i < x_cells {
                        acc += nu_cell(i, j) * coeff;
                        w[1] = (i * y_cells + j, coeff);
                    }
                }
                sys.gy[g.fy(i, j)] = acc;
            }
        }
        sys.face_weights = Some(crate::grid::FaceWeights { x: wx, y: wy });

        if !self.periodic_x {
            if self.bc_x_low == Bc::Zero {
                for j in 0..ny {
                    sys.fixed[g.idx(0, j)] = true;
                }
            }
            if self.bc_x_high == Bc::Zero {
                for j in 0..ny {
                    sys.fixed[g.idx(nx - 1, j)] = true;
                }
            }
        }
        if self.bc_y_low == Bc::Zero {
            for i in 0..nx {
                sys.fixed[g.idx(i, 0)] = true;
            }
        }
        if self.bc_y_high == Bc::Zero {
            for i in 0..nx {
                sys.fixed[g.idx(i, ny - 1)] = true;
            }
        }

        // Node control volumes (x wraps when periodic, so CVs never clip
        // in x; they do clip at y boundaries).
        let cv_x = |i: usize| -> (f64, f64) {
            let c = g.x(i);
            if self.periodic_x {
                (c - 0.5 * dx, c + 0.5 * dx)
            } else {
                ((c - 0.5 * dx).max(x_min), (c + 0.5 * dx).min(x_max))
            }
        };
        let cv_y = |j: usize| -> (f64, f64) {
            let c = g.y(j);
            ((c - 0.5 * dy).max(y_min), (c + 0.5 * dy).min(y_max))
        };

        // Conductor deposits: per-unit-current source vectors.
        let mut unit_sources: Vec<Vec<f64>> = Vec::with_capacity(self.conductors.len());
        for cond in &self.conductors {
            let mut unit = vec![0.0; nx * ny];
            let area = cond.region.area_m2();
            if area > 0.0 {
                let (c_xl, c_xh) = (cond.region.x_min_mm * 1e-3, cond.region.x_max_mm * 1e-3);
                let (c_yl, c_yh) = (cond.region.y_min_mm * 1e-3, cond.region.y_max_mm * 1e-3);
                for i in 0..nx {
                    let (xl, xh) = cv_x(i);
                    let wx = overlap(xl, xh, c_xl, c_xh);
                    if wx == 0.0 {
                        continue;
                    }
                    for j in 0..ny {
                        let (yl, yh) = cv_y(j);
                        let wy = overlap(yl, yh, c_yl, c_yh);
                        if wy > 0.0 {
                            unit[g.idx(i, j)] = wx * wy / area;
                        }
                    }
                }
            }
            for (s, u) in sys.source.iter_mut().zip(unit.iter()) {
                *s += cond.total_current_a * u;
            }
            unit_sources.push(unit);
        }

        // Magnet deposits: bound sheets K = Br/(μ₀·μ_r) on the edges
        // parallel to the magnetization (K_b = M × n̂).
        let mut magnet_sources: Vec<Vec<f64>> = Vec::with_capacity(self.magnets.len());
        for mag in &self.magnets {
            let mut dep = vec![0.0; nx * ny];
            let (m_xl, m_xh) = (mag.region.x_min_mm * 1e-3, mag.region.x_max_mm * 1e-3);
            let (m_yl, m_yh) = (mag.region.y_min_mm * 1e-3, mag.region.y_max_mm * 1e-3);
            // y-magnetization: +K sheet on the left edge, −K on the right.
            let ky = mag.br_y_t / (MU_0 * mag.mu_r);
            if ky != 0.0 {
                for (x_e, sign) in [(m_xl, 1.0), (m_xh, -1.0)] {
                    for i in 0..nx {
                        let (xl, xh) = cv_x(i);
                        if !(xl <= x_e && x_e < xh) {
                            continue;
                        }
                        for j in 0..ny {
                            let (yl, yh) = cv_y(j);
                            let wy = overlap(yl, yh, m_yl, m_yh);
                            if wy > 0.0 {
                                dep[g.idx(i, j)] += sign * ky * wy;
                            }
                        }
                    }
                }
            }
            // x-magnetization: −K on the bottom edge, +K on the top.
            let kx = mag.br_x_t / (MU_0 * mag.mu_r);
            if kx != 0.0 {
                for (y_e, sign) in [(m_yl, -1.0), (m_yh, 1.0)] {
                    for j in 0..ny {
                        let (yl, yh) = cv_y(j);
                        if !(yl <= y_e && y_e < yh) {
                            continue;
                        }
                        for i in 0..nx {
                            let (xl, xh) = cv_x(i);
                            let wx = overlap(xl, xh, m_xl, m_xh);
                            if wx > 0.0 {
                                dep[g.idx(i, j)] += sign * kx * wx;
                            }
                        }
                    }
                }
            }
            for (s, d) in sys.source.iter_mut().zip(dep.iter()) {
                *s += d;
            }
            magnet_sources.push(dep);
        }

        Ok((sys, unit_sources, magnet_sources))
    }

    fn wrap_solution(
        &self,
        sys: FvSystem,
        unit_sources: Vec<Vec<f64>>,
        magnet_sources: Vec<Vec<f64>>,
        sol: crate::grid::FvSolve,
    ) -> PlanarMagSolution {
        PlanarMagSolution {
            currents: self.conductors.iter().map(|c| c.total_current_a).collect(),
            unit_sources,
            magnet_sources,
            a: sol.u,
            sweeps: sol.sweeps,
            residual: sol.residual,
            system: sys,
        }
    }

    /// Solve on an `nx × ny` node grid. Saturating materials are frozen
    /// at their initial permeability — use [`Self::solve_nonlinear`] to
    /// iterate the B–H law.
    pub fn solve(
        &self,
        nx: usize,
        ny: usize,
        opts: &SolveOptions,
    ) -> Result<PlanarMagSolution, SolveError> {
        let (sys, unit, mag) = self.build_system(nx, ny, &self.initial_nu_cells(nx, ny))?;
        let sol = sys.solve(opts)?;
        Ok(self.wrap_solution(sys, unit, mag, sol))
    }

    /// Solve with the B–H law: Picard on the per-cell secant reluctivity,
    /// under-relaxed and warm-started, exactly as in
    /// [`crate::axisym::AxisymMagnetostatics::solve_nonlinear`]. The
    /// energy forms of the result are those of the converged secant
    /// system (see the axisym docs).
    pub fn solve_nonlinear(
        &self,
        nx: usize,
        ny: usize,
        opts: &SolveOptions,
        popts: &PicardOptions,
    ) -> Result<(PlanarMagSolution, PicardReport), SolveError> {
        let sat = self.sat_cells(nx, ny);
        if sat.iter().all(|c| c.is_none()) {
            let solution = self.solve(nx, ny, opts)?;
            return Ok((
                solution,
                PicardReport {
                    iterations: 0,
                    max_rel_delta: 0.0,
                },
            ));
        }
        let (x_cells, y_cells, dx, dy) = self.cell_geometry(nx, ny);
        let (x_min, y_min) = (self.x_min_mm * 1e-3, self.y_min_mm * 1e-3);
        let mut nu = self.initial_nu_cells(nx, ny);
        // Damped on the solved H — see the axisym driver for why both
        // ν-damping and B-damping diverge.
        let mut h_est = vec![0.0_f64; x_cells * y_cells];
        let mut warm: Option<Vec<f64>> = None;
        let mut report = PicardReport {
            iterations: 0,
            max_rel_delta: f64::MAX,
        };
        // While materials are still moving, the inner solve runs at a
        // loosened tolerance (high-contrast SOR sweeps are the whole
        // cost); the returned solution is re-solved at the caller's
        // tolerance below.
        let loose = SolveOptions {
            tol: opts.tol.max(1e-6),
            ..*opts
        };
        for it in 1..=popts.max_iters {
            let (mut sys, unit, mag) = self.build_system(nx, ny, &nu)?;
            if let Some(prev) = &warm {
                for (id, v) in prev.iter().enumerate() {
                    if !sys.fixed[id] {
                        sys.u0[id] = *v;
                    }
                }
            }
            let sol = sys.solve(&loose)?;
            let solution = self.wrap_solution(sys, unit, mag, sol);
            let mut max_db: f64 = 0.0;
            let mut b_scale: f64 = 1e-12;
            for ci in 0..x_cells {
                for cj in 0..y_cells {
                    let id = ci * y_cells + cj;
                    let Some((mu_ri, s)) = sat[id] else {
                        continue;
                    };
                    let xc = x_min + (ci as f64 + 0.5) * dx;
                    let yc = y_min + (cj as f64 + 0.5) * dy;
                    let (bx, by) = solution.b_at(xc, yc);
                    let b = (bx * bx + by * by).sqrt();
                    let h_solved = nu[id] * b;
                    let delta = h_solved - h_est[id];
                    h_est[id] += popts.relax * delta;
                    nu[id] = if h_est[id] > 1e-9 {
                        h_est[id] / b_of_h(mu_ri, s, h_est[id])
                    } else {
                        1.0 / (MU_0 * mu_ri)
                    };
                    max_db = max_db.max((popts.relax * delta).abs());
                    b_scale = b_scale.max(h_est[id].abs());
                }
            }
            let rel = max_db / b_scale;
            report = PicardReport {
                iterations: it,
                max_rel_delta: rel,
            };
            if rel < popts.tol {
                let (mut fsys, funit, fmag) = self.build_system(nx, ny, &nu)?;
                for (id, v) in solution.a.iter().enumerate() {
                    if !fsys.fixed[id] {
                        fsys.u0[id] = *v;
                    }
                }
                let fsol = fsys.solve(opts)?;
                return Ok((self.wrap_solution(fsys, funit, fmag, fsol), report));
            }
            warm = Some(solution.a);
        }
        Err(SolveError::NonlinearNotConverged {
            max_rel_delta: report.max_rel_delta,
            iterations: report.iterations,
        })
    }
}

#[inline]
fn overlap(a_lo: f64, a_hi: f64, b_lo: f64, b_hi: f64) -> f64 {
    (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
}

/// A converged planar magnetostatic field.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanarMagSolution {
    /// The assembled system.
    pub system: FvSystem,
    /// Vector potential `A_z` per node, Wb/m.
    pub a: Vec<f64>,
    /// SOR sweeps used.
    pub sweeps: usize,
    /// Final relative residual.
    pub residual: f64,
    /// Conductor currents at solve time, A.
    pub currents: Vec<f64>,
    /// Per-conductor source vectors at 1 A.
    pub unit_sources: Vec<Vec<f64>>,
    /// Per-magnet bound-current deposit vectors (fixed by Br).
    pub magnet_sources: Vec<Vec<f64>>,
}

impl PlanarMagSolution {
    /// Magnetic field `(B_x, B_y)` at `(x, y)` in **meters**, tesla —
    /// the exact curl of the bilinear `A` patch (divergence-free).
    pub fn b_at(&self, x_m: f64, y_m: f64) -> (f64, f64) {
        let (dax, day) = self.system.grid.grad_at(&self.a, x_m, y_m);
        (day, -dax)
    }

    /// Stored energy per meter of depth: both discrete forms and their
    /// mismatch, J/m. Source form is valid when Dirichlet values are zero
    /// (they always are here).
    pub fn energy_per_m(&self) -> EnergyBalance {
        self.system.energy_balance(&self.a)
    }

    /// `∇A` at node `(i, j)` by central differences (periodic-aware).
    fn node_grad(&self, i: usize, j: usize) -> (f64, f64) {
        let g = &self.system.grid;
        let gx = if g.periodic_x {
            let ip = (i + 1) % g.nx;
            let im = (i + g.nx - 1) % g.nx;
            (self.a[g.idx(ip, j)] - self.a[g.idx(im, j)]) / (2.0 * g.dx)
        } else if i == 0 {
            (self.a[g.idx(1, j)] - self.a[g.idx(0, j)]) / g.dx
        } else if i == g.nx - 1 {
            (self.a[g.idx(i, j)] - self.a[g.idx(i - 1, j)]) / g.dx
        } else {
            (self.a[g.idx(i + 1, j)] - self.a[g.idx(i - 1, j)]) / (2.0 * g.dx)
        };
        let gy = if j == 0 {
            (self.a[g.idx(i, 1)] - self.a[g.idx(i, 0)]) / g.dy
        } else if j == g.ny - 1 {
            (self.a[g.idx(i, j)] - self.a[g.idx(i, j - 1)]) / g.dy
        } else {
            (self.a[g.idx(i, j + 1)] - self.a[g.idx(i, j - 1)]) / (2.0 * g.dy)
        };
        (gx, gy)
    }

    fn force_of_deposits(&self, dep: &[f64], scale: f64) -> (f64, f64) {
        let g = &self.system.grid;
        let mut fx = 0.0;
        let mut fy = 0.0;
        for i in 0..g.nx {
            for j in 0..g.ny {
                let s = dep[g.idx(i, j)];
                if s == 0.0 {
                    continue;
                }
                let (dax, day) = self.node_grad(i, j);
                fx += scale * s * dax;
                fy += scale * s * day;
            }
        }
        (fx, fy)
    }

    fn torque_of_deposits(&self, dep: &[f64], scale: f64, cx_m: f64, cy_m: f64) -> f64 {
        let g = &self.system.grid;
        let mut t = 0.0;
        for i in 0..g.nx {
            for j in 0..g.ny {
                let s = dep[g.idx(i, j)];
                if s == 0.0 {
                    continue;
                }
                let (dax, day) = self.node_grad(i, j);
                t += scale * s * ((g.x(i) - cx_m) * day - (g.y(j) - cy_m) * dax);
            }
        }
        t
    }

    /// `J×B` force on conductor `k` (its own current in the total field),
    /// N per meter of depth.
    pub fn force_on_conductor(&self, k: usize) -> (f64, f64) {
        self.force_of_deposits(&self.unit_sources[k], self.currents[k])
    }

    /// `J×B` force on magnet `k` via its bound-current sheets, N per meter
    /// of depth. Rigid magnet, linear recoil.
    pub fn force_on_magnet(&self, k: usize) -> (f64, f64) {
        self.force_of_deposits(&self.magnet_sources[k], 1.0)
    }

    /// Torque of conductor `k` about `(cx_mm, cy_mm)`, N·m per meter of
    /// depth.
    pub fn torque_on_conductor(&self, k: usize, cx_mm: f64, cy_mm: f64) -> f64 {
        self.torque_of_deposits(
            &self.unit_sources[k],
            self.currents[k],
            cx_mm * 1e-3,
            cy_mm * 1e-3,
        )
    }

    /// Torque of magnet `k` about `(cx_mm, cy_mm)`, N·m per meter of depth.
    pub fn torque_on_magnet(&self, k: usize, cx_mm: f64, cy_mm: f64) -> f64 {
        self.torque_of_deposits(&self.magnet_sources[k], 1.0, cx_mm * 1e-3, cy_mm * 1e-3)
    }

    /// Maxwell-stress force through the full-period horizontal line
    /// `y = y_mm` (force on everything **below** the line), N per meter of
    /// depth, sampled at `n` midpoints. Periodic domains only — a full
    /// period is a closed contour there.
    pub fn force_through_line(&self, y_mm: f64, n: usize) -> (f64, f64) {
        assert!(
            self.system.grid.periodic_x,
            "force_through_line requires a periodic-x domain (closed contour)"
        );
        let g = &self.system.grid;
        let y = y_mm * 1e-3;
        let period = g.nx as f64 * g.dx;
        let dxp = period / n as f64;
        let mut fx = 0.0;
        let mut fy = 0.0;
        for p in 0..n {
            let x = g.x0 + (p as f64 + 0.5) * dxp;
            let (bx, by) = self.b_at(x, y);
            fx += bx * by / MU_0 * dxp;
            fy += (by * by - 0.5 * (bx * bx + by * by)) / MU_0 * dxp;
        }
        (fx, fy)
    }

    /// Maxwell-stress torque about `(cx_mm, cy_mm)` through the circle of
    /// radius `r_mm` (torque on everything inside), N·m per meter of
    /// depth: `T = (r²/μ₀)·∮ B_r·B_θ dθ`. The circle must lie in vacuum.
    pub fn torque_stress_circle(&self, cx_mm: f64, cy_mm: f64, r_mm: f64, n: usize) -> f64 {
        let (cx, cy, r) = (cx_mm * 1e-3, cy_mm * 1e-3, r_mm * 1e-3);
        let mut t = 0.0;
        let dth = 2.0 * std::f64::consts::PI / n as f64;
        for p in 0..n {
            let th = (p as f64 + 0.5) * dth;
            let (c, s) = (th.cos(), th.sin());
            let (bx, by) = self.b_at(cx + r * c, cy + r * s);
            let b_r = bx * c + by * s;
            let b_t = -bx * s + by * c;
            t += r * r / MU_0 * b_r * b_t * dth;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_pair_transmission_line_inductance_is_exact() {
        // Two full-width sheets carrying ±I with Neumann sides realize the
        // fringing-free parallel-plate line exactly. With sheet thickness
        // t, the exact per-depth inductance is μ₀·(gap + 2t/3)/w.
        let w = 40.0; // mm
        let t = 1.0;
        let gap = 8.0; // inner gap between sheets
        let mut dev = PlanarMagnetostatics::new(0.0, w, 0.0, 30.0);
        dev.bc_x_low = Bc::Neumann;
        dev.bc_x_high = Bc::Neumann;
        // One Dirichlet side to gauge A, one Neumann: A = 0 (no field)
        // below the pair, ∂A/∂y = 0 (no field) above it. Fixing A = 0 on
        // BOTH sides would force a spurious return flux — the sheet pair
        // has a net A jump across it.
        dev.bc_y_low = Bc::Zero;
        dev.bc_y_high = Bc::Neumann;
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: w,
                y_min_mm: 10.0,
                y_max_mm: 10.0 + t,
            },
            total_current_a: 3.0,
        });
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: w,
                y_min_mm: 10.0 + t + gap,
                y_max_mm: 10.0 + 2.0 * t + gap,
            },
            total_current_a: -3.0,
        });
        let sol = dev.solve(41, 121, &SolveOptions::default()).unwrap();
        let e = sol.energy_per_m();
        assert!(e.residual < 1e-6, "energy imbalance {:.2e}", e.residual);
        let l_per_m = 2.0 * e.source / (3.0 * 3.0);
        let expect = MU_0 * (gap + 2.0 * t / 3.0) * 1e-3 / (w * 1e-3);
        let rel = (l_per_m - expect).abs() / expect;
        assert!(
            rel < 5e-3,
            "L' = {l_per_m:.6e} vs exact {expect:.6e} (rel {rel:.2e})"
        );
    }

    #[test]
    fn wire_in_uniform_field_feels_f_equals_i_b() {
        // Wide sheet pair makes a uniform B_x between the sheets; a small
        // wire in the middle must feel F = I×B, and the Maxwell-stress
        // circle around it must say the same as J×B.
        let mut dev = PlanarMagnetostatics::new(0.0, 80.0, 0.0, 80.0);
        dev.bc_x_low = Bc::Neumann;
        dev.bc_x_high = Bc::Neumann;
        dev.bc_y_low = Bc::Zero;
        dev.bc_y_high = Bc::Neumann;
        let sheet_i = 100.0;
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 80.0,
                y_min_mm: 10.0,
                y_max_mm: 12.0,
            },
            total_current_a: sheet_i,
        });
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 0.0,
                x_max_mm: 80.0,
                y_min_mm: 68.0,
                y_max_mm: 70.0,
            },
            total_current_a: -sheet_i,
        });
        let wire_i = 5.0;
        dev.conductors.push(Conductor {
            region: Rect {
                x_min_mm: 38.0,
                x_max_mm: 42.0,
                y_min_mm: 38.0,
                y_max_mm: 42.0,
            },
            total_current_a: wire_i,
        });
        let sol = dev.solve(81, 81, &SolveOptions::default()).unwrap();

        // Sheet pair carrying (+I lower, −I upper) makes B_x = −μ₀·I/w
        // between the sheets. The sample point also sees the wire's own
        // 1/ρ field (~2% at 30 mm), so the assert band covers that.
        let b_sheets = -MU_0 * sheet_i / 0.080;
        let (bx, by) = sol.b_at(0.010, 0.040);
        assert!(
            ((bx - b_sheets) / b_sheets).abs() < 0.04,
            "sheet field {bx:.4e} vs {b_sheets:.4e}"
        );
        assert!(by.abs() < 0.04 * b_sheets.abs());

        // F = I ẑ × B = I·B_x·ŷ, priced at the exact sheet field.
        let f_expect = wire_i * b_sheets;
        let (fx_jb, fy_jb) = sol.force_on_conductor(2);
        assert!(
            ((fy_jb - f_expect) / f_expect).abs() < 0.03,
            "J×B force {fy_jb:.4e} vs I·B {f_expect:.4e}"
        );
        assert!(fx_jb.abs() < 0.03 * f_expect.abs());

        // Independent route: stress circle around the wire.
        let g = &sol.system.grid;
        let mut fx_st = 0.0;
        let mut fy_st = 0.0;
        let (cx, cy, r) = (0.040, 0.040, 0.012);
        let n = 720;
        let dth = 2.0 * std::f64::consts::PI / n as f64;
        for p in 0..n {
            let th = (p as f64 + 0.5) * dth;
            let (c, s) = (th.cos(), th.sin());
            let (bx, by) = sol.b_at(cx + r * c, cy + r * s);
            let bn = bx * c + by * s;
            let b2 = bx * bx + by * by;
            fx_st += (bx * bn - 0.5 * b2 * c) / MU_0 * r * dth;
            fy_st += (by * bn - 0.5 * b2 * s) / MU_0 * r * dth;
        }
        let _ = g;
        assert!(
            ((fy_st - f_expect) / f_expect).abs() < 0.03,
            "stress force {fy_st:.4e} vs I·B {f_expect:.4e}"
        );
        assert!(fx_st.abs() < 0.03 * f_expect.abs());
        // Torque about the wire center is zero by symmetry.
        let t = sol.torque_stress_circle(40.0, 40.0, 12.0, 720);
        assert!(t.abs() < 0.03 * f_expect.abs() * 0.012);
    }

    #[test]
    fn magnet_pair_attracts_with_equal_and_opposite_forces() {
        // Two y-magnetized blocks stacked with a gap: aligned remanence →
        // attraction; Newton's third law must hold through the bound-sheet
        // force route.
        let mut dev = PlanarMagnetostatics::new(0.0, 100.0, 0.0, 100.0);
        for (y0, y1) in [(40.0, 46.0), (54.0, 60.0)] {
            dev.magnets.push(MagnetBlock {
                region: Rect {
                    x_min_mm: 42.0,
                    x_max_mm: 58.0,
                    y_min_mm: y0,
                    y_max_mm: y1,
                },
                br_x_t: 0.0,
                br_y_t: 1.2,
                mu_r: 1.05,
            });
        }
        let sol = dev.solve(121, 121, &SolveOptions::default()).unwrap();
        // Field in the gap points +y (both magnets push flux the same
        // way). Open magnetic circuit, wide gap, no iron: a fraction of
        // Br is the honest answer — the sign and order are the check.
        let (_, by) = sol.b_at(0.050, 0.050);
        assert!(by > 0.2, "gap field weak or misdirected: {by}");
        let (fx0, fy0) = sol.force_on_magnet(0);
        let (fx1, fy1) = sol.force_on_magnet(1);
        assert!(fy0 > 0.0, "lower magnet must be pulled up: {fy0}");
        assert!(fy1 < 0.0, "upper magnet must be pulled down: {fy1}");
        // The configuration is exactly mirror-symmetric, so the discrete
        // forces must cancel to solver tolerance — this asserts the
        // assembly stays tie-free (cell-sampled materials), not physics.
        let asym = (fy0 + fy1).abs() / fy0.abs();
        assert!(asym < 1e-3, "action–reaction mismatch: {asym:.3e}");
        assert!(fx0.abs() < 0.02 * fy0.abs());
        assert!(fx1.abs() < 0.02 * fy0.abs());
    }
}
