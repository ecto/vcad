//! Deterministic pseudo-random numbers for Monte Carlo analysis.
//!
//! Hand-rolled **xoshiro256++** (Blackman & Vigna, "Scrambled linear
//! pseudorandom number generators", ACM Trans. Math. Softw. 47(4), 2021;
//! public-domain reference implementation at prng.di.unimi.it), seeded
//! through **SplitMix64** (Steele, Lea & Flood, OOPSLA 2014) as the
//! authors recommend. No `rand` dependency: the reproducibility of a
//! tolerance receipt must not hinge on a third-party crate's version
//! bump.
//!
//! Determinism contract: the same seed produces the same sample stream
//! on every platform — the generator uses only wrapping `u64` arithmetic
//! and IEEE-754 double conversion.

/// Deterministic xoshiro256++ generator.
#[derive(Debug, Clone)]
pub struct Rng {
    s: [u64; 4],
    /// Spare normal deviate from the Marsaglia polar method.
    spare_normal: Option<f64>,
}

impl Rng {
    /// Create a generator from a 64-bit seed via SplitMix64 expansion.
    ///
    /// SplitMix64 guarantees the four state words are well mixed even
    /// for adjacent or zero seeds (xoshiro must never be seeded with an
    /// all-zero state; SplitMix64 output is never all-zero across four
    /// consecutive draws).
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let s = [next(), next(), next(), next()];
        Self {
            s,
            spare_normal: None,
        }
    }

    /// Next raw 64-bit output (xoshiro256++ scrambler).
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[0]
            .wrapping_add(self.s[3])
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform double in [0, 1): the top 53 bits scaled by 2⁻⁵³
    /// (the standard full-precision conversion from the xoshiro paper).
    pub fn next_f64(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / 9_007_199_254_740_992.0; // 2⁻⁵³
        (self.next_u64() >> 11) as f64 * SCALE
    }

    /// Standard normal deviate via the **Marsaglia polar method**
    /// (Marsaglia & Bray, SIAM Review 6(3), 1964): rejection sampling in
    /// the unit disk, no trigonometry. Each accepted pair yields two
    /// deviates; the spare is cached, so the amortized cost is ~1.27
    /// uniform pairs per normal pair. Rejection makes the *count* of
    /// underlying draws sample-dependent, but the stream stays fully
    /// deterministic for a given seed.
    pub fn next_normal(&mut self) -> f64 {
        if let Some(z) = self.spare_normal.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.next_f64() - 1.0;
            let v = 2.0 * self.next_f64() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let f = (-2.0 * s.ln() / s).sqrt();
                self.spare_normal = Some(v * f);
                return u * f;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // Normals too (rejection count is part of the deterministic stream).
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            assert_eq!(a.next_normal().to_bits(), b.next_normal().to_bits());
        }
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let same = (0..64).filter(|_| a.next_u64() == b.next_u64()).count();
        assert_eq!(same, 0, "adjacent seeds must decorrelate via SplitMix64");
    }

    #[test]
    fn zero_seed_is_fine() {
        // xoshiro would be stuck at an all-zero state; SplitMix64 seeding
        // must prevent that.
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
        assert_ne!(first, r.next_u64());
    }

    #[test]
    fn uniform_moments() {
        let mut r = Rng::new(123);
        let n = 200_000;
        let (mut sum, mut sum_sq) = (0.0, 0.0);
        for _ in 0..n {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x));
            sum += x;
            sum_sq += x * x;
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        // SE(mean) ≈ 1/(√12·√n) ≈ 6.5e-4; allow 5 SE.
        assert!((mean - 0.5).abs() < 3.3e-3, "mean = {mean}");
        assert!((var - 1.0 / 12.0).abs() < 2e-3, "var = {var}");
    }

    #[test]
    fn normal_moments_and_tails() {
        let mut r = Rng::new(99);
        let n = 200_000usize;
        let (mut sum, mut sum_sq) = (0.0, 0.0);
        let (mut beyond_1, mut beyond_2, mut beyond_3) = (0usize, 0usize, 0usize);
        for _ in 0..n {
            let z = r.next_normal();
            sum += z;
            sum_sq += z * z;
            let a = z.abs();
            if a > 1.0 {
                beyond_1 += 1;
            }
            if a > 2.0 {
                beyond_2 += 1;
            }
            if a > 3.0 {
                beyond_3 += 1;
            }
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean = {mean}");
        assert!((var - 1.0).abs() < 0.02, "var = {var}");
        // Two-sided tail masses: 31.73%, 4.55%, 0.27%.
        let f1 = beyond_1 as f64 / n as f64;
        let f2 = beyond_2 as f64 / n as f64;
        let f3 = beyond_3 as f64 / n as f64;
        assert!((f1 - 0.3173).abs() < 0.01, "P(|z|>1) = {f1}");
        assert!((f2 - 0.0455).abs() < 0.005, "P(|z|>2) = {f2}");
        assert!((f3 - 0.0027).abs() < 0.001, "P(|z|>3) = {f3}");
    }
}
