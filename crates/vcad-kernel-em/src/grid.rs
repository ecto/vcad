//! Shared finite-volume machinery for every EM formulation in this crate.
//!
//! All four M0 problems — axisymmetric and planar, magnetostatic and
//! electrostatic — discretize the same divergence-form elliptic equation
//!
//! ```text
//!   ∇·(c ∇u) = −s
//! ```
//!
//! on a uniform rectangular node grid. The formulations differ only in what
//! `u` means and in how geometry (the 2πr measure of the axisymmetric
//! problems) and material coefficients (μ, ε) enter the **face
//! conductances**. This module owns the shared part:
//!
//! - the symmetric 5-point system ([`FvSystem`]): one conductance per grid
//!   face, shared by the two nodes it joins, so the discrete operator is
//!   **symmetric by construction** — the property the discrete adjoint
//!   (M2) reuses the forward solver through;
//! - SOR with a Chebyshev-estimate relaxation factor and **scale-invariant
//!   stopping** (objectives and fields may live at 1e−30; an absolute
//!   epsilon must never read them as converged — lesson inherited from
//!   `vcad-kernel-particle`);
//! - the two discrete energy forms (field form `½·Σ_f G·Δu²`, source form
//!   `½·Σ_n s·u`) and their **balance residual**, the solve-quality number
//!   that later feeds receipt provenance;
//! - conservative sampling: [`Grid2D::grad_at`] returns the **exact
//!   gradient of the bilinear interpolant**, not an interpolation of node
//!   differences, so line integrals of the sampled gradient telescope to
//!   interpolant differences exactly.
//!
//! Boundary handling falls out of the face formulation: a boundary node
//! that is not marked Dirichlet simply has no face beyond the edge — a
//! natural zero-flux (Neumann) condition. Dirichlet sides are imposed by
//! fixing their nodes.

/// Truncation condition for one side of a solve domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bc {
    /// Fix the unknown to zero on that side (Dirichlet). For magnetics
    /// this excludes flux from crossing the boundary; for potentials it
    /// grounds it.
    Zero,
    /// Leave the side free: the missing outside face is a natural
    /// zero-normal-flux (Neumann) condition — a symmetry plane.
    Neumann,
}

/// Uniform rectangular node grid.
///
/// Node `(i, j)` sits at `(x0 + i·dx, y0 + j·dy)` with `i ∈ [0, nx)`,
/// `j ∈ [0, ny)`. For axisymmetric problems `x` is the radius `r` (with
/// `x0 = 0`) and `y` is the axial coordinate `z`.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid2D {
    /// Node count along x (or r).
    pub nx: usize,
    /// Node count along y (or z).
    pub ny: usize,
    /// Node spacing along x, m.
    pub dx: f64,
    /// Node spacing along y, m.
    pub dy: f64,
    /// Coordinate of node column `i = 0`, m.
    pub x0: f64,
    /// Coordinate of node row `j = 0`, m.
    pub y0: f64,
    /// Wrap the x direction (column `nx−1` neighbors column `0`). Used by
    /// unrolled rotating machines. The wrap pitch is `nx·dx`.
    pub periodic_x: bool,
}

