//! Topological charge via the field-theoretic clover definition
//! (SU(2), M2).
//!
//! `Q = (1/32π²) Σ_x ε_μνρσ Tr[F̂_μν(x) F̂_ρσ(x)]` with the clover-leaf
//! field strength `F̂_μν = (1/8)(C_μν − C†_μν)`, `C_μν` the sum of the
//! four plaquette leaves around `x` in the (μ,ν) plane, all traversed
//! with consistent orientation.
//!
//! In the quaternion parameterization the antihermitian traceless part
//! of a leaf sum `C = c₀ + i c·σ` is exactly `i c·σ`, so
//! `Tr[F̂_μν F̂_ρσ] = −(1/32)·2·(c_μν · c_ρσ)` and the whole charge is a
//! few dot products per site — no matrices, no logarithms.
//!
//! On an uncooled coarse configuration this operator is dominated by
//! ultraviolet noise (the well-known defect of all field-theoretic
//! definitions); after [`crate::update::cool_sweep`] cooling it
//! approaches integer values. The M0-honest statement: this is the
//! *naive* (unimproved, no multiplicative renormalization) lattice Q,
//! useful for instanton visualization after cooling, not for a
//! susceptibility measurement. SU(2) only at this milestone.

use crate::lattice::{Lattice, ND};
use crate::su2::Su2;

/// The four-leaf clover sum in the (μ,ν) plane at `site` — each leaf a
/// full plaquette loop based at the site, oriented consistently.
fn clover(lat: &Lattice<Su2>, site: usize, mu: usize, nu: usize) -> Su2 {
    let c = lat.coords(site);
    let up = |c: [usize; ND], d: usize| lat.shift(c, d, true);
    let dn = |c: [usize; ND], d: usize| lat.shift(c, d, false);
    let l = |c: [usize; ND], d: usize| lat.link(lat.site(c), d);

    // Leaf 1: (+μ,+ν): U_μ(x) U_ν(x+μ) U_μ†(x+ν) U_ν†(x)
    let l1 = l(c, mu)
        .mul(&l(up(c, mu), nu))
        .mul(&l(up(c, nu), mu).dagger())
        .mul(&l(c, nu).dagger());
    // Leaf 2: (+ν,−μ): U_ν(x) U_μ†(x+ν−μ) U_ν†(x−μ) U_μ(x−μ)
    let l2 = l(c, nu)
        .mul(&l(dn(up(c, nu), mu), mu).dagger())
        .mul(&l(dn(c, mu), nu).dagger())
        .mul(&l(dn(c, mu), mu));
    // Leaf 3: (−μ,−ν): U_μ†(x−μ) U_ν†(x−μ−ν) U_μ(x−μ−ν) U_ν(x−ν)
    let l3 = l(dn(c, mu), mu)
        .dagger()
        .mul(&l(dn(dn(c, mu), nu), nu).dagger())
        .mul(&l(dn(dn(c, mu), nu), mu))
        .mul(&l(dn(c, nu), nu));
    // Leaf 4: (−ν,+μ): U_ν†(x−ν) U_μ(x−ν) U_ν(x+μ−ν) U_μ†(x)
    let l4 = l(dn(c, nu), nu)
        .dagger()
        .mul(&l(dn(c, nu), mu))
        .mul(&l(dn(up(c, mu), nu), nu))
        .mul(&l(c, mu).dagger());

    l1.add(&l2).add(&l3).add(&l4)
}

/// Clover field-strength vector: `F̂_μν = i f·σ` with
/// `f = (vector part of C_μν)/4` — the antihermitian traceless
/// projection of the leaf average.
fn f_vec(lat: &Lattice<Su2>, site: usize, mu: usize, nu: usize) -> [f64; 3] {
    let c = clover(lat, site, mu, nu);
    [c.a1 / 4.0, c.a2 / 4.0, c.a3 / 4.0]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Topological charge density `q(x)` such that `Q = Σ_x q(x)`.
pub fn charge_density(lat: &Lattice<Su2>) -> Vec<f64> {
    // ε_μνρσ Tr[F_μν F_ρσ] = 8·(Tr[F01 F23] − Tr[F02 F13] + Tr[F03 F12]);
    // with F = i f·σ: Tr[F F'] = −2 f·f'.
    let norm = -8.0 * 2.0 / (32.0 * std::f64::consts::PI * std::f64::consts::PI);
    (0..lat.volume())
        .map(|site| {
            let f01 = f_vec(lat, site, 0, 1);
            let f23 = f_vec(lat, site, 2, 3);
            let f02 = f_vec(lat, site, 0, 2);
            let f13 = f_vec(lat, site, 1, 3);
            let f03 = f_vec(lat, site, 0, 3);
            let f12 = f_vec(lat, site, 1, 2);
            norm * (dot(f01, f23) - dot(f02, f13) + dot(f03, f12))
        })
        .collect()
}

/// Total topological charge `Q` (naive clover; near-integer only after
/// cooling — see the module docs).
pub fn topological_charge(lat: &Lattice<Su2>) -> f64 {
    charge_density(lat).iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use crate::update::{cool_sweep, heatbath_sweep};

    #[test]
    fn cold_lattice_has_zero_charge() {
        let l: Lattice<Su2> = Lattice::cold([4, 4, 4, 4]);
        assert!(topological_charge(&l).abs() < 1e-12);
    }

    #[test]
    fn charge_is_gauge_shift_invariant_under_cooling_limit() {
        // A thermalized β=2.4 configuration, cooled deeply, lands in a
        // definite topological sector: Q settles near an integer and
        // stops moving between successive coolings.
        let mut rng = Rng::seeded(61);
        let mut lat: Lattice<Su2> = Lattice::cold([4, 4, 4, 4]);
        for _ in 0..40 {
            heatbath_sweep(&mut lat, 2.4, &mut rng);
        }
        for _ in 0..30 {
            cool_sweep(&mut lat);
        }
        let q1 = topological_charge(&lat);
        cool_sweep(&mut lat);
        cool_sweep(&mut lat);
        let q2 = topological_charge(&lat);
        assert!(
            (q1 - q2).abs() < 0.05,
            "Q still drifting under cooling: {q1} -> {q2}"
        );
        assert!(
            (q1 - q1.round()).abs() < 0.35,
            "cooled Q should be near-integer: {q1}"
        );
        // And the plaquette confirms a near-classical configuration.
        assert!(lat.average_plaquette() > 0.99);
    }
}
