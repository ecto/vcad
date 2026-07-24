//! Link variables on a periodic 4D hypercubic lattice, generic over the
//! gauge group, plus the observables built from them: staples,
//! plaquettes, planar Wilson loops, and Polyakov loops.
//!
//! Storage is a flat `Vec<G>` of length `4·V` indexed `4·site + μ`,
//! with sites in row-major order over `dims = [n₀, n₁, n₂, n₃]`.
//! Direction 3 is time by convention (Polyakov loops wind μ = 3).
//! Conventions: the plaquette is
//! `U_p = U_μ(x) U_ν(x+μ̂) U_μ†(x+ν̂) U_ν†(x)` and the normalized
//! observable is `P = (1/N)⟨Re Tr U_p⟩` averaged over all sites and the
//! 6 μ<ν planes. The staple sum `A_μ(x)` is defined so the local Wilson
//! action weight for the link is `exp((β/N)·Re Tr(U_μ(x)·A_μ(x)))`.

use crate::group::GaugeGroup;
use crate::rng::Rng;

/// Number of spacetime dimensions (fixed: 4).
pub const ND: usize = 4;

/// The time direction (Polyakov loops wind this axis).
pub const TIME_DIR: usize = 3;

/// A periodic 4D lattice of gauge-group link variables.
#[derive(Debug, Clone)]
pub struct Lattice<G: GaugeGroup> {
    /// Extent in each direction.
    pub dims: [usize; ND],
    links: Vec<G>,
}

impl<G: GaugeGroup> Lattice<G> {
    /// Cold start: all links at the identity. Panics if any extent < 2
    /// (an extent-1 direction makes a plaquette wrap onto its own link).
    pub fn cold(dims: [usize; ND]) -> Lattice<G> {
        assert!(dims.iter().all(|&n| n >= 2), "lattice extents must be >= 2");
        let volume: usize = dims.iter().product();
        Lattice {
            dims,
            links: vec![G::identity(); ND * volume],
        }
    }

    /// Hot start: random links.
    pub fn hot(dims: [usize; ND], rng: &mut Rng) -> Lattice<G> {
        let mut l = Lattice::cold(dims);
        for u in l.links.iter_mut() {
            *u = G::random(rng);
        }
        l
    }

    /// Number of sites.
    pub fn volume(&self) -> usize {
        self.dims.iter().product()
    }

    /// Number of links.
    pub fn n_links(&self) -> usize {
        self.links.len()
    }

    /// Decompose a flat site index into coordinates.
    pub(crate) fn coords(&self, site: usize) -> [usize; ND] {
        let mut c = [0usize; ND];
        let mut s = site;
        for mu in (0..ND).rev() {
            c[mu] = s % self.dims[mu];
            s /= self.dims[mu];
        }
        c
    }

    /// Flatten coordinates into a site index.
    pub(crate) fn site(&self, c: [usize; ND]) -> usize {
        c.iter()
            .zip(self.dims.iter())
            .fold(0usize, |s, (&ci, &ni)| s * ni + ci)
    }

    /// Neighbor coordinates one step in `±mu` (periodic).
    pub(crate) fn shift(&self, c: [usize; ND], mu: usize, forward: bool) -> [usize; ND] {
        let mut c = c;
        let n = self.dims[mu];
        c[mu] = if forward {
            (c[mu] + 1) % n
        } else {
            (c[mu] + n - 1) % n
        };
        c
    }

    /// Shift `n` steps forward in `mu`.
    pub(crate) fn shift_n(&self, mut c: [usize; ND], mu: usize, n: usize) -> [usize; ND] {
        for _ in 0..n {
            c = self.shift(c, mu, true);
        }
        c
    }

    /// Link `U_μ(x)` by site index.
    pub fn link(&self, site: usize, mu: usize) -> G {
        self.links[ND * site + mu]
    }

    /// Overwrite link `U_μ(x)` (reunitarized against float drift).
    pub fn set_link(&mut self, site: usize, mu: usize, u: G) {
        self.links[ND * site + mu] = u.reunitarize();
    }

