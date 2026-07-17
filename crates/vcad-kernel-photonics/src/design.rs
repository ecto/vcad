//! Density-based topology parameterization: ρ → filter → project → ε.
//!
//! The design variable is a density ρ ∈ [0, 1] per design-region cell.
//! Three differentiable stages turn it into permittivity:
//!
//! 1. **Cone filter** (radius `filter_radius_cells`): weighted average
//!    `ρ̃_i = Σ_j w_ij·ρ_j / Σ_j w_ij`, `w_ij = max(0, r − |x_i − x_j|)`.
//!    Sets the minimum feature scale — no isolated single-cell islands
//!    can survive a radius-r cone.
//! 2. **Projection** (smoothed Heaviside, threshold η, sharpness β):
//!    `ρ̂ = (tanh(βη) + tanh(β(ρ̃ − η))) / (tanh(βη) + tanh(β(1 − η)))`.
//!    β → 0 is the identity; β → ∞ binarizes. Inverse design ramps β
//!    over the optimization (the *binarization schedule*, owned by the
//!    optimizer) so early exploration is smooth and the final design is
//!    manufacturable two-phase geometry.
//! 3. **Linear interpolation**: `ε = ε_min + ρ̂·(ε_max − ε_min)`.
//!
//! The chain rule runs the stages backward with the **exact transpose**
//! of the filter (`dJ/dρ = Fᵀ·(P′(ρ̃) ⊙ dJ/dε)·(ε_max − ε_min)`), so a
//! gradient from [`crate::adjoint`] w.r.t. ε becomes a gradient w.r.t.
//! the raw densities — validated end to end against finite differences
//! in `tests/validation.rs::topology_chain_gradient_matches_fd`.

use crate::adjoint::DesignRegion;
use crate::grid::Field2;
use crate::sim::Simulation;

/// A topology design: densities plus the filter/projection/interpolation
/// parameters that realize them as permittivity.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyParam {
    /// The Ez-sample region the design covers.
    pub region: DesignRegion,
    /// Raw densities in [0, 1], region-local row-major (`ns_x × ns_y`).
    pub rho: Vec<f64>,
    /// Cone-filter radius in cells (minimum feature scale). 0 disables.
    pub filter_radius_cells: f64,
    /// Projection sharpness β (0 = identity, ramp upward to binarize).
    pub beta: f64,
    /// Projection threshold η (0.5 = symmetric).
    pub eta: f64,
    /// Permittivity at ρ̂ = 0.
    pub eps_min: f64,
    /// Permittivity at ρ̂ = 1.
    pub eps_max: f64,
}

impl TopologyParam {
    /// Uniform-density design over `region`.
    pub fn uniform(region: DesignRegion, rho0: f64, eps_min: f64, eps_max: f64) -> Self {
        assert!((0.0..=1.0).contains(&rho0));
        assert!(eps_min >= 1.0 && eps_max > eps_min);
        Self {
            region,
            rho: vec![rho0; region.len()],
            filter_radius_cells: 2.0,
            beta: 8.0,
            eta: 0.5,
            eps_min,
            eps_max,
        }
    }

    /// The filtered densities ρ̃ = F·ρ.
    pub fn filtered(&self) -> Vec<f64> {
        cone_filter(
            &self.rho,
            self.region.ns_x(),
            self.region.ns_y(),
            self.filter_radius_cells,
        )
    }

    /// Projected densities ρ̂ = P(ρ̃).
    pub fn projected(&self) -> Vec<f64> {
        self.filtered()
            .iter()
            .map(|&x| project(x, self.beta, self.eta))
            .collect()
    }

    /// The realized permittivity per region cell (`ns_x × ns_y`).
    pub fn epsilon_field(&self) -> Field2 {
        let (rx, ry) = (self.region.ns_x(), self.region.ns_y());
        let mut f = Field2::new(rx, ry);
        for (c, &p) in self.projected().iter().enumerate() {
            *f.at_mut(c / ry, c % ry) = self.eps_min + p * (self.eps_max - self.eps_min);
        }
        f
    }

    /// Stamp the realized ε into a simulation (TM ε_z samples; before the
    /// first step). The handoff at the region boundary is sharp — the
    /// design owns exactly its Ez samples.
    pub fn apply(&self, sim: &mut Simulation) {
        let eps = self.epsilon_field();
        let (rx, ry) = (self.region.ns_x(), self.region.ns_y());
        for di in 0..rx {
            for dj in 0..ry {
                sim.set_epsilon_at(self.region.i0 + di, self.region.j0 + dj, eps.at(di, dj));
            }
        }
    }

