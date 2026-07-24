//! SU(2) group elements in the quaternion parameterization.
//!
//! `U = a₀·1 + i(a₁σ₁ + a₂σ₂ + a₃σ₃)` with `a₀² + |a|² = 1`. Products,
//! adjoints, and traces are closed-form in the four real coefficients,
//! so no complex matrices ever exist and unitarity is a normalization,
//! not a hope. The same struct with `norm ≠ 1` represents staple sums
//! (sums of SU(2) elements are proportional to SU(2) elements — the
//! property the heatbath algorithm is built on).

use crate::group::GaugeGroup;
use crate::rng::Rng;

/// Sample `w₀ ∈ [−1,1]` from `P(w₀) ∝ √(1−w₀²)·exp(α·w₀)` (α > 0),
/// Kennedy–Pendleton rejection (Kennedy & Pendleton 1985). Shared by
/// the SU(2) heatbath and each SU(3) Cabibbo–Marinari subgroup draw.
pub(crate) fn sample_w0(alpha: f64, rng: &mut Rng) -> f64 {
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
pub(crate) fn sphere(r: f64, rng: &mut Rng) -> (f64, f64, f64) {
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

/// Draw `W ∈ SU(2)` with weight `exp(alpha·Tr W / 2)·dW` — i.e.
/// scalar part from [`sample_w0`], vector part uniform on the sphere.
/// The workhorse for both the SU(2) heatbath and CM subgroup draws.
pub(crate) fn sample_su2(alpha: f64, rng: &mut Rng) -> Su2 {
    let w0 = sample_w0(alpha, rng);
    let r = (1.0 - w0 * w0).max(0.0).sqrt();
    let (w1, w2, w3) = sphere(r, rng);
    Su2 {
        a0: w0,
        a1: w1,
        a2: w2,
        a3: w3,
    }
}

/// A real quaternion `a₀ + i a·σ`. Unit norm ⇔ SU(2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Su2 {
    /// Coefficient of the identity.
    pub a0: f64,
    /// Coefficient of iσ₁.
    pub a1: f64,
    /// Coefficient of iσ₂.
    pub a2: f64,
    /// Coefficient of iσ₃.
    pub a3: f64,
}

impl Su2 {
    /// The identity element.
    pub const IDENTITY: Su2 = Su2 {
        a0: 1.0,
        a1: 0.0,
        a2: 0.0,
        a3: 0.0,
    };

    /// The zero quaternion (additive identity for staple accumulation).
    pub const ZERO: Su2 = Su2 {
        a0: 0.0,
        a1: 0.0,
        a2: 0.0,
        a3: 0.0,
    };

    /// Group product. For `U = a₀ + i a·σ`, `V = b₀ + i b·σ`:
    /// `UV = (a₀b₀ − a·b) + i(a₀b + b₀a − a×b)·σ`.
    pub fn mul(&self, o: &Su2) -> Su2 {
        Su2 {
            a0: self.a0 * o.a0 - self.a1 * o.a1 - self.a2 * o.a2 - self.a3 * o.a3,
            a1: self.a0 * o.a1 + o.a0 * self.a1 - (self.a2 * o.a3 - self.a3 * o.a2),
            a2: self.a0 * o.a2 + o.a0 * self.a2 - (self.a3 * o.a1 - self.a1 * o.a3),
            a3: self.a0 * o.a3 + o.a0 * self.a3 - (self.a1 * o.a2 - self.a2 * o.a1),
        }
    }

    /// Hermitian conjugate (= inverse for unit norm): negate the vector part.
    pub fn dagger(&self) -> Su2 {
        Su2 {
            a0: self.a0,
            a1: -self.a1,
            a2: -self.a2,
            a3: -self.a3,
        }
    }

    /// Componentwise sum (staple accumulation; leaves the group).
    pub fn add(&self, o: &Su2) -> Su2 {
        Su2 {
            a0: self.a0 + o.a0,
            a1: self.a1 + o.a1,
            a2: self.a2 + o.a2,
            a3: self.a3 + o.a3,
        }
    }

    /// `Re Tr U = 2a₀` (the trace is automatically real in SU(2)).
    pub fn re_trace(&self) -> f64 {
        2.0 * self.a0
    }

    /// Quaternion norm `√(a₀² + |a|²)`. Equals `√det` for a staple sum.
    pub fn norm(&self) -> f64 {
        (self.a0 * self.a0 + self.a1 * self.a1 + self.a2 * self.a2 + self.a3 * self.a3).sqrt()
    }

    /// Rescale to unit norm (projects a near-SU(2) element back onto the
    /// group; used both to normalize staples and to fight float drift).
    /// Returns the identity for the zero quaternion.
    pub fn normalized(&self) -> Su2 {
        let n = self.norm();
        if n <= 0.0 {
            return Su2::IDENTITY;
        }
        Su2 {
            a0: self.a0 / n,
            a1: self.a1 / n,
            a2: self.a2 / n,
            a3: self.a3 / n,
        }
    }

    /// Haar-uniform random SU(2) element: uniform point on S³
    /// (normalized 4D Gaussian via Marsaglia polar pairs).
    pub fn random(rng: &mut Rng) -> Su2 {
        // Two Marsaglia polar pairs give four independent N(0,1) draws.
        let pair = |rng: &mut Rng| loop {
            let u = rng.symmetric();
            let v = rng.symmetric();
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let f = (-2.0 * s.ln() / s).sqrt();
                return (u * f, v * f);
            }
        };
        let (g0, g1) = pair(rng);
        let (g2, g3) = pair(rng);
        Su2 {
            a0: g0,
            a1: g1,
            a2: g2,
            a3: g3,
        }
        .normalized()
    }
}

