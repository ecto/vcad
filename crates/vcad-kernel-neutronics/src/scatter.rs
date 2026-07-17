//! Elastic scattering kinematics (M1).
//!
//! M0 transported *groups* with isotropic lab angles. M1 upgrades the
//! collision physics to **exact two-body elastic kinematics for
//! isotropic-in-CM scattering**: sample the collision nuclide from its
//! share of Σ_s, sample the CM cosine μ_c uniformly, then take *both*
//! the outgoing energy and the lab angle from the same μ_c:
//!
//! - E′/E = (1 + α + (1 − α)·μ_c)/2,  α = ((A−1)/(A+1))²
//! - μ_lab = (1 + A·μ_c)/√(1 + 2A·μ_c + A²)
//!
//! This is strictly better than the originally planned P1 bias: the
//! linearly anisotropic pdf (1 + 3μ̄μ)/2 is not even a valid density for
//! hydrogen (μ̄ = 2/3 > 1/3 drives it negative), while the exact
//! kinematics *is* the distribution P1 approximates — and it carries the
//! angle–energy correlation (small-angle ⇔ small energy loss) that
//! dominates deep shield penetration. For hydrogen, μ_lab = √(E′/E) ≥ 0:
//! protons never backscatter neutrons in the lab, the classic result.
//!
//! Honesty bounds: isotropic-in-CM is exact for s-wave scattering and a
//! stated approximation at MeV energies where p-wave hardening adds
//! extra forward bias (direction: still under-forward — conservative for
//! dose is *not* guaranteed here, which is why the M0/M1 comparison is
//! quantified in the tests instead of hand-waved). The thermal group
//! keeps in-group isotropic scattering with no energy change (free-gas
//! thermal motion neglected — kinematic downscatter alone would
//! unphysically freeze thermal neutrons toward zero energy).

/// Exact elastic outcome for target mass `a_amu` and CM cosine `mu_c`:
/// returns `(energy_ratio, mu_lab)`.
pub fn elastic_outcome(a_amu: f64, mu_c: f64) -> (f64, f64) {
    let a = a_amu;
    let alpha = ((a - 1.0) / (a + 1.0)).powi(2);
    let e_ratio = 0.5 * ((1.0 + alpha) + (1.0 - alpha) * mu_c);
    let denom2 = 1.0 + 2.0 * a * mu_c + a * a;
    if denom2 < 1.0e-24 {
        // A = 1 head-on (μ_c = −1): the neutron stops; μ_lab's limit is
        // √((1+μ_c)/2) → 0. Measure-zero, handled exactly.
        return (e_ratio.max(0.0), 0.0);
    }
    let mu_lab = ((1.0 + a * mu_c) / denom2.sqrt()).clamp(-1.0, 1.0);
    (e_ratio, mu_lab)
}

/// Combine the pre-collision direction cosine `mu` (w.r.t. the geometry
/// axis) with a polar scattering cosine `mu_s` and azimuth `phi`:
/// μ′ = μ·μ_s + √(1−μ²)·√(1−μ_s²)·cos φ.
pub fn rotate_mu(mu: f64, mu_s: f64, phi: f64) -> f64 {
    let s = ((1.0 - mu * mu).max(0.0) * (1.0 - mu_s * mu_s).max(0.0)).sqrt();
    (mu * mu_s + s * phi.cos()).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn hydrogen_never_backscatters_and_correlates() {
        for k in 0..101 {
            let mu_c = -1.0 + 0.02 * k as f64;
            let (er, ml) = elastic_outcome(1.0, mu_c);
            assert!(ml >= -1.0e-12, "H lab cosine {ml} must be ≥ 0");
            // μ_lab = √(E′/E) for A = 1.
            assert!(
                (ml - er.max(0.0).sqrt()).abs() < 1.0e-9,
                "A=1 correlation broken: μ={ml}, √ratio={}",
                er.sqrt()
            );
        }
    }

    #[test]
    fn energy_ratio_bounds_and_infinite_mass_limit() {
        // Carbon: E′/E ∈ [α, 1] with α = (11/13)² = 0.716.
        let (lo, _) = elastic_outcome(12.011, -1.0);
        let (hi, _) = elastic_outcome(12.011, 1.0);
        assert!((lo - 0.7157).abs() < 1.0e-2);
        assert!((hi - 1.0).abs() < 1.0e-12);
        // Huge mass: no energy loss, lab = CM (isotropic in lab).
        for mu_c in [-0.9, -0.3, 0.4, 0.8] {
            let (er, ml) = elastic_outcome(1.0e12, mu_c);
            assert!((er - 1.0).abs() < 1.0e-9);
            assert!((ml - mu_c).abs() < 1.0e-9);
        }
    }

    #[test]
    fn mean_lab_cosine_is_two_over_three_a() {
        // ⟨μ_lab⟩ = 2/(3A) for isotropic-CM elastic scatter.
        for (a, tol) in [(1.0, 0.01), (12.011, 0.01), (207.2, 0.01)] {
            let mut rng = Rng::seeded(2026);
            let n = 200_000;
            let mut sum = 0.0;
            for _ in 0..n {
                let (_, ml) = elastic_outcome(a, rng.uniform_mu());
                sum += ml;
            }
            let mean = sum / n as f64;
            let expect = 2.0 / (3.0 * a);
            assert!(
                (mean - expect).abs() < tol,
                "A={a}: ⟨μ_lab⟩ = {mean}, expect {expect}"
            );
        }
    }

    #[test]
    fn rotation_preserves_isotropy_shape() {
        // Rotating an isotropic polar sample about any axis stays in
        // [-1, 1] and has near-zero mean when μ_s is symmetric.
        let mut rng = Rng::seeded(7);
        let mut sum = 0.0;
        let n = 100_000;
        for _ in 0..n {
            let mu = rng.uniform_mu();
            let mu_s = rng.uniform_mu();
            let phi = 2.0 * std::f64::consts::PI * rng.uniform();
            let m = rotate_mu(mu, mu_s, phi);
            assert!((-1.0..=1.0).contains(&m));
            sum += m;
        }
        assert!((sum / n as f64).abs() < 0.01);
    }
}
