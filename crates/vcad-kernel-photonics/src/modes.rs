//! Symmetric-slab waveguide eigenmodes via the transcendental equation.
//!
//! For a slab of core index n₁, half-width `a`, in infinite cladding n₂,
//! guided modes with propagation constant β satisfy (fundamental **even**
//! branch; u = κa, v = γa, κ² = n₁²k₀² − β², γ² = β² − n₂²k₀²):
//!
//! ```text
//! u² + v² = V²,   V = k₀·a·√(n₁² − n₂²)          (the circle)
//! v = u·tan(u)                    out-of-plane E   (crate TM)
//! v = (n₂²/n₁²)·u·tan(u)          out-of-plane H   (crate TE)
//! ```
//!
//! **Naming cross-map** (see crate docs): this crate's TM (Ez out of
//! plane) is the slab literature's **TE** mode family — continuity of Ez
//! and ∂Ez/∂y gives the unweighted `u·tan(u)`; the crate's TE (Hz out of
//! plane) is the slab literature's **TM** family — continuity of Hz and
//! (1/ε)·∂Hz/∂y brings in the ε ratio.
//!
//! On (0, min(V, π/2)) the left side of each equation increases from 0
//! and the right side `√(V² − u²)` decreases from V, so the fundamental
//! root is unique and **bisection** is exact-arithmetic robust — no
//! Newton, no derivatives, no divergence. The solver is itself a
//! deliverable: the propagation validation test measures n_eff from FDTD
//! phase and closes the loop against this solution.

use crate::sim::Polarization;

/// A solved fundamental even slab mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabMode {
    /// Effective index β/k₀.
    pub n_eff: f64,
    /// Transverse wavenumber in the core, κ.
    pub kappa: f64,
    /// Decay constant in the cladding, γ.
    pub gamma: f64,
    /// Core half-width `a`.
    pub half_width: f64,
    /// Core refractive index.
    pub n_core: f64,
    /// Cladding refractive index.
    pub n_clad: f64,
    /// Vacuum wavenumber k₀ = 2π/λ.
    pub k0: f64,
    /// Residual of the transcendental equation at the returned root.
    pub residual: f64,
}

impl SlabMode {
    /// Transverse mode profile of the out-of-plane field, peak-normalized
    /// to 1 at the core center; `y` is measured from the core center.
    ///
    /// `cos(κy)` in the core, matched exponential `cos(κa)·e^(−γ(|y|−a))`
    /// in the cladding.
    pub fn profile(&self, y: f64) -> f64 {
        let a = self.half_width;
        let ya = y.abs();
        if ya <= a {
            (self.kappa * y).cos()
        } else {
            (self.kappa * a).cos() * (-self.gamma * (ya - a)).exp()
        }
    }

    /// The normalized frequency V = k₀·a·√(n₁² − n₂²).
    pub fn v_number(&self) -> f64 {
        self.k0 * self.half_width * (self.n_core * self.n_core - self.n_clad * self.n_clad).sqrt()
    }
}

/// Mode solver failures.
#[derive(Debug, Clone, PartialEq)]
pub enum ModeError {
    /// n_core must exceed n_clad for guidance.
    NotGuiding,
    /// Non-positive wavelength or half-width.
    BadGeometry,
}

impl std::fmt::Display for ModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModeError::NotGuiding => write!(f, "core index must exceed cladding index"),
            ModeError::BadGeometry => write!(f, "wavelength and half-width must be positive"),
        }
    }
}

impl std::error::Error for ModeError {}

