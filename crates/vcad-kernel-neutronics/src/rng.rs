//! Deterministic pseudo-random numbers: xoshiro256++ with splitmix64
//! seeding (Blackman & Vigna 2019, public-domain reference algorithms).
//!
//! Hand-rolled so runs are reproducible with zero dependencies. Batch
//! independence comes from seeding each batch's generator through
//! splitmix64 on `(seed, stream)` — adequate decorrelation for a design
//! tool (documented limitation: these are not cryptographic streams and
//! carry no formal independence proof; the 1/√N validation test is the
//! empirical check that batch statistics behave).

/// splitmix64 step (Vigna's reference): used for seeding only.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// xoshiro256++ generator.
#[derive(Debug, Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seed deterministically from a single u64.
    pub fn seeded(seed: u64) -> Self {
        let mut sm = seed;
        let s = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        Rng { s }
    }

    /// Independent-ish stream `stream` under `seed` (batch seeding).
    pub fn stream(seed: u64, stream: u64) -> Self {
        // Mix the stream index through splitmix before seeding so
        // consecutive streams start far apart in state space.
        let mut sm = seed ^ 0xA076_1D64_78BD_642F_u64.wrapping_mul(stream.wrapping_add(1));
        let s = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        Rng { s }
    }

    /// Next raw 64-bit output.
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

    /// Uniform in the half-open interval (0, 1] — safe for `-ln(u)`.
    pub fn uniform(&mut self) -> f64 {
        (((self.next_u64() >> 11) + 1) as f64) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in [-1, 1] (isotropic direction cosine).
    pub fn uniform_mu(&mut self) -> f64 {
        2.0 * self.uniform() - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_seed_sensitive() {
        let mut a = Rng::seeded(42);
        let mut b = Rng::seeded(42);
        let mut c = Rng::seeded(43);
        let va: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let vb: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let vc: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(va, vb);
        assert_ne!(va, vc);
    }

    #[test]
    fn uniform_in_bounds_and_mean_near_half() {
        let mut r = Rng::seeded(7);
        let n = 100_000;
        let mut sum = 0.0;
        for _ in 0..n {
            let u = r.uniform();
            assert!(u > 0.0 && u <= 1.0);
            sum += u;
        }
        let mean = sum / n as f64;
        // 3σ band for the mean of U(0,1): σ_mean = 1/√(12n) ≈ 9.1e-4.
        assert!((mean - 0.5).abs() < 3e-3, "mean {mean}");
    }

    #[test]
    fn streams_differ() {
        let mut a = Rng::stream(42, 0);
        let mut b = Rng::stream(42, 1);
        assert_ne!(
            (0..8).map(|_| a.next_u64()).collect::<Vec<_>>(),
            (0..8).map(|_| b.next_u64()).collect::<Vec<_>>()
        );
    }
}
