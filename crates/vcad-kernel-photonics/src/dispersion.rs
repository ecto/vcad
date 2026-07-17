//! The solver's own error model: the discrete FDTD dispersion relation.
//!
//! On the 2D Yee grid the numerical plane wave `exp(i(k·x − ωt))`
//! satisfies (Taflove & Hagness 3rd ed., ch. 4; square cells Δ, c = 1)
//!
//! ```text
//! (1/dt)²·sin²(ω·dt/2) = (1/Δ)²·[sin²(kx·Δ/2) + sin²(ky·Δ/2)]
//! ```
//!
//! For on-axis propagation this inverts in closed form, giving the exact
//! wavenumber the *grid* will exhibit at a given ω. Validation tests
//! measure propagation phase and compare against **this** relation — the
//! measured k must match the discrete relation far better than it matches
//! the continuum k = ω, which proves the solver understands its own
//! discretization error instead of hiding it in a loose tolerance.

/// On-axis numerical wavenumber k(ω) on a grid of pitch `delta` with time
/// step `dt`. Returns `None` past the grid cutoff (where the relation has
/// no real solution and waves go evanescent).
pub fn fdtd_wavenumber(omega: f64, delta: f64, dt: f64) -> Option<f64> {
    fdtd_wavenumber_in_medium(omega, 1.0, delta, dt)
}

/// On-axis numerical wavenumber in a uniform dielectric: the discrete
/// relation gains the factor √ε,
/// `sin(k·Δ/2) = √ε·(Δ/dt)·sin(ω·dt/2)`.
pub fn fdtd_wavenumber_in_medium(omega: f64, eps: f64, delta: f64, dt: f64) -> Option<f64> {
    assert!(omega > 0.0 && delta > 0.0 && dt > 0.0 && eps >= 1.0);
    let s = eps.sqrt() * (delta / dt) * (0.5 * omega * dt).sin();
    if s.abs() > 1.0 {
        return None;
    }
    Some(2.0 / delta * s.asin())
}

/// On-axis numerical phase velocity ω/k(ω) (< 1 for stable steps on a
/// finite grid; → 1 as Δ → 0).
pub fn fdtd_phase_velocity(omega: f64, delta: f64, dt: f64) -> Option<f64> {
    fdtd_wavenumber(omega, delta, dt).map(|k| omega / k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_waves_are_slow_and_converge_to_c() {
        // 20 cells/λ, Courant 0.5 (dt = 0.5·Δ/√2).
        let lambda = 1.0;
        let omega = 2.0 * std::f64::consts::PI / lambda;
        let delta = lambda / 20.0;
        let dt = 0.5 * delta / 2f64.sqrt();
        let vp = fdtd_phase_velocity(omega, delta, dt).unwrap();
        assert!(vp < 1.0);
        // Textbook scale: on-axis error ≈ (kΔ)²(1 − S₁d²)/24 ≈ 0.36 %.
        assert!(
            (1.0 - vp) > 2e-3 && (1.0 - vp) < 5e-3,
            "1 - vp = {}",
            1.0 - vp
        );

        // Refine 8×: error shrinks ~64×.
        let vp_fine = fdtd_phase_velocity(omega, delta / 8.0, dt / 8.0).unwrap();
        assert!((1.0 - vp_fine) < (1.0 - vp) / 50.0);
    }

    #[test]
    fn cutoff_returns_none() {
        // ω so large that sin(kΔ/2) would need to exceed 1.
        let delta = 0.1;
        let dt = 0.5 * delta / 2f64.sqrt();
        assert!(fdtd_wavenumber(2.0 / delta * 1.2 / 0.35, delta, dt).is_none());
    }

    #[test]
    fn magic_time_step_in_1d_is_exact() {
        // With dt = Δ (the 1D magic step; unstable in 2D but fine as a
        // relation check): sin(ωΔ/2)·(Δ/Δ) → k = ω exactly.
        let omega = 1.3;
        let delta = 0.02;
        let k = fdtd_wavenumber(omega, delta, delta).unwrap();
        assert!((k - omega).abs() < 1e-12);
    }
}