/// Solve the fundamental even mode of a symmetric slab by bisection.
///
/// `pol` selects the eigenvalue equation per the naming cross-map above.
/// The fundamental even mode always exists for any V > 0 (no cutoff), so
/// this never fails for a guiding geometry.
pub fn solve_slab_mode_even(
    n_core: f64,
    n_clad: f64,
    half_width: f64,
    wavelength: f64,
    pol: Polarization,
) -> Result<SlabMode, ModeError> {
    // NaN-fail-closed: NaN inputs must land in the error arms, so the
    // guards are written as "not provably valid" rather than "invalid".
    if wavelength.is_nan() || half_width.is_nan() || wavelength <= 0.0 || half_width <= 0.0 {
        return Err(ModeError::BadGeometry);
    }
    if n_core.is_nan() || n_clad.is_nan() || n_core <= n_clad || n_clad <= 0.0 {
        return Err(ModeError::NotGuiding);
    }
    let k0 = 2.0 * std::f64::consts::PI / wavelength;
    let a = half_width;
    let v = k0 * a * (n_core * n_core - n_clad * n_clad).sqrt();
    // Weight on u·tan(u): 1 for out-of-plane E, (n2/n1)² for out-of-plane H.
    let s = match pol {
        Polarization::Tm => 1.0,
        Polarization::Te => (n_clad * n_clad) / (n_core * n_core),
    };
    // f(u) = s·u·tan(u) − √(V² − u²): negative at 0⁺, positive at the top
    // of the fundamental branch; unique sign change.
    let f = |u: f64| s * u * u.tan() - (v * v - u * u).max(0.0).sqrt();
    let mut lo = 0.0;
    let mut hi = v.min(std::f64::consts::FRAC_PI_2 * (1.0 - 1e-12));
    debug_assert!(f(lo + 1e-12 * hi) < 0.0);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let u = 0.5 * (lo + hi);
    let kappa = u / a;
    let gamma = (v * v - u * u).max(0.0).sqrt() / a;
    let beta2 = n_core * n_core * k0 * k0 - kappa * kappa;
    let n_eff = beta2.sqrt() / k0;
    Ok(SlabMode {
        n_eff,
        kappa,
        gamma,
        half_width: a,
        n_core,
        n_clad,
        k0,
        residual: f(u).abs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const N_SI: f64 = 3.48;
    const N_OX: f64 = 1.44;

    #[test]
    fn fundamental_mode_solves_the_transcendental_equation() {
        let m = solve_slab_mode_even(N_SI, N_OX, 0.11, 1.55, Polarization::Tm).unwrap();
        assert!(m.residual < 1e-10, "residual {}", m.residual);
        assert!(m.n_eff > N_OX && m.n_eff < N_SI);
        // Circle constraint: u² + v² = V².
        let u = m.kappa * m.half_width;
        let v = m.gamma * m.half_width;
        let vn = m.v_number();
        assert!((u * u + v * v - vn * vn).abs() < 1e-10);
    }

    #[test]
    fn field_matching_at_the_core_boundary() {
        // The profile and its derivative must match at |y| = a for the
        // out-of-plane-E equation: −κ·sin(κa) = −γ·cos(κa).
        let m = solve_slab_mode_even(N_SI, N_OX, 0.11, 1.55, Polarization::Tm).unwrap();
        let a = m.half_width;
        let lhs = m.kappa * (m.kappa * a).sin();
        let rhs = m.gamma * (m.kappa * a).cos();
        assert!((lhs - rhs).abs() / rhs.abs() < 1e-9);
        // Profile continuity is built in; check the exponential tail decays.
        assert!(m.profile(3.0 * a) < m.profile(1.5 * a));
    }

    #[test]
    fn out_of_plane_h_mode_is_less_confined() {
        // Slab-literature TM (our TE) always has lower n_eff than
        // slab-literature TE (our TM) in the same guide.
        let te = solve_slab_mode_even(N_SI, N_OX, 0.11, 1.55, Polarization::Tm).unwrap();
        let tm = solve_slab_mode_even(N_SI, N_OX, 0.11, 1.55, Polarization::Te).unwrap();
        assert!(tm.n_eff < te.n_eff);
        assert!(tm.n_eff > N_OX);
    }

    #[test]
    fn thick_slab_limit_approaches_core_index() {
        let m = solve_slab_mode_even(N_SI, N_OX, 20.0, 1.55, Polarization::Tm).unwrap();
        // Fundamental even mode of a very thick slab: n_eff → n_core.
        assert!(m.n_eff > 3.47);
    }

    #[test]
    fn rejects_non_guiding() {
        assert_eq!(
            solve_slab_mode_even(1.0, 1.5, 0.1, 1.55, Polarization::Tm),
            Err(ModeError::NotGuiding)
        );
        assert_eq!(
            solve_slab_mode_even(2.0, 1.0, -0.1, 1.55, Polarization::Tm),
            Err(ModeError::BadGeometry)
        );
    }
}
