//! SU(3) group elements as explicit 3×3 complex matrices, updated via
//! Cabibbo–Marinari SU(2) subgroup cycles (Cabibbo & Marinari 1982).
//!
//! The three subgroups embed in the (0,1), (0,2), (1,2) row/column
//! blocks. For each, the relevant part of the local weight
//! `exp((β/3)Re Tr(a·U·A))` restricts to an SU(2) problem: project the
//! 2×2 block of `M = U·A` onto the quaternion basis, and the
//! Kennedy–Pendleton sampler from the SU(2) code does the rest.
//! Reunitarization is Gram–Schmidt on rows with the third row set to
//! the conjugate cross product (det = +1 exactly, not just |det| = 1).

use crate::group::GaugeGroup;
use crate::rng::Rng;
use crate::su2::{sample_su2, Su2};

/// A complex number (re, im). Minimal, local, zero-dependency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct C {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl C {
    /// 0.
    pub const ZERO: C = C { re: 0.0, im: 0.0 };
    /// 1.
    pub const ONE: C = C { re: 1.0, im: 0.0 };

    fn add(self, o: C) -> C {
        C {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
    fn sub(self, o: C) -> C {
        C {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }
    fn mul(self, o: C) -> C {
        C {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
    fn conj(self) -> C {
        C {
            re: self.re,
            im: -self.im,
        }
    }
    fn scale(self, s: f64) -> C {
        C {
            re: self.re * s,
            im: self.im * s,
        }
    }
    fn norm2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// An SU(3) element (or, with non-unitary entries, a staple sum).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Su3 {
    /// Row-major 3×3 complex entries.
    pub m: [[C; 3]; 3],
}

/// The three Cabibbo–Marinari subgroup index pairs.
const SUBGROUPS: [(usize, usize); 3] = [(0, 1), (0, 2), (1, 2)];

impl Su3 {
    /// Embed an SU(2) element `s = s0 + i s·σ` into the (i,j) block of
    /// the identity: `[[s0+is3, s2+is1], [-s2+is1, s0-is3]]` (the
    /// standard fundamental parameterization).
    fn embed(s: &Su2, i: usize, j: usize) -> Su3 {
        let mut m = Su3::identity_();
        m.m[i][i] = C { re: s.a0, im: s.a3 };
        m.m[i][j] = C { re: s.a2, im: s.a1 };
        m.m[j][i] = C {
            re: -s.a2,
            im: s.a1,
        };
        m.m[j][j] = C {
            re: s.a0,
            im: -s.a3,
        };
        m
    }

    /// Project the (i,j) 2×2 block onto the quaternion basis. The
    /// result is a (generally non-unit) quaternion `r`; `|r|` and `r̂`
    /// feed the SU(2) machinery.
    fn project(&self, i: usize, j: usize) -> Su2 {
        let m00 = self.m[i][i];
        let m01 = self.m[i][j];
        let m10 = self.m[j][i];
        let m11 = self.m[j][j];
        Su2 {
            a0: 0.5 * (m00.re + m11.re),
            a1: 0.5 * (m01.im + m10.im),
            a2: 0.5 * (m01.re - m10.re),
            a3: 0.5 * (m00.im - m11.im),
        }
    }

    fn identity_() -> Su3 {
        let mut m = [[C::ZERO; 3]; 3];
        m[0][0] = C::ONE;
        m[1][1] = C::ONE;
        m[2][2] = C::ONE;
        Su3 { m }
    }

    /// Determinant (should be 1 + 0i for group elements).
    pub fn det(&self) -> C {
        let m = &self.m;
        let c0 = m[1][1].mul(m[2][2]).sub(m[1][2].mul(m[2][1]));
        let c1 = m[1][0].mul(m[2][2]).sub(m[1][2].mul(m[2][0]));
        let c2 = m[1][0].mul(m[2][1]).sub(m[1][1].mul(m[2][0]));
        m[0][0].mul(c0).sub(m[0][1].mul(c1)).add(m[0][2].mul(c2))
    }

    /// Imaginary part of the trace (Polyakov loops in SU(3) are
    /// complex; SU(2)'s vanish identically).
    pub fn im_trace(&self) -> f64 {
        self.m[0][0].im + self.m[1][1].im + self.m[2][2].im
    }

    /// One Cabibbo–Marinari cycle of `op` over the three subgroups.
    /// `op` maps (projected staple-block quaternion) → SU(2) multiplier
    /// contribution `W` with the convention `a = W·r̂†`.
    fn cm_cycle<F: FnMut(&Su2, f64, &mut Rng) -> Su2>(
        u: &Su3,
        a: &Su3,
        rng: &mut Rng,
        mut op: F,
    ) -> Su3 {
        let mut u = *u;
        for &(i, j) in SUBGROUPS.iter() {
            let m = u.mul(a);
            let r = m.project(i, j);
            let k = r.norm();
            let multiplier = if k < 1e-300 {
                Su2::random(rng)
            } else {
                let r_bar = r.normalized();
                let w = op(&r_bar, k, rng);
                w.mul(&r_bar.dagger())
            };
            u = Su3::embed(&multiplier, i, j).mul(&u);
        }
        u.reunitarize()
    }
}

impl GaugeGroup for Su3 {
    const NC: usize = 3;

    fn identity() -> Self {
        Su3::identity_()
    }

    fn zero() -> Self {
        Su3 {
            m: [[C::ZERO; 3]; 3],
        }
    }

    fn mul(&self, o: &Self) -> Self {
        let mut r = Su3::zero();
        for i in 0..3 {
            for j in 0..3 {
                let mut acc = C::ZERO;
                for k in 0..3 {
                    acc = acc.add(self.m[i][k].mul(o.m[k][j]));
                }
                r.m[i][j] = acc;
            }
        }
        r
    }

    fn dagger(&self) -> Self {
        let mut r = Su3::zero();
        for i in 0..3 {
            for j in 0..3 {
                r.m[i][j] = self.m[j][i].conj();
            }
        }
        r
    }

    fn add(&self, o: &Self) -> Self {
        let mut r = *self;
        for i in 0..3 {
            for j in 0..3 {
                r.m[i][j] = r.m[i][j].add(o.m[i][j]);
            }
        }
        r
    }

    fn scale(&self, s: f64) -> Self {
        let mut r = *self;
        for i in 0..3 {
            for j in 0..3 {
                r.m[i][j] = r.m[i][j].scale(s);
            }
        }
        r
    }

    fn re_trace(&self) -> f64 {
        self.m[0][0].re + self.m[1][1].re + self.m[2][2].re
    }

    fn norm_trace_im(&self) -> f64 {
        self.im_trace() / Self::NC as f64
    }

    /// Gram–Schmidt rows; third row = conjugate cross product of the
    /// first two, so det = +1 exactly.
    fn reunitarize(&self) -> Self {
        let mut r = *self;
        // Normalize row 0.
        let n0: f64 = r.m[0].iter().map(|c| c.norm2()).sum::<f64>().sqrt();
        if n0 <= 0.0 {
            return Su3::identity_();
        }
        for j in 0..3 {
            r.m[0][j] = r.m[0][j].scale(1.0 / n0);
        }
        // Orthogonalize row 1 against row 0, then normalize.
        let mut dot = C::ZERO; // <row0, row1> = Σ conj(r0)·r1
        for j in 0..3 {
            dot = dot.add(r.m[0][j].conj().mul(r.m[1][j]));
        }
        for j in 0..3 {
            r.m[1][j] = r.m[1][j].sub(dot.mul(r.m[0][j]));
        }
        let n1: f64 = r.m[1].iter().map(|c| c.norm2()).sum::<f64>().sqrt();
        if n1 <= 0.0 {
            return Su3::identity_();
        }
        for j in 0..3 {
            r.m[1][j] = r.m[1][j].scale(1.0 / n1);
        }
        // Row 2 = conj(row0 × row1).
        for j in 0..3 {
            let a = r.m[0][(j + 1) % 3].mul(r.m[1][(j + 2) % 3]);
            let b = r.m[0][(j + 2) % 3].mul(r.m[1][(j + 1) % 3]);
            r.m[2][j] = a.sub(b).conj();
        }
        r
    }

    /// Well-mixed random element: two cycles of random SU(2) subgroup
    /// embeds (adequate mixing for hot starts; ergodicity comes from
    /// the Markov chain, not the initializer).
    fn random(rng: &mut Rng) -> Self {
        let mut u = Su3::identity_();
        for _ in 0..2 {
            for &(i, j) in SUBGROUPS.iter() {
                u = Su3::embed(&Su2::random(rng), i, j).mul(&u);
            }
        }
        u.reunitarize()
    }

    /// One Cabibbo–Marinari heatbath cycle. Per subgroup the weight
    /// restricted to `a = W·r̂†` is `exp((2βk/N)·w₀)` — the same KP
    /// target as SU(2) with `α = 2βk/3`.
    fn heatbath(u: &Self, a: &Self, beta: f64, rng: &mut Rng) -> Self {
        Su3::cm_cycle(u, a, rng, |_r_bar, k, rng| {
            sample_su2(2.0 * beta * k / Self::NC as f64, rng)
        })
    }

    /// Subgroup overrelaxation: per subgroup the reflection is
    /// `W → W†`, i.e. multiplier `a = r̂†·r̂† = (r̂†)²` — exactly
    /// action-preserving within each subgroup step.
    fn overrelax(u: &Self, a: &Self, rng: &mut Rng) -> Self {
        Su3::cm_cycle(u, a, rng, |r_bar, _k, _rng| {
            // W_new = W_cur† = r̂†; op returns W, caller multiplies r̂†.
            r_bar.dagger()
        })
    }

    /// Subgroup cooling: per subgroup the maximizer is `W = 1`.
    fn cool(u: &Self, a: &Self) -> Self {
        // No randomness needed; a throwaway Rng satisfies cm_cycle's
        // signature only on the degenerate-staple branch.
        let mut rng = Rng::seeded(0);
        Su3::cm_cycle(u, a, &mut rng, |_r_bar, _k, _rng| Su2 {
            a0: 1.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() < tol, "{a} vs {b}");
    }

    #[test]
    fn random_is_unitary_with_unit_det() {
        let mut rng = Rng::seeded(41);
        for _ in 0..50 {
            let u = Su3::random(&mut rng);
            let id = u.mul(&u.dagger());
            for i in 0..3 {
                for j in 0..3 {
                    let expect = if i == j { 1.0 } else { 0.0 };
                    close(id.m[i][j].re, expect, 1e-10);
                    close(id.m[i][j].im, 0.0, 1e-10);
                }
            }
            let d = u.det();
            close(d.re, 1.0, 1e-10);
            close(d.im, 0.0, 1e-10);
        }
    }

    #[test]
    fn embed_project_round_trip() {
        let mut rng = Rng::seeded(42);
        for &(i, j) in SUBGROUPS.iter() {
            let s = Su2::random(&mut rng);
            let e = Su3::embed(&s, i, j);
            // Embedded element is SU(3).
            let d = e.det();
            close(d.re, 1.0, 1e-12);
            close(d.im, 0.0, 1e-12);
            // Projection of the embed recovers the quaternion.
            let p = e.project(i, j);
            close(p.a0, s.a0, 1e-12);
            close(p.a1, s.a1, 1e-12);
            close(p.a2, s.a2, 1e-12);
            close(p.a3, s.a3, 1e-12);
        }
    }

    #[test]
    fn projection_is_trace_adjoint() {
        // Re tr_2x2(embed(α)·M) must equal quaternion pairing
        // 2(α₀r₀ − α·r)… i.e. Re Tr(embed(α)·M) − const = tr(α·proj M).
        // Verify Re Tr(E(α)M) = tr_q(α · proj(M)) + Re M[k][k] for the
        // untouched index k.
        let mut rng = Rng::seeded(43);
        for &(i, j) in SUBGROUPS.iter() {
            let alpha = Su2::random(&mut rng);
            let m = Su3::random(&mut rng).add(&Su3::random(&mut rng));
            let k = 3 - i - j;
            let lhs = Su3::embed(&alpha, i, j).mul(&m).re_trace() - m.m[k][k].re;
            let r = m.project(i, j);
            let rhs = alpha.mul(&r).re_trace();
            close(lhs, rhs, 1e-10);
        }
    }

    #[test]
    fn overrelax_preserves_local_action() {
        let mut rng = Rng::seeded(44);
        let u = Su3::random(&mut rng);
        // A staple-like sum of group elements.
        let a = Su3::random(&mut rng)
            .add(&Su3::random(&mut rng))
            .add(&Su3::random(&mut rng));
        let before = u.mul(&a).re_trace();
        let u2 = Su3::overrelax(&u, &a, &mut rng);
        let after = u2.mul(&a).re_trace();
        // Subgroup OR preserves the action per subgroup step exactly;
        // reunitarization adds only float-level drift.
        close(before, after, 1e-8);
        // And the result is still SU(3).
        let d = u2.det();
        close(d.re, 1.0, 1e-10);
    }

    #[test]
    fn cooling_increases_local_action() {
        let mut rng = Rng::seeded(45);
        for _ in 0..20 {
            let u = Su3::random(&mut rng);
            let a = Su3::random(&mut rng).add(&Su3::random(&mut rng));
            let before = u.mul(&a).re_trace();
            let after = Su3::cool(&u, &a).mul(&a).re_trace();
            assert!(after >= before - 1e-10, "{before} -> {after}");
        }
    }
}