    /// The hard-thresholded twin of this design: densities snapped to
    /// {0, 1} where the *projected* field crosses ½, with the filter
    /// disabled so the realized ε is exactly two-phase. This is the
    /// geometry the GDS export ships ([`crate::gds::design_to_gds`]
    /// thresholds the same way), so **claims must be made on the
    /// binarized twin** — gray boundary cells do real optical work
    /// during optimization, and the difference (the binarization gap)
    /// is an honesty metric, not noise.
    pub fn binarized(&self) -> TopologyParam {
        let mut out = self.clone();
        out.rho = self
            .projected()
            .iter()
            .map(|&p| if p >= 0.5 { 1.0 } else { 0.0 })
            .collect();
        out.filter_radius_cells = 0.0;
        out
    }

    /// Chain a dJ/dε field (from [`crate::adjoint::objective_and_gradient`],
    /// region-local) back to dJ/dρ over the raw densities.
    pub fn chain_gradient(&self, d_j_d_eps: &Field2) -> Vec<f64> {
        let (rx, ry) = (self.region.ns_x(), self.region.ns_y());
        assert_eq!(d_j_d_eps.ns_x(), rx);
        assert_eq!(d_j_d_eps.ns_y(), ry);
        let rho_t = self.filtered();
        // dJ/dρ̃ = P′(ρ̃) ⊙ dJ/dε · (ε_max − ε_min)
        let mut g: Vec<f64> = (0..rx * ry)
            .map(|c| {
                d_j_d_eps.at(c / ry, c % ry)
                    * (self.eps_max - self.eps_min)
                    * project_derivative(rho_t[c], self.beta, self.eta)
            })
            .collect();
        // dJ/dρ = Fᵀ·dJ/dρ̃ (exact transpose of the normalized cone).
        g = cone_filter_transpose(&g, rx, ry, self.filter_radius_cells);
        g
    }
}

/// Smoothed Heaviside projection.
pub fn project(x: f64, beta: f64, eta: f64) -> f64 {
    if beta == 0.0 {
        return x;
    }
    let den = (beta * eta).tanh() + (beta * (1.0 - eta)).tanh();
    ((beta * eta).tanh() + (beta * (x - eta)).tanh()) / den
}

/// d(project)/dx.
pub fn project_derivative(x: f64, beta: f64, eta: f64) -> f64 {
    if beta == 0.0 {
        return 1.0;
    }
    let den = (beta * eta).tanh() + (beta * (1.0 - eta)).tanh();
    let c = (beta * (x - eta)).cosh();
    beta / (c * c * den)
}

fn cone_weights(radius: f64) -> Vec<(isize, isize, f64)> {
    let r = radius.max(0.0);
    let ri = r.floor() as isize;
    let mut w = Vec::new();
    for di in -ri..=ri {
        for dj in -ri..=ri {
            let d = ((di * di + dj * dj) as f64).sqrt();
            if d < r {
                w.push((di, dj, r - d));
            }
        }
    }
    if w.is_empty() {
        w.push((0, 0, 1.0));
    }
    w
}

/// Normalized cone filter: `ρ̃_i = Σ w_ij ρ_j / W_i` with border-clipped
/// support (W_i sums only in-region weights, so constants are preserved
/// everywhere including corners).
pub fn cone_filter(rho: &[f64], nx: usize, ny: usize, radius: f64) -> Vec<f64> {
    assert_eq!(rho.len(), nx * ny);
    let w = cone_weights(radius);
    let mut out = vec![0.0; nx * ny];
    for i in 0..nx {
        for j in 0..ny {
            let mut acc = 0.0;
            let mut norm = 0.0;
            for &(di, dj, wk) in &w {
                let (ii, jj) = (i as isize + di, j as isize + dj);
                if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny {
                    acc += wk * rho[ii as usize * ny + jj as usize];
                    norm += wk;
                }
            }
            out[i * ny + j] = acc / norm;
        }
    }
    out
}

