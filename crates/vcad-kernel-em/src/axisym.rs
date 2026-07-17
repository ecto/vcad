//! Axisymmetric magnetostatics: coils of revolution, solved fields,
//! inductance and force extraction.
//!
//! Solves for the flux function `ψ(r, z) = r·A_θ` (2πψ is the magnetic
//! flux through the circle of radius r at height z):
//!
//! ```text
//!   ∇·( ν/r · ∇ψ ) = −J_θ,       ν = 1/(μ₀·μ_r)
//! ```
//!
//! which is the azimuthal component of `∇×(ν ∇×A) = J` — the same
//! divergence-form equation FEMM solves, discretized here on the shared
//! finite-volume core ([`crate::grid`]). Sources are rectangular coil
//! cross-sections (uniform current density), materials are rectangular
//! linear-μ regions.
//!
//! Fields are sampled as the **exact curl of the bilinear ψ patch**
//! (`B_r = −ψ_z/r`, `B_z = ψ_r/r`), which is pointwise divergence-free —
//! the magnetic analog of the conservative-sampling lesson from
//! `vcad-kernel-particle`. Within the first radial cell the patch is
//! replaced by the physical parabolic profile `ψ ∝ r²`, keeping `B` finite
//! and divergence-free through the axis.
//!
//! Outputs:
//! - stored energy (both discrete forms + balance residual),
//! - flux linkage per coil; self/mutual inductance (energy and linkage
//!   routes agree **identically** for the source form, by construction),
//! - axial force on a coil via `J×B` (`F_z = 2π ∫ J_θ ∂ψ/∂z dr dz`) and
//!   independently via the Maxwell stress tensor on a closed cylinder.
//!
//! Boundary truncation: `ψ = 0` (flux excluded) or zero-flux Neumann per
//! side. A finite `ψ = 0` boundary underestimates far-reaching return
//! flux — place it far from the device or use the symmetry the problem
//! actually has. Not modeled at M0: nonlinear saturation, eddy currents,
//! hysteresis, poloidal currents (toroid windings), permanent magnets in
//! this module (planar has them).

use crate::constants::MU_0;
use crate::grid::{Bc, FvSystem, Grid2D, SolveError, SolveOptions};

/// Rectangular region of the (r, z) half-plane, mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Annulus {
    /// Inner radius, mm (≥ 0).
    pub r_inner_mm: f64,
    /// Outer radius, mm.
    pub r_outer_mm: f64,
    /// Lower axial bound, mm.
    pub z_min_mm: f64,
    /// Upper axial bound, mm.
    pub z_max_mm: f64,
}

impl Annulus {
    /// Cross-section area, m².
    pub fn area_m2(&self) -> f64 {
        (self.r_outer_mm - self.r_inner_mm) * (self.z_max_mm - self.z_min_mm) * 1e-6
    }

    /// Whether `(r_m, z_m)` (SI meters) lies inside.
    pub fn contains_m(&self, r_m: f64, z_m: f64) -> bool {
        let r = r_m * 1e3;
        let z = z_m * 1e3;
        r >= self.r_inner_mm && r <= self.r_outer_mm && z >= self.z_min_mm && z <= self.z_max_mm
    }
}

/// A coil of revolution: uniform azimuthal current density over a
/// rectangular cross-section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coil {
    /// Cross-section in the (r, z) plane.
    pub region: Annulus,
    /// Number of turns distributed over the cross-section.
    pub turns: f64,
    /// Current per turn, amperes. `J_θ = turns·current / area`.
    pub current_a: f64,
}

/// A linear magnetic material region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Region it occupies.
    pub region: Annulus,
    /// Relative permeability (constant — M0 is linear; B–H curves are M1).
    pub mu_r: f64,
}

/// An axisymmetric magnetostatic device.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisymMagnetostatics {
    /// Domain outer radius, mm.
    pub r_max_mm: f64,
    /// Domain lower z, mm.
    pub z_min_mm: f64,
    /// Domain upper z, mm.
    pub z_max_mm: f64,
    /// Coils.
    pub coils: Vec<Coil>,
    /// Material regions (later entries win where regions overlap;
    /// background is vacuum).
    pub materials: Vec<Material>,
    /// Boundary condition at `r = r_max`.
    pub bc_r_outer: Bc,
    /// Boundary condition at `z = z_min`.
    pub bc_z_low: Bc,
    /// Boundary condition at `z = z_max`.
    pub bc_z_high: Bc,
}

