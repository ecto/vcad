//! Far field from the solved currents: radiation integral, gain,
//! directivity, and the energy-balance observable.
//!
//! With time convention `e^{+jωt}` and `R ≈ r − r̂·r′` in the radiation
//! zone, the field is `E(r) = (e^{−jkr}/r)·e(θ, φ)` with the pattern
//!
//! ```text
//! e(θ,φ) = −(jkη/4π) [F − (F·r̂)r̂],   F = ∫ I(l′) t̂′ e^{+jk r̂·r′} dl′
//! ```
//!
//! Radiation intensity `U = |e|²/(2η)` (W/sr), radiated power by numerical
//! integration over the sphere (Gauss–Legendre in cos θ × uniform φ —
//! spectrally accurate for these smooth patterns), directivity
//! `4π U_max / P_rad`, gain `4π U / P_in`.
//!
//! **Energy balance is a test, not an assumption:** for a lossless (PEC)
//! wire, the power radiated through the far sphere must equal the power
//! accepted at the feed, `½ Re(V₀ I*)`. The two numbers come from
//! different integrals (far-zone phase integral vs the near-field Galerkin
//! quadratic form), so their agreement — [`radiation_efficiency`] ≈ 1 —
//! is a genuine cross-check of kernel, fill, and solve.

use crate::complex::Complex;
use crate::constants::ETA_0;
use crate::geometry::Mesh;
use crate::mom::DrivenSolution;
use crate::quad::gauss_legendre;

/// Far-field sample at one direction: the complex pattern `e(θ, φ)` in the
/// spherical basis (units: volts, since `E·r` carries V).
#[derive(Debug, Clone, Copy)]
pub struct FarFieldSample {
    /// θ̂ component of the pattern.
    pub e_theta: Complex,
    /// φ̂ component of the pattern.
    pub e_phi: Complex,
}

impl FarFieldSample {
    /// Radiation intensity `U(θ, φ)`, W/sr.
    pub fn intensity(&self) -> f64 {
        (self.e_theta.norm_sqr() + self.e_phi.norm_sqr()) / (2.0 * ETA_0)
    }
}

/// Evaluate the far-field pattern at polar angle `theta` (from +z) and
/// azimuth `phi` (from +x), radians. Z-up, matching the vcad frame.
pub fn far_field(mesh: &Mesh, sol: &DrivenSolution, theta: f64, phi: f64) -> FarFieldSample {
    let (st, ct) = (theta.sin(), theta.cos());
    let (sp, cp) = (phi.sin(), phi.cos());
    let rhat = [st * cp, st * sp, ct];
    let that = [ct * cp, ct * sp, -st];
    let phat = [-sp, cp, 0.0];

    let ends = mesh.segment_endpoint_currents(&sol.currents);
    let (gx, gw) = gauss_legendre(4);

    // F = Σ_seg û ∫ I(t) e^{+jk r̂·y(t)} dt
    let mut f = [Complex::ZERO; 3];
    for (s, &(c0, c1)) in mesh.segments.iter().zip(&ends) {
        if c0 == Complex::ZERO && c1 == Complex::ZERO {
            continue;
        }
        let phase0 = sol.k * (rhat[0] * s.p0[0] + rhat[1] * s.p0[1] + rhat[2] * s.p0[2]);
        let beta =
            sol.k * (rhat[0] * s.tangent[0] + rhat[1] * s.tangent[1] + rhat[2] * s.tangent[2]);
        let mut acc = Complex::ZERO;
        for (&x, &w) in gx.iter().zip(&gw) {
            let t = 0.5 * s.len * (x + 1.0);
            let wt = w * 0.5 * s.len;
            let ramp = t / s.len;
            let cur = c0.scale(1.0 - ramp) + c1.scale(ramp);
            acc += cur * Complex::expj(phase0 + beta * t).scale(wt);
        }
        for (fi, &u) in f.iter_mut().zip(&s.tangent) {
            *fi += acc.scale(u);
        }
    }

    let project = |basis: [f64; 3]| -> Complex {
        f[0].scale(basis[0]) + f[1].scale(basis[1]) + f[2].scale(basis[2])
    };
    // −jkη/4π prefactor.
    let pre = Complex::new(0.0, -sol.k * ETA_0 / (4.0 * std::f64::consts::PI));
    FarFieldSample {
        e_theta: pre * project(that),
        e_phi: pre * project(phat),
    }
}

