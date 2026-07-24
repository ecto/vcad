//! Link variables on a periodic 4D hypercubic lattice, plus the gauge
//! observables built from them: staples, plaquettes, and planar Wilson
//! loops.
//!
//! Storage is a flat `Vec<Su2>` of length `4·V` indexed `4·site + μ`,
//! with sites in row-major order over `dims = [n₀, n₁, n₂, n₃]`.
//! Conventions: the plaquette is
//! `U_p = U_μ(x) U_ν(x+μ̂) U_μ†(x+ν̂) U_ν†(x)` and the normalized
//! observable is `P = (1/2)⟨Re Tr U_p⟩` averaged over all sites and the
//! 6 μ<ν planes. The staple sum `A_μ(x)` is defined so the local Wilson
//! action weight for the link is `exp((β/2)·Re Tr(U_μ(x)·A_μ(x)))`.

use crate::rng::Rng;
use crate::su2::Su2;

/// Number of spacetime dimensions (fixed: 4).
pub const ND: usize = 4;

/// A periodic 4D lattice of SU(2) link variables.
#[derive(Debug, Clone)]
pub struct Lattice {
    /// Extent in each direction.
    pub dims: [usize; ND],
    links: Vec<Su2>,
}

impl Lattice {
    /// Cold start: all links at the identity. Panics if any extent < 2
    /// (an extent-1 direction makes a plaquette wrap onto its own link).
    pub fn cold(dims: [usize; ND]) -> Lattice {
        assert!(dims.iter().all(|&n| n >= 2), "lattice extents must be >= 2");
        let volume: usize = dims.iter().product();
        Lattice {
            dims,
            links: vec![Su2::IDENTITY; ND * volume],
        }
    }