impl AxisymMagnetostatics {
    /// A vacuum device on the given domain with flux-excluded boundaries.
    pub fn new(r_max_mm: f64, z_min_mm: f64, z_max_mm: f64) -> Self {
        Self {
            r_max_mm,
            z_min_mm,
            z_max_mm,
            coils: Vec::new(),
            materials: Vec::new(),
            bc_r_outer: Bc::Zero,
            bc_z_low: Bc::Zero,
            bc_z_high: Bc::Zero,
        }
    }

    fn mu_r_at(&self, r_m: f64, z_m: f64) -> f64 {
        let mut mu = 1.0;
        for m in &self.materials {
            if m.region.contains_m(r_m, z_m) {
                mu = m.mu_r;
            }
        }
        mu
    }

    /// Solve on an `nr × nz` node grid.
    pub fn solve(
        &self,
        nr: usize,
        nz: usize,
        opts: &SolveOptions,
    ) -> Result<AxisymMagSolution, SolveError> {
        if nr < 3 || nz < 3 {
            return Err(SolveError::GridTooSmall);
        }
        let r_max = self.r_max_mm * 1e-3;
        let z_min = self.z_min_mm * 1e-3;
        let z_max = self.z_max_mm * 1e-3;
        let dx = r_max / (nr - 1) as f64;
        let dy = (z_max - z_min) / (nz - 1) as f64;
        let grid = Grid2D {
            nx: nr,
            ny: nz,
            dx,
            dy,
            x0: 0.0,
            y0: z_min,
            periodic_x: false,
        };
        let mut sys = FvSystem::new(grid);
        let g = sys.grid.clone();
        let two_pi = 2.0 * std::f64::consts::PI;

        // Materials live on CELLS, sampled at cell centers (a sample
        // point can never land on a region boundary — point-on-edge float
        // ties are how symmetry quietly breaks). Every face conductance
        // is the parallel sum of its two flanking half-cells.
        let nu_cell = |ci: usize, cj: usize| -> f64 {
            let rc = (ci as f64 + 0.5) * dx;
            let zc = z_min + (cj as f64 + 0.5) * dy;
            1.0 / (MU_0 * self.mu_r_at(rc, zc))
        };
        // Radial faces: G = 2π·ν·(z extent)/(r_face·dx).
        for i in 0..nr - 1 {
            let r_f = (i as f64 + 0.5) * dx;
            for j in 0..nz {
                let mut nu_ext = 0.0;
                if j > 0 {
                    nu_ext += nu_cell(i, j - 1) * 0.5 * dy;
                }
                if j < nz - 1 {
                    nu_ext += nu_cell(i, j) * 0.5 * dy;
                }
                sys.gx[g.fx(i, j)] = two_pi * nu_ext / (r_f * dx);
            }
        }
        // Axial faces: G = 2π·ν·(∫ dr/r over the column CV)/dy, the log
        // integral split at the node between the two flanking cells. The
        // i = 0 integral diverges, but the axis column is Dirichlet ψ = 0,
        // so that face is never used — its (tiny, O(dx⁴)) energy is
        // dropped.
        for i in 1..nr {
            let r_i = i as f64 * dx;
            let log_lo = (r_i / (r_i - 0.5 * dx)).ln();
            let log_hi = if i < nr - 1 {
                ((r_i + 0.5 * dx) / r_i).ln()
            } else {
                0.0
            };
            for j in 0..nz - 1 {
                let mut nu_log = nu_cell(i - 1, j) * log_lo;
                if i < nr - 1 {
                    nu_log += nu_cell(i, j) * log_hi;
                }
                sys.gy[g.fy(i, j)] = two_pi * nu_log / dy;
            }
        }

        // Axis is always ψ = 0 (regularity of A_θ).
        for j in 0..nz {
            sys.fixed[g.idx(0, j)] = true;
        }
        if self.bc_r_outer == Bc::Zero {
            for j in 0..nz {
                sys.fixed[g.idx(nr - 1, j)] = true;
            }
        }
        if self.bc_z_low == Bc::Zero {
            for i in 0..nr {
                sys.fixed[g.idx(i, 0)] = true;
            }
        }
        if self.bc_z_high == Bc::Zero {
            for i in 0..nr {
                sys.fixed[g.idx(i, nz - 1)] = true;
            }
        }

        // Coil sources: S_n = 2π·J_θ·(CV ∩ coil area), deposited by exact
        // rectangle overlap so region edges between nodes contribute
        // fractionally (no staircase in the source).
        let mut unit_sources: Vec<Vec<f64>> = Vec::with_capacity(self.coils.len());
        for coil in &self.coils {
            let mut unit = vec![0.0; nr * nz];
            let area = coil.region.area_m2();
            if area <= 0.0 {
                unit_sources.push(unit);
                continue;
            }
            let j_unit = coil.turns / area; // A/m² at 1 A drive
            let (c_rl, c_rh) = (coil.region.r_inner_mm * 1e-3, coil.region.r_outer_mm * 1e-3);
            let (c_zl, c_zh) = (coil.region.z_min_mm * 1e-3, coil.region.z_max_mm * 1e-3);
            for i in 0..nr {
                let cv_rl = (g.x(i) - 0.5 * dx).max(0.0);
                let cv_rh = (g.x(i) + 0.5 * dx).min(r_max);
                let wr = overlap(cv_rl, cv_rh, c_rl, c_rh);
                if wr == 0.0 {
                    continue;
                }
                for j in 0..nz {
                    let cv_zl = (g.y(j) - 0.5 * dy).max(z_min);
                    let cv_zh = (g.y(j) + 0.5 * dy).min(z_max);
                    let wz = overlap(cv_zl, cv_zh, c_zl, c_zh);
                    if wz > 0.0 {
                        unit[g.idx(i, j)] = two_pi * j_unit * wr * wz;
                    }
                }
            }
            for (s, u) in sys.source.iter_mut().zip(unit.iter()) {
                *s += coil.current_a * u;
            }
            unit_sources.push(unit);
        }

        let sol = sys.solve(opts)?;
        Ok(AxisymMagSolution {
            currents: self.coils.iter().map(|c| c.current_a).collect(),
            coil_regions: self.coils.iter().map(|c| c.region).collect(),
            unit_sources,
            psi: sol.u,
            sweeps: sol.sweeps,
            residual: sol.residual,
            system: sys,
        })
    }
}

