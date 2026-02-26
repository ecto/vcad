#![warn(missing_docs)]

//! Math types for the vcad B-rep kernel.
//!
//! Thin wrappers around tang providing domain-specific types
//! for 3D CAD geometry: points, vectors, directions, transforms,
//! and tolerance constants.
//!
//! This crate also provides exact geometric predicates via the
//! [`predicates`] module, which use adaptive-precision arithmetic
//! for robust geometric computations.

pub mod predicates;

/// A point in 3D space.
pub type Point3 = tang::Point3<f64>;

/// A vector in 3D space.
pub type Vec3 = tang::Vec3<f64>;

/// A unit (normalized) direction vector in 3D space.
pub type Dir3 = tang::Dir3<f64>;

/// A point in 2D parameter space.
pub type Point2 = tang::Point2<f64>;

/// A vector in 2D space.
pub type Vec2 = tang::Vec2<f64>;

/// A 4x4 affine transformation matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Transform {
    /// The underlying 4x4 matrix.
    pub matrix: tang::Mat4<f64>,
}

impl Transform {
    /// Identity transform.
    pub fn identity() -> Self {
        Self {
            matrix: tang::Mat4::identity(),
        }
    }

    /// Translation by `(dx, dy, dz)`.
    pub fn translation(dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            matrix: tang::Mat4::translation(dx, dy, dz),
        }
    }

    /// Non-uniform scale by `(sx, sy, sz)`.
    pub fn scale(sx: f64, sy: f64, sz: f64) -> Self {
        Self {
            matrix: tang::Mat4::scale(sx, sy, sz),
        }
    }

    /// Rotation about the X axis by `angle` radians.
    pub fn rotation_x(angle: f64) -> Self {
        Self {
            matrix: tang::Mat4::rotation_x(angle),
        }
    }

    /// Rotation about the Y axis by `angle` radians.
    pub fn rotation_y(angle: f64) -> Self {
        Self {
            matrix: tang::Mat4::rotation_y(angle),
        }
    }

    /// Rotation about the Z axis by `angle` radians.
    pub fn rotation_z(angle: f64) -> Self {
        Self {
            matrix: tang::Mat4::rotation_z(angle),
        }
    }

    /// Rotation about an arbitrary axis through the origin by `angle` radians.
    ///
    /// Uses Rodrigues' rotation formula.
    pub fn rotation_about_axis(axis: &Dir3, angle: f64) -> Self {
        Self {
            matrix: tang::Mat4::rotation_axis(axis.into_inner(), angle),
        }
    }

    /// Compose: `self` then `other` (self * other).
    pub fn then(&self, other: &Transform) -> Self {
        Self {
            matrix: self.matrix * other.matrix,
        }
    }

    /// Transform a point.
    pub fn apply_point(&self, p: &Point3) -> Point3 {
        self.matrix.transform_point(*p)
    }

    /// Transform a direction vector (ignores translation, applies rotation/scale).
    pub fn apply_vec(&self, v: &Vec3) -> Vec3 {
        self.matrix.transform_vec(*v)
    }

    /// Transform a normal vector (uses inverse transpose of upper-left 3x3).
    pub fn apply_normal(&self, n: &Vec3) -> Vec3 {
        self.matrix.transform_normal(*n)
    }

    /// Inverse of this transform, if it exists.
    pub fn inverse(&self) -> Option<Self> {
        self.matrix.try_inverse().map(|matrix| Self { matrix })
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

/// Tolerance constants for geometric comparisons.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    /// Linear distance tolerance in mm.
    pub linear: f64,
    /// Angular tolerance in radians.
    pub angular: f64,
}

impl Tolerance {
    /// Default CAD tolerances (1e-6 mm linear, 1e-9 rad angular).
    pub const DEFAULT: Self = Self {
        linear: 1e-6,
        angular: 1e-9,
    };