    /// Staple sum `A_μ(x)`: for every plane (μ,ν≠μ), the up staple
    /// `U_ν(x+μ̂) U_μ†(x+ν̂) U_ν†(x)` plus the down staple
    /// `U_ν†(x+μ̂−ν̂) U_μ†(x−ν̂) U_ν(x−ν̂)`, summed componentwise.
    /// `Re Tr(U_μ(x)·A_μ(x))` is then twice the sum of the 6 plaquette
    /// traces containing the link.
    pub fn staple(&self, site: usize, mu: usize) -> G {
        let c = self.coords(site);
        let c_up_mu = self.shift(c, mu, true);
        let mut acc = G::zero();
        for nu in 0..ND {
            if nu == mu {
                continue;
            }
            // Up staple.
            let u1 = self.link(self.site(c_up_mu), nu);
            let u2 = self.link(self.site(self.shift(c, nu, true)), mu);
            let u3 = self.link(site, nu);
            acc = acc.add(&u1.mul(&u2.dagger()).mul(&u3.dagger()));
            // Down staple.
            let c_dn_nu = self.shift(c, nu, false);
            let d1 = self.link(self.site(self.shift(c_up_mu, nu, false)), nu);
            let d2 = self.link(self.site(c_dn_nu), mu);
            let d3 = self.link(self.site(c_dn_nu), nu);
            acc = acc.add(&d1.dagger().mul(&d2.dagger()).mul(&d3));
        }
        acc
    }

    /// The plaquette element `U_μν(x)` (path-ordered, counterclockwise).
    pub fn plaquette(&self, site: usize, mu: usize, nu: usize) -> G {
        let c = self.coords(site);
        let u1 = self.link(site, mu);
        let u2 = self.link(self.site(self.shift(c, mu, true)), nu);
        let u3 = self.link(self.site(self.shift(c, nu, true)), mu);
        let u4 = self.link(site, nu);
        u1.mul(&u2).mul(&u3.dagger()).mul(&u4.dagger())
    }

    /// Average plaquette `P = (1/N)⟨Re Tr U_p⟩` over all sites and the
    /// 6 μ<ν planes. 1 on a cold lattice, → 0 at strong coupling.
    pub fn average_plaquette(&self) -> f64 {
        let volume = self.volume();
        let mut sum = 0.0;
        for site in 0..volume {
            for mu in 0..ND {
                for nu in (mu + 1)..ND {
                    sum += self.plaquette(site, mu, nu).norm_trace();
                }
            }
        }
        sum / (volume * ND * (ND - 1) / 2) as f64
    }

    /// Path-ordered product along `r` links in `+mu` from `c`.
    pub(crate) fn line(&self, mut c: [usize; ND], mu: usize, r: usize) -> (G, [usize; ND]) {
        let mut u = G::identity();
        for _ in 0..r {
            u = u.mul(&self.link(self.site(c), mu));
            c = self.shift(c, mu, true);
        }
        (u, c)
    }

    /// Average planar Wilson loop `W(r,t) = (1/N)⟨Re Tr ∏ U⟩` over all
    /// sites and all ordered plane pairs (μ extent `r`, ν extent `t`).
    /// `wilson_loop(1,1)` coincides with [`Lattice::average_plaquette`].
    pub fn wilson_loop(&self, r: usize, t: usize) -> f64 {
        assert!(r >= 1 && t >= 1, "loop extents must be >= 1");
        let volume = self.volume();
        let mut sum = 0.0;
        let mut count = 0usize;
        for site in 0..volume {
            let c = self.coords(site);
            for mu in 0..ND {
                for nu in 0..ND {
                    if nu == mu {
                        continue;
                    }
                    let (bottom, c_r) = self.line(c, mu, r);
                    let (right, _) = self.line(c_r, nu, t);
                    let (top, _) = self.line(self.shift_n(c, nu, t), mu, r);
                    let (left, _) = self.line(c, nu, t);
                    let w = bottom.mul(&right).mul(&top.dagger()).mul(&left.dagger());
                    sum += w.norm_trace();
                    count += 1;
                }
            }
        }
        sum / count as f64
    }

