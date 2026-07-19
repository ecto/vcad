//! A minimal hand-rolled complex number.
//!
//! Acoustic pressure is a phasor: `p(x, t) = Re{ p(x) · e^{jωt} }` with the
//! **`e^{+jωt}`** time convention used throughout this crate. Impedance
//! boundary conditions couple the real and imaginary parts, so the field
//! solver carries a complex value per node. Rather than take a dependency
//! on `num-complex`, this crate hand-rolls the handful of operations it
//! needs (the same discipline as the rest of the kernel's zero-dependency
//! numerics).

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A complex number `re + j·im`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cplx {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Cplx {
    /// The complex number `re + j·im`.
    #[inline]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// A real number as a complex number (`im = 0`).
    #[inline]
    pub const fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }

    /// Additive identity.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };
    /// Multiplicative identity.
    pub const ONE: Self = Self { re: 1.0, im: 0.0 };
    /// The imaginary unit `j`.
    pub const J: Self = Self { re: 0.0, im: 1.0 };

    /// Modulus `|z| = √(re² + im²)`.
    #[inline]
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Squared modulus `re² + im²` (no square root).
    #[inline]
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// Argument `atan2(im, re)`, radians.
    #[inline]
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    /// Complex conjugate `re − j·im`.
    #[inline]
    pub fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// The unit phasor `e^{jθ} = cos θ + j·sin θ`.
    #[inline]
    pub fn expj(theta: f64) -> Self {
        let (s, c) = theta.sin_cos();
        Self { re: c, im: s }
    }

    /// Scale by a real number.
    #[inline]
    pub fn scale(self, k: f64) -> Self {
        Self {
            re: self.re * k,
            im: self.im * k,
        }
    }

    /// True when both parts are finite.
    #[inline]
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl From<f64> for Cplx {
    #[inline]
    fn from(re: f64) -> Self {
        Self::real(re)
    }
}

impl Add for Cplx {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}

impl Sub for Cplx {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
}

impl Neg for Cplx {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}

impl Mul for Cplx {
    type Output = Self;
    #[inline]
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}

impl Div for Cplx {
    type Output = Self;
    #[inline]
    fn div(self, o: Self) -> Self {
        let d = o.norm_sqr();
        Self::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
}

impl AddAssign for Cplx {
    #[inline]
    fn add_assign(&mut self, o: Self) {
        self.re += o.re;
        self.im += o.im;
    }
}

impl SubAssign for Cplx {
    #[inline]
    fn sub_assign(&mut self, o: Self) {
        self.re -= o.re;
        self.im -= o.im;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_matches_by_hand() {
        let a = Cplx::new(1.0, 2.0);
        let b = Cplx::new(-3.0, 1.0);
        assert_eq!(a + b, Cplx::new(-2.0, 3.0));
        assert_eq!(a - b, Cplx::new(4.0, 1.0));
        // (1+2j)(−3+j) = −3 + j − 6j + 2j² = −5 − 5j
        assert_eq!(a * b, Cplx::new(-5.0, -5.0));
        assert_eq!(Cplx::J * Cplx::J, Cplx::new(-1.0, 0.0));
    }

    #[test]
    fn division_inverts_multiplication() {
        let a = Cplx::new(1.3, -2.7);
        let b = Cplx::new(0.4, 1.1);
        let q = a / b;
        let back = q * b;
        assert!((back.re - a.re).abs() < 1e-12);
        assert!((back.im - a.im).abs() < 1e-12);
    }

    #[test]
    fn expj_is_on_the_unit_circle() {
        for &t in &[0.0, 0.5, 1.0, 2.5, -1.2] {
            let z = Cplx::expj(t);
            assert!((z.abs() - 1.0).abs() < 1e-12);
            assert!(
                (z.arg() - t.rem_euclid(std::f64::consts::TAU)
                    + if t < 0.0 { std::f64::consts::TAU } else { 0.0 })
                .abs()
                    < 1e-9
                    || (z.arg() - t).abs() < 1e-9
            );
        }
        // e^{jπ} = −1.
        let z = Cplx::expj(std::f64::consts::PI);
        assert!((z.re + 1.0).abs() < 1e-12 && z.im.abs() < 1e-12);
    }
}
