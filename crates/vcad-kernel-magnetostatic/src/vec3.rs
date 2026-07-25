//! Minimal 3-vector. SI metres throughout the solver interior; the public
//! machine description takes millimetres per the vcad convention and converts
//! once at the boundary.

use serde::{Deserialize, Serialize};

/// A 3-vector of `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    /// x component.
    pub x: f64,
    /// y component.
    pub y: f64,
    /// z component.
    pub z: f64,
}

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Construct from components.
    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// A point on the z axis.
    #[inline]
    pub const fn z_axis(z: f64) -> Self {
        Self { x: 0.0, y: 0.0, z }
    }

    /// Cylindrical construction: radius, azimuth (rad), height.
    #[inline]
    pub fn cylindrical(r: f64, theta: f64, z: f64) -> Self {
        Self {
            x: r * theta.cos(),
            y: r * theta.sin(),
            z,
        }
    }

    /// Euclidean length.
    #[inline]
    pub fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// Squared length — avoids a `sqrt` where only comparisons matter.
    #[inline]
    pub fn norm_sq(self) -> f64 {
        self.dot(self)
    }

    /// Dot product.
    #[inline]
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    /// Cross product.
    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3 {
            x: self.y * o.z - self.z * o.y,
            y: self.z * o.x - self.x * o.z,
            z: self.x * o.y - self.y * o.x,
        }
    }

    /// Unit vector. Returns [`Vec3::ZERO`] for a zero-length input rather than
    /// NaN — callers in this crate always guard degenerate geometry first, and
    /// a silent NaN is far harder to trace than a silent zero.
    #[inline]
    pub fn normalized(self) -> Vec3 {
        let n = self.norm();
        if n <= 0.0 {
            Vec3::ZERO
        } else {
            self * (1.0 / n)
        }
    }

    /// Rotation about the +z axis by `angle` radians.
    #[inline]
    pub fn rotated_z(self, angle: f64) -> Vec3 {
        let (s, c) = angle.sin_cos();
        Vec3 {
            x: c * self.x - s * self.y,
            y: s * self.x + c * self.y,
            z: self.z,
        }
    }

    /// Distance from the z axis.
    #[inline]
    pub fn radius(self) -> f64 {
        self.x.hypot(self.y)
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, o: Vec3) -> Vec3 {
        Vec3 {
            x: self.x + o.x,
            y: self.y + o.y,
            z: self.z + o.z,
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3 {
            x: self.x - o.x,
            y: self.y - o.y,
            z: self.z - o.z,
        }
    }
}

impl std::ops::Mul<f64> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, s: f64) -> Vec3 {
        Vec3 {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        Vec3 {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

impl std::iter::Sum for Vec3 {
    fn sum<I: Iterator<Item = Vec3>>(iter: I) -> Vec3 {
        iter.fold(Vec3::ZERO, |a, b| a + b)
    }
}