/// Exact transpose of [`cone_filter`]: `(Fᵀg)_j = Σ_i g_i·w_ij / W_i`.
/// The normalization W_i belongs to the *output* cell i of the forward
/// filter, so the transpose scatters `g_i/W_i` with the same weights.
pub fn cone_filter_transpose(g: &[f64], nx: usize, ny: usize, radius: f64) -> Vec<f64> {
    assert_eq!(g.len(), nx * ny);
    let w = cone_weights(radius);
    // Recompute each output cell's normalization (border-clipped).
    let mut norm = vec![0.0; nx * ny];
    for i in 0..nx {
        for j in 0..ny {
            let mut s = 0.0;
            for &(di, dj, wk) in &w {
                let (ii, jj) = (i as isize + di, j as isize + dj);
                if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny {
                    s += wk;
                }
            }
            norm[i * ny + j] = s;
        }
    }
    let mut out = vec![0.0; nx * ny];
    for i in 0..nx {
        for j in 0..ny {
            let gi = g[i * ny + j] / norm[i * ny + j];
            for &(di, dj, wk) in &w {
                let (ii, jj) = (i as isize + di, j as isize + dj);
                if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny {
                    out[ii as usize * ny + jj as usize] += wk * gi;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        // Deterministic pseudo-random in [0, 1) — no dependencies.
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
    }

    #[test]
    fn filter_preserves_constants_including_borders() {
        let rho = vec![0.37; 12 * 9];
        let out = cone_filter(&rho, 12, 9, 2.5);
        for v in out {
            assert!((v - 0.37).abs() < 1e-14);
        }
    }

    #[test]
    fn filter_transpose_is_the_exact_adjoint() {
        // ⟨F·ρ, g⟩ must equal ⟨ρ, Fᵀ·g⟩ for arbitrary vectors.
        let (nx, ny) = (11, 7);
        let mut seed = 42u64;
        let rho: Vec<f64> = (0..nx * ny).map(|_| lcg(&mut seed)).collect();
        let g: Vec<f64> = (0..nx * ny).map(|_| lcg(&mut seed) - 0.5).collect();
        let f_rho = cone_filter(&rho, nx, ny, 2.3);
        let ft_g = cone_filter_transpose(&g, nx, ny, 2.3);
        let lhs: f64 = f_rho.iter().zip(&g).map(|(a, b)| a * b).sum();
        let rhs: f64 = rho.iter().zip(&ft_g).map(|(a, b)| a * b).sum();
        assert!(
            (lhs - rhs).abs() < 1e-12 * lhs.abs().max(1.0),
            "⟨Fρ,g⟩ = {lhs} vs ⟨ρ,Fᵀg⟩ = {rhs}"
        );
    }

    #[test]
    fn projection_limits() {
        // β = 0 is the identity.
        assert_eq!(project(0.3, 0.0, 0.5), 0.3);
        assert_eq!(project_derivative(0.3, 0.0, 0.5), 1.0);
        // Large β binarizes around η.
        assert!(project(0.3, 64.0, 0.5) < 0.02);
        assert!(project(0.7, 64.0, 0.5) > 0.98);
        // Fixed points at 0 and 1 for any β.
        assert!((project(0.0, 8.0, 0.5)).abs() < 1e-12);
        assert!((project(1.0, 8.0, 0.5) - 1.0).abs() < 1e-12);
        // Monotone.
        assert!(project(0.4, 8.0, 0.5) < project(0.6, 8.0, 0.5));
    }

    #[test]
    fn projection_derivative_matches_fd() {
        let (beta, eta) = (6.0, 0.45);
        let h = 1e-6;
        for &x in &[0.1, 0.45, 0.5, 0.9] {
            let fd = (project(x + h, beta, eta) - project(x - h, beta, eta)) / (2.0 * h);
            let an = project_derivative(x, beta, eta);
            assert!((fd - an).abs() < 1e-7, "x={x}: fd {fd} vs analytic {an}");
        }
    }

    #[test]
    fn epsilon_field_bounds_and_apply() {
        use crate::sim::Polarization;
        use crate::{GridSpec, Simulation};
        let region = DesignRegion {
            i0: 4,
            i1: 9,
            j0: 3,
            j1: 7,
        };
        let mut p = TopologyParam::uniform(region, 0.5, 2.0736, 12.1104);
        let mut seed = 7u64;
        for v in p.rho.iter_mut() {
            *v = lcg(&mut seed);
        }
        let eps = p.epsilon_field();
        for v in eps.as_slice() {
            assert!(*v >= 2.0736 - 1e-12 && *v <= 12.1104 + 1e-12);
        }
        let mut sim = Simulation::new(GridSpec::new(20, 12, 0.05), Polarization::Tm);
        p.apply(&mut sim);
        assert!((sim.epsilon().0.at(4, 3) - eps.at(0, 0)).abs() < 1e-15);
        assert!((sim.epsilon().0.at(9, 7) - eps.at(5, 4)).abs() < 1e-15);
    }
}