impl Grid2D {
    /// Row-major node index.
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        i * self.ny + j
    }

    /// x coordinate of column `i`, m.
    #[inline]
    pub fn x(&self, i: usize) -> f64 {
        self.x0 + i as f64 * self.dx
    }

    /// y coordinate of row `j`, m.
    #[inline]
    pub fn y(&self, j: usize) -> f64 {
        self.y0 + j as f64 * self.dy
    }

    /// Number of x-direction faces (between columns `i` and `i+1`, plus the
    /// wrap face when periodic).
    #[inline]
    pub fn n_faces_x(&self) -> usize {
        if self.periodic_x {
            self.nx * self.ny
        } else {
            (self.nx - 1) * self.ny
        }
    }

    /// Number of y-direction faces.
    #[inline]
    pub fn n_faces_y(&self) -> usize {
        self.nx * (self.ny - 1)
    }

    /// Index into the x-face array for the face joining `(i, j)` and
    /// `(i+1 mod nx, j)`.
    #[inline]
    pub fn fx(&self, i: usize, j: usize) -> usize {
        i * self.ny + j
    }

    /// Index into the y-face array for the face joining `(i, j)` and
    /// `(i, j+1)`.
    #[inline]
    pub fn fy(&self, i: usize, j: usize) -> usize {
        i * (self.ny - 1) + j
    }

    /// Locate the cell containing `(x, y)` and the fractional position
    /// inside it. Returns `(i0, j0, fx, fy)`; `i0+1` may wrap when
    /// periodic. Coordinates are clamped to the grid (and wrapped in x
    /// when periodic).
    fn locate(&self, x: f64, y: f64) -> (usize, usize, f64, f64) {
        let eps = 1e-12;
        let u = if self.periodic_x {
            let span = self.nx as f64;
            let mut t = (x - self.x0) / self.dx % span;
            if t < 0.0 {
                t += span;
            }
            t.min(span - eps)
        } else {
            ((x - self.x0) / self.dx).clamp(0.0, (self.nx - 1) as f64 - eps)
        };
        let w = ((y - self.y0) / self.dy).clamp(0.0, (self.ny - 1) as f64 - eps);
        let i0 = u.floor() as usize;
        let j0 = w.floor() as usize;
        (i0, j0, u - i0 as f64, w - j0 as f64)
    }

    /// Column to the +x side of column `i` (wrapping when periodic).
    #[inline]
    pub(crate) fn right(&self, i: usize) -> usize {
        if i + 1 == self.nx && self.periodic_x {
            0
        } else {
            i + 1
        }
    }

    /// Bilinear interpolation of a node field at `(x, y)` in meters.
    pub fn value_at(&self, u: &[f64], x: f64, y: f64) -> f64 {
        let (i0, j0, fu, fw) = self.locate(x, y);
        let i1 = self.right(i0);
        u[self.idx(i0, j0)] * (1.0 - fu) * (1.0 - fw)
            + u[self.idx(i1, j0)] * fu * (1.0 - fw)
            + u[self.idx(i0, j0 + 1)] * (1.0 - fu) * fw
            + u[self.idx(i1, j0 + 1)] * fu * fw
    }

    /// Gradient `(∂u/∂x, ∂u/∂y)` of the **bilinear interpolant** at
    /// `(x, y)` — the exact patch gradient, so the sampled field is
    /// conservative: its line integral between two points equals the
    /// interpolant difference.
    pub fn grad_at(&self, u: &[f64], x: f64, y: f64) -> (f64, f64) {
        let (i0, j0, fu, fw) = self.locate(x, y);
        let i1 = self.right(i0);
        let u00 = u[self.idx(i0, j0)];
        let u10 = u[self.idx(i1, j0)];
        let u01 = u[self.idx(i0, j0 + 1)];
        let u11 = u[self.idx(i1, j0 + 1)];
        let gx = ((u10 - u00) * (1.0 - fw) + (u11 - u01) * fw) / self.dx;
        let gy = ((u01 - u00) * (1.0 - fu) + (u11 - u10) * fu) / self.dy;
        (gx, gy)
    }
}

/// Options for [`FvSystem::solve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// SOR over-relaxation factor in (1, 2). `0.0` selects the Chebyshev
    /// estimate `2 / (1 + sin(π / max(nx, ny)))`.
    pub omega: f64,
    /// Scale-invariant convergence tolerance: the sweep stops when the
    /// largest node update falls below `tol × max|u|` (current solution
    /// scale, Dirichlet nodes included).
    pub tol: f64,
    /// Hard cap on SOR sweeps.
    pub max_sweeps: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            omega: 0.0,
            tol: 1e-8,
            max_sweeps: 200_000,
        }
    }
}

