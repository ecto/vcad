//! Axisymmetric electrostatic Laplace solver.
//!
//! Solves ∇²φ = 0 in cylindrical coordinates (r, z) with axisymmetry
//! (∂/∂θ = 0) on a uniform node grid over the chamber cross-section,
//! using successive over-relaxation. Electrodes and the chamber wall are
//! Dirichlet regions; the axis r = 0 gets the standard symmetry stencil.
//!
//! Vacuum fields only: no space charge (M0 scope — see crate docs).

use crate::device::Device;

/// Options for [`solve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// SOR over-relaxation factor in (1, 2). `0.0` selects the Chebyshev
    /// estimate `2 / (1 + sin(π / max(nr, nz)))` automatically.
    pub omega: f64,
    /// Convergence tolerance, relative to the largest electrode potential:
    /// the sweep stops when the largest node update falls below
    /// `tol × max|V|`.
    pub tol: f64,
    /// Hard cap on SOR sweeps.
    pub max_sweeps: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            omega: 0.0,
            tol: 1e-6,
            max_sweeps: 40_000,
        }
    }
}

/// Failure modes of [`solve`].
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The grid must be at least 3×3 nodes.
    GridTooSmall,
    /// SOR did not reach `tol` within `max_sweeps`.
    NotConverged {
        /// Final relative residual (largest node update / max|V|).
        residual: f64,
        /// Sweeps performed.
        sweeps: usize,
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
        }
    }
}

impl std::error::Error for SolveError {}

/// Discretized potential and field on the (r, z) grid.
///
/// Node (i, j) sits at `r = i·dr`, `z = −z_half + j·dz` with
/// `i ∈ [0, nr)`, `j ∈ [0, nz)`. Sampling accessors take SI meters.
#[derive(Debug, Clone)]
pub struct Solution {
    /// Radial node count.
    pub nr: usize,
    /// Axial node count.
    pub nz: usize,
    /// Radial node spacing, m.
    pub dr: f64,
    /// Axial node spacing, m.
    pub dz: f64,
    /// Domain radius, m.
    pub r_max: f64,
    /// Domain half-height, m.
    pub z_half: f64,
    /// Potential per node, volts (row-major: `i * nz + j`).
    pub phi: Vec<f64>,
    /// Dirichlet mask per node (true = electrode / wall).
    pub fixed: Vec<bool>,
    /// SOR sweeps actually used.
    pub sweeps: usize,
    er: Vec<f64>,
    ez: Vec<f64>,
}

impl Solution {
    #[inline]
    fn bilinear(&self, field: &[f64], r_m: f64, z_m: f64) -> f64 {
        let eps = 1e-12;
        let u = (r_m.abs() / self.dr).clamp(0.0, (self.nr - 1) as f64 - eps);
        let w = ((z_m + self.z_half) / self.dz).clamp(0.0, (self.nz - 1) as f64 - eps);
        let i0 = u.floor() as usize;
        let j0 = w.floor() as usize;
        let fu = u - i0 as f64;
        let fw = w - j0 as f64;
        let idx = |i: usize, j: usize| i * self.nz + j;
        field[idx(i0, j0)] * (1.0 - fu) * (1.0 - fw)
            + field[idx(i0 + 1, j0)] * fu * (1.0 - fw)
            + field[idx(i0, j0 + 1)] * (1.0 - fu) * fw
            + field[idx(i0 + 1, j0 + 1)] * fu * fw
    }

    /// Potential at `(r, z)` in meters, volts. Bilinear interpolation,
    /// clamped to the domain.
    pub fn potential_at(&self, r_m: f64, z_m: f64) -> f64 {
        self.bilinear(&self.phi, r_m, z_m)
    }

    /// Electric field `(E_r, E_z)` at `(r, z)` in meters, V/m.
    pub fn e_at(&self, r_m: f64, z_m: f64) -> (f64, f64) {
        (
            self.bilinear(&self.er, r_m, z_m),
            self.bilinear(&self.ez, r_m, z_m),
        )
    }
}