#[inline]
fn overlap(a_lo: f64, a_hi: f64, b_lo: f64, b_hi: f64) -> f64 {
    (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
}

/// A converged axisymmetric magnetostatic field.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisymMagSolution {
    /// The assembled system (grid, conductances, sources, mask).
    pub system: FvSystem,
    /// Flux function ψ = r·A_θ per node, Wb/rad (2πψ = flux in Wb).
    pub psi: Vec<f64>,
    /// SOR sweeps used.
    pub sweeps: usize,
    /// Final relative residual.
    pub residual: f64,
    /// Coil currents at solve time, A.
    pub currents: Vec<f64>,
    /// Coil cross-sections (for force integration).
    pub coil_regions: Vec<Annulus>,
    /// Per-coil source vectors at 1 A drive (the linkage functionals).
    pub unit_sources: Vec<Vec<f64>>,
}

impl AxisymMagSolution {
    /// Magnetic field `(B_r, B_z)` at `(r, z)` in **meters**, tesla.
    ///
    /// Exact curl of the bilinear ψ patch (divergence-free); inside the
    /// first radial cell the physical `ψ ∝ r²` profile replaces the patch,
    /// so the field stays finite and divergence-free through the axis.
    ///
    /// Pointwise accuracy is staggered, as for any interpolant-exact
    /// field: second-order at cell centers, with an O(h/2r) offset when
    /// sampled exactly on a node (the patch gradient is the face-centered
    /// difference). Integral quantities (flux, force, stress) average
    /// this out; pointwise probes should sit at cell centers.
    pub fn b_at(&self, r_m: f64, z_m: f64) -> (f64, f64) {
        let g = &self.system.grid;
        let r = r_m.abs();
        if r < g.dx {
            // ψ ≈ ψ(dx, z)·(r/dx)²  ⇒  B_z = 2ψ(dx,z)/dx²,
            // B_r = −(r/dx²)·∂ψ/∂z(dx,z); both limits are exact for the
            // near-axis parabolic flux profile.
            let psi_1 = g.value_at(&self.psi, g.dx, z_m);
            let (_, dpsi_dz) = g.grad_at(&self.psi, g.dx, z_m);
            let bz = 2.0 * psi_1 / (g.dx * g.dx);
            let br = -(r / (g.dx * g.dx)) * dpsi_dz;
            return (br, bz);
        }
        let (dpsi_dr, dpsi_dz) = g.grad_at(&self.psi, r, z_m);
        (-dpsi_dz / r, dpsi_dr / r)
    }

