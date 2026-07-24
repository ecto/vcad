//! APE link smearing (M1).
//!
//! `U_μ(x) → Proj[(1−α)·U_μ(x) + (α/n)·Σ_staple paths]` — a gauge-
//! covariant low-pass filter. Smearing spatial links (only) before
//! measuring Wilson loops suppresses the ultraviolet fluctuations that
//! bury the area-law signal, without touching the temporal transporters
//! that carry the physics of the static potential (APE Collaboration,
//! Albanese et al. 1987).

use crate::group::GaugeGroup;
use crate::lattice::{Lattice, ND, TIME_DIR};

/// One APE smearing pass over spatial links, using spatial staples
/// only. `alpha ∈ (0,1)` is the smearing weight (0.5 is customary).
pub fn ape_smear_spatial<G: GaugeGroup>(lat: &Lattice<G>, alpha: f64) -> Lattice<G> {
    let mut out = lat.clone();
    for site in 0..lat.volume() {
        let c = lat.coords(site);
        for mu in 0..ND {
            if mu == TIME_DIR {
                continue;
            }
            // Spatial staple sum (the alternative paths x → x+μ̂ are the
            // daggered staples: staple() is oriented for the action).
            let mut acc = G::zero();
            let mut n = 0usize;
            let c_up_mu = lat.shift(c, mu, true);
            for nu in 0..ND {
                if nu == mu || nu == TIME_DIR {
                    continue;
                }
                let u1 = lat.link(lat.site(c_up_mu), nu);
                let u2 = lat.link(lat.site(lat.shift(c, nu, true)), mu);
                let u3 = lat.link(site, nu);
                acc = acc.add(&u1.mul(&u2.dagger()).mul(&u3.dagger()));
                let c_dn_nu = lat.shift(c, nu, false);
                let d1 = lat.link(lat.site(lat.shift(c_up_mu, nu, false)), nu);
                let d2 = lat.link(lat.site(c_dn_nu), mu);
                let d3 = lat.link(lat.site(c_dn_nu), nu);
                acc = acc.add(&d1.dagger().mul(&d2.dagger()).mul(&d3));
                n += 2;
            }
            let fuzz = lat
                .link(site, mu)
                .scale(1.0 - alpha)
                .add(&acc.dagger().scale(alpha / n as f64));
            out.set_link(site, mu, fuzz.reunitarize());
        }
    }
    out
}

/// `iters` APE passes.
pub fn ape_smear_spatial_n<G: GaugeGroup>(
    lat: &Lattice<G>,
    alpha: f64,
    iters: usize,
) -> Lattice<G> {
    let mut l = lat.clone();
    for _ in 0..iters {
        l = ape_smear_spatial(&l, alpha);
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use crate::su2::Su2;
    use crate::update::heatbath_sweep;

    #[test]
    fn smearing_preserves_cold_lattice() {
        let l: Lattice<Su2> = Lattice::cold([3, 3, 3, 3]);
        let s = ape_smear_spatial_n(&l, 0.5, 3);
        assert!((s.average_plaquette() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn smearing_smooths_spatial_plaquettes() {
        // The spatial-spatial plaquette must rise under smearing (the
        // whole point of the filter).
        let mut rng = Rng::seeded(71);
        let mut lat: Lattice<Su2> = Lattice::cold([4, 4, 4, 4]);
        for _ in 0..30 {
            heatbath_sweep(&mut lat, 2.2, &mut rng);
        }
        let spatial_plaq = |l: &Lattice<Su2>| {
            let mut s = 0.0;
            let mut n = 0;
            for site in 0..l.volume() {
                for mu in 0..3 {
                    for nu in (mu + 1)..3 {
                        s += l.plaquette(site, mu, nu).norm_trace();
                        n += 1;
                    }
                }
            }
            s / n as f64
        };
        let before = spatial_plaq(&lat);
        let after = spatial_plaq(&ape_smear_spatial_n(&lat, 0.5, 3));
        assert!(after > before + 0.05, "smearing {before} -> {after}");
    }
}