/// Failure modes of [`FvSystem::solve`] and the nonlinear drivers built
/// on it.
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The grid must be at least 3×3 nodes.
    GridTooSmall,
    /// SOR did not reach `tol` within `max_sweeps`.
    NotConverged {
        /// Final relative residual (largest node update / solution scale).
        residual: f64,
        /// Sweeps performed.
        sweeps: usize,
    },
    /// The outer Picard loop of a nonlinear solve did not converge.
    NonlinearNotConverged {
        /// Final largest relative reluctivity update.
        max_rel_delta: f64,
        /// Outer iterations performed.
        iterations: usize,
        /// Relaxation in force when the loop gave up (below the starting
        /// `picard_relax` if adaptive damping backed it off).
        relax: f64,
        /// Tolerance the loop was aiming for.
        tol: f64,
        /// Observed per-iteration contraction of the residual over the
        /// trailing window. `< 1` = converging, just not fast enough for
        /// the iteration cap; `>= 1` = not converging at all. `None` when
        /// the loop gave up before the window filled.
        decay_rate: Option<f64>,
    },
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::GridTooSmall => write!(f, "grid must be at least 3x3 nodes"),
            SolveError::NotConverged { residual, sweeps } => write!(
                f,
                "SOR not converged after {sweeps} sweeps (relative residual {residual:.3e})"
            ),
            SolveError::NonlinearNotConverged {
                max_rel_delta,
                iterations,
                relax,
                tol,
                decay_rate,
            } => {
                write!(
                    f,
                    "Picard not converged after {iterations} iterations \
                     (largest reluctivity update {max_rel_delta:.3e}, target \
                     {tol:.1e}, relaxation {relax:.3}). This is the NONLINEAR \
                     outer loop over the B-H law, NOT the SOR inner solve — \
                     `tol` and `max_sweeps` have no effect on it. "
                )?;
                // Two failure modes, opposite fixes. Read the measured
                // contraction rate rather than guessing: a residual still
                // falling geometrically wants a higher iteration cap, and
                // under-relaxing it further would only slow it down.
                match decay_rate {
                    // Too few iterations to have measured a trend — say
                    // that, rather than diagnosing a mode from nothing.
                    None => write!(
                        f,
                        "The loop stopped before its contraction rate could be \
                         measured, so which mode this is (slow convergence vs \
                         oscillation) is not yet known. Raise \
                         `picard_max_iters` and retry; the next failure will \
                         name the mode."
                    ),
                    Some(rate) if *rate < 0.999 => {
                        let need = (max_rel_delta / tol).ln() / (1.0 / rate).ln();
                        let suggested =
                            iterations + ((need * 1.5).ceil().max(50.0) as usize).max(50);
                        write!(
                            f,
                            "The residual IS still falling, at {rate:.4} per \
                             iteration — the iteration cap arrived first. \
                             Retry with `picard_max_iters: {suggested}`. Do \
                             not lower `picard_relax` here; it would only \
                             slow the contraction further."
                        )
                    }
                    _ => {
                        let suggested_relax = (relax * 0.4).max(0.05);
                        write!(
                            f,
                            "The residual has stopped falling, so more \
                             iterations alone will not close it. Try \
                             `picard_relax: {suggested_relax:.2}` (deeper \
                             under-relaxation, the lever for a limit cycle) — \
                             but check the device too: a flux path driven \
                             past what it can carry stalls the same way, and \
                             no solver setting fixes that."
                        )
                    }
                }?;
                write!(
                    f,
                    " Raising `picard_tol` accepts a looser material state \
                     deliberately."
                )
            }
        }
    }
}

impl std::error::Error for SolveError {}

/// Result of a converged [`FvSystem::solve`].
#[derive(Debug, Clone, PartialEq)]
pub struct FvSolve {
    /// Node values.
    pub u: Vec<f64>,
    /// SOR sweeps used.
    pub sweeps: usize,
    /// Final relative residual (largest update / solution scale).
    pub residual: f64,
}

/// The two discrete energy forms of a converged solve and their mismatch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyBalance {
    /// Field form `½·Σ_faces G·Δu²`, joules (or J/m for planar systems).
    pub field: f64,
    /// Source form `½·Σ_nodes s·u`, same unit. Equal to the field form at
    /// the exact solution of a zero-Dirichlet problem (virtual work); the
    /// gap measures solve quality.
    pub source: f64,
    /// `|field − source| / max(|field|, |source|)`, `0` when both vanish.
    pub residual: f64,
}

/// Sentinel for "this face side has no material cell" in
/// [`FaceWeights`].
pub const NO_CELL: usize = usize::MAX;

/// How each face conductance decomposes over its two flanking material
/// cells: `G_f = w_a·ν(cell_a) + w_b·ν(cell_b)`. Populated by the
/// magnetostatic builders; the discrete adjoint turns
/// `dJ/dG_f = −Δu_f·Δλ_f` into per-cell material gradients through
/// these weights without re-deriving the assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceWeights {
    /// Per x-face: `[(cell, weight); 2]`, cell = [`NO_CELL`] when absent.
    pub x: Vec<[(usize, f64); 2]>,
    /// Per y-face: same layout.
    pub y: Vec<[(usize, f64); 2]>,
}

