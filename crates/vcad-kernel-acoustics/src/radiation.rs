//! Baffled circular piston: the canonical radiation oracle.
//!
//! A rigid disk of radius `a` vibrating in an infinite baffle is the textbook
//! model of a loudspeaker cone at low-to-mid frequency, and — unlike the
//! interior cavity problem — it radiates into open space with a **closed-form**
//! on-axis pressure and a **closed-form** far-field directivity. Both come out
//! of the Rayleigh integral, which this module also evaluates numerically, so
//! the numerical radiator can be checked against the analytic one.
//!
//! On axis (Kinsler & Frey §7.4), with `e^{+jωt}` and outgoing waves
//! `e^{−jkR}`:
//!
//! ```text
//! p(z) = ρc·U·( e^{−jkz} − e^{−jk·√(z²+a²)} )
//! |p(z)| = 2ρc·U·|sin( (k/2)(√(z²+a²) − z) )|
//! ```
//!
//! Far field: `|p| ∝ |2·J₁(ka·sinθ)/(ka·sinθ)|`, the piston directivity.
//!
//! M0 scope note: this is the **open-domain** counterpart to the interior
//! field solver. Coupling a radiating boundary onto the grid (a PML) so the
//! solved cavity can itself radiate is a later milestone; here the piston is
//! analytic and stands alone.

use crate::complex::Cplx;
use crate::medium::Medium;

/// Bessel function `J₁(x)` via Abramowitz & Stegun 9.4.4 / 9.4.6
/// (absolute error < 1e-7 across the range that matters for directivity).
pub fn bessel_j1(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 3.0 {
        let t = (x / 3.0) * (x / 3.0);
        let poly = 0.5 - 0.56249985 * t + 0.21093573 * t * t - 0.03954289 * t.powi(3)
            + 0.00443319 * t.powi(4)
            - 0.00031761 * t.powi(5)
            + 0.00001109 * t.powi(6);
        x * poly
    } else {
        let u = 3.0 / ax;
        let f1 = 0.79788456 + 0.00000156 * u + 0.01659667 * u * u + 0.00017105 * u.powi(3)
            - 0.00249511 * u.powi(4)
            + 0.00113653 * u.powi(5)
            - 0.00020033 * u.powi(6);
        let theta1 = ax - 2.35619449 + 0.12499612 * u + 0.00005650 * u * u - 0.00637879 * u.powi(3)
            + 0.00074348 * u.powi(4)
            + 0.00079824 * u.powi(5)
            - 0.00029166 * u.powi(6);
        let val = f1 * theta1.cos() / ax.sqrt();
        if x < 0.0 {
            -val
        } else {
            val
        }
    }
}

/// Exact complex on-axis pressure of a baffled piston (radius `a_m`, normal
/// velocity `u`) at axial distance `z_m`, wavenumber `k`.
pub fn piston_on_axis(medium: &Medium, a_m: f64, u: Cplx, k: f64, z_m: f64) -> Cplx {
    let r = (z_m * z_m + a_m * a_m).sqrt();
    let phase = Cplx::expj(-k * z_m) - Cplx::expj(-k * r);
    u.scale(medium.impedance()) * phase
}

/// Exact on-axis magnitude `2ρc·U·|sin((k/2)(√(z²+a²)−z))|`.
pub fn piston_on_axis_magnitude(medium: &Medium, a_m: f64, u_abs: f64, k: f64, z_m: f64) -> f64 {
    let r = (z_m * z_m + a_m * a_m).sqrt();
    2.0 * medium.impedance() * u_abs * (0.5 * k * (r - z_m)).sin().abs()
}

/// Far-field directivity `|2·J₁(x)/x|`, `x = ka·sinθ`, normalized to 1 on
/// axis. The removable singularity at `x = 0` is handled.
pub fn piston_directivity(ka: f64, theta_rad: f64) -> f64 {
    let x = ka * theta_rad.sin();
    if x.abs() < 1e-9 {
        1.0
    } else {
        (2.0 * bessel_j1(x) / x).abs()
    }
}

/// Far-field on-axis pressure magnitude `ρc·U·k·a²/(2R)` — the piston's
/// low-angle radiation, for scaling the directivity.
pub fn farfield_on_axis_magnitude(medium: &Medium, a_m: f64, u_abs: f64, k: f64, r_m: f64) -> f64 {
    medium.impedance() * u_abs * k * a_m * a_m / (2.0 * r_m)
}

