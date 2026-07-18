//! Axisymmetric Helmholtz field solver on an (r, z) grid.
//!
//! Solves the driven Helmholtz equation
//!
//! ```text
//! (∇² + k²) p = −jωρ · s      k = ω/c,   ω = 2πf
//! ```
//!
//! for the complex pressure phasor `p(r, z)` in a [`Cavity`], with a
//! **vertex-centred finite-volume** discretisation. Integrating the operator
//! over each node's control volume makes the assembled matrix
//!
//! - **conservative** — fluxes across a shared face are equal and opposite,
//!   so nothing is created or destroyed at cell boundaries;
//! - **symmetric** — the shared-face coupling `A/d` is identical in both
//!   nodes' rows, so field **reciprocity** (source ↔ receiver) holds to
//!   round-off (a regression test);
//! - **exact on the axis** — the radial cell area vanishes as `r → 0`, which
//!   reproduces the `2·∂²p/∂r²` axis limit with no special-casing (the same
//!   r-weighted stencil discipline as `vcad-kernel-particle`'s Poisson solve).
//!
//! Boundary conditions: rigid walls are the natural (zero-flux) BC — a wall
//! face is simply omitted. A pressure-release mouth pins `p = 0`. A driven
//! piston contributes a known Neumann flux `∂p/∂n = −jωρ·U` to the
//! right-hand side.
//!
//! The operator is **indefinite** (singular at every resonance), so it is
//! solved directly by block-Thomas ([`crate::linalg`]), never relaxation.

use crate::cavity::{Cavity, EndCondition};
use crate::complex::Cplx;
use crate::linalg::{solve_block_tridiag, Singular};

const MM: f64 = 1e-3;

/// Node role in the discretised domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Outside the fluid — a rigid solid. Not an unknown (`p = 0`).
    Solid,
    /// A fluid interior unknown.
    Fluid,
    /// A pressure-release mouth node — known `p = 0`.
    Open,
}

/// How the field is driven.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// The cavity's [`EndCondition::Piston`] disk, driven at complex normal
    /// velocity `velocity` (m/s). The loudspeaker cone.
    Piston {
        /// Complex normal velocity amplitude, m/s.
        velocity: Cplx,
    },
    /// A point monopole of volume velocity `q` (m³/s) at the fluid node
    /// nearest `(r_mm, z_mm)`.
    Monopole {
        /// Radial position, mm.
        r_mm: f64,
        /// Axial position, mm.
        z_mm: f64,
        /// Volume velocity, m³/s.
        q: Cplx,
    },
}

/// Failure modes of [`solve_driven`].
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The grid must be at least 3×3 nodes.
    GridTooSmall,
    /// No fluid nodes fell on the grid (radius/height too small for the mesh).
    NoFluid,
    /// A monopole source landed outside the fluid.
    SourceNotInFluid,
    /// The cavity has no piston but a [`Source::Piston`] was requested.
    NoPiston,
    /// The discrete operator is singular at this frequency — a resonance sits
    /// exactly on `f`. Fail-closed: perturb `f` (a sweep never lands here).
    Singular,
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::GridTooSmall => write!(f, "grid must be at least 3x3 nodes"),
            SolveError::NoFluid => write!(f, "no fluid nodes on the grid"),
            SolveError::SourceNotInFluid => write!(f, "monopole source is outside the fluid"),
            SolveError::NoPiston => write!(f, "piston source requested but cavity has no piston"),
            SolveError::Singular => {
                write!(
                    f,
                    "operator singular at this frequency (a resonance sits on f)"
                )
            }
        }
    }
}

impl std::error::Error for SolveError {}

/// Locate `x` (in node units) on an `n`-node axis: lower cell index (≤ n−2)
/// and in-cell fraction in `[0, 1]`. Index-space clamping, matching the
/// Poisson sampler in `vcad-kernel-particle` — a float-epsilon ceiling breaks
/// down once the axis size approaches `1/ulp`.
#[inline]
pub(crate) fn cell_index(x: f64, n: usize) -> (usize, f64) {
    let x = x.clamp(0.0, (n - 1) as f64);
    let i0 = (x as usize).min(n - 2);
    (i0, (x - i0 as f64).clamp(0.0, 1.0))
}