/// Symmetric 5-point finite-volume system on a [`Grid2D`].
///
/// The discrete equation at every free node is
/// `Σ_faces G_f (u_nbr − u) + s = 0`, with `G_f` the face conductance
/// (geometry × material, assembled by the formulation modules) and `s` the
/// integrated source over the node's control volume. Faces are shared, so
/// the operator is symmetric.
#[derive(Debug, Clone, PartialEq)]
pub struct FvSystem {
    /// The grid.
    pub grid: Grid2D,
    /// x-face conductances, indexed by [`Grid2D::fx`].
    pub gx: Vec<f64>,
    /// y-face conductances, indexed by [`Grid2D::fy`].
    pub gy: Vec<f64>,
    /// Integrated source per node (units of `G·u`).
    pub source: Vec<f64>,
    /// Dirichlet mask per node.
    pub fixed: Vec<bool>,
    /// Dirichlet values (and the initial guess for free nodes).
    pub u0: Vec<f64>,
    /// Face ← cell incidence weights (magnetostatic builders fill this;
    /// `None` for formulations that haven't).
    pub face_weights: Option<FaceWeights>,
}

impl FvSystem {
    /// A zero-initialized system on `grid`.
    pub fn new(grid: Grid2D) -> Self {
        let n = grid.nx * grid.ny;
        let nfx = grid.n_faces_x();
        let nfy = grid.n_faces_y();
        Self {
            grid,
            gx: vec![0.0; nfx],
            gy: vec![0.0; nfy],
            source: vec![0.0; n],
            fixed: vec![false; n],
            u0: vec![0.0; n],
            face_weights: None,
        }
    }

    /// Solve by SOR. Fails closed: non-convergence is an error carrying the
    /// final relative residual, never a silently degraded field.
    pub fn solve(&self, opts: &SolveOptions) -> Result<FvSolve, SolveError> {
        let g = &self.grid;
        if g.nx < 3 || g.ny < 3 {
            return Err(SolveError::GridTooSmall);
        }
        let omega = if opts.omega > 0.0 {
            opts.omega
        } else {
            let n = g.nx.max(g.ny) as f64;
            2.0 / (1.0 + (std::f64::consts::PI / n).sin())
        };

        let mut u = self.u0.clone();
        let mut scale = u.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let mut rel = f64::MAX;
        let mut sweeps = 0;
        while sweeps < opts.max_sweeps {
            let mut residual = 0.0;
            for i in 0..g.nx {
                for j in 0..g.ny {
                    let id = g.idx(i, j);
                    if self.fixed[id] {
                        continue;
                    }
                    let mut num = self.source[id];
                    let mut den = 0.0;
                    // −x face.
                    if i > 0 {
                        let gf = self.gx[g.fx(i - 1, j)];
                        num += gf * u[g.idx(i - 1, j)];
                        den += gf;
                    } else if g.periodic_x {
                        let gf = self.gx[g.fx(g.nx - 1, j)];
                        num += gf * u[g.idx(g.nx - 1, j)];
                        den += gf;
                    }
                    // +x face.
                    if i + 1 < g.nx {
                        let gf = self.gx[g.fx(i, j)];
                        num += gf * u[g.idx(i + 1, j)];
                        den += gf;
                    } else if g.periodic_x {
                        let gf = self.gx[g.fx(i, j)];
                        num += gf * u[g.idx(0, j)];
                        den += gf;
                    }
                    // −y face.
                    if j > 0 {
                        let gf = self.gy[g.fy(i, j - 1)];
                        num += gf * u[g.idx(i, j - 1)];
                        den += gf;
                    }
                    // +y face.
                    if j + 1 < g.ny {
                        let gf = self.gy[g.fy(i, j)];
                        num += gf * u[g.idx(i, j + 1)];
                        den += gf;
                    }
                    if den == 0.0 {
                        continue;
                    }
                    let updated = num / den;
                    let delta = updated - u[id];
                    u[id] += omega * delta;
                    let ad = delta.abs();
                    // NaN fails closed (all NaN comparisons are false —
                    // without this a poisoned sweep can read as
                    // converged).
                    if !ad.is_finite() {
                        residual = f64::MAX;
                    } else if ad > residual {
                        residual = ad;
                    }
                    let au = u[id].abs();
                    if au > scale {
                        scale = au;
                    }
                }
            }
            sweeps += 1;
            rel = if scale > 0.0 {
                residual / scale
            } else if residual == 0.0 {
                0.0
            } else {
                f64::MAX
            };
            if rel < opts.tol {
                break;
            }
        }
        if rel >= opts.tol {
            return Err(SolveError::NotConverged {
                residual: rel,
                sweeps,
            });
        }
        Ok(FvSolve {
            u,
            sweeps,
            residual: rel,
        })
    }

