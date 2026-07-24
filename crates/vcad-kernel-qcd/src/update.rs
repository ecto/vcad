//! Monte Carlo link updates, generic over the gauge group.
//!
//! [`heatbath_sweep`] draws each link from its local conditional
//! distribution — exactly for SU(2) (Kennedy–Pendleton), via one
//! Cabibbo–Marinari subgroup cycle for SU(3). [`overrelax_sweep`]
//! performs the microcanonical action-preserving reflection, which
//! moves maximally far in the group at zero action cost — interleaving
//! overrelaxation with heatbath decorrelates configurations much
//! faster per unit work. [`cool_sweep`] locally maximizes the action
//! (cooling), the classic noise filter for topological observables.
//!
//! The group-specific kernels live on [`GaugeGroup`]; this module owns
//! only the sweep order (lexicographic sites, then μ).

use crate::group::GaugeGroup;
use crate::lattice::{Lattice, ND};
use crate::rng::Rng;

/// One full heatbath sweep over every link.
pub fn heatbath_sweep<G: GaugeGroup>(lat: &mut Lattice<G>, beta: f64, rng: &mut Rng) {
    for site in 0..lat.volume() {
        for mu in 0..ND {
            let a = lat.staple(site, mu);
            let u = lat.link(site, mu);
            lat.set_link(site, mu, G::heatbath(&u, &a, beta, rng));
        }
    }
}

/// One microcanonical overrelaxation sweep.
pub fn overrelax_sweep<G: GaugeGroup>(lat: &mut Lattice<G>, rng: &mut Rng) {
    for site in 0..lat.volume() {
        for mu in 0..ND {
            let a = lat.staple(site, mu);
            let u = lat.link(site, mu);
            lat.set_link(site, mu, G::overrelax(&u, &a, rng));
        }
    }
}

/// One cooling sweep: every link moved to its local action maximum.
/// Repeated cooling drives the configuration toward a classical
/// solution (plaquette → 1 in the trivial sector), exposing
/// topological content.
pub fn cool_sweep<G: GaugeGroup>(lat: &mut Lattice<G>) {
    for site in 0..lat.volume() {
        for mu in 0..ND {
            let a = lat.staple(site, mu);
            let u = lat.link(site, mu);
            lat.set_link(site, mu, G::cool(&u, &a));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::su2::{sample_w0, Su2};

    #[test]
    fn overrelax_preserves_action() {
        let mut rng = Rng::seeded(21);
        let mut lat: Lattice<Su2> = Lattice::hot([3, 3, 3, 3], &mut rng);
        let site = 5;
        let mu = 2;
        let a = lat.staple(site, mu);
        let before = lat.link(site, mu).mul(&a).re_trace();
        let u = lat.link(site, mu);
        lat.set_link(site, mu, Su2::overrelax(&u, &a, &mut rng));
        let after = lat.link(site, mu).mul(&a).re_trace();
        assert!((before - after).abs() < 1e-10, "{before} vs {after}");
    }

    #[test]
    fn cooling_increases_plaquette_monotonically() {
        let mut rng = Rng::seeded(24);
        let mut lat: Lattice<Su2> = Lattice::hot([3, 3, 3, 3], &mut rng);
        let mut last = lat.average_plaquette();
        for _ in 0..20 {
            cool_sweep(&mut lat);
            let p = lat.average_plaquette();
            assert!(
                p >= last - 1e-12,
                "cooling decreased plaquette {last} -> {p}"
            );
            last = p;
        }
        assert!(last > 0.9, "20 coolings should approach classical: {last}");
    }

    #[test]
    fn heatbath_thermalizes_hot_and_cold_to_same_plaquette() {
        // At β = 2.0 a hot start and a cold start must converge to the
        // same ⟨P⟩ — the classic thermalization crosscheck.
        let beta = 2.0;
        let dims = [4, 4, 4, 4];
        let mut rng = Rng::seeded(22);
        let mut hot: Lattice<Su2> = Lattice::hot(dims, &mut rng);
        let mut cold: Lattice<Su2> = Lattice::cold(dims);
        for _ in 0..60 {
            heatbath_sweep(&mut hot, beta, &mut rng);
            heatbath_sweep(&mut cold, beta, &mut rng);
        }
        let avg = |lat: &mut Lattice<Su2>, rng: &mut Rng| {
            let mut s = 0.0;
            for _ in 0..30 {
                heatbath_sweep(lat, beta, rng);
                s += lat.average_plaquette();
            }
            s / 30.0
        };
        let ph = avg(&mut hot, &mut rng);
        let pc = avg(&mut cold, &mut rng);
        assert!((ph - pc).abs() < 0.02, "hot {ph} vs cold {pc}");
    }

    #[test]
    fn sample_w0_matches_analytic_mean() {
        // ⟨w₀⟩ under P ∝ √(1−w₀²)e^{αw₀} is I₂(α)/I₁(α); compare the
        // sampler against a direct numerical quadrature of the density.
        let alpha = 2.0;
        let quad = |f: &dyn Fn(f64) -> f64| {
            let n = 20_000;
            (0..n)
                .map(|i| {
                    let x = -1.0 + 2.0 * (i as f64 + 0.5) / n as f64;
                    f(x) * (2.0 / n as f64)
                })
                .sum::<f64>()
        };
        let w = |x: f64| (1.0 - x * x).sqrt() * (alpha * x).exp();
        let expect = quad(&|x| x * w(x)) / quad(&w);
        let mut rng = Rng::seeded(23);
        let n = 200_000;
        let mean = (0..n).map(|_| sample_w0(alpha, &mut rng)).sum::<f64>() / n as f64;
        assert!((mean - expect).abs() < 0.005, "mc {mean} vs quad {expect}");
    }
}