/// Solve the vacuum potential for `device` on an `nr × nz` node grid.
pub fn solve(
    device: &Device,
    nr: usize,
    nz: usize,
    opts: &SolveOptions,
) -> Result<Solution, SolveError> {
    if nr < 3 || nz < 3 {
        return Err(SolveError::GridTooSmall);
    }
    let r_max = device.chamber_radius_mm * 1e-3;
    let z_half = device.chamber_half_height_mm * 1e-3;
    let dr = r_max / (nr - 1) as f64;
    let dz = 2.0 * z_half / (nz - 1) as f64;
    let idx = |i: usize, j: usize| i * nz + j;

    let mut phi = vec![0.0_f64; nr * nz];
    let mut fixed = vec![false; nr * nz];

    // Chamber wall: outer radius and both end caps.
    for j in 0..nz {
        phi[idx(nr - 1, j)] = device.wall_potential_v;
        fixed[idx(nr - 1, j)] = true;
    }
    for i in 0..nr {
        phi[idx(i, 0)] = device.wall_potential_v;
        fixed[idx(i, 0)] = true;
        phi[idx(i, nz - 1)] = device.wall_potential_v;
        fixed[idx(i, nz - 1)] = true;
    }

    // Wire rings: every node inside the wire cross-section is Dirichlet.
    // The effective radius is floored at 0.75·max(dr, dz) so a thin wire is
    // always represented by at least its nearest nodes.
    for ring in &device.rings {
        let r0 = ring.ring_radius_mm * 1e-3;
        let z0 = ring.z_mm * 1e-3;
        let a = (ring.wire_radius_mm * 1e-3).max(0.75 * dr.max(dz));
        let i_lo = ((r0 - a) / dr).floor().max(0.0) as usize;
        let i_hi = (((r0 + a) / dr).ceil() as usize).min(nr - 1);
        let j_lo = (((z0 - a + z_half) / dz).floor().max(0.0)) as usize;
        let j_hi = ((((z0 + a + z_half) / dz).ceil()) as usize).min(nz - 1);
        for i in i_lo..=i_hi {
            for j in j_lo..=j_hi {
                let r = i as f64 * dr;
                let z = -z_half + j as f64 * dz;
                if (r - r0).powi(2) + (z - z0).powi(2) <= a * a {
                    phi[idx(i, j)] = ring.potential_v;
                    fixed[idx(i, j)] = true;
                }
            }
        }
    }

    let v_scale = device
        .rings
        .iter()
        .map(|r| r.potential_v.abs())
        .fold(device.wall_potential_v.abs(), f64::max)
        .max(1.0);

    let omega = if opts.omega > 0.0 {
        opts.omega
    } else {
        let n = nr.max(nz) as f64;
        2.0 / (1.0 + (std::f64::consts::PI / n).sin())
    };
    let dr2 = dr * dr;
    let dz2 = dz * dz;

    let mut residual = f64::MAX;
    let mut sweeps = 0;
    while sweeps < opts.max_sweeps {
        residual = 0.0;
        for i in 0..nr - 1 {
            for j in 1..nz - 1 {
                let id = idx(i, j);
                if fixed[id] {
                    continue;
                }
                let pz = (phi[idx(i, j + 1)] + phi[idx(i, j - 1)]) / dz2;
                let (num_r, den_r) = if i == 0 {
                    (4.0 * phi[idx(1, j)] / dr2, 4.0 / dr2)
                } else {
                    let r = i as f64 * dr;
                    let rp = r + 0.5 * dr;
                    let rm = r - 0.5 * dr;
                    (
                        (rp * phi[idx(i + 1, j)] + rm * phi[idx(i - 1, j)]) / (r * dr2),
                        (rp + rm) / (r * dr2),
                    )
                };
                let updated = (num_r + pz) / (den_r + 2.0 / dz2);
                let delta = updated - phi[id];
                phi[id] += omega * delta;
                let ad = delta.abs();
                if ad > residual {
                    residual = ad;
                }
            }
        }
        sweeps += 1;
        if residual < opts.tol * v_scale {
            break;
        }
    }
    if residual >= opts.tol * v_scale {
        return Err(SolveError::NotConverged {
            residual: residual / v_scale,
            sweeps,
        });
    }

    // E = −∇φ, central differences (one-sided at the domain edges).
    let mut er = vec![0.0_f64; nr * nz];
    let mut ez = vec![0.0_f64; nr * nz];
    for i in 0..nr {
        for j in 0..nz {
            let dphi_dr = if i == 0 {
                (phi[idx(1, j)] - phi[idx(0, j)]) / dr
            } else if i == nr - 1 {
                (phi[idx(nr - 1, j)] - phi[idx(nr - 2, j)]) / dr
            } else {
                (phi[idx(i + 1, j)] - phi[idx(i - 1, j)]) / (2.0 * dr)
            };
            let dphi_dz = if j == 0 {
                (phi[idx(i, 1)] - phi[idx(i, 0)]) / dz
            } else if j == nz - 1 {
                (phi[idx(i, nz - 1)] - phi[idx(i, nz - 2)]) / dz
            } else {
                (phi[idx(i, j + 1)] - phi[idx(i, j - 1)]) / (2.0 * dz)
            };
            er[idx(i, j)] = -dphi_dr;
            ez[idx(i, j)] = -dphi_dz;
        }
    }

    Ok(Solution {
        nr,
        nz,
        dr,
        dz,
        r_max,
        z_half,
        phi,
        fixed,
        sweeps,
        er,
        ez,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;

    fn test_device() -> Device {
        Device::shielded_two_ring(100.0, 40.0, 20.0, 3.0, -1_000.0, 0.0)
    }

    #[test]
    fn respects_the_maximum_principle() {
        let sol = solve(&test_device(), 61, 121, &SolveOptions::default()).unwrap();
        for &p in &sol.phi {
            assert!(
                (-1_000.0..=0.0).contains(&p),
                "potential outside Dirichlet bounds: {p}"
            );
        }
    }

    #[test]
    fn axis_is_a_symmetry_plane() {
        let sol = solve(&test_device(), 61, 121, &SolveOptions::default()).unwrap();
        // ∂φ/∂r ≈ 0 on the axis away from the end caps.
        for j in (10..110).step_by(10) {
            let p0 = sol.phi[j];
            let p1 = sol.phi[sol.nz + j];
            let scale = p0.abs().max(1.0);
            assert!(
                (p1 - p0).abs() / scale < 5e-2,
                "axis kink at j={j}: {p0} vs {p1}"
            );
        }
    }

    #[test]
    fn refinement_is_consistent() {
        let coarse = solve(&test_device(), 51, 101, &SolveOptions::default()).unwrap();
        let fine = solve(&test_device(), 101, 201, &SolveOptions::default()).unwrap();
        // Probe the mid-field region (well away from wire surfaces).
        for (r, z) in [(0.01, 0.0), (0.02, 0.03), (0.06, -0.02), (0.0, 0.05)] {
            let a = coarse.potential_at(r, z);
            let b = fine.potential_at(r, z);
            assert!(
                (a - b).abs() < 0.02 * 1_000.0,
                "grid dependence at ({r},{z}): coarse {a}, fine {b}"
            );
        }
    }

    #[test]
    fn cathode_deepens_the_well() {
        let sol = solve(&test_device(), 61, 121, &SolveOptions::default()).unwrap();
        // Center of the device is pulled well below ground by the rings.
        let center = sol.potential_at(0.0, 0.0);
        assert!(center < -300.0, "weak well: {center} V");
        // And the wall really is ground.
        let wall = sol.potential_at(0.099, 0.0);
        assert!(wall > -100.0, "wall not near ground: {wall} V");
    }
}