/// Total radiated power by integrating `U` over the sphere:
/// Gauss–Legendre with `n_polar` nodes in cos θ × `2·n_polar` uniform
/// azimuth samples.
pub fn radiated_power(mesh: &Mesh, sol: &DrivenSolution, n_polar: usize) -> f64 {
    let (cx, cw) = gauss_legendre(n_polar);
    let n_phi = 2 * n_polar;
    let dphi = std::f64::consts::TAU / n_phi as f64;
    let mut p = 0.0;
    for (&c, &w) in cx.iter().zip(&cw) {
        let theta = c.acos();
        for i in 0..n_phi {
            let phi = dphi * i as f64;
            p += w * dphi * far_field(mesh, sol, theta, phi).intensity();
        }
    }
    p
}

/// `P_rad / P_in`: must be ≈ 1 for a lossless wire. This is the
/// energy-balance cross-check between the far-zone integral and the
/// near-field Galerkin quadratic form.
pub fn radiation_efficiency(mesh: &Mesh, sol: &DrivenSolution, n_polar: usize) -> f64 {
    radiated_power(mesh, sol, n_polar) / sol.input_power_w
}

/// Directivity toward (θ, φ): `10 log₁₀(4π U / P_rad)`, dBi.
pub fn directivity_dbi(
    mesh: &Mesh,
    sol: &DrivenSolution,
    theta: f64,
    phi: f64,
    n_polar: usize,
) -> f64 {
    let u = far_field(mesh, sol, theta, phi).intensity();
    let p_rad = radiated_power(mesh, sol, n_polar);
    10.0 * (4.0 * std::f64::consts::PI * u / p_rad).log10()
}

/// Gain toward (θ, φ): `10 log₁₀(4π U / P_in)`, dBi. Equals directivity
/// times radiation efficiency; for the lossless wires of M0 the two
/// coincide to discretization error.
pub fn gain_dbi(mesh: &Mesh, sol: &DrivenSolution, theta: f64, phi: f64) -> f64 {
    let u = far_field(mesh, sol, theta, phi).intensity();
    10.0 * (4.0 * std::f64::consts::PI * u / sol.input_power_w).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::WireGrid;
    use crate::mom::{solve_driven, SolveOptions};

    fn solved_half_wave() -> (Mesh, DrivenSolution) {
        let f = 146e6;
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 20)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let sol = solve_driven(&mesh, feed, f, &SolveOptions::default()).unwrap();
        (mesh, sol)
    }

    #[test]
    fn z_dipole_has_no_cross_polarization_and_a_broadside_peak() {
        let (mesh, sol) = solved_half_wave();
        let broadside = far_field(&mesh, &sol, std::f64::consts::FRAC_PI_2, 0.3);
        assert!(
            broadside.e_phi.abs() < 1e-9 * broadside.e_theta.abs(),
            "a z-directed dipole radiates θ-polarized only"
        );
        // Axial null: 30+ dB below broadside.
        let axial = far_field(&mesh, &sol, 1e-3, 0.0);
        assert!(axial.intensity() < 1e-3 * broadside.intensity());
        // Azimuthal symmetry.
        let a = far_field(&mesh, &sol, 1.0, 0.0).intensity();
        let b = far_field(&mesh, &sol, 1.0, 2.5).intensity();
        assert!((a - b).abs() < 1e-9 * a);
        // Mirror symmetry θ ↔ π − θ.
        let up = far_field(&mesh, &sol, 0.7, 0.0).intensity();
        let dn = far_field(&mesh, &sol, std::f64::consts::PI - 0.7, 0.0).intensity();
        assert!((up - dn).abs() < 1e-6 * up);
    }

    #[test]
    fn sphere_quadrature_is_converged() {
        let (mesh, sol) = solved_half_wave();
        let p16 = radiated_power(&mesh, &sol, 16);
        let p32 = radiated_power(&mesh, &sol, 32);
        assert!(
            (p16 - p32).abs() < 1e-6 * p32,
            "sphere integral must be spectrally converged: {p16:.6e} vs {p32:.6e}"
        );
    }
}