    /// Stored magnetic energy: both discrete forms and their mismatch,
    /// joules. The source form equals `½·Σ_k I_k·Λ_k` identically.
    pub fn energy(&self) -> crate::grid::EnergyBalance {
        self.system.energy_balance(&self.psi)
    }

    /// Flux linkage of coil `k` in the solved field, weber-turns:
    /// `Λ_k = (N_k/A_k)·∫ 2πψ dr dz` over the coil cross-section.
    pub fn flux_linkage(&self, k: usize) -> f64 {
        self.unit_sources[k]
            .iter()
            .zip(self.psi.iter())
            .map(|(u, p)| u * p)
            .sum()
    }

    /// Self-inductance of coil `k` from its flux linkage, henries. Only
    /// meaningful when coil `k` is the sole driven coil; equals `2W/I²`
    /// (source-form W) identically.
    pub fn self_inductance(&self, k: usize) -> f64 {
        self.flux_linkage(k) / self.currents[k]
    }

    /// Axial force on coil `k` via `J×B`:
    /// `F_z = −∫ J_θ·B_r dV = Σ_n S_n·(∂ψ/∂z)_n`, newtons.
    /// Valid for coils in non-magnetic surroundings.
    pub fn axial_force_on_coil(&self, k: usize) -> f64 {
        let g = &self.system.grid;
        let i_k = self.currents[k];
        let mut f = 0.0;
        for i in 0..g.nx {
            for j in 0..g.ny {
                let s = self.unit_sources[k][g.idx(i, j)];
                if s == 0.0 {
                    continue;
                }
                let dz = if j == 0 {
                    (self.psi[g.idx(i, 1)] - self.psi[g.idx(i, 0)]) / g.dy
                } else if j == g.ny - 1 {
                    (self.psi[g.idx(i, j)] - self.psi[g.idx(i, j - 1)]) / g.dy
                } else {
                    (self.psi[g.idx(i, j + 1)] - self.psi[g.idx(i, j - 1)]) / (2.0 * g.dy)
                };
                f += i_k * s * dz;
            }
        }
        f
    }

    /// Axial force through a closed Maxwell-stress cylinder `r ≤ r_mm`,
    /// `z ∈ [z_lo_mm, z_hi_mm]` (force on everything inside), newtons.
    ///
    /// `F_z = ∮ (B_z·B_n − ½B²·n_z)/μ₀ dS`, sampled with `n` midpoint
    /// panels per edge. The surface must lie in vacuum (μ_r = 1) and clear
    /// of sources; the independent cross-check for [`Self::axial_force_on_coil`].
    pub fn axial_force_stress(&self, r_mm: f64, z_lo_mm: f64, z_hi_mm: f64, n: usize) -> f64 {
        let rs = r_mm * 1e-3;
        let (z1, z2) = (z_lo_mm * 1e-3, z_hi_mm * 1e-3);
        let two_pi = 2.0 * std::f64::consts::PI;
        let mut f = 0.0;
        // Lateral surface, n = +ê_r: T_zr = B_z·B_r/μ₀.
        let dzp = (z2 - z1) / n as f64;
        for p in 0..n {
            let z = z1 + (p as f64 + 0.5) * dzp;
            let (br, bz) = self.b_at(rs, z);
            f += bz * br / MU_0 * two_pi * rs * dzp;
        }
        // End disks, n = ±ê_z: T_zz = (B_z² − B_r²)/(2μ₀).
        let drp = rs / n as f64;
        for p in 0..n {
            let r = (p as f64 + 0.5) * drp;
            let (br_t, bz_t) = self.b_at(r, z2);
            let (br_b, bz_b) = self.b_at(r, z1);
            f += (bz_t * bz_t - br_t * br_t) / (2.0 * MU_0) * two_pi * r * drp;
            f -= (bz_b * bz_b - br_b * br_b) / (2.0 * MU_0) * two_pi * r * drp;
        }
        f
    }
}