    /// Check if two points are coincident within tolerance.
    pub fn points_equal(&self, a: &Point3, b: &Point3) -> bool {
        (*a - *b).norm() < self.linear
    }

    /// Check if a scalar distance is effectively zero.
    pub fn is_zero(&self, d: f64) -> bool {
        d.abs() < self.linear
    }

    /// Check if two angles are effectively equal (in radians).
    pub fn angles_equal(&self, a: f64, b: f64) -> bool {
        (a - b).abs() < self.angular
    }
}

impl Default for Tolerance {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn test_identity_transform() {
        let t = Transform::identity();
        let p = Point3::new(1.0, 2.0, 3.0);
        let result = t.apply_point(&p);
        assert!((result - p).norm() < 1e-12);
    }

    #[test]
    fn test_translation() {
        let t = Transform::translation(10.0, 20.0, 30.0);
        let p = Point3::new(1.0, 2.0, 3.0);
        let result = t.apply_point(&p);
        assert!((result.x - 11.0).abs() < 1e-12);
        assert!((result.y - 22.0).abs() < 1e-12);
        assert!((result.z - 33.0).abs() < 1e-12);
    }

    #[test]
    fn test_rotation_z_90() {
        let t = Transform::rotation_z(PI / 2.0);
        let p = Point3::new(1.0, 0.0, 0.0);
        let result = t.apply_point(&p);
        assert!(result.x.abs() < 1e-12);
        assert!((result.y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_scale() {
        let t = Transform::scale(2.0, 3.0, 4.0);
        let p = Point3::new(1.0, 1.0, 1.0);
        let result = t.apply_point(&p);
        assert!((result.x - 2.0).abs() < 1e-12);
        assert!((result.y - 3.0).abs() < 1e-12);
        assert!((result.z - 4.0).abs() < 1e-12);
    }

    #[test]
    fn test_compose() {
        let t1 = Transform::translation(1.0, 0.0, 0.0);
        let t2 = Transform::scale(2.0, 2.0, 2.0);
        let composed = t2.then(&t1);
        let p = Point3::origin();
        let result = composed.apply_point(&p);
        assert!((result.x - 2.0).abs() < 1e-12);
    }

    #[test]
    fn test_inverse() {
        let t = Transform::translation(1.0, 2.0, 3.0);
        let inv = t.inverse().unwrap();
        let composed = t.then(&inv);
        let p = Point3::new(5.0, 6.0, 7.0);
        let result = composed.apply_point(&p);
        assert!((result - p).norm() < 1e-12);
    }

    #[test]
    fn test_rotation_about_axis() {
        // Rotate (1,0,0) by 90° about Z axis → (0,1,0)
        let axis = Dir3::new_normalize(Vec3::z());
        let t = Transform::rotation_about_axis(&axis, PI / 2.0);
        let p = Point3::new(1.0, 0.0, 0.0);
        let result = t.apply_point(&p);
        assert!(result.x.abs() < 1e-12);
        assert!((result.y - 1.0).abs() < 1e-12);
        assert!(result.z.abs() < 1e-12);

        // Rotate about (1,1,0) normalized by 180° — should swap x/y and negate z
        let axis2 = Dir3::new_normalize(Vec3::new(1.0, 1.0, 0.0));
        let t2 = Transform::rotation_about_axis(&axis2, PI);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let r2 = t2.apply_point(&p2);
        assert!((r2.x - 0.0).abs() < 1e-12);
        assert!((r2.y - 1.0).abs() < 1e-12);
        assert!(r2.z.abs() < 1e-12);
    }

    #[test]
    fn test_tolerance_points_equal() {
        let tol = Tolerance::DEFAULT;
        let a = Point3::new(1.0, 2.0, 3.0);
        let b = Point3::new(1.0 + 1e-7, 2.0, 3.0);
        assert!(tol.points_equal(&a, &b));
        let c = Point3::new(1.001, 2.0, 3.0);
        assert!(!tol.points_equal(&a, &c));
    }
}