/// The solved complex pressure field on the grid.
#[derive(Debug, Clone)]
pub struct Field {
    /// Radial node count.
    pub nr: usize,
    /// Axial node count.
    pub nz: usize,
    /// Radial spacing, mm.
    pub dr_mm: f64,
    /// Axial spacing, mm.
    pub dz_mm: f64,
    /// Domain radius, mm.
    pub r_max_mm: f64,
    /// Lowest z, mm.
    pub z_min_mm: f64,
    /// Drive frequency, Hz.
    pub f_hz: f64,
    /// Complex pressure per node, row-major `i*nz + j` (Pa).
    pub p: Vec<Cplx>,
    /// Node classification per node, same layout.
    pub kind: Vec<NodeKind>,
}

impl Field {
    #[inline]
    fn idx(&self, i: usize, j: usize) -> usize {
        i * self.nz + j
    }

    /// Complex pressure at node `(i, j)` (Pa).
    #[inline]
    pub fn node(&self, i: usize, j: usize) -> Cplx {
        self.p[self.idx(i, j)]
    }

    /// Node classification at `(i, j)`.
    #[inline]
    pub fn kind_at(&self, i: usize, j: usize) -> NodeKind {
        self.kind[self.idx(i, j)]
    }

    /// Complex pressure at `(r, z)` in **mm**, bilinear over the grid (Pa).
    /// Solid corners read `p = 0`.
    pub fn pressure_at(&self, r_mm: f64, z_mm: f64) -> Cplx {
        let (i0, fu) = cell_index(r_mm.abs() / self.dr_mm, self.nr);
        let (j0, fw) = cell_index((z_mm - self.z_min_mm) / self.dz_mm, self.nz);
        let p = |i: usize, j: usize| self.p[i * self.nz + j];
        p(i0, j0).scale((1.0 - fu) * (1.0 - fw))
            + p(i0 + 1, j0).scale(fu * (1.0 - fw))
            + p(i0, j0 + 1).scale((1.0 - fu) * fw)
            + p(i0 + 1, j0 + 1).scale(fu * fw)
    }

    /// Sound-pressure-level-style magnitude `|p|` at `(r, z)` in mm (Pa).
    pub fn magnitude_at(&self, r_mm: f64, z_mm: f64) -> f64 {
        self.pressure_at(r_mm, z_mm).abs()
    }
}

