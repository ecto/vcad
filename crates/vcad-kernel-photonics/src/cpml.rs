//! Convolutional PML (CPML) absorbing boundaries.
//!
//! Roden–Gedney complex-frequency-shifted PML via recursive convolution
//! (Taflove & Hagness 3rd ed., §7.9): inside a PML slab of thickness `t`
//! cells, every spatial derivative ∂F/∂u in the update equations becomes
//! `(1/κ_u)·∂F/∂u + ψ`, with the memory variable updated each step as
//!
//! ```text
//! ψ ← b·ψ + a·(∂F/∂u),
//! b = exp(−(σ/κ + α)·dt),        a = σ·(b − 1) / (κ·(σ + κ·α)),
//! ```
//!
//! and graded profiles over normalized depth ρ = d/t ∈ (0, 1]:
//!
//! ```text
//! σ(ρ) = σ_max·ρ^m,   σ_max = sigma_scale·(m+1)/Δ   (η₀ = 1),
//! κ(ρ) = 1 + (κ_max − 1)·ρ^m,
//! α(ρ) = α_max·(1 − ρ)           (CFS pole, max at the interface).
//! ```
//!
//! Coefficients are evaluated at each staggered sample's true depth (Yee
//! half-cell offsets included). Outside the PML, `b = 1, a = 0, κ = 1`, so
//! ψ stays identically zero and the loops stay branchless. The measured
//! reflection floor is a test (`tests/validation.rs`), not a promise.

use crate::grid::Field2;

/// CPML configuration: per-side thicknesses plus the grading profile.
#[derive(Debug, Clone, PartialEq)]
pub struct CpmlSpec {
    /// PML thickness in cells on the low-x side (0 disables).
    pub x_lo: usize,
    /// PML thickness in cells on the high-x side.
    pub x_hi: usize,
    /// PML thickness in cells on the low-y side.
    pub y_lo: usize,
    /// PML thickness in cells on the high-y side.
    pub y_hi: usize,
    /// Polynomial grading order `m` (3–4 typical).
    pub m: f64,
    /// σ_max as a multiple of the textbook optimum `(m+1)/Δ` (0.8 typical).
    pub sigma_scale: f64,
    /// Maximum coordinate-stretch κ (1 = no stretching).
    pub kappa_max: f64,
    /// Maximum CFS pole frequency α (0 = ordinary PML). Units of 1/time.
    pub alpha_max: f64,
}

impl CpmlSpec {
    /// Uniform thickness on all four sides, default grading.
    pub fn uniform(thickness: usize) -> Self {
        Self {
            x_lo: thickness,
            x_hi: thickness,
            y_lo: thickness,
            y_hi: thickness,
            ..Self::default()
        }
    }

    /// PML on the x sides only (y walls stay bare PEC/PMC).
    pub fn x_only(thickness: usize) -> Self {
        Self {
            x_lo: thickness,
            x_hi: thickness,
            y_lo: 0,
            y_hi: 0,
            ..Self::default()
        }
    }

    /// No PML anywhere (bare walls).
    pub fn none() -> Self {
        Self {
            x_lo: 0,
            x_hi: 0,
            y_lo: 0,
            y_hi: 0,
            ..Self::default()
        }
    }
}

impl Default for CpmlSpec {
    fn default() -> Self {
        Self {
            x_lo: 12,
            x_hi: 12,
            y_lo: 12,
            y_hi: 12,
            m: 3.0,
            sigma_scale: 0.8,
            kappa_max: 1.0,
            alpha_max: 0.0,
        }
    }
}

/// Per-sample CPML update coefficients along one axis, for one staggering.
///
/// `b`, `a`, and `inv_kappa` are indexed by the sample's integer index
/// along the axis; the sample's actual coordinate (integer or half-integer)
/// is baked in at construction.
#[derive(Debug, Clone)]
pub struct AxisCoeffs {
    /// ψ recursion decay factor per sample.
    pub b: Vec<f64>,
    /// ψ recursion source factor per sample.
    pub a: Vec<f64>,
    /// 1/κ per sample.
    pub inv_kappa: Vec<f64>,
}

impl AxisCoeffs {
    /// Identity coefficients (no PML) for `n` samples.
    pub fn identity(n: usize) -> Self {
        Self {
            b: vec![1.0; n],
            a: vec![0.0; n],
            inv_kappa: vec![1.0; n],
        }
    }