impl GaugeGroup for Su2 {
    const NC: usize = 2;

    fn identity() -> Self {
        Su2::IDENTITY
    }

    fn zero() -> Self {
        Su2::ZERO
    }

    fn mul(&self, o: &Self) -> Self {
        Su2::mul(self, o)
    }

    fn dagger(&self) -> Self {
        Su2::dagger(self)
    }

    fn add(&self, o: &Self) -> Self {
        Su2::add(self, o)
    }

    fn scale(&self, s: f64) -> Self {
        Su2 {
            a0: self.a0 * s,
            a1: self.a1 * s,
            a2: self.a2 * s,
            a3: self.a3 * s,
        }
    }

    fn re_trace(&self) -> f64 {
        Su2::re_trace(self)
    }

    fn reunitarize(&self) -> Self {
        self.normalized()
    }

    fn random(rng: &mut Rng) -> Self {
        Su2::random(rng)
    }

    /// Exact Kennedy–Pendleton draw. The weight `exp((β/2)Re Tr(U·A))`
    /// with `A = k·Ā` becomes `exp(βk·w₀)` for `W = U·Ā`, sampled
    /// directly; the current link is not needed.
    fn heatbath(_u: &Self, a: &Self, beta: f64, rng: &mut Rng) -> Self {
        let k = a.norm();
        if k < 1e-300 {
            return Su2::random(rng);
        }
        let a_bar = a.normalized();
        // sample_su2 weight exp(alpha·Tr W/2) = exp(alpha·w0): alpha = βk.
        sample_su2(beta * k, rng).mul(&a_bar.dagger())
    }

    /// Microcanonical reflection `U → Ā†U†Ā†` (preserves `Re Tr(U·A)`).
    fn overrelax(u: &Self, a: &Self, rng: &mut Rng) -> Self {
        if a.norm() < 1e-300 {
            return Su2::random(rng);
        }
        let ab = a.normalized().dagger();
        ab.mul(&u.dagger()).mul(&ab)
    }

    /// `Ā†` maximizes `Re Tr(U·A)` exactly.
    fn cool(_u: &Self, a: &Self) -> Self {
        if a.norm() < 1e-300 {
            return Su2::IDENTITY;
        }
        a.normalized().dagger()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() < tol, "{a} vs {b}");
    }

    #[test]
    fn unitarity_and_inverse() {
        let mut rng = Rng::seeded(1);
        for _ in 0..100 {
            let u = Su2::random(&mut rng);
            assert_close(u.norm(), 1.0, 1e-12);
            let id = u.mul(&u.dagger());
            assert_close(id.a0, 1.0, 1e-12);
            assert_close(id.a1, 0.0, 1e-12);
            assert_close(id.a2, 0.0, 1e-12);
            assert_close(id.a3, 0.0, 1e-12);
        }
    }

    #[test]
    fn associativity() {
        let mut rng = Rng::seeded(2);
        for _ in 0..50 {
            let (a, b, c) = (
                Su2::random(&mut rng),
                Su2::random(&mut rng),
                Su2::random(&mut rng),
            );
            let l = a.mul(&b).mul(&c);
            let r = a.mul(&b.mul(&c));
            assert_close(l.a0, r.a0, 1e-12);
            assert_close(l.a1, r.a1, 1e-12);
            assert_close(l.a2, r.a2, 1e-12);
            assert_close(l.a3, r.a3, 1e-12);
        }
    }

    #[test]
    fn trace_of_product_symmetric() {
        // Re Tr(UV) = Re Tr(VU) — cyclicity survives the parameterization.
        let mut rng = Rng::seeded(3);
        for _ in 0..50 {
            let u = Su2::random(&mut rng);
            let v = Su2::random(&mut rng);
            assert_close(u.mul(&v).re_trace(), v.mul(&u).re_trace(), 1e-12);
        }
    }

    #[test]
    fn haar_mean_trace_is_zero() {
        // ∫ dU (1/2)Re Tr U = 0 over SU(2).
        let mut rng = Rng::seeded(4);
        let n = 200_000;
        let mean: f64 = (0..n)
            .map(|_| 0.5 * Su2::random(&mut rng).re_trace())
            .sum::<f64>()
            / n as f64;
        assert!(mean.abs() < 0.005, "haar mean trace {mean}");
    }
}