    /// Field-form energy `½·Σ_faces G·Δu²`.
    pub fn field_energy(&self, u: &[f64]) -> f64 {
        let g = &self.grid;
        let mut w = 0.0;
        let x_pairs = if g.periodic_x { g.nx } else { g.nx - 1 };
        for i in 0..x_pairs {
            let i1 = g.right(i);
            for j in 0..g.ny {
                let d = u[g.idx(i1, j)] - u[g.idx(i, j)];
                w += self.gx[g.fx(i, j)] * d * d;
            }
        }
        for i in 0..g.nx {
            for j in 0..g.ny - 1 {
                let d = u[g.idx(i, j + 1)] - u[g.idx(i, j)];
                w += self.gy[g.fy(i, j)] * d * d;
            }
        }
        0.5 * w
    }

    /// Source-form energy `½·Σ_nodes s·u`. Matches the field form at the
    /// solution of a problem whose Dirichlet values are all zero.
    pub fn source_energy(&self, u: &[f64]) -> f64 {
        0.5 * self
            .source
            .iter()
            .zip(u.iter())
            .map(|(s, v)| s * v)
            .sum::<f64>()
    }

    /// Both energy forms and their relative mismatch.
    pub fn energy_balance(&self, u: &[f64]) -> EnergyBalance {
        let field = self.field_energy(u);
        let source = self.source_energy(u);
        let denom = field.abs().max(source.abs());
        let residual = if denom > 0.0 {
            (field - source).abs() / denom
        } else {
            0.0
        };
        EnergyBalance {
            field,
            source,
            residual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_planar(nx: usize, ny: usize, periodic_x: bool) -> FvSystem {
        // Planar Laplace weights with c = 1, dx = dy = 1 mm.
        let grid = Grid2D {
            nx,
            ny,
            dx: 1e-3,
            dy: 1e-3,
            x0: 0.0,
            y0: 0.0,
            periodic_x,
        };
        let mut sys = FvSystem::new(grid);
        for gf in sys.gx.iter_mut() {
            *gf = 1.0;
        }
        for gf in sys.gy.iter_mut() {
            *gf = 1.0;
        }
        sys
    }

    #[test]
    fn parallel_plate_laplace_is_linear_and_exact() {
        // u = 0 at j = 0, u = 1 at j = ny−1, Neumann sides: the discrete
        // solution is exactly linear in y.
        let mut sys = uniform_planar(9, 11, false);
        let g = sys.grid.clone();
        for i in 0..g.nx {
            let bottom = g.idx(i, 0);
            let top = g.idx(i, g.ny - 1);
            sys.fixed[bottom] = true;
            sys.u0[bottom] = 0.0;
            sys.fixed[top] = true;
            sys.u0[top] = 1.0;
        }
        let sol = sys.solve(&SolveOptions::default()).unwrap();
        for i in 0..g.nx {
            for j in 0..g.ny {
                let expect = j as f64 / (g.ny - 1) as f64;
                assert!(
                    (sol.u[g.idx(i, j)] - expect).abs() < 1e-7,
                    "node ({i},{j}): {} vs {expect}",
                    sol.u[g.idx(i, j)]
                );
            }
        }
    }

    #[test]
    fn solve_is_scale_invariant_down_to_1e30() {
        // The lesson from the particle optimizer: absolute epsilons read
        // tiny-scale problems as converged (or never converged). Solutions
        // must scale exactly linearly with the source, with the same sweep
        // count.
        let mut sys = uniform_planar(17, 17, false);
        let g = sys.grid.clone();
        for i in 0..g.nx {
            for j in [0, g.ny - 1] {
                sys.fixed[g.idx(i, j)] = true;
            }
        }
        for j in 0..g.ny {
            for i in [0, g.nx - 1] {
                sys.fixed[g.idx(i, j)] = true;
            }
        }
        sys.source[g.idx(8, 8)] = 1.0;
        let a = sys.solve(&SolveOptions::default()).unwrap();

        let mut tiny = sys.clone();
        tiny.source[g.idx(8, 8)] = 1e-30;
        let b = tiny.solve(&SolveOptions::default()).unwrap();

        assert_eq!(a.sweeps, b.sweeps, "sweep count must not depend on scale");
        for (va, vb) in a.u.iter().zip(b.u.iter()) {
            assert!(
                (va * 1e-30 - vb).abs() <= 1e-45 + 1e-12 * va.abs() * 1e-30,
                "not linear: {va} vs {vb}"
            );
        }
    }

    #[test]
    fn energy_forms_agree_on_a_source_driven_problem() {
        let mut sys = uniform_planar(21, 21, false);
        let g = sys.grid.clone();
        for i in 0..g.nx {
            for j in [0, g.ny - 1] {
                sys.fixed[g.idx(i, j)] = true;
            }
        }
        for j in 0..g.ny {
            for i in [0, g.nx - 1] {
                sys.fixed[g.idx(i, j)] = true;
            }
        }
        sys.source[g.idx(10, 10)] = 2.5;
        sys.source[g.idx(5, 14)] = -1.0;
        let sol = sys.solve(&SolveOptions::default()).unwrap();
        let bal = sys.energy_balance(&sol.u);
        assert!(bal.field > 0.0);
        assert!(
            bal.residual < 1e-6,
            "energy imbalance {:.3e} (field {:.6e}, source {:.6e})",
            bal.residual,
            bal.field,
            bal.source
        );
    }

    #[test]
    fn sampled_gradient_is_conservative_within_a_cell() {
        // The bilinear patch gradient integrated along a segment must
        // telescope to the interpolant difference. Two-point Gauss is
        // exact for the linear-along-the-segment integrand.
        let sys = uniform_planar(5, 5, false);
        let g = &sys.grid;
        let mut u = vec![0.0; g.nx * g.ny];
        for i in 0..g.nx {
            for j in 0..g.ny {
                // An arbitrary smooth-ish node field.
                u[g.idx(i, j)] = (i as f64).sin() + 0.3 * (j as f64).powi(2) - 0.1 * (i * j) as f64;
            }
        }
        let a = (1.2e-3, 2.3e-3);
        let b = (1.9e-3, 2.8e-3); // same cell (1..2, 2..3)
        let gauss = [0.5 - 0.5 / 3.0_f64.sqrt(), 0.5 + 0.5 / 3.0_f64.sqrt()];
        let mut line = 0.0;
        for t in gauss {
            let x = a.0 + t * (b.0 - a.0);
            let y = a.1 + t * (b.1 - a.1);
            let (gx, gy) = g.grad_at(&u, x, y);
            line += 0.5 * (gx * (b.0 - a.0) + gy * (b.1 - a.1));
        }
        let diff = g.value_at(&u, b.0, b.1) - g.value_at(&u, a.0, a.1);
        assert!(
            (line - diff).abs() < 1e-12 * diff.abs().max(1.0),
            "line integral {line} vs difference {diff}"
        );
    }

    #[test]
    fn periodic_x_wraps_seamlessly() {
        // Same source, grid shifted by half the period: the solution must
        // be the same field translated (mod wrap).
        let build = |i_src: usize| {
            let mut sys = uniform_planar(16, 11, true);
            let g = sys.grid.clone();
            for i in 0..g.nx {
                sys.fixed[g.idx(i, 0)] = true;
                sys.fixed[g.idx(i, g.ny - 1)] = true;
            }
            sys.source[g.idx(i_src, 5)] = 1.0;
            sys.solve(&SolveOptions::default()).unwrap()
        };
        let a = build(2);
        let b = build(10);
        let g = Grid2D {
            nx: 16,
            ny: 11,
            dx: 1e-3,
            dy: 1e-3,
            x0: 0.0,
            y0: 0.0,
            periodic_x: true,
        };
        let scale = a.u.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        for i in 0..16 {
            for j in 0..11 {
                let ia = g.idx(i, j);
                let ib = g.idx((i + 8) % 16, j);
                assert!(
                    (a.u[ia] - b.u[ib]).abs() < 1e-6 * scale,
                    "wrap mismatch at ({i},{j})"
                );
            }
        }
    }

    #[test]
    fn all_dirichlet_zero_converges_immediately() {
        let mut sys = uniform_planar(5, 5, false);
        for f in sys.fixed.iter_mut() {
            *f = true;
        }
        let sol = sys.solve(&SolveOptions::default()).unwrap();
        assert_eq!(sol.sweeps, 1);
        assert!(sol.u.iter().all(|v| *v == 0.0));
    }
}