/// Solve the driven Helmholtz field for `cavity` on an `nr × nz` grid at `f_hz`.
pub fn solve_driven(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    f_hz: f64,
    source: Source,
) -> Result<Field, SolveError> {
    if nr < 3 || nz < 3 {
        return Err(SolveError::GridTooSmall);
    }
    let r_max_mm = cavity.r_max_mm();
    let (z_min_mm, z_max_mm) = cavity.z_span_mm();
    let dr_mm = r_max_mm / (nr - 1) as f64;
    let dz_mm = (z_max_mm - z_min_mm) / (nz - 1) as f64;
    let dr = dr_mm * MM;
    let dz = dz_mm * MM;
    let r_max = r_max_mm * MM;
    let z_max = (z_max_mm - z_min_mm) * MM;

    let omega = std::f64::consts::TAU * f_hz;
    let k = omega / cavity.medium.c;
    let k2 = k * k;
    let rho = cavity.medium.rho;

    // Node coordinates (mm) and classification.
    let r_at = |i: usize| i as f64 * dr_mm;
    let z_at = |j: usize| z_min_mm + j as f64 * dz_mm;

    // Radial-cell cross-section w_i (m²) = axial face area = k²-volume weight
    // per unit z. Half cells at i = 0 and i = nr−1.
    let w = |i: usize| -> f64 {
        let rr = (((i as f64) + 0.5) * dr).min(r_max);
        let rl = (((i as f64) - 0.5) * dr).max(0.0);
        std::f64::consts::PI * (rr * rr - rl * rl)
    };
    // z-extent of row j (m). Half cells at the ends.
    let zext = |j: usize| -> f64 {
        let zr = (((j as f64) + 0.5) * dz).min(z_max);
        let zl = (((j as f64) - 0.5) * dz).max(0.0);
        zr - zl
    };
    // Radial face area at i+½ for row j (m²).
    let a_r =
        |i: usize, j: usize| -> f64 { std::f64::consts::TAU * ((i as f64) + 0.5) * dr * zext(j) };

    // Driver disk (bottom piston) radius, mm.
    let piston_radius_mm = match cavity.bottom {
        EndCondition::Piston { radius_mm } => Some(radius_mm),
        _ => None,
    };

    let n = nr * nz;
    let mut kind = vec![NodeKind::Solid; n];
    let mut any_fluid = false;
    for i in 0..nr {
        for j in 0..nz {
            let (r, z) = (r_at(i), z_at(j));
            if !cavity.contains(r, z) {
                continue;
            }
            // Fluid; possibly reassigned to an open mouth.
            let open_top = j == nz - 1 && cavity.top == EndCondition::Open;
            let open_bottom = j == 0 && cavity.bottom == EndCondition::Open;
            kind[i * nz + j] = if open_top || open_bottom {
                NodeKind::Open
            } else {
                any_fluid = true;
                NodeKind::Fluid
            };
        }
    }
    if !any_fluid {
        return Err(SolveError::NoFluid);
    }

    // Block-tridiagonal system: nb = nz slabs (constant j), block size nr.
    let nb = nz;
    let bs = nr;
    let mut diag: Vec<Vec<Cplx>> = vec![vec![Cplx::ZERO; bs * bs]; nb];
    let mut lower: Vec<Vec<Cplx>> = vec![vec![Cplx::ZERO; bs]; nb];
    let mut upper: Vec<Vec<Cplx>> = vec![vec![Cplx::ZERO; bs]; nb];
    let mut rhs = vec![Cplx::ZERO; n];
    let gidx = |i: usize, j: usize| j * bs + i; // slab-major for the solver

    let kind_at = |i: usize, j: usize| kind[i * nz + j];

    for j in 0..nz {
        for i in 0..nr {
            let g = gidx(i, j);
            match kind_at(i, j) {
                NodeKind::Solid | NodeKind::Open => {
                    // Identity row: p = 0 (known). Decoupled.
                    diag[j][i * bs + i] = Cplx::ONE;
                    rhs[g] = Cplx::ZERO;
                }
                NodeKind::Fluid => {
                    let mut d = k2 * (w(i) * zext(j)); // k²·V_ij (real)
                                                       // +r face
                    if i + 1 < nr {
                        match kind_at(i + 1, j) {
                            NodeKind::Fluid => {
                                let c = a_r(i, j) / dr;
                                diag[j][i * bs + (i + 1)] += Cplx::real(c);
                                d -= c;
                            }
                            NodeKind::Open => d -= a_r(i, j) / dr,
                            NodeKind::Solid => {}
                        }
                    }
                    // −r face (none at i = 0: axis, zero-area face)
                    if i > 0 {
                        match kind_at(i - 1, j) {
                            NodeKind::Fluid => {
                                let c = a_r(i - 1, j) / dr;
                                diag[j][i * bs + (i - 1)] += Cplx::real(c);
                                d -= c;
                            }
                            NodeKind::Open => d -= a_r(i - 1, j) / dr,
                            NodeKind::Solid => {}
                        }
                    }
                    // +z face
                    if j + 1 < nz {
                        match kind_at(i, j + 1) {
                            NodeKind::Fluid => {
                                let c = w(i) / dz;
                                upper[j][i] = Cplx::real(c);
                                d -= c;
                            }
                            NodeKind::Open => d -= w(i) / dz,
                            NodeKind::Solid => {}
                        }
                    }
                    // −z face
                    if j > 0 {
                        match kind_at(i, j - 1) {
                            NodeKind::Fluid => {
                                let c = w(i) / dz;
                                lower[j][i] = Cplx::real(c);
                                d -= c;
                            }
                            NodeKind::Open => d -= w(i) / dz,
                            NodeKind::Solid => {}
                        }
                    } else {
                        // Bottom boundary face of a j = 0 fluid node.
                        if let (Source::Piston { velocity }, Some(pr)) = (&source, piston_radius_mm)
                        {
                            if r_at(i) <= pr + 1e-9 {
                                // Neumann flux g = −jωρ·U over the axial cell
                                // area w(i): RHS += −A·g = A·jωρ·U.
                                let flux = Cplx::J.scale(omega * rho) * *velocity;
                                rhs[g] += flux.scale(w(i));
                            }
                        }
                    }
                    diag[j][i * bs + i] = Cplx::real(d);
                }
            }
        }
    }

    // Interior monopole source.
    if let Source::Monopole { r_mm, z_mm, q } = source {
        let i = (r_mm / dr_mm).round().clamp(0.0, (nr - 1) as f64) as usize;
        let j = ((z_mm - z_min_mm) / dz_mm)
            .round()
            .clamp(0.0, (nz - 1) as f64) as usize;
        if kind_at(i, j) != NodeKind::Fluid {
            return Err(SolveError::SourceNotInFluid);
        }
        // ∫(∇²+k²)p dV = −jωρ q  →  RHS += −jωρ q.
        rhs[gidx(i, j)] += -Cplx::J.scale(omega * rho) * q;
    } else if matches!(source, Source::Piston { .. }) && piston_radius_mm.is_none() {
        return Err(SolveError::NoPiston);
    }

    let sol = match solve_block_tridiag(nb, bs, &diag, &lower, &upper, &rhs) {
        Ok(x) => x,
        Err(Singular) => return Err(SolveError::Singular),
    };

    // Repack slab-major → row-major (i*nz + j) and guard against non-finite.
    let mut p = vec![Cplx::ZERO; n];
    for j in 0..nz {
        for i in 0..nr {
            let v = sol[gidx(i, j)];
            if !v.is_finite() {
                return Err(SolveError::Singular);
            }
            p[i * nz + j] = v;
        }
    }

    Ok(Field {
        nr,
        nz,
        dr_mm,
        dz_mm,
        r_max_mm,
        z_min_mm,
        f_hz,
        p,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::medium::Medium;

    #[test]
    fn cell_index_clamps_in_range() {
        for n in [3usize, 64, 4097] {
            let (i0, f) = cell_index((n - 1) as f64, n);
            assert_eq!(i0, n - 2);
            assert_eq!(f, 1.0);
            let (i0, f) = cell_index(-2.0, n);
            assert_eq!(i0, 0);
            assert_eq!(f, 0.0);
        }
    }

    #[test]
    fn closed_cylinder_solves_and_is_finite() {
        let cav = Cavity::closed_cylinder(20.0, 200.0, Medium::air(20.0));
        let f = solve_driven(
            &cav,
            9,
            81,
            200.0,
            Source::Monopole {
                r_mm: 2.0,
                z_mm: 5.0,
                q: Cplx::ONE,
            },
        )
        .unwrap();
        assert!(f.p.iter().all(|z| z.is_finite()));
        // Off-resonance the response is bounded and nonzero.
        let amp = f.magnitude_at(2.0, 100.0);
        assert!(amp.is_finite() && amp > 0.0);
    }

    #[test]
    fn source_outside_fluid_is_rejected() {
        let cav = Cavity::helmholtz_resonator(60.0, 80.0, 10.0, 20.0, Medium::air(20.0));
        // A point in the annular shoulder above the cavity is solid.
        let err = solve_driven(
            &cav,
            31,
            51,
            100.0,
            Source::Monopole {
                r_mm: 40.0,
                z_mm: 90.0,
                q: Cplx::ONE,
            },
        )
        .unwrap_err();
        assert_eq!(err, SolveError::SourceNotInFluid);
    }
}
