//! Minimal complex arithmetic for the MoM solver.
//!
//! Hand-rolled `(f64, f64)` pairs — no external dependencies, and the
//! operation set is exactly what the EFIE matrix fill, the LU solve, and
//! the far-field sums need. Time convention throughout the crate is
//! `e^{+jωt}`, so the outgoing Green's function is `e^{-jkR}/(4πR)`.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A complex number as a pair of `f64`s.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// Zero.
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };
    /// One.
    pub const ONE: Complex = Complex { re: 1.0, im: 0.0 };
    /// The imaginary unit `j`.
    pub const J: Complex = Complex { re: 0.0, im: 1.0 };

    /// Construct from real and imaginary parts.
    pub const fn new(re: f64, im: f64) -> Self {
        Complex { re, im }
    }

    /// A purely real value.
    pub const fn real(re: f64) -> Self {
        Complex { re, im: 0.0 }
    }

    /// `e^{jθ} = cos θ + j sin θ`.
    pub fn expj(theta: f64) -> Self {
        Complex {
            re: theta.cos(),
            im: theta.sin(),
        }
    }

    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Complex {
            re: self.re,
            im: -self.im,
        }
    }

    /// Squared magnitude `|z|²`.
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// Magnitude `|z|`.
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Multiply by a real scalar.
    pub fn scale(self, s: f64) -> Self {
        Complex {
            re: self.re * s,
            im: self.im * s,
        }
    }

    /// True when both parts are finite.
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl Add for Complex {
    type Output = Complex;
    fn add(self, rhs: Complex) -> Complex {
        Complex::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl Sub for Complex {
    type Output = Complex;
    fn sub(self, rhs: Complex) -> Complex {
        Complex::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl Mul for Complex {
    type Output = Complex;
    fn mul(self, rhs: Complex) -> Complex {
        Complex::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

impl Div for Complex {
    type Output = Complex;
    fn div(self, rhs: Complex) -> Complex {
        // Scaled (Smith) division: robust against intermediate over/underflow.
        if rhs.re.abs() >= rhs.im.abs() {
            let r = rhs.im / rhs.re;
            let d = rhs.re + rhs.im * r;
            Complex::new((self.re + self.im * r) / d, (self.im - self.re * r) / d)
        } else {
            let r = rhs.re / rhs.im;
            let d = rhs.re * r + rhs.im;
            Complex::new((self.re * r + self.im) / d, (self.im * r - self.re) / d)
        }
    }
}

impl Neg for Complex {
    type Output = Complex;
    fn neg(self) -> Complex {
        Complex::new(-self.re, -self.im)
    }
}

impl AddAssign for Complex {
    fn add_assign(&mut self, rhs: Complex) {
        self.re += rhs.re;
        self.im += rhs.im;
    }
}

impl SubAssign for Complex {
    fn sub_assign(&mut self, rhs: Complex) {
        self.re -= rhs.re;
        self.im -= rhs.im;
    }
}

impl Mul<f64> for Complex {
    type Output = Complex;
    fn mul(self, rhs: f64) -> Complex {
        self.scale(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplication_and_conjugate_identities() {
        let a = Complex::new(2.5, -1.25);
        let b = Complex::new(-0.75, 3.0);
        let ab = a * b;
        // |ab| = |a||b|
        assert!((ab.abs() - a.abs() * b.abs()).abs() < 1e-12);
        // z * conj(z) = |z|^2
        let zz = a * a.conj();
        assert!((zz.re - a.norm_sqr()).abs() < 1e-12 && zz.im.abs() < 1e-12);
    }

    #[test]
    fn division_inverts_multiplication() {
        let a = Complex::new(1.7e3, -2.9e2);
        let b = Complex::new(-4.2e-3, 8.1e-4);
        let q = a / b;
        let back = q * b;
        assert!((back - a).abs() < 1e-9 * a.abs());
    }

    #[test]
    fn expj_lies_on_the_unit_circle() {
        for i in 0..8 {
            let th = 0.9 * i as f64;
            let z = Complex::expj(th);
            assert!((z.abs() - 1.0).abs() < 1e-15);
        }
        // e^{ja} e^{jb} = e^{j(a+b)}
        let p = Complex::expj(0.3) * Complex::expj(1.1);
        assert!((p - Complex::expj(1.4)).abs() < 1e-14);
    }
}