    /// Build coefficients for `n_samples` positions along an axis of
    /// `n_cells` cells: sample `i` sits at coordinate `i + offset` cells
    /// (offset 0.0 for integer nodes, 0.5 for half-lattice samples).
    #[allow(clippy::too_many_arguments)] // one call site, all scalars distinct
    pub fn build(
        n_samples: usize,
        n_cells: usize,
        offset: f64,
        t_lo: usize,
        t_hi: usize,
        spec: &CpmlSpec,
        delta: f64,
        dt: f64,
    ) -> Self {
        let sigma_max = spec.sigma_scale * (spec.m + 1.0) / delta;
        let mut c = Self::identity(n_samples);
        for i in 0..n_samples {
            let u = i as f64 + offset;
            // Normalized depth into whichever PML slab (0 outside both).
            let rho_lo = if t_lo > 0 {
                (t_lo as f64 - u) / t_lo as f64
            } else {
                0.0
            };
            let rho_hi = if t_hi > 0 {
                (u - (n_cells - t_hi) as f64) / t_hi as f64
            } else {
                0.0
            };
            let rho = rho_lo.max(rho_hi).clamp(0.0, 1.0);
            if rho <= 0.0 {
                continue;
            }
            let sigma = sigma_max * rho.powf(spec.m);
            let kappa = 1.0 + (spec.kappa_max - 1.0) * rho.powf(spec.m);
            let alpha = spec.alpha_max * (1.0 - rho);
            let b = (-(sigma / kappa + alpha) * dt).exp();
            let a = if sigma == 0.0 {
                0.0
            } else {
                sigma * (b - 1.0) / (kappa * (sigma + kappa * alpha))
            };
            c.b[i] = b;
            c.a[i] = a;
            c.inv_kappa[i] = 1.0 / kappa;
        }
        c
    }
}

/// The ψ memory fields for one polarization's four modified derivatives.
///
/// TM: `psi_a` = ψ for ∂Hy/∂x in the Ez update (sized like Ez),
/// `psi_b` = ψ for ∂Hx/∂y in the Ez update (sized like Ez),
/// `psi_c` = ψ for ∂Ez/∂y in the Hx update (sized like Hx),
/// `psi_d` = ψ for ∂Ez/∂x in the Hy update (sized like Hy).
///
/// TE: `psi_a` = ψ for ∂Hz/∂y in the Ex update (sized like Ex),
/// `psi_b` = ψ for ∂Hz/∂x in the Ey update (sized like Ey),
/// `psi_c` = ψ for ∂Ex/∂y in the Hz update (sized like Hz),
/// `psi_d` = ψ for ∂Ey/∂x in the Hz update (sized like Hz).
#[derive(Debug, Clone)]
pub struct PsiFields {
    /// See type docs.
    pub psi_a: Field2,
    /// See type docs.
    pub psi_b: Field2,
    /// See type docs.
    pub psi_c: Field2,
    /// See type docs.
    pub psi_d: Field2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_outside_pml() {
        let spec = CpmlSpec::uniform(8);
        let c = AxisCoeffs::build(101, 100, 0.0, 8, 8, &spec, 0.05, 0.02);
        // Interior samples untouched.
        assert_eq!(c.b[50], 1.0);
        assert_eq!(c.a[50], 0.0);
        assert_eq!(c.inv_kappa[50], 1.0);
        // Wall samples maximally damped (ρ = 1: σ = σ_max, κ = 1, α = 0
        // ⇒ b = exp(−σ_max·dt)), symmetric across the domain.
        let sigma_max = 0.8 * 4.0 / 0.05;
        assert!((c.b[0] - (-sigma_max * 0.02f64).exp()).abs() < 1e-14);
        assert!((c.b[0] - c.b[100]).abs() < 1e-15);
        assert!(c.a[0] < 0.0);
    }

    #[test]
    fn grading_is_monotone_into_the_slab() {
        let spec = CpmlSpec::uniform(10);
        let c = AxisCoeffs::build(61, 60, 0.5, 10, 10, &spec, 0.1, 0.03);
        for i in 0..10 {
            assert!(
                c.b[i] <= c.b[i + 1] + 1e-15,
                "b must decay toward the wall: b[{i}]={} b[{}]={}",
                c.b[i],
                i + 1,
                c.b[i + 1]
            );
        }
    }

    #[test]
    fn zero_thickness_is_fully_identity() {
        let spec = CpmlSpec::none();
        let c = AxisCoeffs::build(41, 40, 0.0, 0, 0, &spec, 0.1, 0.03);
        assert!(c.b.iter().all(|&b| b == 1.0));
        assert!(c.a.iter().all(|&a| a == 0.0));
        assert!(c.inv_kappa.iter().all(|&k| k == 1.0));
    }
}