/// Numerically evaluate the Rayleigh integral for a baffled piston at field
/// point `(x_m, z_m)` in a meridian plane (`x` is the off-axis distance, `z`
/// the axial distance from the baffle). Midpoint rule, `n_s` radial × `n_phi`
/// azimuthal samples over the disk. This is the general radiator that the
/// closed forms above validate.
///
/// `p = (jωρ / 2π) ∫∫_S U · e^{−jkR}/R dS`.
#[allow(clippy::too_many_arguments)]
pub fn rayleigh_pressure(
    medium: &Medium,
    a_m: f64,
    u: Cplx,
    k: f64,
    x_m: f64,
    z_m: f64,
    n_s: usize,
    n_phi: usize,
) -> Cplx {
    let omega = k * medium.c;
    let coeff = Cplx::J.scale(omega * medium.rho / (2.0 * std::f64::consts::PI));
    let ds = a_m / n_s as f64;
    let dphi = std::f64::consts::TAU / n_phi as f64;
    let mut acc = Cplx::ZERO;
    for is in 0..n_s {
        let s = (is as f64 + 0.5) * ds; // ring radius
        for ip in 0..n_phi {
            let phi = (ip as f64 + 0.5) * dphi;
            let sx = s * phi.cos();
            let sy = s * phi.sin();
            // Field point at (x, 0, z); surface element at (sx, sy, 0).
            let dx = x_m - sx;
            let r = (dx * dx + sy * sy + z_m * z_m).sqrt();
            let area = s * ds * dphi; // ring-sector area element
            acc += Cplx::expj(-k * r).scale(area / r);
        }
    }
    coeff * u * acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_j1_matches_tabulated_values() {
        // Abramowitz & Stegun Table 9.1.
        assert!((bessel_j1(1.0) - 0.4400505857).abs() < 1e-6);
        assert!((bessel_j1(2.0) - 0.5767248078).abs() < 1e-6);
        // First zero of J₁ near 3.8317.
        assert!(bessel_j1(3.8317).abs() < 1e-4);
        assert!((bessel_j1(5.0) - (-0.3275791376)).abs() < 1e-5);
    }

    #[test]
    fn on_axis_magnitude_matches_the_complex_form() {
        let air = Medium::air(20.0);
        let k = air.wavenumber(2000.0);
        for &z in &[0.05, 0.1, 0.2, 0.5, 1.0] {
            let c = piston_on_axis(&air, 0.05, Cplx::ONE, k, z).abs();
            let m = piston_on_axis_magnitude(&air, 0.05, 1.0, k, z);
            assert!((c - m).abs() < 1e-9, "z={z}: {c} vs {m}");
        }
    }

    #[test]
    fn rayleigh_reproduces_the_on_axis_closed_form() {
        // The disk integral is analytically integrable on axis; the numeric
        // integrator must recover it.
        let air = Medium::air(20.0);
        let a = 0.05;
        let k = air.wavenumber(1500.0);
        for &z in &[0.1, 0.25, 0.6] {
            let num = rayleigh_pressure(&air, a, Cplx::ONE, k, 0.0, z, 120, 96).abs();
            let exact = piston_on_axis_magnitude(&air, a, 1.0, k, z);
            let rel = (num - exact).abs() / exact.max(1e-9);
            assert!(rel < 0.02, "z={z}: numeric {num} vs exact {exact} ({rel})");
        }
    }

    #[test]
    fn rayleigh_far_field_matches_the_directivity() {
        // At a large radius, the numeric integral's angular falloff must
        // track |2J₁(ka sinθ)/(ka sinθ)|.
        let air = Medium::air(20.0);
        let a = 0.05;
        let ka = 6.0;
        let k = ka / a;
        let r = 40.0; // far field: r ≫ a²/λ
        let on_axis = rayleigh_pressure(&air, a, Cplx::ONE, k, 0.0, r, 200, 160).abs();
        for &deg in &[10.0_f64, 20.0, 30.0] {
            let th = deg.to_radians();
            let (x, z) = (r * th.sin(), r * th.cos());
            let num = rayleigh_pressure(&air, a, Cplx::ONE, k, x, z, 200, 160).abs();
            let ratio = num / on_axis;
            let analytic = piston_directivity(ka, th);
            assert!(
                (ratio - analytic).abs() < 0.05,
                "θ={deg}°: numeric {ratio:.4} vs analytic {analytic:.4}"
            );
        }
    }

    #[test]
    fn directivity_is_unity_on_axis_and_falls_off() {
        assert!((piston_directivity(5.0, 0.0) - 1.0).abs() < 1e-12);
        assert!(piston_directivity(5.0, 0.5) < 1.0);
    }
}