    /// Hot start: Haar-random links.
    pub fn hot(dims: [usize; ND], rng: &mut Rng) -> Lattice {
        let mut l = Lattice::cold(dims);
        for u in l.links.iter_mut() {
            *u = Su2::random(rng);
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
    fn coords(&self, site: usize) -> [usize; ND] {
        let mut c = [0usize; ND];
        let mut s = site;
        for mu in (0..ND).rev() {
            c[mu] = s % self.dims[mu];
            s /= self.dims[mu];
        }
        c
    }

    /// Flatten coordinates into a site index.
    fn site(&self, c: [usize; ND]) -> usize {
        c.iter()
            .zip(self.dims.iter())
            .fold(0usize, |s, (&ci, &ni)| s * ni + ci)
    }

    /// Neighbor site index one step in `±mu` (periodic).
    fn shift(&self, c: [usize; ND], mu: usize, forward: bool) -> [usize; ND] {
        let mut c = c;
        let n = self.dims[mu];
        c[mu] = if forward {
            (c[mu] + 1) % n
        } else {
            (c[mu] + n - 1) % n
        };
        c
    }

    /// Link `U_μ(x)` by site index.
    pub fn link(&self, site: usize, mu: usize) -> Su2 {
        self.links[ND * site + mu]
    }

    /// Overwrite link `U_μ(x)` (renormalized against float drift).
    pub fn set_link(&mut self, site: usize, mu: usize, u: Su2) {
        self.links[ND * site + mu] = u.normalized();
    }

    /// Staple sum `A_μ(x)`: for every plane (μ,ν≠μ), the up staple
    /// `U_ν(x+μ̂) U_μ†(x+ν̂) U_ν†(x)` plus the down staple
    /// `U_ν†(x+μ̂−ν̂) U_μ†(x−ν̂) U_ν(x−ν̂)`, summed componentwise.
    /// `Re Tr(U_μ(x)·A_μ(x))` is then twice the sum of the 6 plaquette
    /// traces containing the link.
    pub fn staple(&self, site: usize, mu: usize) -> Su2 {
        let c = self.coords(site);
        let c_up_mu = self.shift(c, mu, true);
        let mut acc = Su2::ZERO;
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

    /// Average plaquette `P = (1/2)⟨Re Tr U_p⟩` over all sites and the
    /// 6 μ<ν planes. 1 on a cold lattice, → 0 at strong coupling.
    pub fn average_plaquette(&self) -> f64 {
        let volume = self.volume();
        let mut sum = 0.0;
        for site in 0..volume {
            let c = self.coords(site);
            for mu in 0..ND {
                for nu in (mu + 1)..ND {
                    let u1 = self.link(site, mu);
                    let u2 = self.link(self.site(self.shift(c, mu, true)), nu);
                    let u3 = self.link(self.site(self.shift(c, nu, true)), mu);
                    let u4 = self.link(site, nu);
                    sum += 0.5 * u1.mul(&u2).mul(&u3.dagger()).mul(&u4.dagger()).re_trace();
                }
            }
        }
        sum / (volume * ND * (ND - 1) / 2) as f64
    }

    /// Path-ordered product along `r` links in `+mu` from `c`.
    fn line(&self, mut c: [usize; ND], mu: usize, r: usize) -> (Su2, [usize; ND]) {
        let mut u = Su2::IDENTITY;
        for _ in 0..r {
            u = u.mul(&self.link(self.site(c), mu));
            c = self.shift(c, mu, true);
        }
        (u, c)
    }

    /// Average planar Wilson loop `W(r,t) = (1/2)⟨Re Tr ∏ U⟩` over all
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
                    sum += 0.5 * w.re_trace();
                    count += 1;
                }
            }
        }
        sum / count as f64
    }

    /// Shift `n` steps forward in `mu`.
    fn shift_n(&self, mut c: [usize; ND], mu: usize, n: usize) -> [usize; ND] {
        for _ in 0..n {
            c = self.shift(c, mu, true);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_lattice_observables() {
        let l = Lattice::cold([3, 3, 3, 3]);
        assert!((l.average_plaquette() - 1.0).abs() < 1e-12);
        assert!((l.wilson_loop(2, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn wilson_1x1_equals_plaquette() {
        let mut rng = Rng::seeded(11);
        let l = Lattice::hot([3, 3, 3, 3], &mut rng);
        assert!((l.wilson_loop(1, 1) - l.average_plaquette()).abs() < 1e-10);
    }

    #[test]
    fn staple_matches_plaquette_sum() {
        // Re Tr(U_μ(x)·A_μ(x)) must equal 2·Σ_p Re Tr U_p over the 6
        // plaquettes containing the link (up + down in each of 3 planes,
        // counting Re Tr which is orientation-invariant).
        let mut rng = Rng::seeded(12);
        let l = Lattice::hot([3, 3, 3, 3], &mut rng);
        // Total action two ways: via plaquettes and via staples. Each
        // plaquette contains 4 links, so Σ_links Re Tr(U·A) = 4·2·Σ_p Re Tr U_p
        // (staple double-counts orientation? no: A sums 2(ND-1)=6 staples,
        // one per plaquette containing the link).
        let mut via_plaq = 0.0;
        for site in 0..l.volume() {
            let c = l.coords(site);
            for mu in 0..ND {
                for nu in (mu + 1)..ND {
                    let u1 = l.link(site, mu);
                    let u2 = l.link(l.site(l.shift(c, mu, true)), nu);
                    let u3 = l.link(l.site(l.shift(c, nu, true)), mu);
                    let u4 = l.link(site, nu);
                    via_plaq += u1.mul(&u2).mul(&u3.dagger()).mul(&u4.dagger()).re_trace();
                }
            }
        }
        let mut via_staple = 0.0;
        for site in 0..l.volume() {
            for mu in 0..ND {
                via_staple += l.link(site, mu).mul(&l.staple(site, mu)).re_trace();
            }
        }
        // Each plaquette is seen once per contained link (4) in the staple
        // sum: via_staple = 4 · via_plaq.
        assert!(
            (via_staple - 4.0 * via_plaq).abs() < 1e-8 * via_plaq.abs().max(1.0),
            "staple {via_staple} vs 4·plaq {}",
            4.0 * via_plaq
        );
    }
}