/// Inductance matrix of the device's coils: `L[j][k]` = flux linkage of
/// coil `j` per ampere in coil `k` (henries). Symmetric to solver
/// tolerance (the discrete operator is symmetric).
pub fn inductance_matrix(
    device: &AxisymMagnetostatics,
    nr: usize,
    nz: usize,
    opts: &SolveOptions,
) -> Result<Vec<Vec<f64>>, SolveError> {
    let n = device.coils.len();
    let mut l = vec![vec![0.0; n]; n];
    for k in 0..n {
        let mut d = device.clone();
        for (c, coil) in d.coils.iter_mut().enumerate() {
            coil.current_a = if c == k { 1.0 } else { 0.0 };
        }
        let sol = d.solve(nr, nz, opts)?;
        for (j, row) in l.iter_mut().enumerate() {
            row[k] = sol.flux_linkage(j);
        }
    }
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MU_0;

    /// Infinite solenoid via Neumann boundaries: winding r ∈ [R1, R2]
    /// spanning the full height, `B_z = 0` at r_max, symmetry at both z
    /// ends. Exact target including the linear H falloff through the
    /// winding (Ampère's law, exact at any winding thickness).
    fn infinite_solenoid_expected_l_per_m(
        n_per_m: f64,
        r1: f64,
        r2: f64,
        core_r: f64,
        mu_r: f64,
    ) -> f64 {
        // Bore contribution (with optional core) + winding-region energy.
        let bore = crate::analytic::solenoid_inductance_per_m(n_per_m, r1, core_r, mu_r);
        // In the winding: H(r) = n·I·(r2−r)/(r2−r1);
        // W/ℓ per I² = ½·μ₀·n²·∫ ((r2−r)/(r2−r1))²·2πr dr.
        let t = r2 - r1;
        let mut wind = 0.0;
        let m = 400;
        for p in 0..m {
            let r = r1 + (p as f64 + 0.5) * t / m as f64;
            let h = (r2 - r) / t;
            wind += h * h * 2.0 * std::f64::consts::PI * r * (t / m as f64);
        }
        bore + MU_0 * n_per_m * n_per_m * wind
    }

    fn solenoid_device(mu_core: Option<(f64, f64)>) -> AxisymMagnetostatics {
        // 100 mm tall slab of an infinite solenoid: winding 20–22 mm,
        // 1000 A·turns total → n·I = 10 kA·t/m.
        let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 100.0);
        dev.bc_r_outer = Bc::Neumann;
        dev.bc_z_low = Bc::Neumann;
        dev.bc_z_high = Bc::Neumann;
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 22.0,
                z_min_mm: 0.0,
                z_max_mm: 100.0,
            },
            turns: 1000.0,
            current_a: 1.0,
        });
        if let Some((rc_mm, mu_r)) = mu_core {
            dev.materials.push(Material {
                region: Annulus {
                    r_inner_mm: 0.0,
                    r_outer_mm: rc_mm,
                    z_min_mm: 0.0,
                    z_max_mm: 100.0,
                },
                mu_r,
            });
        }
        dev
    }

    #[test]
    fn infinite_solenoid_inductance_is_exact() {
        let dev = solenoid_device(None);
        let sol = dev.solve(81, 11, &SolveOptions::default()).unwrap();
        let w = sol.energy();
        assert!(w.residual < 1e-6, "energy imbalance {:.2e}", w.residual);
        let l_per_m = 2.0 * w.source / (1.0 * 1.0) / 0.1; // I = 1 A, ℓ = 0.1 m
        let expect = infinite_solenoid_expected_l_per_m(10_000.0, 0.020, 0.022, 0.0, 1.0);
        let rel = (l_per_m - expect).abs() / expect;
        assert!(
            rel < 5e-3,
            "L/ℓ = {l_per_m:.6e} vs exact {expect:.6e} (rel {rel:.2e})"
        );
        // Linkage route must agree with the energy route identically.
        let l_linkage = sol.self_inductance(0) / 0.1;
        assert!(
            ((l_linkage - l_per_m) / l_per_m).abs() < 1e-12,
            "linkage {l_linkage:.6e} vs 2W/I² {l_per_m:.6e}"
        );
        // And the interior field is the textbook μ₀·n·I. Sample at a cell
        // center: the patch gradient is the face-centered difference, so
        // node-aligned samples of B_z = ψ_r/r carry an O(dx/2r) offset
        // bias (the field is piecewise, like every interpolant-exact
        // sampler); cell centers are second-order.
        let (_, bz) = sol.b_at(0.00525, 0.05);
        let b_expect = MU_0 * 10_000.0;
        assert!(
            ((bz - b_expect) / b_expect).abs() < 5e-3,
            "bore field {bz:.4e} vs {b_expect:.4e}"
        );
    }

    #[test]
    fn mu_core_scales_the_exact_inductance() {
        // Core of μ_r = 50 filling r < 15 mm: L/ℓ from Ampère's law is
        // exact — H in the bore is set by the free currents alone.
        let dev = solenoid_device(Some((15.0, 50.0)));
        let sol = dev.solve(81, 11, &SolveOptions::default()).unwrap();
        let l_per_m = 2.0 * sol.energy().source / 0.1;
        let expect = infinite_solenoid_expected_l_per_m(10_000.0, 0.020, 0.022, 0.015, 50.0);
        let rel = (l_per_m - expect).abs() / expect;
        assert!(
            rel < 1e-2,
            "L/ℓ with core = {l_per_m:.6e} vs exact {expect:.6e} (rel {rel:.2e})"
        );
    }

    #[test]
    fn sampled_b_is_divergence_free_through_the_axis() {
        // Net B flux through closed cylinders must vanish, including ones
        // crossing the axis cell — the conservative-sampling regression.
        let mut dev = AxisymMagnetostatics::new(60.0, -60.0, 60.0);
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 28.0,
                r_outer_mm: 32.0,
                z_min_mm: -3.0,
                z_max_mm: 3.0,
            },
            turns: 100.0,
            current_a: 2.0,
        });
        let sol = dev.solve(61, 121, &SolveOptions::default()).unwrap();
        let n = 400;
        for (rs, z1, z2) in [
            (0.010, -0.02, 0.015),
            (0.045, -0.05, 0.05),
            (0.020, 0.005, 0.04),
        ] {
            let mut flux = 0.0;
            let dz = (z2 - z1) / n as f64;
            for p in 0..n {
                let z = z1 + (p as f64 + 0.5) * dz;
                let (br, _) = sol.b_at(rs, z);
                flux += br * 2.0 * std::f64::consts::PI * rs * dz;
            }
            let dr = rs / n as f64;
            let mut bmax = 0.0_f64;
            for p in 0..n {
                let r = (p as f64 + 0.5) * dr;
                let (_, bz_t) = sol.b_at(r, z2);
                let (_, bz_b) = sol.b_at(r, z1);
                flux += (bz_t - bz_b) * 2.0 * std::f64::consts::PI * r * dr;
                bmax = bmax.max(bz_t.abs()).max(bz_b.abs());
            }
            let scale = bmax * std::f64::consts::PI * rs * rs;
            assert!(
                flux.abs() < 2e-3 * scale,
                "closed-surface flux {flux:.3e} vs scale {scale:.3e} at rs={rs}"
            );
        }
    }

    #[test]
    fn inductance_matrix_is_symmetric() {
        let mut dev = AxisymMagnetostatics::new(80.0, -80.0, 80.0);
        for z in [-15.0, 15.0] {
            dev.coils.push(Coil {
                region: Annulus {
                    r_inner_mm: 28.0,
                    r_outer_mm: 32.0,
                    z_min_mm: z - 2.0,
                    z_max_mm: z + 2.0,
                },
                turns: 50.0,
                current_a: 0.0,
            });
        }
        let l = inductance_matrix(&dev, 41, 81, &SolveOptions::default()).unwrap();
        assert!(l[0][0] > 0.0 && l[1][1] > 0.0);
        assert!(l[0][1] > 0.0, "coaxial same-sense coils couple positively");
        let asym = (l[0][1] - l[1][0]).abs() / l[0][1].abs();
        assert!(asym < 1e-6, "reciprocity violated: {asym:.2e}");
        // Coupling coefficient must be physical.
        let kc = l[0][1] / (l[0][0] * l[1][1]).sqrt();
        assert!(kc > 0.0 && kc < 1.0, "coupling {kc}");
    }
}
