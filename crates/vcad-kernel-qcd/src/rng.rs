//! Deterministic pseudo-random numbers: xoshiro256++ with splitmix64
//! seeding (Blackman & Vigna 2019, public-domain reference algorithms).
//!
//! Hand-rolled so runs are reproducible with zero dependencies, same
//! recipe as `vcad-kernel-neutronics::rng`. Not cryptographic; the
//! jackknife 1/√N test is the empirical check that the statistics
//! behave.

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

    /// Uniform in the open interval (0, 1) — never exactly 0, so it is
    /// safe under `ln`.
    pub fn uniform(&mut self) -> f64 {
        loop {
            let u = (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
            if u > 0.0 {
                return u;
            }
        }
    }

    /// Uniform in [-1, 1).
    pub fn symmetric(&mut self) -> f64 {
        2.0 * ((self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let mut a = Rng::seeded(42);
        let mut b = Rng::seeded(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn uniform_in_range() {
        let mut r = Rng::seeded(7);
        for _ in 0..10_000 {
            let u = r.uniform();
            assert!(u > 0.0 && u < 1.0);
            let s = r.symmetric();
            assert!((-1.0..1.0).contains(&s));
        }
    }
}