    /// Average planar Wilson loop restricted to spatial×temporal
    /// planes: `r` along a spatial axis, `t` along [`TIME_DIR`]. This
    /// is the loop the static potential is extracted from — use it on
    /// spatially smeared configurations, where the temporal
    /// transporters are untouched.
    pub fn wilson_loop_temporal(&self, r: usize, t: usize) -> f64 {
        assert!(r >= 1 && t >= 1, "loop extents must be >= 1");
        let volume = self.volume();
        let mut sum = 0.0;
        let mut count = 0usize;
        for site in 0..volume {
            let c = self.coords(site);
            for mu in 0..ND {
                if mu == TIME_DIR {
                    continue;
                }
                let nu = TIME_DIR;
                let (bottom, c_r) = self.line(c, mu, r);
                let (right, _) = self.line(c_r, nu, t);
                let (top, _) = self.line(self.shift_n(c, nu, t), mu, r);
                let (left, _) = self.line(c, nu, t);
                let w = bottom.mul(&right).mul(&top.dagger()).mul(&left.dagger());
                sum += w.norm_trace();
                count += 1;
            }
        }
        sum / count as f64
    }

    /// Polyakov loop at spatial coordinates `c` (the `c[TIME_DIR]`
    /// entry is ignored): `(1/N)Re Tr ∏_t U_t(x⃗,t)`.
    pub fn polyakov(&self, c: [usize; ND]) -> f64 {
        let mut c0 = c;
        c0[TIME_DIR] = 0;
        let (u, _) = self.line(c0, TIME_DIR, self.dims[TIME_DIR]);
        u.norm_trace()
    }

    /// Volume-averaged Polyakov loop `⟨L⟩` over all spatial sites (real
    /// part; the deconfinement order parameter is its magnitude).
    pub fn average_polyakov(&self) -> f64 {
        let mut sum = 0.0;
        let mut count = 0usize;
        for i in 0..self.dims[0] {
            for j in 0..self.dims[1] {
                for k in 0..self.dims[2] {
                    let c = [i, j, k, 0];
                    sum += self.polyakov(c);
                    count += 1;
                }
            }
        }
        sum / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::su2::Su2;

    #[test]
    fn cold_lattice_observables() {
        let l: Lattice<Su2> = Lattice::cold([3, 3, 3, 3]);
        assert!((l.average_plaquette() - 1.0).abs() < 1e-12);
        assert!((l.wilson_loop(2, 2) - 1.0).abs() < 1e-12);
        assert!((l.average_polyakov() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn wilson_1x1_equals_plaquette() {
        let mut rng = Rng::seeded(11);
        let l: Lattice<Su2> = Lattice::hot([3, 3, 3, 3], &mut rng);
        assert!((l.wilson_loop(1, 1) - l.average_plaquette()).abs() < 1e-10);
    }

    #[test]
    fn staple_matches_plaquette_sum() {
        // Each plaquette is seen once per contained link (4) in the
        // staple sum: Σ_links Re Tr(U·A) = 4·Σ_p Re Tr U_p.
        let mut rng = Rng::seeded(12);
        let l: Lattice<Su2> = Lattice::hot([3, 3, 3, 3], &mut rng);
        let mut via_plaq = 0.0;
        for site in 0..l.volume() {
            for mu in 0..ND {
                for nu in (mu + 1)..ND {
                    via_plaq += l.plaquette(site, mu, nu).re_trace();
                }
            }
        }
        let mut via_staple = 0.0;
        for site in 0..l.volume() {
            for mu in 0..ND {
                via_staple += l.link(site, mu).mul(&l.staple(site, mu)).re_trace();
            }
        }
        assert!(
            (via_staple - 4.0 * via_plaq).abs() < 1e-8 * via_plaq.abs().max(1.0),
            "staple {via_staple} vs 4·plaq {}",
            4.0 * via_plaq
        );
    }
}
