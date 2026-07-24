//! Monte Carlo link updates for the SU(2) Wilson action.
//!
//! [`heatbath_sweep`] draws each link fresh from its exact local
//! conditional distribution using the Kennedy–Pendleton algorithm
//! (Kennedy & Pendleton 1985; the SU(2) heatbath goes back to
//! Creutz 1980). [`overrelax_sweep`] performs the microcanonical
//! reflection `U → Ā† U† Ā†`, which preserves the local action exactly
//! while moving maximally far in the group — interleaving
//! overrelaxation with heatbath decorrelates configurations much
//! faster per unit work.
//!
//! The local weight for a link with staple sum `A = k·Ā` (with
//! `Ā ∈ SU(2)`, `k = |A|`) is `exp((β/2)Re Tr(U A))`; writing
//! `W = U Ā` this is `exp(βk·w₀)` over the Haar measure
//! `√(1−w₀²) dw₀ dΩ`, which Kennedy–Pendleton samples by rejection.

use crate::lattice::{Lattice, ND};
use crate::rng::Rng;
use crate::su2::Su2;

/// Sample `w₀ ∈ [−1,1]` from `P(w₀) ∝ √(1−w₀²)·exp(α·w₀)` (α > 0),
/// Kennedy–Pendleton rejection. Returns the accepted `w₀`.
fn sample_w0(alpha: f64, rng: &mut Rng) -> f64 {
    loop {
        let r1 = rng.uniform();
        let r2 = rng.uniform();
        let r3 = rng.uniform();
        let c = (2.0 * std::f64::consts::PI * r2).cos();
        let lambda2 = -(r1.ln() + c * c * r3.ln()) / (2.0 * alpha);
        if lambda2 > 1.0 {
            continue;
        }
        let r4 = rng.uniform();
        if r4 * r4 <= 1.0 - lambda2 {
            return 1.0 - 2.0 * lambda2;
        }
    }
}

/// Uniform point on the 2-sphere scaled to radius `r` (Marsaglia).
fn sphere(r: f64, rng: &mut Rng) -> (f64, f64, f64) {
    loop {
        let u = rng.symmetric();
        let v = rng.symmetric();
        let s = u * u + v * v;
        if s < 1.0 {
            let f = 2.0 * (1.0 - s).sqrt();
            return (r * u * f, r * v * f, r * (1.0 - 2.0 * s));
        }
    }
}

/// One heatbath update of a single link given inverse coupling `beta`.
fn heatbath_link(lat: &mut Lattice, site: usize, mu: usize, beta: f64, rng: &mut Rng) {
    let a = lat.staple(site, mu);
    let k = a.norm();
    if k < 1e-300 {
        // Degenerate staple: the conditional is Haar-uniform.
        lat.set_link(site, mu, Su2::random(rng));
        return;
    }
    let a_bar = a.normalized();
    let w0 = sample_w0(beta * k, rng);
    let r = (1.0 - w0 * w0).max(0.0).sqrt();
    let (w1, w2, w3) = sphere(r, rng);
    let w = Su2 {
        a0: w0,
        a1: w1,
        a2: w2,
        a3: w3,
    };
    lat.set_link(site, mu, w.mul(&a_bar.dagger()));
}

/// One full heatbath sweep over every link.
pub fn heatbath_sweep(lat: &mut Lattice, beta: f64, rng: &mut Rng) {
    for site in 0..lat.volume() {
        for mu in 0..ND {
            heatbath_link(lat, site, mu, beta, rng);
        }
    }
}

/// One microcanonical overrelaxation sweep: `U → Ā† U† Ā†`, which
/// leaves `Re Tr(U A)` (hence the action) exactly invariant.
pub fn overrelax_sweep(lat: &mut Lattice, rng: &mut Rng) {
    for site in 0..lat.volume() {
        for mu in 0..ND {
            let a = lat.staple(site, mu);
            if a.norm() < 1e-300 {
                lat.set_link(site, mu, Su2::random(rng));
                continue;
            }
            let a_bar = a.normalized();
            let u = lat.link(site, mu);
            lat.set_link(
                site,
                mu,
                a_bar.dagger().mul(&u.dagger()).mul(&a_bar.dagger()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrelax_preserves_action() {
        let mut rng = Rng::seeded(21);
        let mut lat = Lattice::hot([3, 3, 3, 3], &mut rng);
        // A single overrelaxation of one link preserves its local action;
        // check via the global plaquette sum for one-link updates.
        let site = 5;
        let mu = 2;
        let a = lat.staple(site, mu);
        let before = lat.link(site, mu).mul(&a).re_trace();
        let a_bar = a.normalized();
        let u = lat.link(site, mu);
        lat.set_link(
            site,
            mu,
            a_bar.dagger().mul(&u.dagger()).mul(&a_bar.dagger()),
        );
        let after = lat.link(site, mu).mul(&a).re_trace();
        assert!((before - after).abs() < 1e-10, "{before} vs {after}");
    }

    #[test]
    fn heatbath_thermalizes_hot_and_cold_to_same_plaquette() {
        // At β = 2.0 a hot start and a cold start must converge to the
        // same ⟨P⟩ — the classic thermalization crosscheck.
        let beta = 2.0;
        let dims = [4, 4, 4, 4];
        let mut rng = Rng::seeded(22);
        let mut hot = Lattice::hot(dims, &mut rng);
        let mut cold = Lattice::cold(dims);
        for _ in 0..60 {
            heatbath_sweep(&mut hot, beta, &mut rng);
            heatbath_sweep(&mut cold, beta, &mut rng);
        }
        let avg = |lat: &mut Lattice, rng: &mut Rng| {
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
